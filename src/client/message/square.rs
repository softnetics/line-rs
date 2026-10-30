//! [`SquareMessage`] / [`SquareThreadMessage`]: OpenChat messages (linejs
//! `features/message/square.ts`).

use std::ops::Deref;
use std::sync::{Arc, OnceLock};

use super::types::{
    AppLinkInfo, CallInfo, ChatEventInfo, ContactMeta, Decoration, FileInfo, FlexMeta, GiftInfo, MentionTarget, Mid,
    PostNotificationInfo, ReactionType,
};
use super::utils::{fetch_bytes, metadata_value, reply_target, type_error, Content, MessageInput};
use crate::client::Client;
use crate::error::{Error, Result};
use crate::thrift::Value;
use crate::tv;

/// An OpenChat message. `raw` is the thrift `SquareMessage`
/// (`{message, fromType, squareMessageRevision, threadInfo}`).
#[derive(Clone)]
pub struct SquareMessage {
    client: Client,
    pub raw: Value,
    author_is_me: Arc<OnceLock<bool>>,
}

impl std::fmt::Debug for SquareMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SquareMessage").field("raw", &self.raw).finish()
    }
}

impl SquareMessage {
    pub fn new(client: Client, raw: Value) -> Self {
        Self { client, raw, author_is_me: Arc::default() }
    }

    /// From a `NOTIFICATION_MESSAGE` square event.
    pub fn from_event(client: Client, event: &Value) -> Self {
        Self::new(client, event["payload"]["notificationMessage"]["squareMessage"].clone())
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The inner thrift `Message`.
    pub fn message(&self) -> &Value {
        &self.raw["message"]
    }

    pub fn id(&self) -> &str {
        self.message()["id"].as_str().unwrap_or("")
    }

    pub fn text(&self) -> &str {
        self.message()["text"].as_str().unwrap_or("")
    }

    pub fn content_type(&self) -> String {
        self.content().content_type
    }

    pub fn content_metadata(&self) -> indexmap::IndexMap<String, String> {
        self.message()["contentMetadata"].string_map()
    }

    /// The square chat.
    pub fn to(&self) -> Mid {
        Mid {
            id: self.message()["to"].as_str().unwrap_or("").to_string(),
            kind: super::types::enum_name(&self.message()["toType"], "MIDType").unwrap_or_else(|| "SQUARE_CHAT".into()),
        }
    }

    /// The sender (a square member mid).
    pub fn from(&self) -> Mid {
        Mid {
            id: self.message()["from"].as_str().unwrap_or("").to_string(),
            kind: super::types::enum_name(&self.raw["fromType"], "MIDType").unwrap_or_else(|| "SQUARE_MEMBER".into()),
        }
    }

    /// Thread mid, when the message lives in a thread.
    pub fn thread_mid(&self) -> Option<&str> {
        self.raw["threadInfo"]["chatThreadMid"].as_str()
    }

    fn content(&self) -> Content<'_> {
        Content::of(self.message())
    }

    /// `reply` always targets this message; `send` keeps an explicit
    /// `related_message_id` and otherwise also relates to this message
    /// (linejs behaviour).
    pub(crate) fn send_args(&self, mut input: MessageInput, reply: bool) -> MessageInput {
        if reply || input.related_message_id.is_none() {
            input.related_message_id = Some(self.id().to_string());
        }
        input
    }

    /// Reply to this message.
    pub async fn reply(&self, input: impl Into<MessageInput>) -> Result<Value> {
        let input = self.send_args(input.into(), true);
        self.client.send_square_message(&self.to().id, input).await
    }

    /// Send a message to the same square chat.
    pub async fn send(&self, input: impl Into<MessageInput>) -> Result<Value> {
        let input = self.send_args(input.into(), false);
        self.client.send_square_message(&self.to().id, input).await
    }

    pub async fn react(&self, reaction: ReactionType) -> Result<()> {
        self.client
            .base
            .square()
            .call(
                "reactToMessage",
                tv!({"request": {
                    "reqSeq": 0,
                    "reactionType": reaction as i32,
                    "messageId": self.id(),
                    "squareChatMid": self.to().id,
                }}),
            )
            .await?;
        Ok(())
    }

    pub async fn read(&self) -> Result<()> {
        self.client
            .base
            .square()
            .call("markAsRead", tv!({"request": {"messageId": self.id(), "squareChatMid": self.to().id}}))
            .await?;
        Ok(())
    }

    /// Pin the message as the square chat announcement.
    pub async fn announce(&self) -> Result<()> {
        if self.text().is_empty() {
            return Err(type_error("The message is not text message."));
        }
        self.client
            .base
            .square()
            .call(
                "createSquareChatAnnouncement",
                tv!({"createSquareChatAnnouncementRequest": {
                    "reqSeq": 0,
                    "squareChatMid": self.to().id,
                    "squareChatAnnouncement": {
                        "announcementSeq": 0,
                        "contents": {"textMessageAnnouncementContents": {
                            "senderSquareMemberMid": self.from().id,
                            "messageId": self.id(),
                            "text": self.text(),
                        }},
                        "createdAt": self.message()["createdTime"].as_i64().unwrap_or(0),
                        "type": 0,
                    },
                }}),
            )
            .await?;
        Ok(())
    }

    /// Unsend the message (must be ours).
    pub async fn unsend(&self) -> Result<()> {
        if !self.is_my_message().await? {
            return Err(type_error("Cannot unsend the message which is not yours."));
        }
        self.client
            .base
            .square()
            .call("unsendMessage", tv!({"request": {"messageId": self.id(), "squareChatMid": self.to().id}}))
            .await?;
        Ok(())
    }

    /// Delete (destroy) the message.
    pub async fn delete(&self) -> Result<()> {
        self.client
            .base
            .square()
            .call("destroyMessage", tv!({"request": {"messageId": self.id(), "squareChatMid": self.to().id}}))
            .await?;
        Ok(())
    }

    pub fn sticker_url(&self) -> Result<String> {
        self.content().sticker_url()
    }

    pub fn emoji_urls(&self) -> Result<Vec<String>> {
        self.content().emoji_urls()
    }

    pub fn mentions(&self) -> Result<Vec<MentionTarget>> {
        self.content().mentions()
    }

    pub fn text_decorations(&self) -> Result<Vec<Decoration>> {
        self.content().text_decorations()
    }

    pub fn shared_contact(&self) -> Result<ContactMeta> {
        self.content().shared_contact()
    }

    pub fn flex(&self) -> Result<FlexMeta> {
        self.content().flex()
    }

    pub fn file_info(&self) -> Result<FileInfo> {
        self.content().file_info()
    }

    /// Link card of an `APPLINK` / `LINK` / `MUSIC` message.
    pub fn app_link(&self) -> Result<AppLinkInfo> {
        self.content().app_link()
    }

    pub fn gift(&self) -> Result<GiftInfo> {
        self.content().gift()
    }

    /// Call log details of a `CALL` message.
    pub fn call(&self) -> Result<CallInfo> {
        self.content().call()
    }

    pub fn post_notification(&self) -> Result<PostNotificationInfo> {
        self.content().post_notification()
    }

    /// Localization key/args of a `CHATEVENT` system message.
    pub fn chat_event(&self) -> Result<ChatEventInfo> {
        self.content().chat_event()
    }

    pub fn reply_target(&self) -> Option<String> {
        reply_target(self.message())
    }

    /// Whether `mid` is mentioned (by mid) in this message.
    pub fn mentions_mid(&self, mid: &str) -> bool {
        self.text_decorations()
            .map(|d| d.iter().any(|d| d.mention == Some(MentionTarget::User(mid.to_string()))))
            .unwrap_or(false)
    }

    pub async fn data(&self, preview: bool) -> Result<Vec<u8>> {
        let content = self.content();
        if !content.has_media() {
            return Err(type_error("message have no contents"));
        }
        if content.metadata.contains_key("DOWNLOAD_URL") {
            let key = if preview { "PREVIEW_URL" } else { "DOWNLOAD_URL" };
            let url = content.metadata.get(key).cloned().unwrap_or_default();
            return fetch_bytes(&self.client.base.http, &url).await;
        }
        let file = self.client.base.obs().download_message_data(self.id(), preview, true).await?;
        Ok(file.data.to_vec())
    }

    /// Whether we sent this message (compares with our member mid in the
    /// chat; cached).
    pub async fn is_my_message(&self) -> Result<bool> {
        if let Some(v) = self.author_is_me.get() {
            return Ok(*v);
        }
        let chat = self.client.base.square().call("getSquareChat", tv!({"request": {"squareChatMid": self.to().id}})).await?;
        let mine = chat["squareChatMember"]["squareMemberMid"].as_str() == Some(self.from().id.as_str());
        Ok(*self.author_is_me.get_or_init(|| mine))
    }
}

/// A message inside an OpenChat thread. Derefs to [`SquareMessage`] for the
/// accessors; the actions below are thread-aware.
#[derive(Clone, Debug)]
pub struct SquareThreadMessage(pub SquareMessage);

impl Deref for SquareThreadMessage {
    type Target = SquareMessage;
    fn deref(&self) -> &SquareMessage {
        &self.0
    }
}

impl SquareThreadMessage {
    pub fn new(client: Client, raw: Value) -> Self {
        Self(SquareMessage::new(client, raw))
    }

    /// From a `NOTIFICATION_THREAD_MESSAGE` square event.
    pub fn from_event(client: Client, event: &Value) -> Self {
        Self::new(client, event["payload"]["notificationThreadMessage"]["squareMessage"].clone())
    }

    fn thread(&self) -> String {
        self.thread_mid().unwrap_or("").to_string()
    }

    /// Reply in the thread.
    pub async fn reply(&self, input: impl Into<MessageInput>) -> Result<Value> {
        let mut input = input.into();
        input.related_message_id = Some(self.thread());
        self.send_thread_message(input).await
    }

    /// Send a message to the thread.
    pub async fn send(&self, input: impl Into<MessageInput>) -> Result<Value> {
        self.send_thread_message(input.into()).await
    }

    async fn send_thread_message(&self, input: MessageInput) -> Result<Value> {
        let base = &self.client.base;
        let to = self.to().id;
        let mut message = tv!({"to": to, "text": input.text, "contentType": "NONE", "toType": "SQUARE_THREAD"});
        if !input.content_metadata.is_empty() {
            message.insert("contentMetadata", metadata_value(&input.content_metadata));
        }
        if let Some(rel) = input.related_message_id {
            message.insert("relatedMessageId", rel);
            message.insert("relatedMessageServiceCode", "SQUARE");
            message.insert("messageRelationType", "REPLY");
        }
        let mut thread_message = Value::object();
        thread_message.insert("message", message);
        let mut request = tv!({"reqSeq": base.get_reqseq("sq").await?, "chatMid": to, "threadMid": self.thread()});
        request.insert("threadMessage", thread_message);
        let mut args = Value::object();
        args.insert("request", request);
        base.square().call("sendSquareThreadMessage", args).await
    }

    pub async fn react(&self, reaction: ReactionType) -> Result<()> {
        self.client
            .base
            .square()
            .call(
                "reactToMessage",
                tv!({"request": {
                    "reqSeq": 0,
                    "reactionType": reaction as i32,
                    "messageId": self.id(),
                    "squareChatMid": self.to().id,
                    "threadMid": self.thread(),
                }}),
            )
            .await?;
        Ok(())
    }

    pub async fn read(&self) -> Result<()> {
        self.client
            .base
            .square()
            .call(
                "markAsRead",
                tv!({"request": {"messageId": self.id(), "squareChatMid": self.to().id, "threadMid": self.thread()}}),
            )
            .await?;
        Ok(())
    }

    /// Not supported for thread messages (as in linejs).
    pub async fn announce(&self) -> Result<()> {
        Err(Error::internal("Unimplemented", "announce is not implemented for thread messages"))
    }

    pub async fn unsend(&self) -> Result<()> {
        if !self.is_my_message().await? {
            return Err(type_error("Cannot unsend the message which is not yours."));
        }
        self.client
            .base
            .square()
            .call(
                "unsendMessage",
                tv!({"request": {"messageId": self.id(), "squareChatMid": self.to().id, "threadMid": self.thread()}}),
            )
            .await?;
        Ok(())
    }

    pub async fn delete(&self) -> Result<()> {
        self.client
            .base
            .square()
            .call(
                "destroyMessage",
                tv!({"request": {"messageId": self.id(), "squareChatMid": self.to().id, "threadMid": self.thread()}}),
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::BaseClient;
    use crate::device::Device;

    fn message() -> SquareMessage {
        let base = BaseClient::new(Device::Android).unwrap();
        base.set_profile(Some(tv!({"mid": "u-self"})));
        SquareMessage::new(
            Client::new(base),
            tv!({
                "message": {"id": "sq-1", "to": "sq-chat", "from": "u-other", "toType": "SQUARE_CHAT",
                            "contentType": "NONE", "contentMetadata": {}, "text": "hi", "createdTime": 0},
                "fromType": "SQUARE_MEMBER",
            }),
        )
    }

    // linejs once tested `isMyMessage` as a property (always truthy), so the
    // ownership guard never fired. Here the cached answer short-circuits the
    // network lookup.
    #[tokio::test]
    async fn unsend_rejects_messages_that_are_not_mine() {
        let msg = message();
        msg.author_is_me.set(false).unwrap();
        let err = msg.unsend().await.unwrap_err();
        assert!(err.to_string().contains("Cannot unsend the message which is not yours."));
        // Thread messages share the guard.
        let thread = SquareThreadMessage(msg);
        assert!(thread.unsend().await.is_err());
    }

    #[test]
    fn accessors() {
        let msg = message();
        assert_eq!(msg.to(), Mid { id: "sq-chat".into(), kind: "SQUARE_CHAT".into() });
        assert_eq!(msg.from().id, "u-other");
        assert_eq!(msg.text(), "hi");
        assert_eq!(msg.content_type(), "NONE");
        assert!(msg.thread_mid().is_none());
        assert_eq!(msg.send_args(MessageInput::text("x"), true).related_message_id.as_deref(), Some("sq-1"));
        let explicit = MessageInput::text("x").reply_to("other");
        assert_eq!(msg.send_args(explicit.clone(), false).related_message_id.as_deref(), Some("other"));
        assert_eq!(msg.send_args(explicit, true).related_message_id.as_deref(), Some("sq-1"));
    }
}
