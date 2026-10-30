//! Local web dashboard for the authenticated live tests.
//!
//! ```sh
//! cargo run --example test_ui            # TEST_UI_PORT=7878 by default
//! ```
//!
//! Open the printed `http://127.0.0.1:<port>/?k=<secret>` URL. The server
//! binds 127.0.0.1 only, and every request must carry the per-process
//! secret and a `127.0.0.1`/`localhost` Host header. Read
//! `tests/LIVE_TESTING.md` first: this acts on a real LINE account.

mod cases;
mod picker;
mod report;
mod security;

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::{self, Stream, StreamExt};
use line_rs::client::{
    login_with_auth_token, login_with_qr, Client, ClientEvent, InitOptions, ListenHandle, ListenOptions,
};
use line_rs::storage::{FileStorage, Storage};
use line_rs::Device;
use serde::Deserialize;
use serde_json::{json, Value as JValue};
use tokio::sync::{broadcast, Notify};
use tokio::task::AbortHandle;

use cases::{Ctx, Inputs, Outcome, Throttle, CATALOGUE, MIN_GAP_SECS};

const STORAGE_PATH: &str = "./storage.test.json";
/// Storage keys for the token saved after login (server side only).
const KEY_TOKEN: &str = "testUi:authToken";
const KEY_DEVICE: &str = "testUi:device";
const INDEX_HTML: &str = include_str!("index.html");
const DEVICES: &[&str] = &["DESKTOPWIN", "DESKTOPMAC", "IOSIPAD", "ANDROIDSECONDARY", "WATCHOS", "WEAROS"];

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Default, Clone, serde::Serialize)]
struct LoginState {
    /// logged_out | logging_in | logged_in | failed | kicked_out
    status: &'static str,
    message: String,
    mid: Option<String>,
    device: Option<String>,
    qr_svg: Option<String>,
    pin: Option<String>,
}

#[derive(Default, Clone, serde::Serialize)]
struct TestState {
    /// idle | queued | running | pass | fail | skipped
    status: &'static str,
    started_ms: Option<u64>,
    duration_ms: Option<u64>,
    result: Option<JValue>,
    error: Option<String>,
    log: Vec<String>,
    /// "ok" | "bad"
    verdict: Option<String>,
    notes: String,
}

#[derive(Clone, serde::Serialize)]
struct Prompt {
    test: &'static str,
    text: String,
    deadline_ms: u64,
}

#[derive(Default)]
struct Inner {
    login: LoginState,
    client: Option<Client>,
    listen: Option<ListenHandle>,
    background: Vec<AbortHandle>,
    login_task: Option<AbortHandle>,
    saved_device: Option<String>,
    risk_accepted: bool,
    inputs: Inputs,
    tests: HashMap<&'static str, TestState>,
    queue: VecDeque<&'static str>,
    running: Option<(&'static str, AbortHandle)>,
    prompt: Option<Prompt>,
    groups: picker::PickList,
    peers: picker::PickList,
    keep: picker::PickList,
    /// Bumped whenever the client is dropped, so a picker load started for
    /// an old session cannot write into a new one.
    session: u64,
}

impl Inner {
    fn test(&mut self, id: &'static str) -> &mut TestState {
        self.tests.entry(id).or_insert_with(|| TestState { status: "idle", ..Default::default() })
    }

    fn picks(&mut self, kind: picker::Kind) -> &mut picker::PickList {
        match kind {
            picker::Kind::Groups => &mut self.groups,
            picker::Kind::Peers => &mut self.peers,
            picker::Kind::Keep => &mut self.keep,
        }
    }

    /// Drop the client and everything attached to it.
    fn drop_client(&mut self) {
        self.groups = Default::default();
        self.peers = Default::default();
        self.keep = Default::default();
        self.session += 1;
        if let Some(h) = self.listen.take() {
            h.abort();
        }
        for h in self.background.drain(..) {
            h.abort();
        }
        if let Some(h) = self.login_task.take() {
            h.abort();
        }
        self.client = None;
    }
}

struct App {
    secret: String,
    port: u16,
    commit: String,
    inner: StdMutex<Inner>,
    throttle: Arc<Throttle>,
    /// SSE fan-out: (event name, JSON payload).
    tx: broadcast::Sender<(&'static str, String)>,
    wake: Notify,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl App {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn snapshot_of(&self, g: &Inner) -> JValue {
        let cases: Vec<JValue> = CATALOGUE
            .iter()
            .map(|c| {
                let st = g.tests.get(c.id).cloned().unwrap_or(TestState { status: "idle", ..Default::default() });
                json!({"info": c, "state": st})
            })
            .collect();
        json!({
            "login": g.login,
            "has_saved_token": g.saved_device.is_some(),
            "saved_device": g.saved_device,
            "risk_accepted": g.risk_accepted,
            "throttle_secs": self.throttle.gap(),
            "min_throttle_secs": MIN_GAP_SECS,
            "inputs": g.inputs,
            "prompt": g.prompt,
            "picks": {"groups": g.groups, "peers": g.peers, "keep": g.keep},
            "running": g.running.as_ref().map(|r| r.0),
            "queue": g.queue,
            "modules": cases::MODULES,
            "devices": DEVICES,
            "cases": cases,
            "now_ms": now_ms(),
        })
    }

    fn snapshot(&self) -> JValue {
        let g = self.lock();
        self.snapshot_of(&g)
    }

    /// Push the current state to every page.
    fn changed(&self) {
        let snap = self.snapshot().to_string();
        let _ = self.tx.send(("state", snap));
    }

    fn line_event(&self, v: JValue) {
        let _ = self.tx.send(("line", v.to_string()));
    }

    fn ensure_listen(&self) {
        let mut g = self.lock();
        let Some(client) = g.client.clone() else { return };
        if g.listen.as_ref().is_none_or(ListenHandle::is_finished) {
            g.listen = Some(client.listen(ListenOptions { talk: true, square: false, polling: false }));
        }
    }
}

// ---------------------------------------------------------------------------
// LINE events -> browser
// ---------------------------------------------------------------------------

fn event_json(ev: &ClientEvent) -> JValue {
    let (kind, data) = match ev {
        ClientEvent::Message(m) => ("Message".to_string(), m.raw.to_json()),
        ClientEvent::MessageEdit(m) => ("MessageEdit".to_string(), m.raw.to_json()),
        ClientEvent::Event(v) => {
            let t = v["type"].as_str().map(str::to_string).unwrap_or_else(|| v["type"].to_string());
            (format!("Event {t}"), v.to_json())
        }
        ClientEvent::Talk(t) => ("Talk".to_string(), JValue::String(format!("{t:?}"))),
        ClientEvent::SquareMessage(m) => ("SquareMessage".to_string(), m.raw.to_json()),
        ClientEvent::SquareEvent(v) => ("SquareEvent".to_string(), v.to_json()),
    };
    json!({"type": kind, "time_ms": now_ms(), "data": data})
}

fn spawn_forwarders(app: &Arc<App>, client: &Client) -> Vec<AbortHandle> {
    let mut rx = client.subscribe();
    let a = app.clone();
    let talk = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => a.line_event(event_json(&ev)),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    a.line_event(json!({"type": "lagged", "time_ms": now_ms(), "data": format!("{n} events dropped")}))
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Base events: keep the rotated token server side, surface kick-outs.
    let mut rx = client.base.subscribe();
    let a = app.clone();
    let c = client.clone();
    let base = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(line_rs::ClientEvent::UpdateAuthToken(t)) => {
                    let saved = c.base.storage.set(KEY_TOKEN, t.into()).await;
                    let note = if saved.is_ok() { "rotated token saved to storage" } else { "rotated token NOT saved" };
                    a.line_event(json!({"type": "UpdateAuthToken", "time_ms": now_ms(), "data": note}));
                }
                Ok(line_rs::ClientEvent::KickedOut(r)) => {
                    {
                        let mut g = a.lock();
                        g.login.status = "kicked_out";
                        g.login.message = format!("{r:?}");
                    }
                    a.line_event(json!({"type": "KickedOut", "time_ms": now_ms(), "data": format!("{r:?}")}));
                    a.changed();
                }
                Ok(line_rs::ClientEvent::End(v)) => {
                    let data = v.map(|v| v.to_json()).unwrap_or(JValue::Null);
                    a.line_event(json!({"type": "End", "time_ms": now_ms(), "data": data}));
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    vec![talk.abort_handle(), base.abort_handle()]
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct LoginReq {
    device: String,
    /// qr | token | saved
    method: String,
    #[serde(default)]
    token: String,
}

async fn read_saved() -> Option<(String, String)> {
    let s = FileStorage::new(STORAGE_PATH).ok()?;
    let token = s.get_string(KEY_TOKEN).await.ok()??;
    let device = s.get_string(KEY_DEVICE).await.ok()?.unwrap_or_else(|| "DESKTOPWIN".into());
    Some((token, device))
}

fn qr_svg(url: &str) -> Option<String> {
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    Some(
        code.render::<qrcode::render::svg::Color>()
            .min_dimensions(220, 220)
            .dark_color(qrcode::render::svg::Color("#000"))
            .light_color(qrcode::render::svg::Color("#fff"))
            .build(),
    )
}

async fn do_login(app: Arc<App>, req: LoginReq) {
    let set = |status: &'static str, msg: String| {
        let mut g = app.lock();
        g.login.status = status;
        g.login.message = msg;
    };
    let (token, device_name) = match req.method.as_str() {
        "token" => (Some(req.token.trim().to_string()), req.device.clone()),
        "saved" => match read_saved().await {
            Some((t, d)) => (Some(t), d),
            None => {
                set("failed", "no saved token in ./storage.test.json".into());
                app.changed();
                return;
            }
        },
        _ => (None, req.device.clone()),
    };
    let device: Device = match device_name.parse() {
        Ok(d) => d,
        Err(_) => {
            set("failed", format!("unknown device {device_name}"));
            app.changed();
            return;
        }
    };
    let storage = match FileStorage::new(STORAGE_PATH) {
        Ok(s) => s,
        Err(e) => {
            set("failed", format!("open {STORAGE_PATH}: {e}"));
            app.changed();
            return;
        }
    };
    let init = InitOptions::new(device).storage(storage);
    {
        let mut g = app.lock();
        g.login = LoginState {
            status: "logging_in",
            message: "waiting for request throttle…".into(),
            device: Some(device.as_str().into()),
            ..Default::default()
        };
    }
    app.changed();
    app.throttle.wait().await;
    set("logging_in", "logging in…".into());
    app.changed();

    let result = match token {
        Some(t) if !t.is_empty() => login_with_auth_token(&t, init).await,
        Some(_) => {
            set("failed", "auth token is empty".into());
            app.changed();
            return;
        }
        None => {
            let (a1, a2) = (app.clone(), app.clone());
            login_with_qr(
                init,
                move |url| {
                    {
                        let mut g = a1.lock();
                        g.login.qr_svg = qr_svg(&url);
                        g.login.pin = None;
                        g.login.message = "สแกน QR ด้วยแอป LINE บนมือถือ".into();
                    }
                    a1.changed();
                },
                move |pin| {
                    {
                        let mut g = a2.lock();
                        g.login.qr_svg = None;
                        g.login.pin = Some(pin);
                        g.login.message = "พิมพ์ PIN นี้บนมือถือ".into();
                    }
                    a2.changed();
                },
            )
            .await
        }
    };

    match result {
        Ok(client) => {
            let mid = client.mid();
            if let Some(t) = client.auth_token() {
                let _ = client.base.storage.set(KEY_TOKEN, t.into()).await;
                let _ = client.base.storage.set(KEY_DEVICE, device.as_str().into()).await;
            }
            println!("logged in as {} ({})", mid.clone().unwrap_or_default(), device.as_str());
            let fwd = spawn_forwarders(&app, &client);
            {
                let mut g = app.lock();
                g.login = LoginState {
                    status: "logged_in",
                    message: String::new(),
                    mid,
                    device: Some(device.as_str().into()),
                    ..Default::default()
                };
                g.saved_device = Some(device.as_str().into());
                g.background.extend(fwd);
                g.client = Some(client);
                g.login_task = None;
            }
            app.ensure_listen();
        }
        Err(e) => {
            let mut g = app.lock();
            g.login = LoginState { status: "failed", message: format!("login failed (not retried): {e}"), ..Default::default() };
            g.login_task = None;
        }
    }
    app.changed();
}

// ---------------------------------------------------------------------------
// Runner: one test at a time, no retries
// ---------------------------------------------------------------------------

async fn worker(app: Arc<App>) {
    loop {
        let next = {
            let mut g = app.lock();
            if g.running.is_none() { g.queue.pop_front() } else { None }
        };
        let Some(id) = next else {
            app.wake.notified().await;
            continue;
        };
        run_one(&app, id).await;
    }
}

async fn run_one(app: &Arc<App>, id: &'static str) {
    let info = cases::find(id).expect("queued ids come from the catalogue");
    let (client, inputs, risk) = {
        let g = app.lock();
        (g.client.clone(), g.inputs.clone(), g.risk_accepted)
    };
    let fail_now = |msg: &str| {
        let mut g = app.lock();
        let t = g.test(id);
        t.status = "fail";
        t.error = Some(msg.to_string());
        t.duration_ms = None;
    };
    let Some(client) = client.filter(|c| !c.base.is_disabled()) else {
        fail_now("not logged in");
        app.changed();
        return;
    };
    if !risk {
        fail_now("risk checkbox not ticked");
        app.changed();
        return;
    }
    if info.interactive {
        app.ensure_listen();
    }

    let log = Arc::new(StdMutex::new(Vec::new()));
    let a = app.clone();
    let prompt: cases::PromptFn = Arc::new(move |p: Option<(String, u64)>| {
        {
            let mut g = a.lock();
            g.prompt = p.map(|(text, secs)| Prompt { test: id, text, deadline_ms: now_ms() + secs * 1000 });
        }
        a.changed();
    });
    let ctx = Ctx { client, throttle: app.throttle.clone(), inputs, prompt, log: log.clone() };
    let started = Instant::now();
    let task = tokio::spawn(async move { cases::run_case(id, &ctx).await });
    {
        let mut g = app.lock();
        g.running = Some((id, task.abort_handle()));
        let t = g.test(id);
        *t = TestState {
            status: "running",
            started_ms: Some(now_ms()),
            // a rerun needs a fresh look on the phone; notes are kept
            notes: std::mem::take(&mut t.notes),
            ..Default::default()
        };
    }
    app.changed();

    let outcome = task.await;
    let elapsed = started.elapsed().as_millis() as u64;
    {
        let mut g = app.lock();
        g.running = None;
        g.prompt = None;
        let t = g.test(id);
        t.duration_ms = Some(elapsed);
        t.log = log.lock().map(|l| l.clone()).unwrap_or_default();
        match outcome {
            Ok(Ok(Outcome::Pass(v))) => {
                t.status = "pass";
                t.result = Some(v);
            }
            Ok(Ok(Outcome::Skip(reason))) => {
                t.status = "skipped";
                t.error = Some(format!("skipped: {reason}"));
            }
            Ok(Err(e)) => {
                t.status = "fail";
                t.error = Some(e);
            }
            Err(e) if e.is_cancelled() => {
                t.status = "fail";
                t.error = Some("cancelled by user".into());
            }
            Err(e) => {
                t.status = "fail";
                t.error = Some(format!("panic: {e}"));
            }
        }
    }
    app.changed();
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

async fn gate(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let h = req.headers();
    let s = |name: &str| h.get(name).and_then(|v| v.to_str().ok());
    let ok = security::authorized(
        &app.secret,
        app.port,
        s(header::HOST.as_str()),
        s(header::ORIGIN.as_str()),
        s(security::SECRET_HEADER),
        req.uri().query(),
    );
    if !ok {
        return (StatusCode::FORBIDDEN, "403: missing/invalid secret or Host. Use the URL printed in the terminal.\n")
            .into_response();
    }
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'",
        ),
    );
    res
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn get_state(State(app): State<Arc<App>>) -> Json<JValue> {
    Json(app.snapshot())
}

async fn sse(State(app): State<Arc<App>>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = app.tx.subscribe();
    let first = Event::default().event("state").data(app.snapshot().to_string());
    let rest = stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok((name, data)) => return Some((Ok(Event::default().event(name).data(data)), rx)),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream::once(async { Ok(first) }).chain(rest)).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
struct SettingsReq {
    risk_accepted: Option<bool>,
    throttle_secs: Option<u64>,
    inputs: Option<Inputs>,
}

async fn post_settings(State(app): State<Arc<App>>, Json(req): Json<SettingsReq>) -> Json<JValue> {
    if let Some(s) = req.throttle_secs {
        app.throttle.set_gap(s);
    }
    {
        let mut g = app.lock();
        if let Some(r) = req.risk_accepted {
            g.risk_accepted = r;
        }
        if let Some(i) = req.inputs {
            g.inputs = i;
        }
    }
    app.changed();
    Json(json!({"ok": true}))
}

fn err(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({"ok": false, "error": msg}))).into_response()
}

async fn post_login(State(app): State<Arc<App>>, Json(req): Json<LoginReq>) -> Response {
    {
        let g = app.lock();
        if !g.risk_accepted {
            return err(StatusCode::BAD_REQUEST, "ติ๊กช่องยอมรับความเสี่ยงก่อน");
        }
        if g.login.status == "logging_in" || g.client.is_some() {
            return err(StatusCode::CONFLICT, "already logged in or logging in; use Stop all first");
        }
    }
    let task = tokio::spawn(do_login(app.clone(), req));
    app.lock().login_task = Some(task.abort_handle());
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize, Default)]
struct PickReq {
    #[serde(default)]
    refresh: bool,
}

/// Load the group or peer list in the background (state updates over SSE).
/// Cached until `refresh` or logout; requests go through the shared throttle.
async fn post_pick(
    State(app): State<Arc<App>>,
    axum::extract::Path(kind): axum::extract::Path<String>,
    body: Option<Json<PickReq>>,
) -> Response {
    let Some(kind) = picker::Kind::parse(&kind) else { return err(StatusCode::NOT_FOUND, "unknown picker") };
    let refresh = body.map(|b| b.refresh).unwrap_or_default();
    let (client, session) = {
        let mut g = app.lock();
        if !g.risk_accepted {
            return err(StatusCode::BAD_REQUEST, "ติ๊กช่องยอมรับความเสี่ยงก่อน");
        }
        let Some(client) = g.client.clone().filter(|c| !c.base.is_disabled()) else {
            return err(StatusCode::BAD_REQUEST, "login first");
        };
        let session = g.session;
        let list = g.picks(kind);
        if list.status == "loading" || (list.status == "ready" && !refresh) {
            return Json(json!({"ok": true})).into_response();
        }
        *list = picker::PickList { status: "loading", items: std::mem::take(&mut list.items), ..Default::default() };
        (client, session)
    };
    app.changed();
    let a = app.clone();
    let task = tokio::spawn(async move {
        let res = picker::load(kind, &client, &a.throttle).await;
        {
            let mut g = a.lock();
            if g.session != session {
                return;
            }
            let list = g.picks(kind);
            if list.status != "loading" {
                return;
            }
            *list = match res {
                Ok(l) => picker::PickList {
                    status: "ready",
                    items: l.items,
                    note: l.note,
                    fetched_ms: Some(now_ms()),
                    ..Default::default()
                },
                Err(e) => picker::PickList { status: "error", error: Some(e), ..Default::default() },
            };
        }
        a.changed();
    });
    app.lock().background.push(task.abort_handle());
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct RunReq {
    ids: Vec<String>,
}

async fn post_run(State(app): State<Arc<App>>, Json(req): Json<RunReq>) -> Response {
    {
        let mut g = app.lock();
        if !g.risk_accepted {
            return err(StatusCode::BAD_REQUEST, "ติ๊กช่องยอมรับความเสี่ยงก่อน");
        }
        if g.client.is_none() {
            return err(StatusCode::BAD_REQUEST, "login first");
        }
        for id in &req.ids {
            let Some(info) = cases::find(id) else { continue };
            let busy = g.queue.contains(&info.id) || g.running.as_ref().is_some_and(|r| r.0 == info.id);
            if !busy {
                g.queue.push_back(info.id);
                let t = g.test(info.id);
                t.status = "queued";
                t.error = None;
                t.result = None;
            }
        }
    }
    app.wake.notify_one();
    app.changed();
    Json(json!({"ok": true})).into_response()
}

/// Cancel the running test only.
async fn post_cancel(State(app): State<Arc<App>>) -> Json<JValue> {
    if let Some((_, h)) = app.lock().running.as_ref() {
        h.abort();
    }
    Json(json!({"ok": true}))
}

/// Kill switch: clear the queue, abort the running test, drop the client.
async fn post_stop(State(app): State<Arc<App>>) -> Json<JValue> {
    {
        let mut g = app.lock();
        let queued: Vec<_> = g.queue.drain(..).collect();
        for id in queued {
            g.test(id).status = "idle";
        }
        if let Some((_, h)) = g.running.as_ref() {
            h.abort();
        }
        g.prompt = None;
        g.drop_client();
        g.login = LoginState { status: "logged_out", message: "stopped; client dropped".into(), ..Default::default() };
    }
    println!("stop-all: queue cleared, client dropped");
    app.changed();
    Json(json!({"ok": true}))
}

#[derive(Deserialize)]
struct VerdictReq {
    id: String,
    verdict: Option<String>,
    #[serde(default)]
    notes: String,
}

async fn post_verdict(State(app): State<Arc<App>>, Json(req): Json<VerdictReq>) -> Response {
    let Some(info) = cases::find(&req.id) else { return err(StatusCode::NOT_FOUND, "unknown test") };
    {
        let mut g = app.lock();
        let t = g.test(info.id);
        t.verdict = req.verdict.filter(|v| v == "ok" || v == "bad");
        t.notes = req.notes.chars().take(2000).collect();
    }
    app.changed();
    Json(json!({"ok": true})).into_response()
}

async fn export_md(State(app): State<Arc<App>>) -> impl IntoResponse {
    let g = app.lock();
    let empty = TestState { status: "idle", ..Default::default() };
    let rows: Vec<report::ReportRow<'_>> = CATALOGUE
        .iter()
        .map(|c| {
            let t = g.tests.get(c.id).unwrap_or(&empty);
            report::ReportRow {
                id: c.id,
                auto: t.status,
                duration_ms: t.duration_ms,
                verdict: t.verdict.as_deref(),
                notes: &t.notes,
                error: t.error.as_deref(),
            }
        })
        .collect();
    let device = g.login.device.clone().or_else(|| g.saved_device.clone()).unwrap_or_default();
    let md = report::render(&report::utc_datetime(report::now_secs()), &app.commit, &device, &rows);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/markdown; charset=utf-8"));
    headers.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"results.md\""));
    (headers, md)
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let port: u16 = std::env::var("TEST_UI_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(7878);
    let (tx, _) = broadcast::channel(512);
    let saved_device = read_saved().await.map(|(_, d)| d);
    let app = Arc::new(App {
        secret: security::new_secret(),
        port,
        commit: report::git_commit(),
        inner: StdMutex::new(Inner {
            login: LoginState { status: "logged_out", ..Default::default() },
            saved_device,
            ..Default::default()
        }),
        throttle: Arc::new(Throttle::new(MIN_GAP_SECS)),
        tx,
        wake: Notify::new(),
    });
    tokio::spawn(worker(app.clone()));

    let router = Router::new()
        .route("/", get(index))
        .route("/api/state", get(get_state))
        .route("/api/events", get(sse))
        .route("/api/settings", post(post_settings))
        .route("/api/login", post(post_login))
        .route("/api/run", post(post_run))
        .route("/api/pick/{kind}", post(post_pick))
        .route("/api/cancel", post(post_cancel))
        .route("/api/stop", post(post_stop))
        .route("/api/verdict", post(post_verdict))
        .route("/api/export.md", get(export_md))
        .layer(middleware::from_fn_with_state(app.clone(), gate))
        .with_state(app.clone());

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("line-rs test UI (read tests/LIVE_TESTING.md first)");
    println!("open: http://127.0.0.1:{port}/?k={}", app.secret);
    axum::serve(listener, router).await
}
