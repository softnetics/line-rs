//! Offline tests of the request pipeline against a local plain-HTTP server:
//! token rotation, proactive refresh, kick-out and host failover.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::base::{ClientEvent, LogoutReason};
use crate::device::Device;
use crate::storage::{MemoryStorage, Storage};

#[derive(Debug, Clone)]
struct Seen {
    path: String,
    access: Option<String>,
    /// Thrift method name of the request body.
    method: String,
}

struct Reply {
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

fn reply(method: &str, fields: Vec<(i16, TValue)>) -> Reply {
    Reply {
        headers: Vec::new(),
        body: thrift::write_message(method, &fields, Protocol::Compact).unwrap(),
    }
}

/// Serve `replies` in order (one connection each) and record requests.
async fn serve(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let replies = Mutex::new(replies.into_iter());
    serve_with(move |_| replies.lock().unwrap().next().expect("no reply left")).await
}

/// Answer every request (one connection each) with `answer` and record it.
async fn serve_with(
    answer: impl Fn(&Seen) -> Reply + Send + 'static,
) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let head_end = loop {
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
                let n = sock.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break buf.len();
                }
                buf.extend_from_slice(&chunk[..n]);
            };
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let header = |name: &str| {
                head.lines().find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.trim()
                        .eq_ignore_ascii_case(name)
                        .then(|| v.trim().to_string())
                })
            };
            let len: usize = header("content-length")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            while buf.len() < head_end + len {
                let n = sock.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let path = head
                .lines()
                .next()
                .unwrap_or_default()
                .split(' ')
                .nth(1)
                .unwrap_or_default()
                .to_string();
            let method = thrift::read_message(&buf[head_end..], Protocol::Compact)
                .map(|m| m.name)
                .unwrap_or_default();
            let request = Seen {
                path,
                access: header("x-line-access"),
                method,
            };
            log.lock().unwrap().push(request.clone());
            let r = answer(&request);
            let mut out = format!(
                "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: {}\r\n",
                r.body.len()
            );
            for (k, v) in &r.headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str("\r\n");
            sock.write_all(out.as_bytes()).await.unwrap();
            sock.write_all(&r.body).await.unwrap();
            sock.shutdown().await.ok();
        }
    });
    (format!("http://{addr}"), seen)
}

fn client(endpoint: &str, storage: Arc<MemoryStorage>) -> BaseClient {
    client_with(endpoint, storage, crate::base::Config::default())
}

/// Adopt `X-Line-Next-Access` (off by default).
fn extras() -> crate::base::Config {
    crate::base::Config {
        apply_next_access: true,
        ..Default::default()
    }
}

fn client_with(
    endpoint: &str,
    storage: Arc<MemoryStorage>,
    config: crate::base::Config,
) -> BaseClient {
    let c = BaseClient::builder(Device::DesktopMac)
        .config(config)
        .endpoint(endpoint)
        .legy(LegyMode::Never)
        .storage_arc(storage)
        .build()
        .unwrap();
    c.set_auth_token(Some("OLD".into()));
    c
}

fn profile_reply() -> Reply {
    reply(
        "getProfile",
        vec![(0, TValue::Struct(vec![(1, TValue::string("u1"))]))],
    )
}

async fn get_profile(c: &BaseClient) -> Result<Value> {
    c.call(
        "getProfile",
        crate::tv!({"syncReason": "INTERNAL"}),
        "/S4",
        Protocol::Compact,
    )
    .await
}

#[test]
fn endpoint_url_keeps_explicit_scheme() {
    assert_eq!(
        endpoint_url("legy.line-apps.com", "/S4"),
        "https://legy.line-apps.com/S4"
    );
    assert_eq!(
        endpoint_url("http://127.0.0.1:1/", "/S4"),
        "http://127.0.0.1:1/S4"
    );
}

#[tokio::test]
async fn next_access_header_ignored_by_default() {
    let mut first = profile_reply();
    first.headers.push(("x-line-next-access", "NEW".into()));
    let (endpoint, seen) = serve(vec![first, profile_reply()]).await;
    let c = client(&endpoint, Arc::new(MemoryStorage::new()));
    let mut events = c.subscribe();
    get_profile(&c).await.unwrap();
    assert_eq!(c.auth_token().as_deref(), Some("OLD"));
    // A token that is not adopted is not reported either.
    assert!(events.try_recv().is_err());
    get_profile(&c).await.unwrap();
    assert_eq!(seen.lock().unwrap()[1].access.as_deref(), Some("OLD"));
}

#[tokio::test]
async fn next_access_header_rotates_token() {
    let mut first = profile_reply();
    first.headers.push(("x-line-next-access", "NEW".into()));
    let (endpoint, seen) = serve(vec![first, profile_reply()]).await;
    let c = client_with(&endpoint, Arc::new(MemoryStorage::new()), extras());
    let mut events = c.subscribe();
    get_profile(&c).await.unwrap();
    assert_eq!(c.auth_token().as_deref(), Some("NEW"));
    assert!(matches!(events.try_recv(), Ok(ClientEvent::UpdateAuthToken(t)) if t == "NEW"));
    get_profile(&c).await.unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].access.as_deref(), Some("OLD"));
    assert_eq!(seen[1].access.as_deref(), Some("NEW"));
}

fn refresh_ok(token: &str) -> Reply {
    reply(
        "refresh",
        vec![(
            0,
            TValue::Struct(vec![
                (1, TValue::string(token)),
                (2, TValue::I64(3600)),
                (
                    3,
                    TValue::Struct(vec![
                        (1, TValue::I64(5)),
                        (2, TValue::I64(20)),
                        (3, TValue::Double(2.0)),
                        (4, TValue::Double(0.0)),
                    ]),
                ),
                (4, TValue::I64(crate::base::session_now())),
                (5, TValue::string("R2")),
            ]),
        )],
    )
}

/// `refresh_result { e: AccessTokenRefreshException { errorCode } }`.
fn refresh_error(code: i32) -> Reply {
    reply(
        "refresh",
        vec![(1, TValue::Struct(vec![(1, TValue::I32(code))]))],
    )
}

fn by_method(seen: &Seen) -> Reply {
    match seen.method.as_str() {
        "refresh" => refresh_ok("FRESH"),
        "reportRefreshedAccessToken" => reply(
            "reportRefreshedAccessToken",
            vec![(0, TValue::Struct(vec![]))],
        ),
        _ => profile_reply(),
    }
}

async fn wait_for(cond: impl Fn() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !cond() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition not reached");
}

#[tokio::test]
async fn proactive_refresh_runs_in_the_background_when_expire_passed() {
    let storage = Arc::new(MemoryStorage::new());
    let (endpoint, seen) = serve_with(by_method).await;
    let c = client(&endpoint, storage.clone());
    storage.set("refreshToken", "R1".into()).await.unwrap();
    c.set_token_expire(crate::base::session_now() - 10)
        .await
        .unwrap();
    let mut events = c.subscribe();

    // The request that notices goes out with the current token.
    get_profile(&c).await.unwrap();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await,
        Ok(Ok(ClientEvent::UpdateAuthToken(t))) if t == "FRESH"
    ));
    assert_eq!(c.auth_token().as_deref(), Some("FRESH"));
    assert_eq!(
        storage.get_string("refreshToken").await.unwrap().as_deref(),
        Some("R2")
    );
    assert!(c.token_expire().await.unwrap().unwrap() > crate::base::session_now() + 3000);
    // The server's retry policy is kept for the next refresh.
    assert_eq!(
        storage
            .get(REFRESH_RETRY_POLICY_KEY)
            .await
            .unwrap()
            .unwrap()["initialDelayInMillis"],
        5
    );
    // The new token is reported (in the background too).
    wait_for(|| {
        seen.lock()
            .unwrap()
            .iter()
            .any(|s| s.method == "reportRefreshedAccessToken")
    })
    .await;
    // Not due any more: the next request goes straight out.
    get_profile(&c).await.unwrap();

    let seen = seen.lock().unwrap();
    let profiles: Vec<_> = seen.iter().filter(|s| s.method == "getProfile").collect();
    assert_eq!(profiles[0].access.as_deref(), Some("OLD"));
    assert_eq!(profiles[1].access.as_deref(), Some("FRESH"));
    assert_eq!(seen.iter().filter(|s| s.method == "refresh").count(), 1);
    let report = seen
        .iter()
        .find(|s| s.method == "reportRefreshedAccessToken")
        .unwrap();
    assert_eq!(report.path, TOKEN_REFRESH_PATH);
    assert_eq!(report.access.as_deref(), Some("FRESH"));
}

#[tokio::test]
async fn proactive_refresh_can_be_turned_off() {
    let storage = Arc::new(MemoryStorage::new());
    let (endpoint, seen) = serve_with(by_method).await;
    let config = crate::base::Config {
        proactive_refresh: false,
        ..Default::default()
    };
    let c = client_with(&endpoint, storage.clone(), config);
    storage.set("refreshToken", "R1".into()).await.unwrap();
    c.set_token_expire(crate::base::session_now() - 10)
        .await
        .unwrap();
    get_profile(&c).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(c.auth_token().as_deref(), Some("OLD"));
}

#[tokio::test]
async fn refresh_retries_on_retry_required_with_the_saved_policy() {
    let storage = Arc::new(MemoryStorage::new());
    // 1001 = RETRY_REQUIRED
    let (endpoint, seen) = serve(vec![
        refresh_error(1001),
        refresh_error(1001),
        refresh_ok("FRESH"),
    ])
    .await;
    let config = crate::base::Config {
        report_refreshed_token: false,
        ..Default::default()
    };
    let c = client_with(&endpoint, storage.clone(), config);
    storage.set("refreshToken", "R1".into()).await.unwrap();
    storage
        .set(
            REFRESH_RETRY_POLICY_KEY,
            serde_json::json!({"initialDelayInMillis": 1, "maxDelayInMillis": 5,
                "multiplier": 1.5, "jitterRate": 0.1}),
        )
        .await
        .unwrap();
    c.try_refresh_token().await.unwrap();
    assert_eq!(c.auth_token().as_deref(), Some("FRESH"));
    assert_eq!(seen.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn refresh_gives_up_on_invalid_request() {
    let storage = Arc::new(MemoryStorage::new());
    // 1000 = INVALID_REQUEST
    let (endpoint, seen) = serve(vec![refresh_error(1000)]).await;
    let c = client(&endpoint, storage.clone());
    storage.set("refreshToken", "R1".into()).await.unwrap();
    let err = c.try_refresh_token().await.unwrap_err();
    assert!(err.to_string().contains("INVALID_REQUEST"), "{err}");
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(c.auth_token().as_deref(), Some("OLD"));
}

#[test]
fn refresh_retry_policy_defaults_and_parsing() {
    let d = DEFAULT_REFRESH_RETRY;
    let ms: Vec<u128> = (0..12).map(|a| d.delay(a, 0.0).as_millis()).collect();
    assert_eq!(
        ms,
        [
            1000, 1500, 2250, 3375, 5062, 7593, 11390, 17085, 25628, 38443, 57665, 60000
        ]
    );
    assert_eq!(d.delay(0, 1.0).as_millis(), 1100);
    let p = retry_policy_from_json(&serde_json::json!({
        "initialDelayInMillis": 200, "maxDelayInMillis": 800, "multiplier": 2.0, "jitterRate": 0.0
    }));
    assert_eq!(
        (
            p.initial.as_millis(),
            p.max.as_millis(),
            p.multiplier,
            p.jitter
        ),
        (200, 800, 2.0, 0.0)
    );
    // Missing or bad fields fall back to the defaults.
    assert_eq!(
        retry_policy_from_json(&serde_json::json!({"multiplier": 0.5, "jitterRate": 7})),
        d
    );
}

#[test]
fn retryable_refresh_errors() {
    let exc = |code: &str| Error::Request {
        method: "refresh".into(),
        path: TOKEN_REFRESH_PATH.into(),
        data: crate::tv!({"errorCode": code}),
    };
    assert!(is_retryable_refresh_error(&exc("RETRY_REQUIRED")));
    assert!(!is_retryable_refresh_error(&exc("INVALID_REQUEST")));
    assert!(is_retryable_refresh_error(&Error::internal(
        "TimeoutError",
        "x"
    )));
    assert!(!is_retryable_refresh_error(&Error::internal(
        "RefreshError",
        "x"
    )));
}

#[tokio::test]
async fn not_due_token_is_not_refreshed() {
    let storage = Arc::new(MemoryStorage::new());
    let (endpoint, seen) = serve(vec![profile_reply()]).await;
    let c = client(&endpoint, storage.clone());
    storage.set("refreshToken", "R1".into()).await.unwrap();
    storage
        .set("expire", (crate::base::session_now() + 3600).into())
        .await
        .unwrap();
    get_profile(&c).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(c.auth_token().as_deref(), Some("OLD"));
}

#[tokio::test]
async fn kicked_out_session_stops_the_client() {
    let (endpoint, seen) = serve(vec![reply(
        "getProfile",
        vec![(
            1,
            TValue::Struct(vec![
                (1, TValue::I32(8)),
                (2, TValue::string("device removed")),
            ]),
        )],
    )])
    .await;
    let c = client(&endpoint, Arc::new(MemoryStorage::new()));
    let mut events = c.subscribe();
    let err = get_profile(&c).await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_AUTHORIZED_DEVICE"));
    assert_eq!(c.kicked_out(), Some(LogoutReason::NotAuthorizedDevice));
    assert!(c.is_disabled());
    assert!(c.auth_token().is_none());
    assert!(matches!(
        events.try_recv(),
        Ok(ClientEvent::KickedOut(LogoutReason::NotAuthorizedDevice))
    ));
    assert!(matches!(events.try_recv(), Ok(ClientEvent::End(_))));

    let err = get_profile(&c).await.unwrap_err();
    assert!(err.to_string().starts_with("LoggedOut:"), "{err}");
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn authentication_failed_ends_a_secondary_session() {
    // TalkException { code: AUTHENTICATION_FAILED (1) }
    let (endpoint, _seen) = serve(vec![reply(
        "getProfile",
        vec![(1, TValue::Struct(vec![(1, TValue::I32(1))]))],
    )])
    .await;
    let c = client(&endpoint, Arc::new(MemoryStorage::new()));
    let err = get_profile(&c).await.unwrap_err();
    assert_eq!(err.code(), Some("AUTHENTICATION_FAILED"));
    assert_eq!(c.kicked_out(), Some(LogoutReason::AuthenticationFailed));
    assert!(c.is_disabled());
}

#[tokio::test]
async fn fails_over_to_backup_host_on_connect_error() {
    // A port nothing listens on.
    let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_addr = format!("http://{}", dead.local_addr().unwrap());
    drop(dead);
    let (live, seen) = serve(vec![profile_reply(), profile_reply()]).await;
    let c = BaseClient::builder(Device::DesktopMac)
        .endpoint(dead_addr.clone())
        .backup_endpoints([live.clone()])
        .legy(LegyMode::Never)
        .build()
        .unwrap();
    c.set_auth_token(Some("T".into()));
    assert_eq!(c.current_endpoint(), dead_addr);
    get_profile(&c).await.unwrap();
    assert_eq!(c.current_endpoint(), live);
    // Sticks to the working host.
    get_profile(&c).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[test]
fn x_lal_follows_the_configured_language() {
    let lal = |c: &BaseClient| {
        c.line_headers("getProfile")
            .into_iter()
            .find(|(k, _)| k == "x-lal")
            .map(|(_, v)| v)
    };
    let storage = || Arc::new(MemoryStorage::new());
    let default = client_with("legy.example", storage(), crate::base::Config::default());
    assert_eq!(lal(&default).as_deref(), Some("th_TH"));
    let japanese = crate::base::Config {
        language: "ja_JP".into(),
        ..Default::default()
    };
    assert_eq!(
        lal(&client_with("legy.example", storage(), japanese)).as_deref(),
        Some("ja_JP")
    );
}

/// `<method>_result { e: TalkException { code } }`.
fn talk_exception(method: &str, code: i32) -> Reply {
    reply(
        method,
        vec![(1, TValue::Struct(vec![(1, TValue::I32(code))]))],
    )
}

fn logout_client(endpoint: &str, storage: Arc<MemoryStorage>) -> BaseClient {
    let config = crate::base::Config {
        report_refreshed_token: false,
        ..Default::default()
    };
    let c = client_with(endpoint, storage, config);
    c.set_profile(Some(crate::tv!({"mid": "u1"})));
    c
}

fn assert_logged_out(c: &BaseClient, events: &mut tokio::sync::broadcast::Receiver<ClientEvent>) {
    assert!(c.is_disabled());
    assert!(c.auth_token().is_none());
    assert!(
        std::iter::from_fn(|| events.try_recv().ok()).any(|e| matches!(e, ClientEvent::End(_)))
    );
}

#[tokio::test]
async fn logout_uses_logout_v2() {
    let (endpoint, seen) = serve(vec![reply("logoutV2", vec![])]).await;
    let c = logout_client(&endpoint, Arc::new(MemoryStorage::new()));
    let mut events = c.subscribe();
    c.auth().logout().await.unwrap();
    assert_logged_out(&c, &mut events);
    let seen = seen.lock().unwrap();
    assert_eq!(
        (seen[0].path.as_str(), seen[0].method.as_str()),
        ("/AS4", "logoutV2")
    );
}

#[tokio::test]
async fn logout_refreshes_and_retries_on_must_refresh() {
    let storage = Arc::new(MemoryStorage::new());
    // 119 = MUST_REFRESH_V3_TOKEN
    let (endpoint, seen) = serve(vec![
        talk_exception("logoutV2", 119),
        refresh_ok("FRESH"),
        reply("logoutV2", vec![]),
    ])
    .await;
    let c = logout_client(&endpoint, storage.clone());
    storage.set("refreshToken", "R1".into()).await.unwrap();
    let mut events = c.subscribe();
    c.auth().logout().await.unwrap();
    assert_logged_out(&c, &mut events);
    let seen = seen.lock().unwrap();
    let calls: Vec<_> = seen
        .iter()
        .map(|s| (s.method.as_str(), s.access.as_deref()))
        .collect();
    assert_eq!(
        calls,
        [
            ("logoutV2", Some("OLD")),
            ("refresh", Some("OLD")),
            ("logoutV2", Some("FRESH"))
        ]
    );
}

#[tokio::test]
async fn logout_falls_back_to_logout_z() {
    // 20 = INTERNAL_ERROR
    let (endpoint, seen) = serve(vec![
        talk_exception("logoutV2", 20),
        reply("logoutZ", vec![]),
    ])
    .await;
    let c = logout_client(&endpoint, Arc::new(MemoryStorage::new()));
    let mut events = c.subscribe();
    c.auth().logout().await.unwrap();
    assert_logged_out(&c, &mut events);
    assert_eq!(seen.lock().unwrap()[1].path, "/RS4");
}

#[tokio::test]
async fn logout_z_refreshes_instead_of_counting_must_refresh_as_success() {
    let storage = Arc::new(MemoryStorage::new());
    let (endpoint, seen) = serve(vec![
        talk_exception("logoutZ", 119),
        refresh_ok("FRESH"),
        reply("logoutZ", vec![]),
    ])
    .await;
    let c = logout_client(&endpoint, storage.clone());
    storage.set("refreshToken", "R1".into()).await.unwrap();
    let mut events = c.subscribe();
    c.auth().logout_z().await.unwrap();
    assert_logged_out(&c, &mut events);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[2].access.as_deref(), Some("FRESH"));
}

#[tokio::test]
async fn logout_z_error_keeps_the_session() {
    let (endpoint, _seen) = serve(vec![talk_exception("logoutZ", 20)]).await;
    let c = logout_client(&endpoint, Arc::new(MemoryStorage::new()));
    let err = c.auth().logout_z().await.unwrap_err();
    assert_eq!(err.code(), Some("INTERNAL_ERROR"));
    assert!(!c.is_disabled());
    assert_eq!(c.auth_token().as_deref(), Some("OLD"));
}

#[tokio::test]
async fn logout_of_an_ended_session_counts_as_done() {
    // 8 = NOT_AUTHORIZED_DEVICE
    let (endpoint, seen) = serve(vec![talk_exception("logoutV2", 8)]).await;
    let c = logout_client(&endpoint, Arc::new(MemoryStorage::new()));
    let mut events = c.subscribe();
    c.auth().logout().await.unwrap();
    let ends = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|e| matches!(e, ClientEvent::End(_)))
        .count();
    assert_eq!(ends, 1);
    assert!(c.is_disabled());
    assert_eq!(seen.lock().unwrap().len(), 1);
}
