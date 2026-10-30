//! [`TalkMessage`]: a message in a 1:1 / group / room chat (linejs
//! `features/message/talk.ts`).

use super::types::{
    AppLinkInfo, CallInfo, ChatEventInfo, ContactMeta, Decoration, FileInfo, FlexMeta, GiftInfo, MentionTarget, Mid,
    PostNotificationInfo, ReactionType,
};
use super::utils::{fetch_bytes, reply_target, type_error, Content, MessageInput};
use crate::client::Client;
use crate::e2ee::E2eeContent;
use crate::error::Result;
use crate::thrift::Value;
use crate::tv;

/// A talk message. `raw` is the (decrypted) thrift `Message`.
#[derive(Clone)]
pub struct TalkMessage {
    client: Client,
    pub raw: Value,
}

impl std::fmt::Debug for TalkMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TalkMessage").field("raw", &self.raw).finish()
    }
}

impl TalkMessage {
    /// Wrap an already decrypted `Message`.
    pub fn new(client: Client, raw: Value) -> Self {
        Self { client, raw }
    }

    /// Wrap a `Message`, decrypting it first when it carries `e2eeVersion`.
    pub async fn from_raw(client: Client, raw: Value) -> Result<Self> {
        let raw = if raw["contentMetadata"].get("e2eeVersion").is_some() {
            client.base.decrypt_e2ee_message(raw).await?
        } else {
            raw
        };
        Ok(Self { client, raw })
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Message id.
    pub fn id(&self) -> &str {
        self.raw["id"].as_str().unwrap_or("")
    }

    pub fn text(&self) -> &str {
        self.raw["text"].as_str().unwrap_or("")
    }

    /// `ContentType` variant name.
    pub fn content_type(&self) -> String {
        self.content().content_type
    }

    pub fn content_metadata(&self) -> indexmap::IndexMap<String, String> {
        self.raw["contentMetadata"].string_map()
    }

    /// The recipient (user, group or room).
    pub fn to(&self) -> Mid {
        Mid {
            id: self.raw["to"].as_str().unwrap_or("").to_string(),
            kind: super::types::enum_name(&self.raw["toType"], "MIDType").unwrap_or_else(|| "USER".into()),
        }
    }

    /// The sender (always a user).
    pub fn from(&self) -> Mid {
        Mid { id: self.raw["from"].as_str().unwrap_or("").to_string(), kind: "USER".into() }
    }

    pub fn is_my_message(&self) -> bool {
        self.client.base.mid().is_some_and(|m| m == self.from().id)
    }

    /// Whether `to` is a group or room.
    pub fn is_group(&self) -> bool {
        matches!(self.to().kind.as_str(), "GROUP" | "ROOM")
    }

    /// The chat this message belongs to: the group/room, or for 1:1 chats the
    /// counterpart's mid.
    pub fn chat_mid(&self) -> String {
        if self.is_group() || self.is_my_message() {
            self.to().id
        } else {
            self.from().id
        }
    }

    /// Whether the message was edited (`EDITED` / `UPDATED_TIME` metadata or
    /// a non-zero `updatedTime`).
    pub fn is_edited(&self) -> bool {
        self.raw["contentMetadata"]["EDITED"].as_str() == Some("true") || self.updated_time().is_some()
    }

    /// Time of the last edit (ms), or `None` if never edited.
    pub fn updated_time(&self) -> Option<i64> {
        if let Some(t) = self.raw["contentMetadata"]["UPDATED_TIME"].as_str().and_then(|s| s.trim().parse::<f64>().ok())
        {
            return Some(t as i64);
        }
        self.raw["updatedTime"].as_i64().filter(|t| *t != 0)
    }

    fn content(&self) -> Content<'_> {
        Content::of(&self.raw)
    }

    /// Reply to this message (in the same chat).
    pub async fn reply(&self, input: impl Into<MessageInput>) -> Result<TalkMessage> {
        let mut input = input.into();
        input.related_message_id = Some(self.id().to_string());
        if self.raw.get("chunks").is_some() {
            input.e2ee = Some(true);
        }
        self.client.send_message(&self.chat_mid(), input).await
    }

    /// Send a message to the same chat (not threaded as a reply unless
    /// `related_message_id` is set).
    pub async fn send(&self, input: impl Into<MessageInput>) -> Result<TalkMessage> {
        let mut input = input.into();
        if self.raw.get("chunks").is_some() && input.e2ee.is_none() {
            input.e2ee = Some(true);
        }
        self.client.send_message(&self.chat_mid(), input).await
    }

    /// React to this message.
    pub async fn react(&self, reaction: ReactionType) -> Result<()> {
        let base = &self.client.base;
        let req_seq = base.get_reqseq("talk").await?;
        base.talk()
            .call(
                "react",
                tv!({"reactRequest": {
                    "reqSeq": req_seq,
                    "messageId": self.id(),
                    "reactionType": {"predefinedReactionType": reaction.as_str()},
                }}),
            )
            .await?;
        Ok(())
    }

    /// `cancelReaction` arguments (the message id must be numeric).
    pub(crate) async fn cancel_reaction_args(&self) -> Result<Value> {
        let id: i64 = self.id().parse().map_err(|_| type_error("The message id is not numeric."))?;
        let req_seq = self.client.base.get_reqseq("talk").await?;
        Ok(tv!({"cancelReactionRequest": {"reqSeq": req_seq, "messageId": id}}))
    }

    /// Withdraw our reaction from this message.
    pub async fn cancel_reaction(&self) -> Result<()> {
        let args = self.cancel_reaction_args().await?;
        self.client.base.talk().call("cancelReaction", args).await?;
        Ok(())
    }

    /// Report a Flex postback action on this message: `url` is the action's
    /// postback URL (`sendPostback`, origin = the message sender).
    pub async fn send_postback(&self, url: &str) -> Result<()> {
        self.client.base.talk().send_flex_postback(self.id(), url, &self.chat_mid(), &self.from().id).await?;
        Ok(())
    }

    /// `sendChatChecked` arguments for [`read`](Self::read).
    pub(crate) async fn read_args(&self) -> Result<Value> {
        let seq = self.client.base.get_reqseq("talk").await?;
        Ok(tv!({"chatMid": self.chat_mid(), "lastMessageId": self.id(), "seq": seq}))
    }

    /// Mark the chat as read up to this message.
    pub async fn read(&self) -> Result<()> {
        let args = self.read_args().await?;
        self.client.base.talk().call("sendChatChecked", args).await?;
        Ok(())
    }

    /// Pin the message as the chat announcement.
    pub async fn announce(&self) -> Result<()> {
        let text = self.text();
        if text.is_empty() {
            return Err(type_error("The message is not text message."));
        }
        if !self.is_group() {
            return Err(type_error("Cannot announce out of group."));
        }
        let to = self.to().id;
        self.client
            .base
            .talk()
            .call(
                "createChatRoomAnnouncement",
                tv!({
                    "chatRoomMid": to,
                    "type": "MESSAGE",
                    "contents": {
                        "text": text,
                        "link": format!("line://nv/chatMsg?chatId={to}&messageId={}", self.id()),
                    },
                }),
            )
            .await?;
        Ok(())
    }

    /// Replace the text of one of our text messages (`editMessage`; gated by
    /// LINE/LYP Premium server-side). An E2EE message is re-encrypted for the
    /// same recipient: plaintext edits of Letter Sealing messages are refused
    /// like plain sends (`E2EE_RETRY_ENCRYPT`).
    pub async fn edit(&self, text: &str) -> Result<Value> {
        if !self.is_my_message() {
            return Err(type_error("Cannot edit the message which is not yours."));
        }
        let mut edited = self.raw.clone();
        // The sendMessage echo can omit `toType`, which editMessage requires
        // ("toType field is required").
        if edited["toType"].is_null()
            && let Some(t) = crate::base::BaseClient::get_to_type(&self.to().id)
        {
            edited.insert("toType", t);
        }
        if self.raw["chunks"].as_list().is_some_and(|c| !c.is_empty()) {
            let chunks = self
                .client
                .base
                .encrypt_e2ee_message(&self.to().id, E2eeContent::Text(text.to_string()), 0, 2)
                .await?;
            edited.insert("chunks", Value::List(chunks.into_iter().map(Value::Binary).collect()));
            edited.insert("text", Value::Null);
        } else {
            edited.insert("text", text);
        }
        self.client.base.talk().edit_message(edited).await
    }

    /// Unsend the message (must be ours).
    pub async fn unsend(&self) -> Result<()> {
        if !self.is_my_message() {
            return Err(type_error("Cannot unsend the message which is not yours."));
        }
        self.client.base.talk().call("unsendMessage", tv!({"messageId": self.id()})).await?;
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

    /// Split the text into plain / mention / emoji runs.
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

    /// Id of the message this one replies to.
    pub fn reply_target(&self) -> Option<String> {
        reply_target(&self.raw)
    }

    /// Download the attached media (image / video / audio / file).
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
        let obs = self.client.base.obs();
        if self.raw["chunks"].as_list().is_some_and(|c| !c.is_empty())
            && let Some(file) = obs.download_e2ee_message_media(&self.raw).await? {
                return Ok(file.data.to_vec());
            }
        Ok(obs.download_message_data(self.id(), preview, false).await?.data.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::BaseClient;
    use crate::device::Device;

    fn client() -> Client {
        let base = BaseClient::new(Device::Android).unwrap();
        base.set_profile(Some(tv!({"mid": "u-self"})));
        Client::new(base)
    }

    fn message(raw: Value) -> TalkMessage {
        TalkMessage::new(client(), raw)
    }

    #[test]
    fn is_edited_never_edited() {
        assert!(!message(tv!({"text": "hi"})).is_edited());
        assert!(!message(tv!({"text": "hi", "updatedTime": 0})).is_edited());
        assert!(!message(tv!({"text": "hi", "contentMetadata": {"e2eeVersion": "2"}})).is_edited());
    }

    #[test]
    fn is_edited_edit_operation_metadata() {
        let m = message(tv!({
            "id": "1234567890123456789",
            "contentMetadata": {"e2eeVersion": "2", "EDITED": "true", "UPDATED_TIME": "1787927539000"},
        }));
        assert!(m.is_edited());
        assert_eq!(m.updated_time(), Some(1787927539000));
    }

    #[test]
    fn is_edited_updated_time_field() {
        assert!(message(tv!({"text": "hi", "updatedTime": 1700000000000_i64})).is_edited());
    }

    #[test]
    fn is_edited_message_box_metadata() {
        let m = message(tv!({"id": "1234567890123456789", "contentMetadata": {"UPDATED_TIME": "1788432321630"}}));
        assert!(m.is_edited());
        assert_eq!(m.updated_time(), Some(1788432321630));
    }

    #[test]
    fn updated_time() {
        assert_eq!(message(tv!({"text": "hi"})).updated_time(), None);
        assert_eq!(message(tv!({"text": "hi", "updatedTime": 0})).updated_time(), None);
        assert_eq!(message(tv!({"updatedTime": 1700000000000_i64})).updated_time(), Some(1700000000000));
        assert_eq!(
            message(tv!({"updatedTime": 1700000000000_i64, "contentMetadata": {"UPDATED_TIME": "1787927539000"}}))
                .updated_time(),
            Some(1787927539000)
        );
    }

    fn raw(to: &str, from: &str, to_type: &str) -> Value {
        tv!({"id": "msg-1", "to": to, "from": from, "toType": to_type, "contentType": "NONE",
             "contentMetadata": {}, "text": "hi"})
    }

    #[tokio::test]
    async fn read_marks_the_group_chat_itself() {
        let m = message(raw("g-group", "u-them", "GROUP"));
        let args = m.read_args().await.unwrap();
        assert_eq!(args["chatMid"].as_str(), Some("g-group"));
        assert_eq!(args["lastMessageId"].as_str(), Some("msg-1"));
        assert_eq!(args["seq"].as_i64(), Some(0));
    }

    #[tokio::test]
    async fn read_keeps_counterpart_mid_for_dms() {
        let m = message(raw("u-self", "u-them", "USER"));
        assert_eq!(m.read_args().await.unwrap()["chatMid"].as_str(), Some("u-them"));
        // Numeric toType values are resolved through the IDL enum.
        let mut sent = raw("u-them", "u-self", "USER");
        sent.insert("toType", 0);
        let m = message(sent);
        assert!(m.is_my_message());
        assert_eq!(m.to().kind, "USER");
        assert_eq!(m.chat_mid(), "u-them");
    }

    #[tokio::test]
    async fn cancel_reaction_args_need_numeric_id() {
        let m = message(raw("g", "u-them", "GROUP"));
        assert!(m.cancel_reaction_args().await.is_err());
        let mut r = raw("g", "u-them", "GROUP");
        r.insert("id", "9007199254740993");
        let args = message(r).cancel_reaction_args().await.unwrap();
        assert_eq!(args["cancelReactionRequest"]["messageId"].as_i64(), Some(9007199254740993));
        crate::thrift::encode_struct(crate::thrift::Schema::line(), "cancelReaction_args", &args).unwrap();
    }

    #[test]
    fn typed_content_accessors() {
        let m = message(tv!({"contentType": "FILE", "contentMetadata": {"FILE_NAME": "a.txt", "FILE_SIZE": "3"}}));
        assert_eq!(m.file_info().unwrap().name, "a.txt");
        let m = message(tv!({"contentType": "GIFT", "contentMetadata": {"PRDID": "1", "PRDTYPE": "THEME"}}));
        assert_eq!(m.gift().unwrap().product_type, "THEME");
        assert!(m.call().is_err() && m.app_link().is_err() && m.chat_event().is_err() && m.post_notification().is_err());
    }

    #[tokio::test]
    async fn unsend_rejects_foreign_messages() {
        let m = message(raw("u-self", "u-them", "USER"));
        let err = m.unsend().await.unwrap_err();
        assert!(err.to_string().contains("Cannot unsend the message which is not yours."));
    }
}
