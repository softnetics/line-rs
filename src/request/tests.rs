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
}

struct Reply {
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

fn reply(method: &str, fields: Vec<(i16, TValue)>) -> Reply {
    Reply { headers: Vec::new(), body: thrift::write_message(method, &fields, Protocol::Compact).unwrap() }
}

/// Serve `replies` in order (one connection each) and record requests.
async fn serve(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        for r in replies {
            let (mut sock, _) = listener.accept().await.unwrap();
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
                    k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
                })
            };
            let len: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
            while buf.len() < head_end + len {
                let n = sock.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let path = head.lines().next().unwrap_or_default().split(' ').nth(1).unwrap_or_default().to_string();
            log.lock().unwrap().push(Seen { path, access: header("x-line-access") });
            let mut out = format!("HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: {}\r\n", r.body.len());
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

/// Opt-in session extras (off by default to match linejs).
fn extras() -> crate::base::Config {
    crate::base::Config {
        proactive_refresh: true,
        report_refreshed_token: true,
        apply_next_access: true,
        ..Default::default()
    }
}

fn client_with(endpoint: &str, storage: Arc<MemoryStorage>, config: crate::base::Config) -> BaseClient {
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
    reply("getProfile", vec![(0, TValue::Struct(vec![(1, TValue::string("u1"))]))])
}

async fn get_profile(c: &BaseClient) -> Result<Value> {
    c.call("getProfile", crate::tv!({"syncReason": "INTERNAL"}), "/S4", Protocol::Compact).await
}

#[test]
fn endpoint_url_keeps_explicit_scheme() {
    assert_eq!(endpoint_url("legy.line-apps.com", "/S4"), "https://legy.line-apps.com/S4");
    assert_eq!(endpoint_url("http://127.0.0.1:1/", "/S4"), "http://127.0.0.1:1/S4");
}

#[tokio::test]
async fn next_access_header_only_reported_by_default() {
    let mut first = profile_reply();
    first.headers.push(("x-line-next-access", "NEW".into()));
    let (endpoint, seen) = serve(vec![first, profile_reply()]).await;
    let c = client(&endpoint, Arc::new(MemoryStorage::new()));
    let mut events = c.subscribe();
    get_profile(&c).await.unwrap();
    assert_eq!(c.auth_token().as_deref(), Some("OLD"));
    assert!(matches!(events.try_recv(), Ok(ClientEvent::UpdateAuthToken(t)) if t == "NEW"));
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

#[tokio::test]
async fn proactive_refresh_when_expire_reached() {
    let storage = Arc::new(MemoryStorage::new());
    let (endpoint, seen) = serve(vec![
        reply(
            "refresh",
            vec![(
                0,
                TValue::Struct(vec![
                    (1, TValue::string("FRESH")),
                    (2, TValue::I64(3600)),
                    (4, TValue::I64(crate::base::session_now())),
                    (5, TValue::string("R2")),
                ]),
            )],
        ),
        reply("reportRefreshedAccessToken", vec![(0, TValue::Struct(vec![]))]),
        profile_reply(),
        profile_reply(),
    ])
    .await;
    let c = client_with(&endpoint, storage.clone(), extras());
    storage.set("refreshToken", "R1".into()).await.unwrap();
    c.set_token_expire(crate::base::session_now() - 10).await.unwrap();

    get_profile(&c).await.unwrap();
    assert_eq!(c.auth_token().as_deref(), Some("FRESH"));
    assert_eq!(storage.get_string("refreshToken").await.unwrap().as_deref(), Some("R2"));
    assert!(c.token_expire().await.unwrap().unwrap() > crate::base::session_now() + 3000);
    // Not due any more: the next request goes straight out.
    get_profile(&c).await.unwrap();

    let seen = seen.lock().unwrap();
    let paths: Vec<&str> = seen.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths, [TOKEN_REFRESH_PATH, TOKEN_REFRESH_PATH, "/S4", "/S4"]);
    assert_eq!(seen[1].access.as_deref(), Some("FRESH"));
    assert_eq!(seen[2].access.as_deref(), Some("FRESH"));
}

#[tokio::test]
async fn not_due_token_is_not_refreshed() {
    let storage = Arc::new(MemoryStorage::new());
    let (endpoint, seen) = serve(vec![profile_reply()]).await;
    let c = client(&endpoint, storage.clone());
    storage.set("refreshToken", "R1".into()).await.unwrap();
    storage.set("expire", (crate::base::session_now() + 3600).into()).await.unwrap();
    get_profile(&c).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(c.auth_token().as_deref(), Some("OLD"));
}

#[tokio::test]
async fn kicked_out_session_stops_the_client() {
    let (endpoint, seen) = serve(vec![reply(
        "getProfile",
        vec![(1, TValue::Struct(vec![(1, TValue::I32(8)), (2, TValue::string("device removed"))]))],
    )])
    .await;
    let c = client(&endpoint, Arc::new(MemoryStorage::new()));
    let mut events = c.subscribe();
    let err = get_profile(&c).await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_AUTHORIZED_DEVICE"));
    assert_eq!(c.kicked_out(), Some(LogoutReason::NotAuthorizedDevice));
    assert!(c.is_disabled());
    assert!(c.auth_token().is_none());
    assert!(matches!(events.try_recv(), Ok(ClientEvent::KickedOut(LogoutReason::NotAuthorizedDevice))));
    assert!(matches!(events.try_recv(), Ok(ClientEvent::End(_))));

    let err = get_profile(&c).await.unwrap_err();
    assert!(err.to_string().starts_with("LoggedOut:"), "{err}");
    assert_eq!(seen.lock().unwrap().len(), 1);
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
