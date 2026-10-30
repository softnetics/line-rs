//! Offline tests against a local mock HTTP server.

use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::home::{HomeProfileQuery, PageInfoQuery, UpdateHomeCover};
use super::keep::{KeepCreate, KeepUpdate};
use super::note::SearchNote;
use super::translate::TranslateText;
use super::*;

#[derive(Debug, Clone, Default)]
pub(crate) struct Seen {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn header(&self, k: &str) -> Option<&str> {
        self.headers.iter().find(|(hk, _)| hk.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// Serves `n` requests, answering each with `(status, body)`; records them.
pub(crate) async fn serve(responses: Vec<(u16, &'static str)>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rec = seen.clone();
    tokio::spawn(async move {
        for (status, reply) in responses {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let head_end = loop {
                let n = sock.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break p + 4;
                }
                if n == 0 {
                    break buf.len();
                }
            };
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let mut lines = head.lines();
            let first: Vec<String> = lines.next().unwrap_or_default().split(' ').map(str::to_string).collect();
            let headers: Vec<(String, String)> = lines
                .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())))
                .collect();
            let len: usize =
                headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
            let mut body = buf[head_end..].to_vec();
            while body.len() < len {
                let n = sock.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..n]);
            }
            rec.lock().unwrap().push(Seen {
                method: first.first().cloned().unwrap_or_default(),
                target: first.get(1).cloned().unwrap_or_default(),
                headers,
                body,
            });
            let resp = format!(
                "HTTP/1.1 {status} X\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{reply}",
                reply.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.shutdown().await.ok();
        }
    });
    (format!("http://{addr}"), seen)
}

fn rest(base: &str) -> RestClient {
    let c = crate::BaseClient::new(crate::Device::DesktopMac).unwrap();
    c.set_auth_token(Some("ACCESS".into()));
    c.set_profile(Some(crate::tv!({"mid": "u-self", "regionCode": "TH"})));
    let r = c.rest().with_base_url(base);
    for ch in [channels::KEEP, channels::MYHOME, channels::TIMELINE] {
        r.set_channel_token(ch, format!("TOKEN-{ch}"));
    }
    r
}

const OK: (u16, &str) = (200, r#"{"code":0,"result":{}}"#);

#[test]
fn urls_and_prefixes() {
    let req = RestRequest::get(RestPrefix::Keep, keep::GET, RestAuth::Access).query("contentId", "a b&c");
    assert_eq!(
        req.url("https://legy.line-apps.com").unwrap().as_str(),
        "https://legy.line-apps.com/kp/api/v27/keep/get.json?contentId=a+b%26c"
    );
    assert_eq!(RestPrefix::note_for("s1"), RestPrefix::SquareNote);
    assert_eq!(RestPrefix::note_for("c1"), RestPrefix::Note);
    assert_eq!(RestPrefix::PagePoker.as_str(), "/PAGEPOKER");
    assert_eq!(RestPrefix::Translation.as_str(), "/ds");
}

#[tokio::test]
async fn requires_login() {
    let c = crate::BaseClient::new(crate::Device::DesktopMac).unwrap();
    let err = c.rest().keep().init().await.unwrap_err();
    assert!(err.to_string().contains("login"), "{err}");
}

#[tokio::test]
async fn keep_requests() {
    let (base, seen) = serve(vec![OK, OK, OK, OK, OK, (500, "")]).await;
    let r = rest(&base);
    let k = r.keep();
    k.get_content("C1").await.unwrap();
    k.sync(7, Some(100), Some("KEEP-HOME")).await.unwrap();
    k.create(KeepCreate { client_id: Some("cli".into()), content_data: Some(json!({"x": 1})), ..Default::default() })
        .await
        .unwrap();
    k.update(KeepUpdate { content_id: "C1".into(), revision: Some(3), force: Some(true), ..Default::default() })
        .await
        .unwrap();
    k.pin(&["C1", "C2"]).await.unwrap();
    let err = k.delete(&["C1"]).await.unwrap_err();
    assert!(err.to_string().contains("/kp/api/v27/keep/delete.json HTTP 500"), "{err}");

    let s = seen.lock().unwrap();
    assert_eq!((s[0].method.as_str(), s[0].target.as_str()), ("POST", "/kp/api/v27/keep/get.json?contentId=C1"));
    assert_eq!(s[0].header("x-lhm"), Some("GET"));
    assert!(s[0].body.is_empty());
    assert_eq!(s[0].header("x-line-channeltoken"), Some("TOKEN-1433572998"));
    assert_eq!(s[0].header("x-line-access"), Some("ACCESS"));
    assert_eq!(s[0].header("x-line-mid"), Some("u-self"));
    assert_eq!(s[0].header("x-lsr"), Some("TH"));
    assert_eq!(s[0].header("accept"), Some("application/json"));
    assert_eq!(s[1].target, "/kp/api/v27/keep/sync.json?revision=7&limit=100");
    assert_eq!(s[1].header("x-source"), Some("KEEP-HOME"));
    assert_eq!((s[2].method.as_str(), s[2].header("x-lhm")), ("POST", Some("POST")));
    assert_eq!(s[2].json(), json!({"clientId": "cli", "contentData": {"x": 1}}));
    assert_eq!(s[3].target, "/kp/api/v27/keep/update.json?force=true");
    assert_eq!(s[3].json(), json!({"contentId": "C1", "revision": 3}));
    assert_eq!(s[4].target, "/kp/api/v27/keep/contents/pin.json");
    assert_eq!(s[4].json(), json!({"contentIds": ["C1", "C2"]}));
    assert_eq!(s[5].json(), json!({"contentIds": ["C1"]}));
}

#[tokio::test]
async fn note_requests() {
    let (base, seen) = serve(vec![OK, OK, OK, OK, OK, OK]).await;
    let r = rest(&base);
    let n = r.note();
    n.cancel_like("c1", "P1").await.unwrap();
    n.list_comments("s1", "P1", None, Some(20), None).await.unwrap();
    n.search(SearchNote { home_id: "c1".into(), query: "hi".into(), post_limit: Some(5), ..Default::default() })
        .await
        .unwrap();
    n.search(SearchNote { home_id: "s1".into(), query: "hi".into(), ..Default::default() }).await.unwrap();
    n.create_announcement("s1", "P1").await.unwrap();
    n.delete_comment("c1", "CM", Some("u2")).await.unwrap();

    let s = seen.lock().unwrap();
    assert_eq!(s[0].target, "/ext/note/nt/api/v57/like/cancel.json?homeId=c1&contentId=P1");
    assert_eq!(s[0].header("x-line-channeltoken"), Some("TOKEN-1341209850"));
    assert_eq!(s[1].target, "/sn/api/v57/comment/getList.json?homeId=s1&contentId=P1&limit=20");
    assert_eq!(
        (s[2].method.as_str(), s[2].target.as_str()),
        ("POST", "/ext/note/nt/api/v57/search/note.json?homeId=c1")
    );
    assert_eq!(s[2].json(), json!({"query": "hi", "homeId": "c1", "postLimit": 5}));
    assert_eq!((s[3].method.as_str(), s[3].target.as_str()), ("POST", "/sn/api/v57/search/note?query=hi&homeId=s1"));
    assert_eq!(s[3].header("x-lhm"), Some("GET"));
    assert_eq!(s[4].target, "/sn/api/v57/announce/create.json?homeId=s1");
    assert_eq!(s[4].json(), json!({"postId": "P1"}));
    assert_eq!(s[5].target, "/ext/note/nt/api/v57/comment/delete.json?homeId=c1&commentId=CM&actorId=u2");
}

#[tokio::test]
async fn home_requests() {
    let (base, seen) = serve(vec![OK, OK, OK, OK, OK]).await;
    let r = rest(&base);
    let h = r.home();
    h.profile(HomeProfileQuery { home_id: "u1".into(), ..Default::default() }).await.unwrap();
    h.profile(HomeProfileQuery { home_id: "c1".into(), ..Default::default() }).await.unwrap();
    h.update_cover(UpdateHomeCover { home_id: "u1".into(), cover_object_id: Some("O".into()), ..Default::default() })
        .await
        .unwrap();
    h.delete_cover("u1", None).await.unwrap();
    h.page_info(PageInfoQuery {
        url: "https://example.com/?a=1".into(),
        phishing_check: Some(true),
        ..Default::default()
    })
    .await
    .unwrap();

    let s = seen.lock().unwrap();
    assert_eq!(s[0].target, "/hm/api/v1/home/profile?homeId=u1");
    assert_eq!(s[1].target, "/hm/api/v1/home/groupprofile?homeId=c1");
    assert_eq!(s[2].json(), json!({"homeId": "u1", "coverObjectId": "O"}));
    assert_eq!((s[3].method.as_str(), s[3].target.as_str()), ("DELETE", "/hm/api/v1/home/cover"));
    assert_eq!(s[3].header("x-lhm"), Some("DELETE"));
    assert_eq!(
        s[4].target,
        "/PAGEPOKER/api/v2/pageinfo/get.json?url=https%3A%2F%2Fexample.com%2F%3Fa%3D1&phishingCheck=true"
    );
}

#[tokio::test]
async fn translate_request() {
    let (base, seen) = serve(vec![(200, r#"{"translatedText":"konnichiwa"}"#)]).await;
    let r = rest(&base);
    let res = r.translate(&TranslateText::new("hello", "ja"), None).await.unwrap();
    assert_eq!(res["translatedText"], "konnichiwa");
    let s = seen.lock().unwrap();
    assert_eq!((s[0].method.as_str(), s[0].target.as_str()), ("POST", "/ds/translate/legyTransAPI.nhn"));
    assert_eq!(s[0].header("x-line-translate-from"), Some("line_desktop"));
    assert_eq!(s[0].header("x-line-channeltoken"), Some("TOKEN-1341209950"));
    assert_eq!(s[0].header("x-line-access"), Some("ACCESS"));
    assert_eq!(s[0].json()["originalText"], "hello");
}
