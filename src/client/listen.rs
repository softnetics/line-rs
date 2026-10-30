//! Event listening (linejs `Client.listen`): turn talk `Operation`s and
//! `SquareEvent`s into [`ClientEvent`]s.
//!
//! [`Client::listen_with`] consumes any pair of event streams. [`Client::listen`]
//! feeds it from the LEGY push connection, or from plain polling
//! (`sync` / `fetchMyEvents`) when [`ListenOptions::polling`] is set.

use futures::stream::{BoxStream, StreamExt};
use tokio::task::JoinHandle;

use super::message::{ReactionType, SquareMessage, TalkMessage};
use super::{Client, ClientEvent};
use crate::error::{Error, Result};
use crate::thrift::Value;

/// What [`Client::listen`] subscribes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenOptions {
    /// Talk operations (default `true`).
    pub talk: bool,
    /// Square (OpenChat) events (default `true`).
    pub square: bool,
    /// Poll instead of using the push connection (default `false`).
    pub polling: bool,
}

impl Default for ListenOptions {
    fn default() -> Self {
        Self { talk: true, square: true, polling: false }
    }
}

/// Running listen loops. Dropping the handle leaves them running; call
/// [`abort`](Self::abort) to stop.
pub struct ListenHandle {
    tasks: Vec<JoinHandle<()>>,
}

impl ListenHandle {
    pub fn abort(&self) {
        for t in &self.tasks {
            t.abort();
        }
    }

    pub fn is_finished(&self) -> bool {
        self.tasks.iter().all(JoinHandle::is_finished)
    }

    /// Wait until every loop ended (stream exhausted, failed or aborted).
    pub async fn join(self) {
        for t in self.tasks {
            let _ = t.await;
        }
    }
}

/// A reaction from [`TalkEvent::Reaction`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReactionEvent {
    /// `param1`.
    pub message_id: String,
    /// `chatMid` of the `param2` JSON, when present.
    pub chat_mid: Option<String>,
    /// `param3`: who reacted.
    pub reactor_mid: String,
    /// `curr.predefinedReactionType` of the `param2` JSON; `None` when it
    /// is missing or unknown (e.g. a paid reaction).
    pub reaction: Option<ReactionType>,
    /// `param2` parsed as JSON (`Null` if it is not JSON).
    pub detail: serde_json::Value,
}

/// Typed view of common talk `Operation`s. Emitted as
/// [`ClientEvent::Talk`] right after the raw [`ClientEvent::Event`], which
/// is still sent for every operation. Fields are the operation's
/// `param1..3`; mid lists are `U+001E`-separated on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TalkEvent {
    /// `NOTIFIED_READ_MESSAGE`: `reader_mid` read `chat_mid` up to
    /// `message_id`.
    Read { chat_mid: String, reader_mid: String, message_id: String },
    /// `SEND_CHAT_CHECKED`: we marked `chat_mid` as read (any device).
    ChatChecked { chat_mid: String, message_id: String },
    /// `NOTIFIED_SEND_REACTION`.
    Reaction(ReactionEvent),
    /// `NOTIFIED_DESTROY_MESSAGE`: a message was unsent.
    Unsend { chat_mid: String, message_id: String },
    /// `NOTIFIED_RECEIVED_CALL` (`canceled = false`) or `CANCEL_CALL`
    /// (`canceled = true`). The params are passed through as is.
    Call { canceled: bool, param1: String, param2: String, param3: String },
    /// `NOTIFIED_INVITE_INTO_CHAT` / `NOTIFIED_INVITE_INTO_GROUP`.
    Invite { chat_mid: String, inviter_mid: String, invitee_mids: Vec<String> },
    /// `NOTIFIED_ACCEPT_CHAT_INVITATION` / `NOTIFIED_ACCEPT_GROUP_INVITATION`.
    Join { chat_mid: String, mid: String },
    /// `NOTIFIED_DELETE_SELF_FROM_CHAT` / `NOTIFIED_LEAVE_GROUP` /
    /// `NOTIFIED_LEAVE_ROOM`.
    Leave { chat_mid: String, mid: String },
    /// `NOTIFIED_DELETE_OTHER_FROM_CHAT` / `NOTIFIED_KICKOUT_FROM_GROUP`.
    Kick { chat_mid: String, kicker_mid: String, kicked_mids: Vec<String> },
    /// `ADD_CONTACT` (`by_me`) or `NOTIFIED_ADD_CONTACT` (someone added us).
    ContactAdded { mid: String, by_me: bool },
    /// `BLOCK_CONTACT` (`blocked`) / `UNBLOCK_CONTACT`, done by us.
    ContactBlocked { mid: String, blocked: bool },
}

fn split_mids(s: &str) -> Vec<String> {
    s.split('\u{1e}').filter(|m| !m.is_empty()).map(str::to_string).collect()
}

impl TalkEvent {
    /// Classify an `Operation`; `None` for operations without a typed view.
    pub fn from_operation(op: &Value) -> Option<Self> {
        let p = |n: u8| op[format!("param{n}").as_str()].as_str().unwrap_or("").to_string();
        let kind = op_type(op);
        Some(match kind.as_str() {
            "NOTIFIED_READ_MESSAGE" => TalkEvent::Read { chat_mid: p(1), reader_mid: p(2), message_id: p(3) },
            "SEND_CHAT_CHECKED" => TalkEvent::ChatChecked { chat_mid: p(1), message_id: p(2) },
            "NOTIFIED_SEND_REACTION" => {
                let detail: serde_json::Value = serde_json::from_str(&p(2)).unwrap_or_default();
                let curr = &detail["curr"]["predefinedReactionType"];
                let reaction =
                    curr.as_i64().and_then(ReactionType::from_i64).or_else(|| curr.as_str().and_then(ReactionType::from_name));
                TalkEvent::Reaction(ReactionEvent {
                    message_id: p(1),
                    chat_mid: detail["chatMid"].as_str().map(str::to_string),
                    reactor_mid: p(3),
                    reaction,
                    detail,
                })
            }
            "NOTIFIED_DESTROY_MESSAGE" => TalkEvent::Unsend { chat_mid: p(1), message_id: p(2) },
            "NOTIFIED_RECEIVED_CALL" | "CANCEL_CALL" => {
                TalkEvent::Call { canceled: kind == "CANCEL_CALL", param1: p(1), param2: p(2), param3: p(3) }
            }
            "NOTIFIED_INVITE_INTO_CHAT" | "NOTIFIED_INVITE_INTO_GROUP" => {
                TalkEvent::Invite { chat_mid: p(1), inviter_mid: p(2), invitee_mids: split_mids(&p(3)) }
            }
            "NOTIFIED_ACCEPT_CHAT_INVITATION" | "NOTIFIED_ACCEPT_GROUP_INVITATION" => {
                TalkEvent::Join { chat_mid: p(1), mid: p(2) }
            }
            "NOTIFIED_DELETE_SELF_FROM_CHAT" | "NOTIFIED_LEAVE_GROUP" | "NOTIFIED_LEAVE_ROOM" => {
                TalkEvent::Leave { chat_mid: p(1), mid: p(2) }
            }
            "NOTIFIED_DELETE_OTHER_FROM_CHAT" | "NOTIFIED_KICKOUT_FROM_GROUP" => {
                TalkEvent::Kick { chat_mid: p(1), kicker_mid: p(2), kicked_mids: split_mids(&p(3)) }
            }
            "ADD_CONTACT" => TalkEvent::ContactAdded { mid: p(1), by_me: true },
            "NOTIFIED_ADD_CONTACT" => TalkEvent::ContactAdded { mid: p(1), by_me: false },
            "BLOCK_CONTACT" => TalkEvent::ContactBlocked { mid: p(1), blocked: true },
            "UNBLOCK_CONTACT" => TalkEvent::ContactBlocked { mid: p(1), blocked: false },
            _ => return None,
        })
    }
}

/// Talk operation type (enum name, or the number when the IDL lacks it).
fn op_type(op: &Value) -> String {
    super::message::types::enum_name(&op["type"], "OpType").unwrap_or_default()
}

impl Client {
    /// Listen for talk and/or square events (push by default).
    pub fn listen(&self, opts: ListenOptions) -> ListenHandle {
        let base = &self.base;
        let (talk, square) = if opts.polling {
            (
                opts.talk.then(|| self.skip_errors(base.poll_talk_events(crate::polling::DEFAULT_TALK_POLL_INTERVAL))),
                opts.square
                    .then(|| self.skip_errors(base.poll_square_events(crate::polling::DEFAULT_SQUARE_POLL_INTERVAL))),
            )
        } else {
            (opts.talk.then(|| base.listen_talk_events().boxed()), opts.square.then(|| base.listen_square_events().boxed()))
        };
        self.listen_with(talk, square)
    }

    /// Polling streams keep going after a failed poll; log and drop errors
    /// so [`listen_with`](Self::listen_with) does not stop on them.
    fn skip_errors(
        &self,
        stream: impl futures::Stream<Item = Result<Value>> + Send + 'static,
    ) -> BoxStream<'static, Result<Value>> {
        let client = self.clone();
        stream
            .filter_map(move |item| {
                let out = match item {
                    Ok(v) => Some(Ok(v)),
                    Err(e) => {
                        client.report_listen_failure(&e);
                        None
                    }
                };
                async move { out }
            })
            .boxed()
    }

    /// Consume talk `Operation` and `SquareEvent` streams, emitting
    /// [`ClientEvent`]s. A failing item (e.g. undecryptable message) is
    /// logged and skipped; an `Err` from a stream is logged as
    /// `LegyPusherError` and ends that stream only.
    pub fn listen_with(
        &self,
        talk: Option<BoxStream<'static, Result<Value>>>,
        square: Option<BoxStream<'static, Result<Value>>>,
    ) -> ListenHandle {
        let mut tasks = Vec::new();
        if let Some(mut talk) = talk {
            let client = self.clone();
            tasks.push(tokio::spawn(async move {
                while let Some(item) = talk.next().await {
                    match item {
                        Ok(op) => {
                            if let Err(e) = client.handle_talk_op(op).await {
                                client.report_listen_failure(&e);
                            }
                        }
                        Err(e) => {
                            client.report_listen_failure(&e);
                            break;
                        }
                    }
                }
            }));
        }
        if let Some(mut square) = square {
            let client = self.clone();
            tasks.push(tokio::spawn(async move {
                while let Some(item) = square.next().await {
                    match item {
                        Ok(event) => client.handle_square_event(event),
                        Err(e) => {
                            client.report_listen_failure(&e);
                            break;
                        }
                    }
                }
            }));
        }
        ListenHandle { tasks }
    }

    fn report_listen_failure(&self, e: &Error) {
        self.base.log("LegyPusherError", serde_json::json!({"error": e.to_string()}));
    }

    /// Emit events for one talk `Operation`.
    pub(crate) async fn handle_talk_op(&self, op: Value) -> Result<()> {
        let kind = op_type(&op);
        let message = op["message"].clone();
        let typed = TalkEvent::from_operation(&op);
        self.emit(ClientEvent::Event(op));
        if let Some(t) = typed {
            self.emit(ClientEvent::Talk(t));
        }
        match kind.as_str() {
            "SEND_MESSAGE" | "RECEIVE_MESSAGE" => {
                let raw = self.base.decrypt_e2ee_message(message).await?;
                self.emit(ClientEvent::Message(TalkMessage::new(self.clone(), raw)));
            }
            // 158 = EDIT_MESSAGE (ours, from another device), 159 =
            // NOTIFIED_EDIT_MESSAGE; not in the bundled IDL yet.
            "EDIT_MESSAGE" | "NOTIFIED_EDIT_MESSAGE" | "158" | "159" => {
                let raw = self.base.decrypt_e2ee_message(message).await?;
                self.emit(ClientEvent::MessageEdit(TalkMessage::new(self.clone(), raw)));
            }
            _ => {}
        }
        Ok(())
    }

    /// Emit events for one `SquareEvent`.
    pub(crate) fn handle_square_event(&self, event: Value) {
        let kind = super::message::types::enum_name(&event["type"], "SquareEventType").unwrap_or_default();
        let message = (kind == "NOTIFICATION_MESSAGE").then(|| SquareMessage::from_event(self.clone(), &event));
        self.emit(ClientEvent::SquareEvent(event));
        if let Some(m) = message {
            self.emit(ClientEvent::SquareMessage(m));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tv;
    use std::time::Duration;
    use crate::base::{BaseClient, ClientEvent as BaseEvent};
    use crate::device::Device;

    fn client() -> Client {
        Client::new(BaseClient::new(Device::Android).unwrap())
    }

    fn drain(rx: &mut tokio::sync::broadcast::Receiver<ClientEvent>) -> Vec<ClientEvent> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn logs(rx: &mut tokio::sync::broadcast::Receiver<BaseEvent>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|e| match e {
                BaseEvent::Log { kind, .. } => Some(kind),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn talk_stream_failure_is_reported_and_earlier_events_delivered() {
        let c = client();
        let (mut rx, mut base_rx) = (c.subscribe(), c.base.subscribe());
        let talk = futures::stream::iter(vec![
            Ok(tv!({"type": "NOTIFIED_READ_MESSAGE"})),
            Err(Error::internal("TypeError", "stream failed")),
            Ok(tv!({"type": "NOTIFIED_READ_MESSAGE"})),
        ])
        .boxed();
        c.listen_with(Some(talk), None).join().await;
        let events = drain(&mut rx);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], ClientEvent::Event(_)));
        assert!(matches!(events[1], ClientEvent::Talk(TalkEvent::Read { .. })));
        assert_eq!(logs(&mut base_rx), ["LegyPusherError"]);
    }

    #[tokio::test]
    async fn messages_edits_and_square_events() {
        let c = client();
        let mut rx = c.subscribe();
        let talk = futures::stream::iter(vec![
            Ok(tv!({"type": "RECEIVE_MESSAGE", "message": {"id": "1", "text": "hi"}})),
            Ok(tv!({"type": 159, "message": {"id": "1", "text": "edited"}})),
            Ok(tv!({"type": "SEND_MESSAGE", "message": {"id": "2"}})),
        ])
        .boxed();
        let square = futures::stream::iter(vec![
            Ok(tv!({"type": "NOTIFICATION_MESSAGE", "payload": {"notificationMessage": {
                "squareMessage": {"message": {"id": "s1", "text": "yo"}}}}})),
            Err(Error::internal("TypeError", "square failed")),
        ])
        .boxed();
        c.listen_with(Some(talk), Some(square)).join().await;
        let events = drain(&mut rx);
        let talk: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                ClientEvent::Message(m) => Some(format!("msg:{}", m.text())),
                ClientEvent::MessageEdit(m) => Some(format!("edit:{}", m.text())),
                ClientEvent::Event(_) => Some("event".into()),
                _ => None,
            })
            .collect();
        assert_eq!(talk, ["event", "msg:hi", "event", "edit:edited", "event", "msg:"]);
        let square: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                ClientEvent::SquareEvent(_) => Some("event".to_string()),
                ClientEvent::SquareMessage(m) => Some(format!("msg:{}", m.text())),
                _ => None,
            })
            .collect();
        assert_eq!(square, ["event", "msg:yo"]);
    }

    #[test]
    fn classifies_typed_talk_events() {
        let ev = |v: Value| TalkEvent::from_operation(&v);
        let s = |v: &str| v.to_string();
        assert_eq!(
            ev(tv!({"type": 55, "param1": "c1", "param2": "u1", "param3": "100"})),
            Some(TalkEvent::Read { chat_mid: s("c1"), reader_mid: s("u1"), message_id: s("100") })
        );
        assert_eq!(
            ev(tv!({"type": "SEND_CHAT_CHECKED", "param1": "c1", "param2": "9"})),
            Some(TalkEvent::ChatChecked { chat_mid: s("c1"), message_id: s("9") })
        );
        let Some(TalkEvent::Reaction(r)) = ev(tv!({"type": 140, "param1": "100",
            "param2": r#"{"chatMid":"c1","curr":{"predefinedReactionType":3}}"#, "param3": "u2"}))
        else {
            panic!("not a reaction")
        };
        assert_eq!((r.message_id.as_str(), r.chat_mid.as_deref(), r.reactor_mid.as_str()), ("100", Some("c1"), "u2"));
        assert_eq!(r.reaction, Some(ReactionType::Love));
        let Some(TalkEvent::Reaction(r)) = ev(tv!({"type": 140, "param1": "1", "param2": "garbage"})) else {
            panic!("not a reaction")
        };
        assert_eq!((r.reaction, r.chat_mid), (None, None));
        assert_eq!(
            ev(tv!({"type": 65, "param1": "c1", "param2": "100"})),
            Some(TalkEvent::Unsend { chat_mid: s("c1"), message_id: s("100") })
        );
        assert!(matches!(ev(tv!({"type": 50, "param1": "a"})), Some(TalkEvent::Call { canceled: false, .. })));
        assert!(matches!(ev(tv!({"type": 51})), Some(TalkEvent::Call { canceled: true, .. })));
        assert_eq!(
            ev(tv!({"type": 124, "param1": "c1", "param2": "u1", "param3": "u2\u{1e}u3"})),
            Some(TalkEvent::Invite { chat_mid: s("c1"), inviter_mid: s("u1"), invitee_mids: vec![s("u2"), s("u3")] })
        );
        assert_eq!(ev(tv!({"type": 130, "param1": "c1", "param2": "u2"})), Some(TalkEvent::Join { chat_mid: s("c1"), mid: s("u2") }));
        assert_eq!(ev(tv!({"type": 15, "param1": "c1", "param2": "u2"})), Some(TalkEvent::Leave { chat_mid: s("c1"), mid: s("u2") }));
        assert_eq!(
            ev(tv!({"type": 133, "param1": "c1", "param2": "u1", "param3": "u2"})),
            Some(TalkEvent::Kick { chat_mid: s("c1"), kicker_mid: s("u1"), kicked_mids: vec![s("u2")] })
        );
        assert_eq!(ev(tv!({"type": 5, "param1": "u9"})), Some(TalkEvent::ContactAdded { mid: s("u9"), by_me: false }));
        assert_eq!(ev(tv!({"type": 7, "param1": "u9"})), Some(TalkEvent::ContactBlocked { mid: s("u9"), blocked: false }));
        assert_eq!(ev(tv!({"type": 25})), None);
        assert_eq!(ev(tv!({"type": 9999})), None);
    }

    #[tokio::test]
    async fn typed_event_follows_raw_event() {
        let c = client();
        let mut rx = c.subscribe();
        let talk = futures::stream::iter(vec![
            Ok(tv!({"type": "NOTIFIED_DESTROY_MESSAGE", "param1": "c1", "param2": "5"})),
            Ok(tv!({"type": "NOTIFIED_TYPING"})),
        ])
        .boxed();
        c.listen_with(Some(talk), None).join().await;
        let kinds: Vec<_> = drain(&mut rx)
            .iter()
            .map(|e| match e {
                ClientEvent::Event(_) => "event",
                ClientEvent::Talk(TalkEvent::Unsend { .. }) => "unsend",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["event", "unsend", "event"]);
    }

    #[tokio::test]
    async fn polling_stops_without_auth_token() {
        let c = client();
        let h = c.listen(ListenOptions::default());
        tokio::time::timeout(Duration::from_secs(5), h.join()).await.unwrap();
    }
}
