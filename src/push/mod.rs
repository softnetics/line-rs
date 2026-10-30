//! LEGY HTTP/2 push connection (`base/push` in linejs).
//!
//! A single long-lived `POST /PUSH/1/subs?m=…` carries framed sign-on
//! requests (square `fetchMyEvents`, talk `sync`) and the server's pings,
//! sign-on responses and push notifications. One connection feeds both the
//! talk operation stream and the square event stream; see
//! [`BaseClient::listen_talk_events`](crate::BaseClient::listen_talk_events).
//!
//! * [`frame`] — packet layouts (`connData.ts`).
//! * [`conn`] — packet reassembly and the full-duplex transport (`conn.ts`).
//! * [`tmc`] — the TMoreCompact reader used by talk sign-on responses.

pub mod conn;
pub mod frame;
pub(crate) mod manager;
pub mod tmc;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use futures::FutureExt;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::base::{BaseClient, ClientEvent};
use crate::error::{Error, Result};
use crate::polling::SyncData;
use crate::request::RequestOptions;
use crate::thrift::{Protocol, Value};

use conn::Conn;
use manager::PushHost;

/// Events kept for a stream nobody listens to yet (linejs buffers them
/// without bound; we cap it).
const BACKLOG_LIMIT: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Talk,
    Square,
}

type Sender = mpsc::UnboundedSender<Result<Value>>;

#[derive(Default)]
struct Channel {
    subs: Vec<Sender>,
    backlog: VecDeque<Value>,
}

impl Channel {
    fn prune(&mut self) {
        self.subs.retain(|s| !s.is_closed());
    }

    fn deliver(&mut self, v: Value) {
        self.prune();
        if self.subs.is_empty() {
            if self.backlog.len() == BACKLOG_LIMIT {
                self.backlog.pop_front();
            }
            self.backlog.push_back(v);
            return;
        }
        for s in &self.subs {
            let _ = s.send(Ok(v.clone()));
        }
    }

    fn subscribe(&mut self) -> mpsc::UnboundedReceiver<Result<Value>> {
        let (tx, rx) = mpsc::unbounded_channel();
        for v in self.backlog.drain(..) {
            let _ = tx.send(Ok(v));
        }
        self.subs.push(tx);
        rx
    }

    /// End every subscriber's stream, after an error if one is given.
    fn close(&mut self, err: Option<&Error>) {
        for s in self.subs.drain(..) {
            if let Some(e) = err {
                let _ = s.send(Err(clone_error(e)));
            }
        }
    }
}

/// [`Error`] is not `Clone`; keep the kind when there is one.
fn clone_error(e: &Error) -> Error {
    match e {
        Error::Internal { kind, message } => Error::internal(kind.clone(), message.clone()),
        Error::Request { method, path, data } => {
            Error::Request { method: method.clone(), path: path.clone(), data: data.clone() }
        }
        other => Error::internal("LegyPusherError", other.to_string()),
    }
}

#[derive(Default)]
struct HubInner {
    talk: Channel,
    square: Channel,
    /// The running pusher task and its generation.
    task: Option<(u64, AbortHandle)>,
    generation: u64,
}

impl HubInner {
    fn channel(&mut self, kind: Kind) -> &mut Channel {
        match kind {
            Kind::Talk => &mut self.talk,
            Kind::Square => &mut self.square,
        }
    }
}

/// Fan-out of push events to stream subscribers, plus the slot of the
/// pusher task that serves them (the `opStream`/`sqStream` of linejs).
#[derive(Default)]
pub(crate) struct Hub {
    inner: Mutex<HubInner>,
}

impl Hub {
    /// Subscribe to `kind`; `spawn` starts the pusher (given its generation)
    /// when none is running.
    pub fn subscribe(
        &self,
        kind: Kind,
        spawn: impl FnOnce(u64) -> AbortHandle,
    ) -> mpsc::UnboundedReceiver<Result<Value>> {
        let mut inner = self.inner.lock().unwrap();
        let rx = inner.channel(kind).subscribe();
        if inner.task.is_none() {
            inner.generation += 1;
            let generation = inner.generation;
            inner.task = Some((generation, spawn(generation)));
        }
        rx
    }

    #[cfg(test)]
    pub fn subscribe_raw(&self, kind: Kind) -> mpsc::UnboundedReceiver<Result<Value>> {
        self.inner.lock().unwrap().channel(kind).subscribe()
    }

    pub fn deliver(&self, kind: Kind, v: Value) {
        self.inner.lock().unwrap().channel(kind).deliver(v);
    }

    /// No live subscriber on either stream.
    pub fn is_idle(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        inner.talk.prune();
        inner.square.prune();
        inner.talk.subs.is_empty() && inner.square.subs.is_empty()
    }

    pub fn is_running(&self) -> bool {
        self.inner.lock().unwrap().task.is_some()
    }

    /// Called by pusher `generation` before reconnecting: if nobody listens,
    /// release the task slot and tell the caller to stop.
    pub fn exit_if_idle(&self, generation: u64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.task.as_ref().map(|t| t.0) != Some(generation) {
            return true;
        }
        inner.talk.prune();
        inner.square.prune();
        if inner.talk.subs.is_empty() && inner.square.subs.is_empty() {
            inner.task = None;
            return true;
        }
        false
    }

    /// Pusher `generation` gave up: end all streams (with `err`) and release
    /// the slot so a later subscription can retry.
    pub fn finish(&self, generation: u64, err: Option<&Error>) {
        let mut inner = self.inner.lock().unwrap();
        if inner.task.as_ref().map(|t| t.0) != Some(generation) {
            return;
        }
        inner.task = None;
        inner.talk.close(err);
        inner.square.close(err);
    }

    /// Abort the pusher and end all streams.
    pub fn stop(&self) {
        let mut inner = self.inner.lock().unwrap();
        if let Some((_, h)) = inner.task.take() {
            h.abort();
        }
        inner.talk.close(None);
        inner.square.close(None);
    }
}

/// Push state owned by [`BaseClient`].
#[derive(Default)]
pub struct PushState {
    pub(crate) hub: Arc<Hub>,
}

impl PushHost for BaseClient {
    fn auth_token(&self) -> Option<String> {
        BaseClient::auth_token(self)
    }

    fn sync_state(&self) -> &tokio::sync::Mutex<SyncData> {
        &self.sync
    }

    fn emit_sync(&self, data: SyncData) {
        self.emit(ClientEvent::UpdateSyncData(data));
    }

    fn log(&self, kind: &str, data: serde_json::Value) {
        BaseClient::log(self, kind, data);
    }

    fn open_conn(&self, path: &str) -> Result<Conn> {
        let mut headers = self.line_headers("POST");
        headers.retain(|(k, _)| k != "content-type" && k != "accept");
        headers.push(("content-type".into(), "application/octet-stream".into()));
        headers.push(("accept".into(), "application/octet-stream".into()));
        Ok(Conn::open(&self.push_http, &format!("https://{}{}", self.endpoint, path), headers))
    }

    fn fetch_my_events(&self, args: Value) -> BoxFuture<'static, Result<Value>> {
        let c = self.clone();
        async move { c.square().call("fetchMyEvents", args).await }.boxed()
    }

    fn noop(&self) -> BoxFuture<'static, Result<()>> {
        let c = self.clone();
        async move {
            c.request(Vec::new(), "noop", RequestOptions::new("/S4", Protocol::Compact)).await?;
            Ok(())
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tv;

    #[test]
    fn backlog_is_flushed_to_first_subscriber() {
        let hub = Hub::default();
        hub.deliver(Kind::Talk, tv!({"revision": 1}));
        let mut rx = hub.subscribe_raw(Kind::Talk);
        assert_eq!(rx.try_recv().unwrap().unwrap()["revision"].as_i64(), Some(1));
        hub.deliver(Kind::Talk, tv!({"revision": 2}));
        assert_eq!(rx.try_recv().unwrap().unwrap()["revision"].as_i64(), Some(2));
    }

    #[test]
    fn dropping_one_stream_keeps_the_other() {
        let hub = Hub::default();
        let talk = hub.subscribe_raw(Kind::Talk);
        let mut square = hub.subscribe_raw(Kind::Square);
        drop(talk);
        assert!(!hub.is_idle());
        hub.deliver(Kind::Square, tv!({"x": 1}));
        assert!(square.try_recv().unwrap().is_ok());
        drop(square);
        assert!(hub.is_idle());
    }

    #[test]
    fn finish_ends_streams_with_error_and_frees_slot() {
        let hub = Hub::default();
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let _g = rt.enter();
        let spawn = |_| tokio::spawn(async {}).abort_handle();
        let mut a = hub.subscribe(Kind::Talk, spawn);
        let mut b = hub.subscribe(Kind::Square, |_| unreachable!("already running"));
        assert!(hub.is_running());
        // A stale generation is ignored.
        hub.finish(99, None);
        assert!(hub.is_running());
        hub.finish(1, Some(&Error::internal("X", "boom")));
        assert!(!hub.is_running());
        assert!(matches!(a.try_recv(), Ok(Err(Error::Internal { kind, .. })) if kind == "X"));
        assert!(matches!(b.try_recv(), Ok(Err(_))));
        assert!(matches!(a.try_recv(), Err(mpsc::error::TryRecvError::Disconnected)));
    }
}
