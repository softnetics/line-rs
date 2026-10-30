//! Event listening (`base/polling/mod.ts` in linejs): push-based streams of
//! talk operations and square events over the shared LEGY push connection,
//! plus plain polling variants (`sync` / `fetchMyEvents` in a loop).

use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::base::{BaseClient, ClientEvent};
use crate::error::{Error, Result};
use crate::push::manager::{apply_sync_response, talk_sync_args, ConnManager, PushHost};
use crate::push::{Hub, Kind};
use crate::request::RequestOptions;
use crate::thrift::{Protocol, Value};
use crate::tv;

/// Sync cursors shared by push and polling (serialized like linejs'
/// `SyncData`: `{"square": …, "talk": {"revision", "globalRev", "individualRev"}}`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncData {
    /// Square `fetchMyEvents` sync token.
    pub square: Option<String>,
    pub talk: TalkSync,
}

/// Talk `sync` revisions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TalkSync {
    pub revision: i64,
    pub global_rev: i64,
    pub individual_rev: i64,
}

/// Default interval of [`BaseClient::poll_talk_events`] in linejs.
pub const DEFAULT_TALK_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Default interval of [`BaseClient::poll_square_events`] in linejs.
pub const DEFAULT_SQUARE_POLL_INTERVAL: Duration = Duration::from_millis(1000);

/// Push services the listener signs on to (square events, talk sync).
const LISTEN_TARGET: &[u8] = &[3, 8];
/// Delay before reconnecting a dropped push connection.
const RECONNECT_DELAY: Duration = Duration::from_secs(4);

/// The pusher loop (`initLegyPusher`): reconnect while logged in and while
/// anybody listens. A connection that cannot even be set up ends every
/// stream with that error; a later subscription retries.
pub(crate) async fn run_pusher<H: PushHost>(
    host: H,
    hub: Arc<Hub>,
    generation: u64,
    targets: &'static [u8],
    reconnect_delay: Duration,
) {
    let manager = ConnManager::new(host.clone(), hub.clone());
    while host.auth_token().is_some() {
        let conn = match manager.initialize_conn(targets) {
            Ok(c) => c,
            Err(e) => {
                host.log("LegyPusherError_cannot_init", json!({"error": e.to_string()}));
                hub.finish(generation, Some(&e));
                host.log("LegyPusherError", json!({"error": e.to_string()}));
                return;
            }
        };
        if let Err(e) = manager.init_and_read(conn, targets).await {
            host.log("LegyPusherError", json!({"error": e.to_string()}));
        }
        if hub.exit_if_idle(generation) {
            return;
        }
        tokio::time::sleep(reconnect_delay).await;
        if hub.exit_if_idle(generation) {
            return;
        }
    }
    hub.finish(generation, Some(&Error::internal("NotAuthorized", "push listener stopped: no auth token")));
}

fn receiver_stream(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Result<Value>>,
) -> impl Stream<Item = Result<Value>> + Send + 'static {
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

/// Call `fetch` repeatedly, yielding every value it returns and sleeping
/// `interval` between calls. Errors are yielded and polling continues.
fn poll_stream<F, Fut>(interval: Duration, fetch: F) -> impl Stream<Item = Result<Value>> + Send + 'static
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = Result<Vec<Value>>> + Send + 'static,
{
    futures::stream::unfold((fetch, VecDeque::new(), true), move |(mut fetch, mut buf, mut first)| async move {
        loop {
            if let Some(v) = buf.pop_front() {
                return Some((Ok(v), (fetch, buf, first)));
            }
            if !first {
                tokio::time::sleep(interval).await;
            }
            first = false;
            match fetch().await {
                Ok(items) => buf.extend(items),
                Err(e) => return Some((Err(e), (fetch, buf, first))),
            }
        }
    })
}

impl BaseClient {
    /// Current sync cursors.
    pub async fn sync_data(&self) -> SyncData {
        self.sync.lock().await.clone()
    }

    /// Restore sync cursors (e.g. persisted from [`ClientEvent::UpdateSyncData`])
    /// so listening resumes where it left off.
    pub async fn set_sync_data(&self, data: SyncData) {
        *self.sync.lock().await = data;
    }

    /// Stream of talk `Operation`s delivered over the LEGY push connection.
    ///
    /// The connection is shared with [`listen_square_events`](Self::listen_square_events)
    /// and started on first use (requires a Tokio runtime); it reconnects
    /// while logged in and closes once every stream is dropped. Events that
    /// arrive while no talk stream exists are buffered (up to 1024) for the
    /// next one. Emits [`ClientEvent::UpdateSyncData`] as cursors advance.
    pub fn listen_talk_events(&self) -> impl Stream<Item = Result<Value>> + Send + 'static {
        self.listen(Kind::Talk)
    }

    /// Stream of `SquareEvent`s delivered over the LEGY push connection. See
    /// [`listen_talk_events`](Self::listen_talk_events).
    pub fn listen_square_events(&self) -> impl Stream<Item = Result<Value>> + Send + 'static {
        self.listen(Kind::Square)
    }

    fn listen(&self, kind: Kind) -> impl Stream<Item = Result<Value>> + Send + 'static {
        let hub = self.push.hub.clone();
        let client = self.clone();
        let task_hub = hub.clone();
        let rx = hub.subscribe(kind, move |generation| {
            tokio::spawn(run_pusher(client, task_hub, generation, LISTEN_TARGET, RECONNECT_DELAY)).abort_handle()
        });
        receiver_stream(rx)
    }

    /// Close the push connection and end every listening stream.
    pub fn stop_listening(&self) {
        self.push.hub.stop();
    }

    /// Whether the push listener is running.
    pub fn is_listening(&self) -> bool {
        self.push.hub.is_running()
    }

    /// Poll talk `sync` every `interval` (linejs `_listenTalkEvents`,
    /// default [`DEFAULT_TALK_POLL_INTERVAL`]). Errors are yielded and polling
    /// continues; drop the stream to stop.
    pub fn poll_talk_events(&self, interval: Duration) -> impl Stream<Item = Result<Value>> + Send + 'static {
        let client = self.clone();
        poll_stream(interval, move || {
            let client = client.clone();
            async move { client.poll_talk_once().await }
        })
    }

    /// Poll square `fetchMyEvents` every `interval` (linejs
    /// `_listenSquareEvents`, default [`DEFAULT_SQUARE_POLL_INTERVAL`]).
    pub fn poll_square_events(&self, interval: Duration) -> impl Stream<Item = Result<Value>> + Send + 'static {
        let client = self.clone();
        let continuation: Arc<Mutex<Option<String>>> = Arc::default();
        poll_stream(interval, move || {
            let (client, continuation) = (client.clone(), continuation.clone());
            async move { client.poll_square_once(&continuation).await }
        })
    }

    async fn poll_talk_once(&self) -> Result<Vec<Value>> {
        let args = talk_sync_args(&*self.sync.lock().await);
        let opts = RequestOptions::new("/SYNC4", Protocol::Compact).timeout(self.config.long_timeout);
        let res = self.call_with("sync", args, opts).await?;
        let (ops, snapshot) = {
            let mut sync = self.sync.lock().await;
            (apply_sync_response(&mut sync, &res), sync.clone())
        };
        self.emit(ClientEvent::UpdateSyncData(snapshot));
        Ok(ops)
    }

    async fn poll_square_once(&self, continuation: &Mutex<Option<String>>) -> Result<Vec<Value>> {
        let sync_token = self.sync.lock().await.square.clone();
        let cont = continuation.lock().unwrap().clone();
        let res = self
            .square()
            .call("fetchMyEvents", tv!({"request": {"syncToken": sync_token, "continuationToken": cont, "limit": 100}}))
            .await?;
        *continuation.lock().unwrap() = res["continuationToken"].as_str().map(str::to_string);
        let snapshot = {
            let mut sync = self.sync.lock().await;
            sync.square = res["syncToken"].as_str().map(str::to_string);
            sync.clone()
        };
        self.emit(ClientEvent::UpdateSyncData(snapshot));
        Ok(res["events"].as_list().map(<[Value]>::to_vec).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use futures::{FutureExt, StreamExt};

    use super::*;
    use crate::push::conn::{Conn, ConnWriter};
    use crate::push::manager::tests::FakeHost;

    fn spawn_with(host: &FakeHost, hub: &Arc<Hub>, delay: Duration) -> impl FnOnce(u64) -> tokio::task::AbortHandle {
        let (host, hub) = (host.clone(), hub.clone());
        move |g| tokio::spawn(run_pusher(host, hub, g, LISTEN_TARGET, delay)).abort_handle()
    }

    async fn next_err(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Result<Value>>) -> Error {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await.expect("reader hung") {
            Some(Err(e)) => e,
            other => panic!("expected an error, got {other:?}"),
        }
    }

    fn client_logs(host: &FakeHost) -> Vec<String> {
        host.log_kinds().into_iter().filter(|k| !k.starts_with("[LEGY/PUSH]")).collect()
    }

    #[test]
    fn sync_data_serializes_like_linejs() {
        let data = SyncData { square: Some("t".into()), talk: TalkSync { revision: 1, global_rev: 2, individual_rev: 3 } };
        let j = serde_json::to_value(&data).unwrap();
        assert_eq!(j, json!({"square": "t", "talk": {"revision": 1, "globalRev": 2, "individualRev": 3}}));
        let back: SyncData = serde_json::from_value(json!({"talk": {"revision": 9}})).unwrap();
        assert_eq!(back.talk.revision, 9);
        assert_eq!(back.square, None);
    }

    // A poll that yields nothing must still sleep between calls.
    #[tokio::test]
    async fn poll_stream_sleeps_between_polls() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let stream = poll_stream(Duration::from_millis(50), move || {
            c.fetch_add(1, Ordering::SeqCst);
            async { Ok(Vec::new()) }
        });
        let start = Instant::now();
        let r = tokio::time::timeout(Duration::from_millis(150), stream.boxed().next()).await;
        assert!(r.is_err());
        let n = calls.load(Ordering::SeqCst);
        assert!((2..=8).contains(&n), "expected a bounded number of polls, got {n}");
        assert!(start.elapsed() >= Duration::from_millis(100));
    }

    #[tokio::test]
    async fn poll_stream_yields_values_and_errors() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let stream = poll_stream(Duration::from_millis(1), move || {
            let n = c.fetch_add(1, Ordering::SeqCst);
            async move {
                match n {
                    0 => Ok(vec![tv!(1), tv!(2)]),
                    1 => Err(Error::internal("TestError", "x")),
                    _ => Ok(vec![tv!(3)]),
                }
            }
        });
        let items: Vec<_> = stream.take(4).collect().await;
        assert_eq!(items[0].as_ref().unwrap().as_i64(), Some(1));
        assert_eq!(items[1].as_ref().unwrap().as_i64(), Some(2));
        assert!(items[2].is_err());
        assert_eq!(items[3].as_ref().unwrap().as_i64(), Some(3));
    }

    #[tokio::test]
    async fn failed_pusher_initialization_resets_listening_state_for_retry() {
        let host = FakeHost::new();
        let hub = Arc::new(Hub::default());
        let mut rx = hub.subscribe(Kind::Talk, spawn_with(&host, &hub, Duration::from_millis(10)));
        next_err(&mut rx).await;
        assert!(!hub.is_running());
        let mut rx = hub.subscribe(Kind::Talk, spawn_with(&host, &hub, Duration::from_millis(10)));
        next_err(&mut rx).await;
        assert_eq!(host.opens.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn pusher_failure_reaches_the_stream_reader() {
        let host = FakeHost::new();
        let hub = Arc::new(Hub::default());
        let mut rx = hub.subscribe(Kind::Talk, spawn_with(&host, &hub, Duration::from_millis(10)));
        let e = next_err(&mut rx).await;
        assert!(matches!(e, Error::Internal { ref kind, ref message } if kind == "TestError" && message == "connection failed"));
        // The stream ends after the error.
        assert!(rx.recv().await.is_none());
        assert_eq!(client_logs(&host), vec!["LegyPusherError_cannot_init", "LegyPusherError"]);
    }

    #[tokio::test]
    async fn shared_pusher_failure_rejects_both_streams() {
        let host = FakeHost::new();
        let hub = Arc::new(Hub::default());
        let mut talk = hub.subscribe(Kind::Talk, spawn_with(&host, &hub, Duration::from_millis(10)));
        let mut square = hub.subscribe(Kind::Square, |_| unreachable!("pusher already running"));
        next_err(&mut talk).await;
        next_err(&mut square).await;
        assert!(!hub.is_running());
        assert_eq!(host.opens.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn read_failures_reconnect_until_nobody_listens() {
        let host = FakeHost::new();
        for _ in 0..3 {
            let (w, _rx) = ConnWriter::channel();
            host.conns
                .lock()
                .unwrap()
                .push(Conn::from_parts(w, async { Err(Error::internal("LegyPusherError", "fetch failed")) }.boxed()));
        }
        let hub = Arc::new(Hub::default());
        let rx = hub.subscribe(Kind::Talk, spawn_with(&host, &hub, Duration::from_millis(20)));
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(host.opens.load(Ordering::SeqCst) >= 2);
        assert!(hub.is_running());
        drop(rx);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(!hub.is_running());
        assert!(client_logs(&host).iter().all(|k| k == "LegyPusherError"));
    }

    #[tokio::test]
    async fn pusher_stops_without_auth_token() {
        let host = FakeHost::new();
        *host.token.lock().unwrap() = None;
        let hub = Arc::new(Hub::default());
        let mut rx = hub.subscribe(Kind::Square, spawn_with(&host, &hub, Duration::from_millis(10)));
        let e = next_err(&mut rx).await;
        assert!(matches!(e, Error::Internal { ref kind, .. } if kind == "NotAuthorized"));
        assert_eq!(host.opens.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn stop_listening_ends_streams() {
        let client = BaseClient::new(crate::Device::IosIpad).unwrap();
        let mut talk = client.listen_talk_events().boxed();
        let mut square = client.listen_square_events().boxed();
        assert!(client.is_listening());
        client.stop_listening();
        assert!(!client.is_listening());
        // Without an auth token the pusher may already have ended the
        // streams with `NotAuthorized`; either way they finish.
        for stream in [&mut talk, &mut square] {
            let rest: Vec<_> = tokio::time::timeout(Duration::from_millis(500), stream.collect()).await.unwrap();
            assert!(rest.iter().all(|r| r.is_err()));
        }
    }
}
