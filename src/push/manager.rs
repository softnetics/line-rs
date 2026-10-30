//! Push connection state machine (`connManager.ts`): sign-on requests for
//! square `fetchMyEvents` (service 3) and talk `sync` (services 5/8), ping
//! handling, push frames and the read loop of one connection.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use futures::future::BoxFuture;
use futures::StreamExt;
use serde_json::json;
use tokio::task::JoinSet;

use super::conn::{ByteStream, Conn, ConnWriter, FrameDecoder, Packet};
use super::frame::{services_mask, sign_on_request_packet, status_packet, AckType, PushFrame};
use super::{tmc, Hub, Kind};
use crate::error::{Error, Result};
use crate::polling::SyncData;
use crate::thrift::{self, Protocol, Schema, TValue, Value};
use crate::tv;

/// Ping interval announced in the status frame (seconds).
const PING_INTERVAL: u8 = 30;

/// What the push machinery needs from the client. Implemented by
/// [`BaseClient`](crate::BaseClient); tests use fakes.
pub(crate) trait PushHost: Clone + Send + Sync + 'static {
    fn auth_token(&self) -> Option<String>;
    fn sync_state(&self) -> &tokio::sync::Mutex<SyncData>;
    fn emit_sync(&self, data: SyncData);
    fn log(&self, kind: &str, data: serde_json::Value);
    /// Start the `/PUSH` request at `path`.
    fn open_conn(&self, path: &str) -> Result<Conn>;
    /// `SquareService.fetchMyEvents(args)` over the normal RPC path.
    fn fetch_my_events(&self, args: Value) -> BoxFuture<'static, Result<Value>>;
    /// `TalkService.noop()`.
    fn noop(&self) -> BoxFuture<'static, Result<()>>;
}

#[derive(Debug, Default)]
struct State {
    /// request id → (service type, method name)
    sign_on_requests: HashMap<u16, (u8, &'static str)>,
    /// square subscription id → last refresh (unix seconds)
    subscription_ids: HashMap<i64, f64>,
    subscription_id: i64,
    curr_ping_id: u16,
    event_synced: bool,
    auth_token: Option<String>,
}

pub(crate) struct ConnManager<H> {
    host: H,
    hub: Arc<Hub>,
    state: Arc<Mutex<State>>,
}

impl<H> Clone for ConnManager<H>
where
    H: Clone,
{
    fn clone(&self) -> Self {
        Self { host: self.host.clone(), hub: self.hub.clone(), state: self.state.clone() }
    }
}

fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or_default()
}

pub(crate) fn talk_sync_args(sync: &SyncData) -> Value {
    tv!({"request": {
        "lastRevision": sync.talk.revision,
        "count": 100,
        "lastGlobalRevision": sync.talk.global_rev,
        "lastIndividualRevision": sync.talk.individual_rev,
    }})
}

fn error_json(e: &Error) -> serde_json::Value {
    json!({"error": e.to_string()})
}

impl<H: PushHost> ConnManager<H> {
    pub fn new(host: H, hub: Arc<Hub>) -> Self {
        Self { host, hub, state: Arc::default() }
    }

    fn log(&self, text: &str, data: serde_json::Value) {
        self.host.log(&format!("[LEGY/PUSH] {text}"), data);
    }

    #[cfg(test)]
    pub fn subscription_id(&self) -> i64 {
        self.state.lock().unwrap().subscription_id
    }

    /// Open a new connection subscribed to `services`.
    pub fn initialize_conn(&self, services: &[u8]) -> Result<Conn> {
        {
            let mut st = self.state.lock().unwrap();
            st.sign_on_requests.clear();
            st.auth_token = self.host.auth_token();
        }
        let m = services_mask(services);
        self.log(&format!("Using `m={m}` on `/PUSH`"), json!(""));
        self.host.open_conn(&format!("/PUSH/1/subs?m={m}"))
    }

    /// Encode and queue a sign-on request; returns its request id.
    pub fn build_and_send_sign_on_request(&self, w: &ConnWriter, service: u8, args: &Value) -> Result<u16> {
        let (method, args_struct) = match service {
            3 => ("fetchMyEvents", Some("SquareService_fetchMyEvents_args")),
            5 | 8 => ("sync", Some("sync_args")),
            _ => ("", None),
        };
        let req = match args_struct {
            Some(name) => thrift::write_message(method, &thrift::encode_struct(Schema::line(), name, args)?, Protocol::Compact)?,
            None => Vec::new(),
        };
        let id = {
            let mut st = self.state.lock().unwrap();
            let id = st.sign_on_requests.len() as u16 + 1;
            st.sign_on_requests.insert(id, (service, method));
            id
        };
        self.log(&format!("[H2][PUSH] send sign-on-request. requestId:{id}, service:{service}"), json!(""));
        w.write(sign_on_request_packet(id, service, &req))?;
        Ok(id)
    }

    /// Handle a complete sign-on response. Failures are logged; like linejs
    /// they end that service's request chain on this connection.
    pub async fn on_sign_on_response(&self, w: &ConnWriter, req_id: u16, data: &[u8]) {
        let entry = self.state.lock().unwrap().sign_on_requests.get(&req_id).copied();
        let Some((service, _method)) = entry else {
            self.log(&format!("[PUSH] unknown sign-on-response requestId:{req_id}"), json!(""));
            return;
        };
        self.log(
            &format!(
                "receives sign-on-response frame. requestId:{req_id}, service:{service}, payload:{}",
                data.len()
            ),
            json!(""),
        );
        let res = match service {
            3 => self.on_square_sign_on(data).await,
            5 | 8 => self.on_talk_sign_on(w, service, data).await,
            _ => Err(Error::internal(
                "LegyPusherError",
                format!("[PUSH] receives invalid sign-on-response frame. requestId:{req_id}, service:{service}"),
            )),
        };
        if let Err(e) = res {
            self.log("sign-on-response failed", error_json(&e));
        }
    }

    async fn on_square_sign_on(&self, data: &[u8]) -> Result<()> {
        let msg = thrift::read_message(data, Protocol::Compact)?;
        let res = thrift::decode_struct(Schema::line(), Some("SquareService_fetchMyEvents_result"), &msg.body);
        if !res["e"].is_null() {
            self.log(&format!("can't use PUSH for OpenChat:{}", res["e"]["errorCode"]), res["e"].to_json());
            return Ok(());
        }
        let success = &res["success"];
        let bad = |what: &str| Error::internal("LegyPusherError", format!("{what}: {}", success));
        let events = success["events"].as_list().ok_or_else(|| bad("events should be list"))?;
        let sync_token = success["syncToken"].as_str().ok_or_else(|| bad("syncToken should be str"))?.to_string();
        let subscription_id =
            success["subscription"]["subscriptionId"].as_i64().ok_or_else(|| bad("subscriptionId should be int"))?;
        self.log(
            &format!("response fetchMyEvent({subscription_id}) events:{}, syncToken:{sync_token}", events.len()),
            json!(""),
        );
        self.state.lock().unwrap().subscription_ids.insert(subscription_id, now_secs());
        for ev in events {
            self.hub.deliver(Kind::Square, ev.clone());
        }
        let snapshot = {
            let mut sync = self.host.sync_state().lock().await;
            sync.square = Some(sync_token.clone());
            sync.clone()
        };
        self.host.emit_sync(snapshot);
        let first = {
            let mut st = self.state.lock().unwrap();
            st.subscription_id = subscription_id;
            !std::mem::replace(&mut st.event_synced, true)
        };
        if first {
            self.log(&format!("myEvents start({subscription_id}) : syncToken:{sync_token}"), json!(""));
        }
        Ok(())
    }

    async fn on_talk_sign_on(&self, w: &ConnWriter, service: u8, data: &[u8]) -> Result<()> {
        let body = match tmc::read_result(data) {
            Ok(tv) => tv,
            Err(e) => {
                self.log(&format!("[PUSH] parse error: {e}"), json!(""));
                thrift::read_message(data, Protocol::Compact)?.body
            }
        };
        let res = thrift::decode_struct(Schema::line(), Some("sync_result"), &body);
        if !res["e"].is_null() {
            self.log("sync error:", res["e"].to_json());
            return Ok(());
        }
        let response = &res["success"];
        if response.is_null() {
            return Ok(());
        }
        let ops = {
            let mut sync = self.host.sync_state().lock().await;
            apply_sync_response(&mut sync, response)
        };
        for op in ops {
            self.hub.deliver(Kind::Talk, op);
        }
        let snapshot = self.host.sync_state().lock().await.clone();
        self.host.emit_sync(snapshot.clone());
        let args = talk_sync_args(&snapshot);
        self.log("request talk fetcher:", args.to_json());
        self.build_and_send_sign_on_request(w, service, &args)?;
        Ok(())
    }

    /// Handle a push frame (service 3 = new square events: fetch them).
    pub async fn on_push_response(&self, frame: PushFrame) {
        self.log("_OnPushResponse", json!({"serviceType": frame.service_type, "pushId": frame.push_id}));
        if frame.service_type != 3 {
            return;
        }
        if let Err(e) = self.fetch_square_after_push(&frame).await {
            self.log("fetchMyEvents failed", error_json(&e));
        }
    }

    async fn fetch_square_after_push(&self, frame: &PushFrame) -> Result<()> {
        let payload = thrift::read_struct(&frame.payload, Protocol::Compact)?;
        let pushed_id = payload.field(1).and_then(TValue::as_i64).unwrap_or_default();
        self.state.lock().unwrap().subscription_id = pushed_id;
        let sync_token = self.host.sync_state().lock().await.square.clone();
        let res = self
            .host
            .fetch_my_events(tv!({"request": {"subscriptionId": pushed_id, "syncToken": sync_token, "limit": 100}}))
            .await?;
        for ev in res["events"].as_list().unwrap_or_default() {
            self.hub.deliver(Kind::Square, ev.clone());
        }
        let snapshot = {
            let mut sync = self.host.sync_state().lock().await;
            if let Some(t) = res["syncToken"].as_str() {
                sync.square = Some(t.to_string());
            }
            sync.clone()
        };
        self.host.emit_sync(snapshot);
        let sub_id = res["subscription"]["subscriptionId"].as_i64().unwrap_or_default();
        self.state.lock().unwrap().subscription_id = sub_id;
        self.log("SQ_fetchMyEvents", json!({"syncToken": res["syncToken"].to_json(), "subscriptionId": sub_id}));
        Ok(())
    }

    /// Ping bookkeeping; every third ping runs `noop` and reconnects when the
    /// access token has changed since the connection was opened.
    pub async fn on_ping(&self, w: &ConnWriter, ping_id: u16) {
        let refresh: Vec<i64> = {
            let mut st = self.state.lock().unwrap();
            st.curr_ping_id = ping_id;
            let t1 = now_secs();
            let mut ids = Vec::new();
            for (id, t2) in st.subscription_ids.iter_mut() {
                if t1 - *t2 >= 3000.0 {
                    *t2 = t1;
                    ids.push(*id);
                }
            }
            ids
        };
        if !refresh.is_empty() {
            self.log(&format!("?refresh square subscriptionId: {refresh:?}"), json!(""));
        }
        if self.hub.is_idle() {
            self.log("no listeners left, closing push conn", json!(""));
            w.close();
            return;
        }
        if !ping_id.is_multiple_of(3) {
            return;
        }
        match self.host.noop().await {
            Ok(()) => {
                let new = self.host.auth_token();
                let changed = {
                    let mut st = self.state.lock().unwrap();
                    if new.is_some() && st.auth_token != new {
                        st.auth_token = new;
                        true
                    } else {
                        false
                    }
                };
                if changed {
                    self.log("renew push conn for new authToken...", json!(""));
                    w.close();
                }
            }
            Err(e) => self.log("noop failed", error_json(&e)),
        }
    }

    /// Send the status frame and initial sign-on requests, then read the
    /// connection until it ends (`InitAndRead`).
    pub async fn init_and_read(&self, conn: Conn, services: &[u8]) -> Result<()> {
        let Conn { writer: w, response } = conn;
        w.write(status_packet(0, PING_INTERVAL))?;
        self.log(&format!("send status frame. flag:0, pi:{PING_INTERVAL}"), json!(""));

        for &service in services {
            self.log(&format!("Init service: {service}"), json!(""));
            match service {
                3 => {
                    let subscription_id =
                        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or_default();
                    let sync_token = self.host.sync_state().lock().await.square.clone();
                    self.log(
                        &format!("request fetchMyEvent({subscription_id}), syncToken:{sync_token:?}"),
                        json!(""),
                    );
                    self.state.lock().unwrap().subscription_ids.clear();
                    let args = tv!({"request": {"subscriptionId": subscription_id, "syncToken": sync_token, "limit": 100}});
                    self.build_and_send_sign_on_request(&w, service, &args)?;
                }
                5 | 8 => {
                    let args = talk_sync_args(&*self.host.sync_state().lock().await);
                    self.log(&format!("request talk fetcher: {}", args.to_json()), json!(""));
                    self.build_and_send_sign_on_request(&w, service, &args)?;
                }
                _ => {}
            }
        }

        self.log("CONN start read push.", json!(""));
        let res = self.read(&w, response).await;
        let ping = self.state.lock().unwrap().curr_ping_id;
        self.log(&format!("CONN died on PingId={ping}"), json!(""));
        res
    }

    async fn read(&self, w: &ConnWriter, response: BoxFuture<'static, Result<ByteStream>>) -> Result<()> {
        let mut body = tokio::select! {
            _ = w.closed() => return Ok(()),
            r = response => r?,
        };
        let mut decoder = FrameDecoder::new();
        // Handlers that call back into the network run concurrently so pings
        // keep being acked; dropping the set aborts them with the connection.
        let mut tasks = JoinSet::new();
        loop {
            let chunk = tokio::select! {
                _ = w.closed() => return Ok(()),
                c = body.next() => c,
            };
            let Some(chunk) = chunk else { return Ok(()) };
            for packet in decoder.feed(&chunk?)? {
                match packet {
                    Packet::Ping(f) => {
                        w.write(f.ack_packet())?;
                        self.log(&format!("[PUSH] send ping ack. pingId:{}", f.ping_id), json!(""));
                        let (m, w) = (self.clone(), w.clone());
                        tasks.spawn(async move { m.on_ping(&w, f.ping_id).await });
                    }
                    Packet::SignOnResponse(f) => self.on_sign_on_response(w, f.request_id, &f.payload).await,
                    Packet::Push(f) => {
                        if f.push_type == AckType::AckRequired as u8 {
                            w.write(f.ack_packet())?;
                        }
                        let m = self.clone();
                        tasks.spawn(async move { m.on_push_response(f).await });
                    }
                }
            }
            while tasks.try_join_next().is_some() {}
        }
    }
}

/// Fold a talk `sync` response into the cursors and return its operations
/// (shared with the polling path).
pub(crate) fn apply_sync_response(sync: &mut SyncData, response: &Value) -> Vec<Value> {
    let nonzero = |v: &Value| v.as_i64().filter(|&n| n != 0);
    if let Some(r) = nonzero(&response["fullSyncResponse"]["nextRevision"]) {
        sync.talk.revision = r;
    }
    let ops_res = &response["operationResponse"];
    if let Some(r) = nonzero(&ops_res["globalEvents"]["lastRevision"]) {
        sync.talk.global_rev = r;
    }
    if let Some(r) = nonzero(&ops_res["individualEvents"]["lastRevision"]) {
        sync.talk.individual_rev = r;
    }
    let ops = ops_res["operations"].as_list().map(<[Value]>::to_vec).unwrap_or_default();
    for op in &ops {
        if let Some(r) = op["revision"].as_i64() {
            sync.talk.revision = r;
        }
    }
    ops
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use bytes::Bytes;
    use futures::FutureExt;
    use tokio::sync::mpsc;

    use super::*;
    use crate::push::frame::{request_packet, FRAME_PING, FRAME_PUSH, FRAME_SIGN_ON_RESPONSE};

    type Responder = Arc<dyn Fn() -> Result<Value> + Send + Sync>;

    #[derive(Clone)]
    pub(crate) struct FakeHost {
        pub token: Arc<Mutex<Option<String>>>,
        pub sync: Arc<tokio::sync::Mutex<SyncData>>,
        pub logs: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
        pub emitted: Arc<Mutex<Vec<SyncData>>>,
        pub fetch: Responder,
        pub noop_ok: bool,
        pub opens: Arc<AtomicUsize>,
        /// Connections handed out by `open_conn`; `None` → fail to open.
        pub conns: Arc<Mutex<Vec<Conn>>>,
    }

    impl FakeHost {
        pub fn new() -> Self {
            Self {
                token: Arc::new(Mutex::new(Some("token".into()))),
                sync: Arc::default(),
                logs: Arc::default(),
                emitted: Arc::default(),
                fetch: Arc::new(|| Err(Error::internal("TestError", "unexpected fetchMyEvents"))),
                noop_ok: true,
                opens: Arc::default(),
                conns: Arc::default(),
            }
        }
        pub fn log_kinds(&self) -> Vec<String> {
            self.logs.lock().unwrap().iter().map(|(k, _)| k.clone()).collect()
        }
    }

    impl PushHost for FakeHost {
        fn auth_token(&self) -> Option<String> {
            self.token.lock().unwrap().clone()
        }
        fn sync_state(&self) -> &tokio::sync::Mutex<SyncData> {
            &self.sync
        }
        fn emit_sync(&self, data: SyncData) {
            self.emitted.lock().unwrap().push(data);
        }
        fn log(&self, kind: &str, data: serde_json::Value) {
            self.logs.lock().unwrap().push((kind.to_string(), data));
        }
        fn open_conn(&self, _path: &str) -> Result<Conn> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            self.conns.lock().unwrap().pop().ok_or_else(|| Error::internal("TestError", "connection failed"))
        }
        fn fetch_my_events(&self, _args: Value) -> BoxFuture<'static, Result<Value>> {
            let r = (self.fetch)();
            async move { r }.boxed()
        }
        fn noop(&self) -> BoxFuture<'static, Result<()>> {
            let ok = self.noop_ok;
            async move { if ok { Ok(()) } else { Err(Error::internal("TimeoutError", "noop timed out")) } }.boxed()
        }
    }

    /// A connection whose response body is fed from the returned sender.
    pub(crate) fn fake_conn() -> (Conn, mpsc::UnboundedReceiver<Bytes>, mpsc::UnboundedSender<Result<Bytes>>) {
        let (w, written) = ConnWriter::channel();
        let (tx, rx) = mpsc::unbounded_channel::<Result<Bytes>>();
        let body = futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|c| (c, rx)) }).boxed();
        (Conn::from_parts(w, async move { Ok(body) }.boxed()), written, tx)
    }

    fn square_push(sub_id: i64) -> PushFrame {
        let payload = thrift::write_struct(&[(1, TValue::I64(sub_id))], Protocol::Compact).unwrap();
        PushFrame { push_type: 0, service_type: 3, push_id: 1, payload }
    }

    fn manager(host: &FakeHost) -> (ConnManager<FakeHost>, Arc<Hub>) {
        let hub = Arc::new(Hub::default());
        (ConnManager::new(host.clone(), hub.clone()), hub)
    }

    #[tokio::test]
    async fn ping_reports_failed_noop() {
        let mut host = FakeHost::new();
        host.noop_ok = false;
        let (m, hub) = manager(&host);
        let _rx = hub.subscribe_raw(Kind::Talk);
        let (w, _written) = ConnWriter::channel();
        m.on_ping(&w, 3).await;
        assert_eq!(host.log_kinds(), vec!["[LEGY/PUSH] noop failed"]);
        assert!(host.logs.lock().unwrap()[0].1["error"].as_str().unwrap().contains("TimeoutError"));
    }

    #[tokio::test]
    async fn ping_reconnects_on_new_token() {
        let host = FakeHost::new();
        let (m, hub) = manager(&host);
        let _rx = hub.subscribe_raw(Kind::Talk);
        m.state.lock().unwrap().auth_token = Some("old".into());
        let (w, _written) = ConnWriter::channel();
        m.on_ping(&w, 1).await;
        assert!(host.log_kinds().is_empty());
        m.on_ping(&w, 3).await;
        assert_eq!(host.log_kinds(), vec!["[LEGY/PUSH] renew push conn for new authToken..."]);
        tokio::time::timeout(Duration::from_millis(100), w.closed()).await.unwrap();
    }

    #[tokio::test]
    async fn ping_closes_when_nobody_listens() {
        let host = FakeHost::new();
        let (m, _hub) = manager(&host);
        let (w, _written) = ConnWriter::channel();
        m.on_ping(&w, 1).await;
        tokio::time::timeout(Duration::from_millis(100), w.closed()).await.unwrap();
    }

    #[tokio::test]
    async fn push_reports_failed_fetch_my_events() {
        let host = FakeHost::new();
        host.sync.lock().await.square = Some("before".into());
        let (m, _hub) = manager(&host);
        m.on_push_response(square_push(1)).await;
        assert_eq!(host.log_kinds(), vec!["[LEGY/PUSH] _OnPushResponse", "[LEGY/PUSH] fetchMyEvents failed"]);
        // The sync state is left as it was, so the next push retries from there.
        assert_eq!(host.sync.lock().await.square.as_deref(), Some("before"));
        assert_eq!(m.subscription_id(), 1);
        assert!(host.emitted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn push_delivers_events_when_fetch_succeeds() {
        let mut host = FakeHost::new();
        host.fetch = Arc::new(|| {
            Ok(tv!({"events": [{"type": "NOTIFICATION_MESSAGE"}], "syncToken": "after", "subscription": {"subscriptionId": 42}}))
        });
        let (m, hub) = manager(&host);
        let mut rx = hub.subscribe_raw(Kind::Square);
        m.on_push_response(square_push(1)).await;
        let ev = rx.recv().await.unwrap().unwrap();
        assert_eq!(ev["type"].as_str(), Some("NOTIFICATION_MESSAGE"));
        assert_eq!(host.sync.lock().await.square.as_deref(), Some("after"));
        assert_eq!(m.subscription_id(), 42);
        assert_eq!(host.emitted.lock().unwrap().len(), 1);
        assert_eq!(host.log_kinds(), vec!["[LEGY/PUSH] _OnPushResponse", "[LEGY/PUSH] SQ_fetchMyEvents"]);
    }

    #[test]
    fn sign_on_request_ids_increment() {
        let host = FakeHost::new();
        let (m, _) = manager(&host);
        let (w, mut written) = ConnWriter::channel();
        let args = talk_sync_args(&SyncData::default());
        assert_eq!(m.build_and_send_sign_on_request(&w, 8, &args).unwrap(), 1);
        assert_eq!(m.build_and_send_sign_on_request(&w, 8, &args).unwrap(), 2);
        let first = written.try_recv().unwrap();
        assert_eq!(first[2], 2); // sign-on frame
        assert_eq!(&first[3..6], &[0, 1, 8]);
        let req_len = u16::from_be_bytes([first[7], first[8]]) as usize;
        assert_eq!(first.len(), 9 + req_len);
        let msg = thrift::read_message(&first[9..], Protocol::Compact).unwrap();
        assert_eq!(msg.name, "sync");
        let decoded = thrift::decode_struct(Schema::line(), Some("sync_args"), &msg.body);
        assert_eq!(decoded["request"]["count"].as_i64(), Some(100));
    }

    fn compact_result(method: &str, fields: Vec<(i16, TValue)>) -> Vec<u8> {
        // A reply message carries the result struct as its body.
        thrift::write_message(method, &fields, Protocol::Compact).unwrap()
    }

    fn sign_on_response(id: u16, payload: &[u8]) -> Vec<u8> {
        let mut dd = (id | 0x8000).to_be_bytes().to_vec();
        dd.extend_from_slice(payload);
        request_packet(FRAME_SIGN_ON_RESPONSE, &dd)
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<Bytes>) -> Vec<Bytes> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    #[tokio::test]
    async fn init_and_read_full_flow() {
        let host = FakeHost::new();
        let (m, hub) = manager(&host);
        let mut talk = hub.subscribe_raw(Kind::Talk);
        let mut square = hub.subscribe_raw(Kind::Square);
        let (conn, mut written, body) = fake_conn();
        let task = tokio::spawn({
            let m = m.clone();
            async move { m.init_and_read(conn, &[3, 8]).await }
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        let sent = drain(&mut written);
        assert_eq!(sent[0].as_ref(), &status_packet(0, 30)[..]);
        assert_eq!(&sent[1][3..6], &[0, 1, 3]);
        assert_eq!(&sent[2][3..6], &[0, 2, 8]);

        // Square sign-on response (request 1).
        let sq = compact_result(
            "fetchMyEvents",
            vec![(
                0,
                TValue::Struct(vec![
                    (1, TValue::Struct(vec![(1, TValue::I64(77))])),
                    (2, TValue::List(crate::thrift::TType::Struct, vec![TValue::Struct(vec![(2, TValue::I64(5))])])),
                    (3, TValue::string("tok1")),
                ]),
            )],
        );
        // Talk sign-on response (request 2), TMoreCompact: fullSyncResponse.nextRevision = 5.
        let talk_tmc = [0x82, 0x21, 0x01, 0x02, 0xdd, 0x00, 0x00, 0x01, 0x04, 0x04, 0x0a];
        let mut chunk = request_packet(FRAME_PING, &[2, 0, 1]);
        chunk.extend(sign_on_response(1, &sq));
        chunk.extend(sign_on_response(2, &talk_tmc));
        // Split mid-packet to exercise reassembly.
        body.send(Ok(Bytes::copy_from_slice(&chunk[..10]))).unwrap();
        body.send(Ok(Bytes::copy_from_slice(&chunk[10..]))).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        let ev = square.recv().await.unwrap().unwrap();
        assert_eq!(ev["createdTime"].as_i64(), Some(5));
        assert!(talk.try_recv().is_err());
        {
            let sync = host.sync.lock().await;
            assert_eq!(sync.square.as_deref(), Some("tok1"));
            assert_eq!(sync.talk.revision, 5);
        }
        assert_eq!(m.subscription_id(), 77);
        assert_eq!(host.emitted.lock().unwrap().len(), 2);

        let sent = drain(&mut written);
        assert_eq!(sent[0].as_ref(), &[0, 3, 1, 1, 0, 1]); // ping ack
        // Next talk sync request (id 3) carries the new revision.
        assert_eq!(&sent[1][3..6], &[0, 3, 8]);
        let msg = thrift::read_message(&sent[1][9..], Protocol::Compact).unwrap();
        let args = thrift::decode_struct(Schema::line(), Some("sync_args"), &msg.body);
        assert_eq!(args["request"]["lastRevision"].as_i64(), Some(5));

        // An ack-required push frame is acked.
        let push = request_packet(FRAME_PUSH, &[2, 5, 0, 0, 0, 9]);
        body.send(Ok(Bytes::from(push))).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let sent = drain(&mut written);
        assert_eq!(sent[0].as_ref(), &[0, 6, 4, 1, 5, 0, 0, 0, 9]);

        drop(body);
        task.await.unwrap().unwrap();
        assert!(host.log_kinds().iter().any(|k| k.starts_with("[LEGY/PUSH] CONN died")));
    }

    #[tokio::test]
    async fn init_and_read_reports_transport_failure() {
        let host = FakeHost::new();
        let (m, _) = manager(&host);
        let (w, _written) = ConnWriter::channel();
        let conn = Conn::from_parts(w, async { Err(Error::internal("LegyPusherError", "fetch failed")) }.boxed());
        assert!(m.init_and_read(conn, &[3, 8]).await.is_err());
    }

    #[tokio::test]
    async fn bad_frame_ends_connection_with_error() {
        let host = FakeHost::new();
        let (m, _) = manager(&host);
        let (conn, _written, body) = fake_conn();
        body.send(Ok(Bytes::from(request_packet(9, &[0])))).unwrap();
        assert!(m.init_and_read(conn, &[]).await.is_err());
    }

    #[test]
    fn apply_sync_response_tracks_revisions() {
        let mut sync = SyncData::default();
        let res = tv!({"operationResponse": {
            "operations": [{"revision": 10}, {"revision": 11}],
            "globalEvents": {"lastRevision": 7},
            "individualEvents": {"lastRevision": 0},
        }});
        let ops = apply_sync_response(&mut sync, &res);
        assert_eq!(ops.len(), 2);
        assert_eq!(sync.talk.revision, 11);
        assert_eq!(sync.talk.global_rev, 7);
        assert_eq!(sync.talk.individual_rev, 0);
        apply_sync_response(&mut sync, &tv!({}));
        assert_eq!(sync.talk.revision, 11);
    }
}
