//! Live checks for the calls recovered from LINE Android 26.x (friends,
//! message request box). Same rules as `tests/live_auth.rs`; read `tests/LIVE_TESTING.md`.
//!
//! ```sh
//! LINE_TEST_ACCOUNT_CONFIRM=yes LINE_AUTH_TOKEN=... LINE_TEST_DEVICE=... \
//!   cargo test --test live_android26 read_only -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! * Calls are spaced by a random 1–3 s gap, so the traffic has no fixed
//!   rhythm.
//! * `LINE_TEST_STORAGE` (default `./storage.android26.json`) holds reqseq.
//!   Never put a `refreshToken` in it when the token belongs to another
//!   client that is still running: a refresh here would sign that client
//!   out. Without one, `MUST_REFRESH_V3_TOKEN` fails the test instead.
//! * Output is limited to field names and counts, not account data.
//! * Read-only. Calls that change the account are not run here:
//!   delete_friend, the friend setting changes, sync_contacts_v3,
//!   update_chat_status_bulk, createSessionV2, the smart-switch keys and
//!   requestDeletePaidSquare.

use std::future::Future;
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use line_rs::Device;
use line_rs::client::{Client, InitOptions, login_with_auth_token};
use line_rs::storage::FileStorage;
use line_rs::thrift::Value;
use tokio::sync::{Mutex, OnceCell};

static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
});

static CLIENT: OnceCell<Result<Client, String>> = OnceCell::const_new();
static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::const_new(None);

fn run<F: Future<Output = ()>>(body: F) {
    if std::env::var("LINE_TEST_ACCOUNT_CONFIRM").as_deref() != Ok("yes") {
        panic!(
            "refusing to run: these tests act on a real LINE account. \
             Read tests/LIVE_TESTING.md, then set LINE_TEST_ACCOUNT_CONFIRM=yes"
        );
    }
    RUNTIME.block_on(body);
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// A gap of 1000–3000 ms, from the clock's sub-second noise.
fn random_gap() -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    Duration::from_millis(1000 + (nanos ^ (nanos >> 13)) % 2001)
}

/// Wait a random 1–3 s since the previous call.
async fn throttle() {
    let mut last = LAST_REQUEST.lock().await;
    if let Some(at) = *last {
        let wait = random_gap().saturating_sub(at.elapsed());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
    *last = Some(Instant::now());
}

async fn client() -> Client {
    let res = CLIENT
        .get_or_init(|| async {
            let token = env("LINE_AUTH_TOKEN").ok_or("LINE_AUTH_TOKEN is required")?;
            let device: Device = env("LINE_TEST_DEVICE")
                .unwrap_or_else(|| "DESKTOPWIN".into())
                .parse()
                .map_err(|_| "LINE_TEST_DEVICE is not a known device name".to_string())?;
            let path =
                env("LINE_TEST_STORAGE").unwrap_or_else(|| "./storage.android26.json".into());
            let storage = FileStorage::new(&path).map_err(|e| format!("open {path}: {e}"))?;
            throttle().await;
            let client = login_with_auth_token(&token, InitOptions::new(device).storage(storage))
                .await
                .map_err(|e| format!("login_with_auth_token failed: {e}"))?;
            println!("logged in ({})", device.as_str());
            Ok(client)
        })
        .await;
    match res {
        Ok(c) => c.clone(),
        Err(e) => panic!("login unavailable (not retried): {e}"),
    }
}

/// Field names of a struct value, and the length of each list/set/map.
/// Numeric names mean the response did not match the IDL.
fn shape(v: &Value) -> String {
    let Some(map) = v.as_struct() else {
        return format!("{v:?}").chars().take(60).collect();
    };
    map.iter()
        .map(|(k, v)| match v {
            Value::List(l) => format!("{k}[{}]", l.len()),
            Value::Map(m) => format!("{k}{{{}}}", m.len()),
            Value::Struct(_) => format!("{k}{{..}}"),
            _ => k.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn report(name: &str, res: line_rs::Result<Value>) -> Option<Value> {
    match res {
        Ok(v) => {
            println!("{name}: ok ({})", shape(&v));
            assert!(
                v.as_struct()
                    .is_none_or(|m| m.keys().all(|k| k.parse::<i16>().is_err())),
                "{name}: response has unnamed fields; the IDL layout is off"
            );
            Some(v)
        }
        Err(e) => panic!("{name}: {e}"),
    }
}

mod read_only {
    use super::*;

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn relation_id_lists() {
        run(async {
            let c = client().await;
            let r = c.base.relation();
            throttle().await;
            report("getUserBlockIds", r.get_user_block_ids(None).await);
            throttle().await;
            report("getBotBlockIds", r.get_bot_block_ids(None).await);
            throttle().await;
            report("getUserRecommendIds", r.get_user_recommend_ids(None).await);
            throttle().await;
            report("getBotFriendIds", r.get_bot_friend_ids(None).await);
        });
    }

    #[test]
    #[ignore = "hits the real LINE server with a test account"]
    fn message_request_box() {
        run(async {
            let c = client().await;
            throttle().await;
            report(
                "getMessageRequestBoxList",
                c.base
                    .talk()
                    .get_message_request_box_list("PEER", None)
                    .await,
            );
        });
    }
}
