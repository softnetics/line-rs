//! Authenticated smoke tests against the real LINE server.
//!
//! These run with a real account and are written for single-account use:
//! by default every write goes to the account's own self-chat (Keep Memo,
//! the chat whose mid is your own mid). Nothing is sent to, blocked for, or
//! otherwise aimed at another user unless you opt in with
//! `LINE_PEER_MID=u...` **and** `LINE_ALLOW_PEER_TESTS=yes`.
//!
//! Read `tests/LIVE_TESTING.md` first. Short version:
//!
//! ```sh
//! LINE_TEST_ACCOUNT_CONFIRM=yes LINE_AUTH_TOKEN=... \
//!   cargo test --test live_auth read_only -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! * `--test-threads=1` is required: tests share one client and one
//!   request throttle (at least 5 s between API calls).
//! * Env: `LINE_TEST_ACCOUNT_CONFIRM=yes` (hard gate), `LINE_AUTH_TOKEN`
//!   (required), `LINE_TEST_DEVICE` (default `DESKTOPWIN`; must match the
//!   device the token was issued for), `LINE_GROUP_MID`, `LINE_PEER_MID` +
//!   `LINE_ALLOW_PEER_TESTS=yes`, `LINE_KEEP_CONTENT_ID`,
//!   `LINE_PRODUCT_REGION` (default `TH`), `LINE_ALLOW_KICKOUT_TEST=yes`.
//!   A missing optional input makes the test print `skipped: <reason>`
//!   and pass.
//! * No retries: the first error fails the test with the error printed.
//! * Some library calls make more than one request internally (E2EE key
//!   lookups, OBS upload + send, channel-token issue for REST). The throttle
//!   runs before each library call, so those sub-requests are not spaced.

use std::future::Future;
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use line_rs::client::{
    login_with_auth_token, Chat, Client, ClientEvent, InitOptions, ListenHandle, ListenOptions, MessageInput,
    ReactionType, TalkEvent, TalkMessage, User,
};
use line_rs::rest::home::PageInfoQuery;
use line_rs::storage::FileStorage;
use line_rs::{tv, Device};
use tokio::sync::{broadcast, Mutex, OnceCell};

const STORAGE_PATH: &str = "./storage.test.json";
/// Minimum spacing between API calls.
const MIN_GAP: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// One runtime for the whole binary so the client (and its push/listen
/// tasks) can be shared between tests.
static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("build tokio runtime")
});

/// Logged in once per process; a failed login is remembered, never retried.
static CLIENT: OnceCell<Result<Client, String>> = OnceCell::const_new();

/// When the last API call was started.
static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::const_new(None);

/// Run an async test body on the shared runtime, after the hard gate.
fn run<F: Future<Output = ()>>(body: F) {
    gate();
    RUNTIME.block_on(body);
}

/// Refuse to run unless the user explicitly confirmed.
fn gate() {
    if std::env::var("LINE_TEST_ACCOUNT_CONFIRM").as_deref() != Ok("yes") {
        panic!(
            "refusing to run: these tests act on a real LINE account. \
             Read tests/LIVE_TESTING.md, then set LINE_TEST_ACCOUNT_CONFIRM=yes"
        );
    }
}

/// Wait until at least [`MIN_GAP`] has passed since the previous call.
async fn throttle() {
    let mut last = LAST_REQUEST.lock().await;
    if let Some(at) = *last {
        let wait = MIN_GAP.saturating_sub(at.elapsed());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
    *last = Some(Instant::now());
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn skip(reason: &str) {
    println!("skipped: {reason}");
}

/// The other user to target, only when explicitly allowed.
fn peer() -> Option<String> {
    let mid = env("LINE_PEER_MID")?;
    (std::env::var("LINE_ALLOW_PEER_TESTS").as_deref() == Ok("yes")).then_some(mid)
}

const PEER_SKIP: &str = "targets another user; set LINE_PEER_MID and LINE_ALLOW_PEER_TESTS=yes";

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The shared, logged-in client.
async fn client() -> Client {
    let res = CLIENT
        .get_or_init(|| async {
            let token = env("LINE_AUTH_TOKEN").ok_or("LINE_AUTH_TOKEN is required")?;
            let device: Device = env("LINE_TEST_DEVICE")
                .unwrap_or_else(|| "DESKTOPWIN".into())
                .parse()
                .map_err(|_| "LINE_TEST_DEVICE is not a known device name".to_string())?;
            let storage = FileStorage::new(STORAGE_PATH).map_err(|e| format!("open {STORAGE_PATH}: {e}"))?;
            throttle().await;
            let client = login_with_auth_token(&token, InitOptions::new(device).storage(storage))
                .await
                .map_err(|e| format!("login_with_auth_token failed: {e}"))?;
            watch_token_updates(&client);
            println!("logged in as {} ({})", client.mid().unwrap_or_default(), device.as_str());
            Ok(client)
        })
        .await;
    match res {
        Ok(c) => c.clone(),
        Err(e) => panic!("login unavailable (not retried): {e}"),
    }
}

/// The access token is not persisted by the library; print rotations so
/// `LINE_AUTH_TOKEN` can be updated for the next run.
fn watch_token_updates(client: &Client) {
    let mut rx = client.base.subscribe();
    RUNTIME.spawn(async move {
        loop {
            match rx.recv().await {
                Ok(line_rs::ClientEvent::UpdateAuthToken(t)) => {
                    println!("NOTE: auth token rotated; use this as LINE_AUTH_TOKEN next time:\n{t}");
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn own_mid(client: &Client) -> String {
    client.mid().expect("logged-in client has no mid")
}

/// The self-chat (Keep Memo) as a [`Chat`].
fn self_chat(client: &Client) -> Chat {
    Chat::new(client.clone(), tv!({"chatMid": own_mid(client)}))
}

fn unique_text(label: &str) -> String {
    format!("[line-rs live test] {label} {}", now_secs())
}

/// Send an E2EE text to `to` (Letter Sealing accounts refuse plain sends).
async fn post_text(client: &Client, to: &str, label: &str) -> TalkMessage {
    throttle().await;
    let msg = client
        .send_message(to, MessageInput::text(unique_text(label)).e2ee(true))
        .await
        .unwrap_or_else(|e| panic!("send_message to {to} failed: {e}"));
    assert!(!msg.id().is_empty(), "sent message has no id: {:?}", msg.raw);
    println!("sent message {} to {to}", msg.id());
    msg
}

/// A valid `size`x`size` solid-colour RGB PNG (stored deflate, no deps).
fn tiny_png(size: u32) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &x in data {
            a = (a + u32::from(x)) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }

    // Each row: filter byte 0 (none), then RGB pixels in LINE green.
    let mut row = vec![0u8];
    row.extend([0x06, 0xc7, 0x55].repeat(size as usize));
    let raw = row.repeat(size as usize);
    assert!(raw.len() <= 0xffff, "tiny_png: too large for one stored block");
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    zlib.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    zlib.extend_from_slice(&raw);
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

/// Listen for talk events until `pick` returns `Some` or `secs` elapse.
/// Every raw operation type is printed to help map unknown layouts.
async fn wait_for<T>(
    rx: &mut broadcast::Receiver<ClientEvent>,
    secs: u64,
    what: &str,
    mut pick: impl FnMut(&ClientEvent) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let ev = match tokio::time::timeout(left, rx.recv()).await {
            Err(_) => panic!("timed out after {secs}s waiting for {what}"),
            Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                println!("(lagged, {n} events dropped)");
                continue;
            }
            Ok(Err(broadcast::error::RecvError::Closed)) => panic!("event channel closed while waiting for {what}"),
            Ok(Ok(ev)) => ev,
        };
        match &ev {
            ClientEvent::Event(op) => println!("op: {} {}", op["type"], op),
            ClientEvent::Talk(t) => println!("talk: {t:?}"),
            ClientEvent::Message(m) => println!("message {} from {} ({})", m.id(), m.from().id, m.content_type()),
            _ => {}
        }
        if let Some(found) = pick(&ev) {
            return found;
        }
    }
}

/// Start talk listening; the handle aborts the loop when dropped.
struct Listening(ListenHandle);

impl Drop for Listening {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn listen_talk(client: &Client) -> Listening {
    Listening(client.listen(ListenOptions { talk: true, square: false, polling: false }))
}

// ---------------------------------------------------------------------------
// read_only: no state changes
// ---------------------------------------------------------------------------

mod read_only {
    use super::*;

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn pinned_chats() {
        run(async {
            let c = client().await;
            throttle().await;
            let res = c.base.talk().get_pinned_chats(None).await.unwrap_or_else(|e| panic!("getPinnedChats: {e}"));
            println!("getPinnedChats: {res}");
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn profiles() {
        run(async {
            let c = client().await;
            throttle().await;
            let res =
                c.base.multi_profile().get_profiles("INITIALIZATION").await.unwrap_or_else(|e| panic!("getProfiles: {e}"));
            println!("getProfiles: {res}");
            assert!(!res.is_null(), "getProfiles returned null");
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn block_details_for_peer() {
        run(async {
            let Some(peer) = peer() else { return skip(PEER_SKIP) };
            let c = client().await;
            throttle().await;
            match c.base.relation().get_block_details(&[peer], None).await {
                Ok(res) => println!("getBlockDetails: {res}"),
                // device capability (e.g. DESKTOPWIN), not a client bug
                Err(e) if e.to_string().contains("API method not capable") => {
                    skip(&format!("getBlockDetails not available on this device type: {e}"))
                }
                Err(e) => panic!("getBlockDetails: {e}"),
            }
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn product_tiers_for_client() {
        run(async {
            let region = env("LINE_PRODUCT_REGION").unwrap_or_else(|| "TH".into());
            let c = client().await;
            throttle().await;
            let res = c
                .base
                .premium_product_definition()
                .get_product_tiers_for_client(tv!({"req": {"region": region}}))
                .await
                .unwrap_or_else(|e| panic!("getProductTiersForClient: {e}"));
            println!("getProductTiersForClient: {res}");
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn user_ticket() {
        run(async {
            let c = client().await;
            throttle().await;
            let res = c.base.talk().get_user_ticket().await.unwrap_or_else(|e| panic!("getUserTicket: {e}"));
            println!("getUserTicket: {res}");
            assert!(!res.is_null(), "getUserTicket returned null");
        });
    }

    /// Fetch the newest self-chat message, then read it back by id.
    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn messages_by_ids() {
        run(async {
            let c = client().await;
            let chat = self_chat(&c);
            throttle().await;
            let mut fetcher = chat.message_fetcher().await.unwrap_or_else(|e| panic!("getMessageBoxes: {e}"));
            throttle().await;
            let page = fetcher.fetch(1).await.unwrap_or_else(|e| panic!("getPreviousMessagesV2WithRequest: {e}"));
            let Some(msg) = page.first() else {
                return skip("self-chat has no messages; send one to Keep Memo first");
            };
            let id: i64 = msg.id().parse().unwrap_or_else(|_| panic!("message id {:?} is not numeric", msg.id()));
            let delivered = msg.raw["deliveredTime"].as_i64().unwrap_or(0);
            throttle().await;
            let res = c
                .base
                .talk()
                .get_messages_by_ids(tv!({"messagesByIdsRequests": [{
                    "chatMid": chat.mid(),
                    "messageIds": [{"deliveredTime": delivered, "messageId": id}],
                }]}))
                .await
                .unwrap_or_else(|e| panic!("getMessagesByIds: {e}"));
            println!("getMessagesByIds: {res}");
            let found = res.as_list().is_some_and(|l| l.iter().any(|m| m["id"].as_str() == Some(msg.id())));
            assert!(found, "getMessagesByIds did not return message {}: {res}", msg.id());
        });
    }

    #[test]
    #[ignore = "LEGY does not route /kp (live run 2026-10-21: every /kp path 404s from server=legy)"]
    fn keep_size() {
        run(async {
            let c = client().await;
            let rest = c.base.rest();
            throttle().await;
            let res = rest.keep().size().await.unwrap_or_else(|e| panic!("keep/size.json: {e}"));
            println!("keep/size.json: {res}");
        });
    }

    #[test]
    #[ignore = "LEGY does not route /kp (live run 2026-10-21: every /kp path 404s from server=legy)"]
    fn keep_get_content() {
        run(async {
            let Some(id) = env("LINE_KEEP_CONTENT_ID") else {
                return skip("set LINE_KEEP_CONTENT_ID to a Keep content id");
            };
            let c = client().await;
            let rest = c.base.rest();
            throttle().await;
            let res = rest.keep().get_content(&id).await.unwrap_or_else(|e| panic!("keep/get.json: {e}"));
            println!("keep/get.json: {res}");
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn page_info() {
        run(async {
            let c = client().await;
            let rest = c.base.rest();
            throttle().await;
            let q = PageInfoQuery { url: "https://line.me".into(), ..Default::default() };
            let res = rest.home().page_info(q).await.unwrap_or_else(|e| panic!("pageinfo/get.json: {e}"));
            println!("pageinfo/get.json: {res}");
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn translate_hello_to_thai() {
        // Optional feature; legyTransAPI.nhn answered {"code":401} with every
        // combination tried (DESKTOPWIN), so this sends nothing.
        skip("translation is optional; legyTransAPI.nhn answers 401 for this client");
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn group_announcements() {
        run(async {
            let Some(group) = env("LINE_GROUP_MID") else { return skip("set LINE_GROUP_MID") };
            let c = client().await;
            throttle().await;
            let chat = c.get_chat(&group).await.unwrap_or_else(|e| panic!("getChats({group}): {e}"));
            throttle().await;
            let list = chat.announcements().await.unwrap_or_else(|e| panic!("getChatRoomAnnouncements: {e}"));
            println!("{} announcement(s): {list:?}", list.len());
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn group_read_range() {
        run(async {
            let Some(group) = env("LINE_GROUP_MID") else { return skip("set LINE_GROUP_MID") };
            let c = client().await;
            throttle().await;
            let chat = c.get_chat(&group).await.unwrap_or_else(|e| panic!("getChats({group}): {e}"));
            throttle().await;
            let range = chat.read_range().await.unwrap_or_else(|e| panic!("getMessageReadRange: {e}"));
            println!("getMessageReadRange: {range:?}");
        });
    }
}

// ---------------------------------------------------------------------------
// write: self-chat by default; peer/group only when configured
// ---------------------------------------------------------------------------

mod write {
    use super::*;

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn send_text() {
        run(async {
            let c = client().await;
            let me = own_mid(&c);
            let msg = post_text(&c, &me, "send_text").await;
            assert!(msg.is_my_message(), "sent message is not ours: {:?}", msg.raw);
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn send_image() {
        run(async {
            let c = client().await;
            let png = tiny_png(64);
            throttle().await;
            let sent = self_chat(&c)
                .send_image(png, Some("line-rs-live-test.png"))
                .await
                .unwrap_or_else(|e| panic!("send_image: {e}"));
            println!("send_image: {sent:?}");
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn send_file() {
        run(async {
            let c = client().await;
            let data = unique_text("send_file").into_bytes();
            throttle().await;
            let sent = self_chat(&c)
                .send_file(data, "line-rs-live-test.txt")
                .await
                .unwrap_or_else(|e| panic!("send_file: {e}"));
            println!("send_file: {sent:?}");
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn send_contact_of_self() {
        run(async {
            let c = client().await;
            let me = own_mid(&c);
            let name = c.base.profile().and_then(|p| p["displayName"].as_str().map(str::to_string)).unwrap_or_default();
            throttle().await;
            let msg = self_chat(&c).send_contact(&me, &name).await.unwrap_or_else(|e| panic!("send_contact: {e}"));
            println!("send_contact: {} {:?}", msg.id(), msg.content_metadata());
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn edit_message() {
        run(async {
            let c = client().await;
            let me = own_mid(&c);
            let msg = post_text(&c, &me, "edit_message (before)").await;
            throttle().await;
            match msg.edit(&unique_text("edit_message (after)")).await {
                Ok(res) => println!("editMessage: {res}"),
                Err(e) if e.code() == Some("NOT_PREMIUM") => skip(&format!("needs LYP Premium: {e}")),
                Err(e) => panic!("editMessage: {e}"),
            }
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn silently_unsend_message() {
        run(async {
            let c = client().await;
            let me = own_mid(&c);
            let msg = post_text(&c, &me, "silently_unsend").await;
            throttle().await;
            match c.base.talk().silently_unsend_message(msg.id()).await {
                Ok(res) => println!("silentlyUnsendMessage: {res}"),
                Err(e) if e.code() == Some("NOT_PREMIUM") => skip(&format!("needs LYP Premium: {e}")),
                Err(e) => panic!("silentlyUnsendMessage: {e}"),
            }
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn pin_roundtrip() {
        run(async {
            let c = client().await;
            let me = own_mid(&c);
            throttle().await;
            let res = c.base.talk().update_pin_state(&me, true).await.unwrap_or_else(|e| panic!("updatePinState(true): {e}"));
            println!("updatePinState(true): {res}");
            throttle().await;
            let res =
                c.base.talk().update_pin_state(&me, false).await.unwrap_or_else(|e| panic!("updatePinState(false): {e}"));
            println!("updatePinState(false): {res}");
        });
    }

    /// Reacting to one's own message may be refused by the server; that is
    /// a finding, not a harness bug.
    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn react_roundtrip() {
        run(async {
            let c = client().await;
            let me = own_mid(&c);
            let msg = post_text(&c, &me, "react").await;
            throttle().await;
            msg.react(ReactionType::Nice).await.unwrap_or_else(|e| panic!("react: {e}"));
            throttle().await;
            msg.cancel_reaction().await.unwrap_or_else(|e| panic!("cancelReaction: {e}"));
        });
    }

    /// Mute/favorite a group (local settings, not visible to members).
    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn group_mute_favorite_roundtrip() {
        run(async {
            let Some(group) = env("LINE_GROUP_MID") else { return skip("set LINE_GROUP_MID") };
            let c = client().await;
            throttle().await;
            let chat = c.get_chat(&group).await.unwrap_or_else(|e| panic!("getChats({group}): {e}"));
            throttle().await;
            chat.mute().await.unwrap_or_else(|e| panic!("mute: {e}"));
            throttle().await;
            chat.unmute().await.unwrap_or_else(|e| panic!("unmute: {e}"));
            throttle().await;
            chat.favorite().await.unwrap_or_else(|e| panic!("favorite: {e}"));
            throttle().await;
            chat.unfavorite().await.unwrap_or_else(|e| panic!("unfavorite: {e}"));
        });
    }

    /// Mute/favorite the peer contact (opt-in).
    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn peer_mute_favorite_roundtrip() {
        run(async {
            let Some(peer) = peer() else { return skip(PEER_SKIP) };
            let c = client().await;
            let user = User::new(c.clone(), tv!({"targetUserMid": peer}));
            throttle().await;
            user.set_notification_disabled(true).await.unwrap_or_else(|e| panic!("mute contact: {e}"));
            throttle().await;
            user.set_notification_disabled(false).await.unwrap_or_else(|e| panic!("unmute contact: {e}"));
            throttle().await;
            user.set_favorite(true).await.unwrap_or_else(|e| panic!("favorite contact: {e}"));
            throttle().await;
            user.set_favorite(false).await.unwrap_or_else(|e| panic!("unfavorite contact: {e}"));
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn block_roundtrip() {
        run(async {
            let Some(peer) = peer() else { return skip(PEER_SKIP) };
            let c = client().await;
            throttle().await;
            let res = c.base.relation().block_relation(&peer).await.unwrap_or_else(|e| panic!("blockRelation: {e}"));
            println!("blockRelation: {res}");
            throttle().await;
            let res = c
                .base
                .relation()
                .unblock_relation(&peer, None)
                .await
                .unwrap_or_else(|e| panic!("unblockRelation (peer is still BLOCKED, unblock on the phone): {e}"));
            println!("unblockRelation: {res}");
        });
    }
}

// ---------------------------------------------------------------------------
// events: you act on your phone (same account), the test waits
// ---------------------------------------------------------------------------

mod events {
    use super::*;

    const WAIT_SECS: u64 = 120;

    /// Read receipts only exist between two users.
    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn read_receipt() {
        run(async {
            let Some(peer) = peer() else {
                return skip("no read receipts in the self-chat; needs LINE_PEER_MID + LINE_ALLOW_PEER_TESTS=yes");
            };
            let c = client().await;
            let mut rx = c.subscribe();
            let _listen = listen_talk(&c);
            post_text(&c, &peer, "read_receipt").await;
            println!(">>> On the PEER's phone: open the chat with this account within {WAIT_SECS}s.");
            let (chat, reader) = wait_for(&mut rx, WAIT_SECS, "TalkEvent::Read", |ev| match ev {
                ClientEvent::Talk(TalkEvent::Read { chat_mid, reader_mid, .. }) => {
                    Some((chat_mid.clone(), reader_mid.clone()))
                }
                _ => None,
            })
            .await;
            assert_eq!(reader, peer, "Read.reader_mid (param2) is not the peer; chat_mid={chat}");
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn reaction() {
        run(async {
            // LINE does not allow reacting to your own messages: the peer reacts.
            let Some(peer) = peer() else { return skip(PEER_SKIP) };
            let c = client().await;
            let mut rx = c.subscribe();
            let _listen = listen_talk(&c);
            let msg = post_text(&c, &peer, "react to me").await;
            println!(">>> Ask the peer to react to \"{}\" in their chat with this account within {WAIT_SECS}s.", msg.text());
            let r = wait_for(&mut rx, WAIT_SECS, "TalkEvent::Reaction", |ev| match ev {
                ClientEvent::Talk(TalkEvent::Reaction(r)) => Some(r.clone()),
                _ => None,
            })
            .await;
            println!("reaction: {r:?}");
            assert_eq!(r.message_id, msg.id(), "Reaction.message_id (param1) does not match the sent message");
            assert_eq!(r.reactor_mid, peer, "Reaction.reactor_mid (param3) is not the peer");
            assert!(r.reaction.is_some(), "reaction type not decoded from param2: {}", r.detail);
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn unsend() {
        run(async {
            let c = client().await;
            let me = own_mid(&c);
            let mut rx = c.subscribe();
            let _listen = listen_talk(&c);
            println!(">>> On your phone: in Keep Memo send a text, then unsend it, within {WAIT_SECS}s.");
            let mut sent_from_phone = None;
            let (chat_mid, message_id) = wait_for(&mut rx, WAIT_SECS, "TalkEvent::Unsend", |ev| match ev {
                ClientEvent::Message(m) if m.to().id == me => {
                    sent_from_phone = Some(m.id().to_string());
                    None
                }
                ClientEvent::Talk(TalkEvent::Unsend { chat_mid, message_id }) => {
                    Some((chat_mid.clone(), message_id.clone()))
                }
                _ => None,
            })
            .await;
            println!("unsend: chat_mid={chat_mid} message_id={message_id} (phone message seen: {sent_from_phone:?})");
            assert!(!message_id.is_empty(), "Unsend.message_id (param2) is empty");
            if let Some(id) = sent_from_phone {
                assert_eq!(message_id, id, "Unsend.message_id (param2) does not match the message sent from the phone");
            }
        });
    }
}

// ---------------------------------------------------------------------------
// e2ee: decrypt an image sent from your phone
// ---------------------------------------------------------------------------

mod e2ee {
    use super::*;

    fn is_image(data: &[u8]) -> bool {
        data.starts_with(&[0xff, 0xd8, 0xff])
            || data.starts_with(b"\x89PNG\r\n\x1a\n")
            || data.starts_with(b"GIF8")
            || (data.len() > 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP")
            || (data.len() > 12 && &data[4..8] == b"ftyp")
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn incoming_image_download() {
        run(async {
            let c = client().await;
            let me = own_mid(&c);
            let mut rx = c.subscribe();
            let _listen = listen_talk(&c);
            println!(">>> On your phone: send a photo to Keep Memo within 120s.");
            let msg = wait_for(&mut rx, 120, "an IMAGE message in the self-chat", |ev| match ev {
                ClientEvent::Message(m) if m.to().id == me && m.content_type() == "IMAGE" => Some(m.clone()),
                _ => None,
            })
            .await;
            println!("image message {} metadata {:?} e2ee={}", msg.id(), msg.content_metadata(), msg.raw.get("chunks").is_some());
            throttle().await;
            let data = msg.data(false).await.unwrap_or_else(|e| panic!("download/decrypt image {}: {e}", msg.id()));
            println!("downloaded {} bytes, head {:02x?}", data.len(), &data[..data.len().min(16)]);
            assert!(!data.is_empty(), "downloaded image is empty");
            assert!(is_image(&data), "downloaded data has no known image magic: {:02x?}", &data[..data.len().min(16)]);
        });
    }
}

// ---------------------------------------------------------------------------
// session: token lifecycle
// ---------------------------------------------------------------------------

mod session {
    use super::*;

    /// Mark the stored token as expired and check the next call refreshes
    /// it first. Only V3 tokens (with a stored `refreshToken`) can refresh.
    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn proactive_refresh() {
        run(async {
            let c = client().await;
            let has_refresh = matches!(c.base.storage.get_string("refreshToken").await, Ok(Some(_)));
            if !has_refresh {
                return skip("no refreshToken stored (not a V3 token); proactive refresh does not apply");
            }
            if !c.base.config.proactive_refresh {
                return skip("Config::proactive_refresh is off");
            }
            let before = c.auth_token();
            c.base.set_token_expire(now_secs() - 10).await.unwrap_or_else(|e| panic!("set_token_expire: {e}"));
            throttle().await;
            c.base
                .talk()
                .call("getProfile", tv!({"syncReason": "INTERNAL"}))
                .await
                .unwrap_or_else(|e| panic!("getProfile after forcing expiry: {e}"));
            let expire = c.base.token_expire().await.unwrap_or_else(|e| panic!("token_expire: {e}"));
            println!("expire now {expire:?}");
            assert_ne!(c.auth_token(), before, "token was not refreshed");
            assert!(expire.is_some_and(|e| e > now_secs()), "expire not moved forward: {expire:?}");
        });
    }

    /// Opt-in only: needs you to end this session from elsewhere.
    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn kicked_out() {
        run(async {
            if std::env::var("LINE_ALLOW_KICKOUT_TEST").as_deref() != Ok("yes") {
                return skip("opt-in only (LINE_ALLOW_KICKOUT_TEST=yes); see LIVE_TESTING.md before enabling");
            }
            let c = client().await;
            let mut rx = c.base.subscribe();
            let _listen = listen_talk(&c);
            println!("!!! WARNING: this ends the test session for good; the token becomes unusable.");
            println!(">>> On your phone: Settings > Account > Devices, log out the linejs session, within 180s.");
            let deadline = Instant::now() + Duration::from_secs(180);
            let reason = loop {
                let left = deadline.saturating_duration_since(Instant::now());
                match tokio::time::timeout(left, rx.recv()).await {
                    Err(_) => panic!("timed out after 180s waiting for ClientEvent::KickedOut"),
                    Ok(Ok(line_rs::ClientEvent::KickedOut(r))) => break r,
                    Ok(Ok(_)) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
                    Ok(Err(broadcast::error::RecvError::Closed)) => panic!("base event channel closed"),
                }
            };
            println!("kicked out: {reason}");
            assert!(c.base.is_disabled(), "client still enabled after KickedOut");
        });
    }
}

#[test]
fn tiny_png_is_well_formed() {
    let png = tiny_png(64);
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(png.ends_with(&[0xae, 0x42, 0x60, 0x82]), "IEND CRC");
}
