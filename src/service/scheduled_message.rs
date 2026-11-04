//! `ScheduledMessageService` (`/SCHD4`): messages queued to be sent later.
//!
//! Recovered by static RE of LINE Android 26.x; unverified against a live
//! server. The server sends a scheduled message as given, so for an E2EE chat
//! pass an already-encrypted `Message` (as built for `sendMessage`).

use super::ScheduledMessageService;
use crate::error::Result;
use crate::thrift::Value;

fn request_arg(req: Value) -> Value {
    let mut args = Value::object();
    args.insert("request", req);
    args
}

/// `ScheduledMessageRequest`: the message and when to send it (epoch ms).
fn scheduled(message: Value, scheduled_time_ms: i64) -> Value {
    let mut req = crate::tv!({"schedule": {"initialScheduledTime": scheduled_time_ms}});
    req.insert("message", message);
    req
}

impl ScheduledMessageService<'_> {
    /// Schedule `message` to be sent at `scheduled_time_ms` (epoch ms).
    /// Returns `{scheduledMessageId, registeredTime}`. `reqSeq` auto.
    pub async fn register(
        &self,
        message: impl Into<Value>,
        scheduled_time_ms: i64,
    ) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await?;
        let mut req = crate::tv!({"reqSeq": seq});
        req.insert(
            "scheduledMessage",
            scheduled(message.into(), scheduled_time_ms),
        );
        self.call("doRegister", request_arg(req)).await
    }

    /// One page of scheduled messages, starting after
    /// `start_scheduled_message_id` (`None` for the first page). The
    /// response carries `nextScheduledMessageId` for the next page.
    pub async fn get_by_pagination(
        &self,
        start_scheduled_message_id: Option<i64>,
        page_size: i32,
    ) -> Result<Value> {
        let mut req = crate::tv!({"pageSize": page_size});
        req.insert("startScheduledMessageId", start_scheduled_message_id);
        self.call("getByPagination", request_arg(req)).await
    }

    /// Replace a scheduled message's content and time. `reqSeq` auto.
    pub async fn update(
        &self,
        scheduled_message_id: i64,
        message: impl Into<Value>,
        scheduled_time_ms: i64,
    ) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await?;
        let mut req = crate::tv!({"reqSeq": seq, "scheduledMessageId": scheduled_message_id});
        req.insert(
            "scheduledMessage",
            scheduled(message.into(), scheduled_time_ms),
        );
        self.call("update", request_arg(req)).await
    }

    /// Cancel scheduled messages. The response lists which ids succeeded and
    /// which failed (with a `ScheduledMessageFailureCode`).
    pub async fn unregister(&self, scheduled_message_ids: &[i64]) -> Result<Value> {
        self.call(
            "unregister",
            crate::tv!({"request": {"scheduledMessageIds": scheduled_message_ids}}),
        )
        .await
    }

    /// Send a scheduled message right away. `message` is its current content.
    pub async fn send_now(
        &self,
        scheduled_message_id: i64,
        message: impl Into<Value>,
    ) -> Result<Value> {
        let mut req = crate::tv!({"scheduledMessageId": scheduled_message_id});
        req.insert("message", message.into());
        self.call("sendNow", request_arg(req)).await
    }

    /// Re-upload scheduled E2EE messages encrypted with a new key, as a map
    /// of scheduled message id to `Message` (after a key rotation; see
    /// `lastE2EEKeyIds` in [`get_by_pagination`](Self::get_by_pagination)).
    pub async fn e2ee_reencrypt(&self, messages: impl Into<Value>) -> Result<Value> {
        let mut req = Value::object();
        req.insert("messages", messages.into());
        self.call("e2eeReencrypt", request_arg(req)).await
    }
}

#[cfg(test)]
mod tests {
    use crate::service::test_util::{encode, path};
    use crate::thrift::TValue;

    #[test]
    fn register_request_layout() {
        let args = encode(
            "doRegister_args",
            &crate::tv!({"request": {
                "reqSeq": 4,
                "scheduledMessage": {
                    "message": {"to": "u1", "text": "hi"},
                    "schedule": {"initialScheduledTime": 1700000000000_i64},
                },
            }}),
        );
        assert!(matches!(path(&args, &[1, 1]), Some(TValue::I32(4))));
        assert!(matches!(path(&args, &[1, 2, 1, 10]), Some(TValue::Binary(b)) if b == b"hi"));
        assert!(matches!(
            path(&args, &[1, 2, 2, 1]),
            Some(TValue::I64(1700000000000))
        ));
    }

    #[test]
    fn unregister_is_a_set_of_ids() {
        let args = encode(
            "unregister_args",
            &crate::tv!({"request": {"scheduledMessageIds": [1, 2]}}),
        );
        assert!(matches!(path(&args, &[1, 1]), Some(TValue::Set(_, ids)) if ids.len() == 2));
    }
}
