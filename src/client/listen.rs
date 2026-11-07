//! Event listening (linejs `Client.listen`): turn talk `Operation`s and
//! `SquareEvent`s into [`ClientEvent`]s.
//!
//! [`Client::listen_with`] consumes any pair of event streams. [`Client::listen`]
//! feeds it from the LEGY push connection, or from plain polling
//! (`sync` / `fetchMyEvents`) when [`ListenOptions::polling`] is set.

use futures::stream::{BoxStream, StreamExt};
use tokio::task::JoinHandle;

use super::message::{PaidReaction, ReactionType, SquareMessage, TalkMessage};
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
        Self {
            talk: true,
            square: true,
            polling: false,
        }
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
    /// `curr.paidReactionType`: a reaction with a LINE emoji.
    pub paid: Option<PaidReaction>,
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
    Read {
        chat_mid: String,
        reader_mid: String,
        message_id: String,
    },
    /// `SEND_CHAT_CHECKED`: we marked `chat_mid` as read (any device).
    ChatChecked {
        chat_mid: String,
        message_id: String,
    },
    /// `NOTIFIED_SEND_REACTION`.
    Reaction(ReactionEvent),
    /// `NOTIFIED_DESTROY_MESSAGE`: a message was unsent.
    Unsend {
        chat_mid: String,
        message_id: String,
    },
    /// `NOTIFIED_RECEIVED_CALL` (`canceled = false`) or `CANCEL_CALL`
    /// (`canceled = true`). The params are passed through as is.
    Call {
        canceled: bool,
        param1: String,
        param2: String,
        param3: String,
    },
    /// `NOTIFIED_INVITE_INTO_CHAT` / `NOTIFIED_INVITE_INTO_GROUP`.
    Invite {
        chat_mid: String,
        inviter_mid: String,
        invitee_mids: Vec<String>,
    },
    /// `NOTIFIED_ACCEPT_CHAT_INVITATION` / `NOTIFIED_ACCEPT_GROUP_INVITATION`.
    Join { chat_mid: String, mid: String },
    /// `NOTIFIED_DELETE_SELF_FROM_CHAT` / `NOTIFIED_LEAVE_CHAT` /
    /// `NOTIFIED_LEAVE_GROUP` / `NOTIFIED_LEAVE_ROOM`: `mid` left
    /// `chat_mid`.
    Leave { chat_mid: String, mid: String },
    /// `DELETE_SELF_FROM_CHAT`: we left `chat_mid` (on any device).
    LeftChat { chat_mid: String },
    /// `UPDATE_CHAT` (`by_me`, on any device) or `NOTIFIED_UPDATE_CHAT`
    /// (someone else): the chat's name, picture or settings changed;
    /// re-fetch it (`getChats`). `param2` and
    /// `param3` are passed through as is.
    ChatUpdated {
        chat_mid: String,
        by_me: bool,
        param2: String,
        param3: String,
    },
    /// `SEND_CHAT_REMOVED`: we deleted `chat_mid` (on any device, the
    /// chat list's "delete"). Only `param1` matters: it deletes a 1:1 chat, group or memo chat
    /// from the chat list with its messages (`deleteChatWithoutMuteStatus`),
    /// and only the messages (and album cache) of a room, whose row stays. `last_message_id` is
    /// `param2`, the
    /// `sendChatRemoved` request's `lastMessageId` the app sends from the
    /// chat's last message (inferred from the request, unverified on the
    /// wire; the app ignores it on receipt).
    ChatRemoved {
        chat_mid: String,
        last_message_id: String,
    },
    /// `SEND_CHAT_HIDDEN`: we hid (`hidden`) or unhid `chat_mid` up to
    /// `last_message_id` (on any device; `param3` is `"1"` or `"0"`).
    ChatHidden {
        chat_mid: String,
        last_message_id: String,
        hidden: bool,
    },
    /// `REMOVE_ALL_MESSAGES`: we deleted the messages of every chat (on any
    /// device). It has no params and clears the messages of every chat.
    AllMessagesRemoved,
    /// `UPDATE_MESSAGE_REQUEST_BOX` (`by_me`, on any device) or
    /// `NOTIFIED_UPDATE_MESSAGE_REQUEST_BOX`: `chat_mid` (`param1`) moved
    /// into (`in_box`) or out of the message request box (`param2` > 0 or
    /// not). The app ignores the op when `param2` is not a number. On
    /// `NOTIFIED_…` with `in_box` the app lights a dot on the request
    /// box entry of its chat list.
    MessageRequestBox {
        chat_mid: String,
        in_box: bool,
        by_me: bool,
    },
    /// `REGISTER_E2EE_PUBLICKEY`, `(NOTIFIED_)UPDATE_PUBLICKEYCHAIN` or
    /// `NOTIFIED_E2EE_KEY_UPDATE`: Letter Sealing keys changed. The client
    /// has already forgotten the key ids it remembered as unresolvable;
    /// messages that could not be decrypted may open now.
    E2eeKeysChanged { op: crate::e2ee::E2eeOpKind },
    /// `NOTIFIED_DELETE_OTHER_FROM_CHAT` / `NOTIFIED_KICKOUT_FROM_GROUP`.
    Kick {
        chat_mid: String,
        kicker_mid: String,
        kicked_mids: Vec<String>,
    },
    /// `ADD_CONTACT` (`by_me`) or `NOTIFIED_ADD_CONTACT` (someone added us).
    ContactAdded { mid: String, by_me: bool },
    /// `BLOCK_CONTACT` (`blocked`) / `UNBLOCK_CONTACT`, done by us.
    ContactBlocked { mid: String, blocked: bool },
    /// `UPDATE_SETTINGS`: account settings changed (on any device). The
    /// `SettingsAttributeEx` ids to fetch again; see
    /// [`settings::NOTIFICATION_ATTRIBUTES`](super::settings::NOTIFICATION_ATTRIBUTES).
    SettingsUpdated { attributes: Vec<i32> },
}

fn split_mids(s: &str) -> Vec<String> {
    s.split('\u{1e}')
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .collect()
}

impl TalkEvent {
    /// Classify an `Operation`; `None` for operations without a typed view.
    pub fn from_operation(op: &Value) -> Option<Self> {
        let p = |n: u8| {
            op[format!("param{n}").as_str()]
                .as_str()
                .unwrap_or("")
                .to_string()
        };
        let kind = op_type(op);
        Some(match kind.as_str() {
            "NOTIFIED_READ_MESSAGE" => TalkEvent::Read {
                chat_mid: p(1),
                reader_mid: p(2),
                message_id: p(3),
            },
            "SEND_CHAT_CHECKED" => TalkEvent::ChatChecked {
                chat_mid: p(1),
                message_id: p(2),
            },
            "NOTIFIED_SEND_REACTION" => {
                let detail: serde_json::Value = serde_json::from_str(&p(2)).unwrap_or_default();
                let curr = &detail["curr"]["predefinedReactionType"];
                let reaction = curr
                    .as_i64()
                    .and_then(ReactionType::from_i64)
                    .or_else(|| curr.as_str().and_then(ReactionType::from_name));
                TalkEvent::Reaction(ReactionEvent {
                    message_id: p(1),
                    chat_mid: detail["chatMid"].as_str().map(str::to_string),
                    reactor_mid: p(3),
                    reaction,
                    paid: PaidReaction::from_json(&detail["curr"]["paidReactionType"]),
                    detail,
                })
            }
            "NOTIFIED_DESTROY_MESSAGE" => TalkEvent::Unsend {
                chat_mid: p(1),
                message_id: p(2),
            },
            "NOTIFIED_RECEIVED_CALL" | "CANCEL_CALL" => TalkEvent::Call {
                canceled: kind == "CANCEL_CALL",
                param1: p(1),
                param2: p(2),
                param3: p(3),
            },
            "NOTIFIED_INVITE_INTO_CHAT" | "NOTIFIED_INVITE_INTO_GROUP" => TalkEvent::Invite {
                chat_mid: p(1),
                inviter_mid: p(2),
                invitee_mids: split_mids(&p(3)),
            },
            "NOTIFIED_ACCEPT_CHAT_INVITATION" | "NOTIFIED_ACCEPT_GROUP_INVITATION" => {
                TalkEvent::Join {
                    chat_mid: p(1),
                    mid: p(2),
                }
            }
            "NOTIFIED_DELETE_SELF_FROM_CHAT"
            | "NOTIFIED_LEAVE_CHAT"
            | "NOTIFIED_LEAVE_GROUP"
            | "NOTIFIED_LEAVE_ROOM" => TalkEvent::Leave {
                chat_mid: p(1),
                mid: p(2),
            },
            "DELETE_SELF_FROM_CHAT" => TalkEvent::LeftChat { chat_mid: p(1) },
            "UPDATE_CHAT" | "NOTIFIED_UPDATE_CHAT" => TalkEvent::ChatUpdated {
                chat_mid: p(1),
                by_me: kind == "UPDATE_CHAT",
                param2: p(2),
                param3: p(3),
            },
            "SEND_CHAT_REMOVED" => TalkEvent::ChatRemoved {
                chat_mid: p(1),
                last_message_id: p(2),
            },
            "UPDATE_MESSAGE_REQUEST_BOX"
            | "NOTIFIED_UPDATE_MESSAGE_REQUEST_BOX"
            | "155"
            | "156" => {
                let chat_mid = p(1);
                let in_box = p(2).trim().parse::<i64>().ok()? > 0;
                if chat_mid.is_empty() {
                    return None;
                }
                TalkEvent::MessageRequestBox {
                    chat_mid,
                    in_box,
                    by_me: matches!(kind.as_str(), "UPDATE_MESSAGE_REQUEST_BOX" | "155"),
                }
            }
            // The op means nothing unless param3 is "0" or "1".
            "UPDATE_SETTINGS" => TalkEvent::SettingsUpdated {
                attributes: super::settings::updated_setting_attributes(&p(3)),
            },
            "SEND_CHAT_HIDDEN" => TalkEvent::ChatHidden {
                chat_mid: p(1),
                last_message_id: p(2),
                hidden: match p(3).as_str() {
                    "1" => true,
                    "0" => false,
                    _ => return None,
                },
            },
            "REMOVE_ALL_MESSAGES" => TalkEvent::AllMessagesRemoved,
            "NOTIFIED_DELETE_OTHER_FROM_CHAT" | "NOTIFIED_KICKOUT_FROM_GROUP" => TalkEvent::Kick {
                chat_mid: p(1),
                kicker_mid: p(2),
                kicked_mids: split_mids(&p(3)),
            },
            "ADD_CONTACT" => TalkEvent::ContactAdded {
                mid: p(1),
                by_me: true,
            },
            "NOTIFIED_ADD_CONTACT" => TalkEvent::ContactAdded {
                mid: p(1),
                by_me: false,
            },
            "BLOCK_CONTACT" => TalkEvent::ContactBlocked {
                mid: p(1),
                blocked: true,
            },
            "UNBLOCK_CONTACT" => TalkEvent::ContactBlocked {
                mid: p(1),
                blocked: false,
            },
            _ => {
                use crate::e2ee::{E2eeOpKind, E2eeOperation};
                let op = E2eeOperation::from_op(op)?.kind;
                match op {
                    E2eeOpKind::RegisterPublicKey
                    | E2eeOpKind::UpdatePublicKeyChain
                    | E2eeOpKind::KeyUpdate => TalkEvent::E2eeKeysChanged { op },
                    _ => return None,
                }
            }
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
                opts.talk.then(|| {
                    self.skip_errors(
                        base.poll_talk_events(crate::polling::DEFAULT_TALK_POLL_INTERVAL),
                    )
                }),
                opts.square.then(|| {
                    self.skip_errors(
                        base.poll_square_events(crate::polling::DEFAULT_SQUARE_POLL_INTERVAL),
                    )
                }),
            )
        } else {
            (
                opts.talk.then(|| base.listen_talk_events().boxed()),
                opts.square.then(|| base.listen_square_events().boxed()),
            )
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
        self.base.log(
            "LegyPusherError",
            serde_json::json!({"error": e.to_string()}),
        );
    }

    /// Emit events for one talk `Operation`.
    pub(crate) async fn handle_talk_op(&self, op: Value) -> Result<()> {
        let kind = op_type(&op);
        let message = op["message"].clone();
        let typed = TalkEvent::from_operation(&op);
        if matches!(typed, Some(TalkEvent::E2eeKeysChanged { .. })) {
            self.base.forget_unresolvable_e2ee_keys();
        }
        self.emit(ClientEvent::Event(op));
        if let Some(t) = typed {
            self.emit(ClientEvent::Talk(t));
        }
        match kind.as_str() {
            "SEND_MESSAGE" | "RECEIVE_MESSAGE" => {
                let raw = self.decrypt_or_keep(message).await;
                self.emit(ClientEvent::Message(TalkMessage::new(self.clone(), raw)));
            }
            // 158 = EDIT_MESSAGE (ours, from another device), 159 =
            // NOTIFIED_EDIT_MESSAGE; not in the bundled IDL yet.
            "EDIT_MESSAGE" | "NOTIFIED_EDIT_MESSAGE" | "158" | "159" => {
                let raw = self.decrypt_or_keep(message).await;
                self.emit(ClientEvent::MessageEdit(TalkMessage::new(
                    self.clone(),
                    raw,
                )));
            }
            _ => {}
        }
        Ok(())
    }

    /// Decrypt an E2EE message, or keep it as it came when that fails
    /// (an `E2EE_UNDECRYPTED` message) instead of dropping it: its `chunks` stay, `text` stays
    /// empty (see
    /// [`is_undecrypted`](crate::e2ee::is_undecrypted)), and the failure
    /// is logged (`e2ee` / `decryptFailed`) with the ids from
    /// [`describe_e2ee_message`](crate::e2ee::describe_e2ee_message).
    async fn decrypt_or_keep(&self, message: Value) -> Value {
        match self.base.decrypt_e2ee_message(message.clone()).await {
            Ok(raw) => raw,
            Err(e) => {
                let mut info = crate::e2ee::describe_e2ee_message(&message);
                info["error"] = e.to_string().into();
                self.base.log(
                    "e2ee",
                    serde_json::json!({"type": "decryptFailed", "message": info}),
                );
                message
            }
        }
    }

    /// Emit events for one `SquareEvent`.
    pub(crate) fn handle_square_event(&self, event: Value) {
        let kind =
            super::message::types::enum_name(&event["type"], "SquareEventType").unwrap_or_default();
        let message = (kind == "NOTIFICATION_MESSAGE")
            .then(|| SquareMessage::from_event(self.clone(), &event));
        self.emit(ClientEvent::SquareEvent(event));
        if let Some(m) = message {
            self.emit(ClientEvent::SquareMessage(m));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{BaseClient, ClientEvent as BaseEvent};
    use crate::device::Device;
    use crate::tv;
    use std::time::Duration;

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
        assert!(matches!(
            events[1],
            ClientEvent::Talk(TalkEvent::Read { .. })
        ));
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
            Ok(
                tv!({"type": "NOTIFICATION_MESSAGE", "payload": {"notificationMessage": {
                "squareMessage": {"message": {"id": "s1", "text": "yo"}}}}}),
            ),
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
        assert_eq!(
            talk,
            ["event", "msg:hi", "event", "edit:edited", "event", "msg:"]
        );
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
            Some(TalkEvent::Read {
                chat_mid: s("c1"),
                reader_mid: s("u1"),
                message_id: s("100")
            })
        );
        assert_eq!(
            ev(tv!({"type": 36, "param3": "0\u{1e}40"})),
            Some(TalkEvent::SettingsUpdated {
                attributes: vec![0, 40]
            })
        );
        assert_eq!(
            ev(tv!({"type": "SEND_CHAT_CHECKED", "param1": "c1", "param2": "9"})),
            Some(TalkEvent::ChatChecked {
                chat_mid: s("c1"),
                message_id: s("9")
            })
        );
        let Some(TalkEvent::Reaction(r)) = ev(tv!({"type": 140, "param1": "100",
            "param2": r#"{"chatMid":"c1","curr":{"predefinedReactionType":3}}"#, "param3": "u2"}))
        else {
            panic!("not a reaction")
        };
        assert_eq!(
            (
                r.message_id.as_str(),
                r.chat_mid.as_deref(),
                r.reactor_mid.as_str()
            ),
            ("100", Some("c1"), "u2")
        );
        assert_eq!(r.reaction, Some(ReactionType::Love));
        let Some(TalkEvent::Reaction(r)) =
            ev(tv!({"type": 140, "param1": "1", "param2": "garbage"}))
        else {
            panic!("not a reaction")
        };
        assert_eq!((r.reaction, r.chat_mid, r.paid), (None, None, None));
        let Some(TalkEvent::Reaction(r)) = ev(tv!({"type": 140, "param1": "1",
            "param2": r#"{"curr":{"paidReactionType":{"productId":"p1","emojiId":"007","resourceType":2,"version":4}}}"#}))
        else {
            panic!("not a reaction")
        };
        assert_eq!(r.reaction, None);
        let paid = r.paid.unwrap();
        assert_eq!(
            (
                paid.product_id.as_str(),
                paid.emoji_id.as_str(),
                paid.animated,
                paid.version
            ),
            ("p1", "007", true, 4)
        );
        assert!(
            paid.image_url()
                .ends_with("/p1/sticon/android/007_animation.png?v=4")
        );
        // Numbered fields, as a schema without field 2 decoded them.
        let numbered =
            PaidReaction::from_json(&serde_json::json!({"1": "p2", "2": "001", "3": 1})).unwrap();
        assert_eq!((numbered.animated, numbered.version), (false, 1));
        assert!(PaidReaction::from_json(&serde_json::Value::Null).is_none());
        assert_eq!(
            ev(tv!({"type": 65, "param1": "c1", "param2": "100"})),
            Some(TalkEvent::Unsend {
                chat_mid: s("c1"),
                message_id: s("100")
            })
        );
        assert!(matches!(
            ev(tv!({"type": 50, "param1": "a"})),
            Some(TalkEvent::Call {
                canceled: false,
                ..
            })
        ));
        assert!(matches!(
            ev(tv!({"type": 51})),
            Some(TalkEvent::Call { canceled: true, .. })
        ));
        assert_eq!(
            ev(tv!({"type": 124, "param1": "c1", "param2": "u1", "param3": "u2\u{1e}u3"})),
            Some(TalkEvent::Invite {
                chat_mid: s("c1"),
                inviter_mid: s("u1"),
                invitee_mids: vec![s("u2"), s("u3")]
            })
        );
        assert_eq!(
            ev(tv!({"type": 130, "param1": "c1", "param2": "u2"})),
            Some(TalkEvent::Join {
                chat_mid: s("c1"),
                mid: s("u2")
            })
        );
        assert_eq!(
            ev(tv!({"type": 15, "param1": "c1", "param2": "u2"})),
            Some(TalkEvent::Leave {
                chat_mid: s("c1"),
                mid: s("u2")
            })
        );
        assert_eq!(
            ev(tv!({"type": 133, "param1": "c1", "param2": "u1", "param3": "u2"})),
            Some(TalkEvent::Kick {
                chat_mid: s("c1"),
                kicker_mid: s("u1"),
                kicked_mids: vec![s("u2")]
            })
        );
        assert_eq!(
            ev(tv!({"type": 5, "param1": "u9"})),
            Some(TalkEvent::ContactAdded {
                mid: s("u9"),
                by_me: false
            })
        );
        assert_eq!(
            ev(tv!({"type": 7, "param1": "u9"})),
            Some(TalkEvent::ContactBlocked {
                mid: s("u9"),
                blocked: false
            })
        );
        assert_eq!(ev(tv!({"type": 25})), None);
        assert_eq!(ev(tv!({"type": 9999})), None);
    }

    #[test]
    fn classifies_chat_list_and_membership_ops() {
        let ev = |v: Value| TalkEvent::from_operation(&v);
        let s = |v: &str| v.to_string();
        // 61 = NOTIFIED_LEAVE_CHAT
        assert_eq!(
            ev(tv!({"type": 61, "param1": "c1", "param2": "u2"})),
            Some(TalkEvent::Leave {
                chat_mid: s("c1"),
                mid: s("u2")
            })
        );
        // 127 = DELETE_SELF_FROM_CHAT
        assert_eq!(
            ev(tv!({"type": 127, "param1": "c1"})),
            Some(TalkEvent::LeftChat { chat_mid: s("c1") })
        );
        // 121 = UPDATE_CHAT, 122 = NOTIFIED_UPDATE_CHAT
        assert_eq!(
            ev(tv!({"type": 121, "param1": "c1", "param2": "u1", "param3": "1"})),
            Some(TalkEvent::ChatUpdated {
                chat_mid: s("c1"),
                by_me: true,
                param2: s("u1"),
                param3: s("1")
            })
        );
        assert!(matches!(
            ev(tv!({"type": "NOTIFIED_UPDATE_CHAT", "param1": "c1"})),
            Some(TalkEvent::ChatUpdated { by_me: false, ref chat_mid, .. }) if chat_mid == "c1"
        ));
        // 41 = SEND_CHAT_REMOVED
        assert_eq!(
            ev(tv!({"type": 41, "param1": "u3", "param2": "200"})),
            Some(TalkEvent::ChatRemoved {
                chat_mid: s("u3"),
                last_message_id: s("200")
            })
        );
        // 155/156 = (NOTIFIED_)UPDATE_MESSAGE_REQUEST_BOX
        assert_eq!(
            ev(tv!({"type": 156, "param1": "u4", "param2": "1"})),
            Some(TalkEvent::MessageRequestBox {
                chat_mid: s("u4"),
                in_box: true,
                by_me: false
            })
        );
        assert_eq!(
            ev(tv!({"type": "UPDATE_MESSAGE_REQUEST_BOX", "param1": "u4", "param2": "0"})),
            Some(TalkEvent::MessageRequestBox {
                chat_mid: s("u4"),
                in_box: false,
                by_me: true
            })
        );
        assert_eq!(ev(tv!({"type": 156, "param1": "u4", "param2": "x"})), None);
        assert_eq!(ev(tv!({"type": 156, "param2": "1"})), None);
        // 77 = NOTIFIED_E2EE_KEY_UPDATE, 72 = REGISTER_E2EE_PUBLICKEY
        assert_eq!(
            ev(tv!({"type": 77})),
            Some(TalkEvent::E2eeKeysChanged {
                op: crate::e2ee::E2eeOpKind::KeyUpdate
            })
        );
        assert!(matches!(
            ev(tv!({"type": 72})),
            Some(TalkEvent::E2eeKeysChanged { .. })
        ));
        // 73 = NOTIFIED_E2EE_KEY_EXCHANGE_REQ: not a key change by itself.
        assert_eq!(ev(tv!({"type": 73})), None);
        // 137 = SEND_CHAT_HIDDEN
        assert_eq!(
            ev(tv!({"type": 137, "param1": "c1", "param2": "300", "param3": "1"})),
            Some(TalkEvent::ChatHidden {
                chat_mid: s("c1"),
                last_message_id: s("300"),
                hidden: true
            })
        );
        assert_eq!(
            ev(tv!({"type": 137, "param1": "c1", "param2": "300", "param3": "0"})),
            Some(TalkEvent::ChatHidden {
                chat_mid: s("c1"),
                last_message_id: s("300"),
                hidden: false
            })
        );
        assert_eq!(ev(tv!({"type": 137, "param1": "c1", "param3": "x"})), None);
        // 46 = REMOVE_ALL_MESSAGES
        assert_eq!(
            ev(tv!({"type": 46, "param1": "a"})),
            Some(TalkEvent::AllMessagesRemoved)
        );
    }

    #[tokio::test]
    async fn undecryptable_message_is_delivered_and_logged() {
        let c = client();
        let (mut rx, mut base_rx) = (c.subscribe(), c.base.subscribe());
        let mut message = tv!({"id": "9", "from": "u-them", "to": "u-self", "toType": "USER", "contentType": "NONE"});
        message.insert(
            "contentMetadata",
            Value::Map(vec![("e2eeVersion".into(), "2".into())]),
        );
        message.insert(
            "chunks",
            Value::List(vec![
                Value::Binary(vec![1; 16]),
                Value::Binary(vec![2; 32]),
                Value::Binary(vec![3; 12]),
                Value::Binary(vec![0, 0, 0, 7]),
                Value::Binary(vec![0, 0, 0, 8]),
            ]),
        );
        let mut op = tv!({"type": "RECEIVE_MESSAGE"});
        op.insert("message", message);
        let talk = futures::stream::iter(vec![Ok(op)]).boxed();
        c.listen_with(Some(talk), None).join().await;
        let delivered: Vec<_> = drain(&mut rx)
            .into_iter()
            .filter_map(|e| match e {
                ClientEvent::Message(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(delivered.len(), 1);
        assert!(crate::e2ee::is_undecrypted(&delivered[0].raw));
        assert!(logs(&mut base_rx).contains(&"e2ee".to_string()));
    }

    #[tokio::test]
    async fn key_change_forgets_unresolvable_keys() {
        let c = client();
        let talk =
            futures::stream::iter(vec![Ok(tv!({"type": "NOTIFIED_E2EE_KEY_UPDATE"}))]).boxed();
        let mut rx = c.subscribe();
        c.listen_with(Some(talk), None).join().await;
        assert!(
            drain(&mut rx)
                .iter()
                .any(|e| matches!(e, ClientEvent::Talk(TalkEvent::E2eeKeysChanged { .. })))
        );
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
        tokio::time::timeout(Duration::from_secs(5), h.join())
            .await
            .unwrap();
    }
}
