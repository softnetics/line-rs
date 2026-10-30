//! [`Chat`]: a talk group chat (linejs `features/chat`).

use super::message::{MessageInput, TalkMessage};
use super::{list, Client};
use crate::error::{Error, Result};
use crate::obs::{E2eeMediaUpload, ObjType, ObsFile};
use crate::thrift::Value;
use crate::tv;

/// Result of [`Chat::send_media`].
#[derive(Debug, Clone)]
pub enum SentMedia {
    /// Plain OBS upload in "reqseq" mode: the server created the message.
    Plain { obj_id: String, obj_hash: String },
    /// Letter Sealing upload: the sent (still encrypted) `Message`.
    E2ee(Value),
}

/// Whether media to `mid` goes through the E2EE path: requested, and the
/// mid is a user or group (rooms have no Letter Sealing media path).
pub(crate) fn uses_e2ee_media(mid: &str, e2ee: bool) -> bool {
    e2ee && (mid.starts_with('u') || mid.starts_with('c'))
}

/// `setChatHiddenStatus` arguments.
pub(crate) fn hidden_status_args(req_seq: i64, chat_mid: &str, last_message_id: i64, hidden: bool) -> Value {
    tv!({"setChatHiddenStatusRequest": {
        "reqSeq": req_seq, "chatMid": chat_mid, "lastMessageId": last_message_id, "hidden": hidden,
    }})
}

/// `sendChatChecked` / `sendChatRemoved` arguments.
pub(crate) fn chat_marker_args(seq: i64, chat_mid: &str, last_message_id: &str) -> Value {
    tv!({"seq": seq, "chatMid": chat_mid, "lastMessageId": last_message_id, "sessionId": 0})
}

/// A talk chat (group), not an OpenChat. `raw` is the thrift `Chat`.
#[derive(Clone)]
pub struct Chat {
    client: Client,
    pub raw: Value,
}

impl std::fmt::Debug for Chat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Chat").field("raw", &self.raw).finish()
    }
}

impl Chat {
    pub fn new(client: Client, raw: Value) -> Self {
        Self { client, raw }
    }

    pub fn mid(&self) -> &str {
        self.raw["chatMid"].as_str().unwrap_or("")
    }

    pub fn name(&self) -> &str {
        self.raw["chatName"].as_str().unwrap_or("")
    }

    /// Send a message. E2EE defaults to on (`input.e2ee = Some(false)` to
    /// disable).
    pub async fn send_message(&self, input: impl Into<MessageInput>) -> Result<TalkMessage> {
        let mut input = input.into();
        input.e2ee.get_or_insert(true);
        self.client.send_message(self.mid(), input).await
    }

    /// Send a compact talk message through `/CA5` or `/ECA5`.
    pub async fn send_compact_message(&self, text: &str) -> Result<crate::service::talk::compact::CompactMessageResponse> {
        self.client.send_compact_message(self.mid(), text).await
    }

    /// Send media (image / gif / video / audio / file). With `e2ee` (and a
    /// user or group mid) it is encrypted and uploaded with Letter Sealing,
    /// otherwise uploaded in "reqseq" mode so the server creates the
    /// message (not a reply). `duration_ms` is the audio/video length.
    pub async fn send_media(
        &self,
        o_type: ObjType,
        file: ObsFile,
        duration_ms: Option<u64>,
        e2ee: bool,
    ) -> Result<SentMedia> {
        let obs = self.client.base.obs();
        if uses_e2ee_media(self.mid(), e2ee) {
            let sent = obs
                .upload_media_by_e2ee(E2eeMediaUpload {
                    file,
                    o_type,
                    to: self.mid().to_string(),
                    preview: None,
                    duration_ms: duration_ms.map(|d| d as f64),
                })
                .await?;
            return Ok(SentMedia::E2ee(sent));
        }
        let up = obs.upload_obj_talk(self.mid(), o_type, file, None, duration_ms).await?;
        Ok(SentMedia::Plain { obj_id: up.obj_id, obj_hash: up.obj_hash })
    }

    fn named(data: Vec<u8>, filename: Option<&str>) -> ObsFile {
        let file = ObsFile::new(data);
        match filename {
            Some(name) => file.name(name),
            None => file,
        }
    }

    /// Send an image (E2EE; see [`send_media`](Self::send_media)).
    pub async fn send_image(&self, data: Vec<u8>, filename: Option<&str>) -> Result<SentMedia> {
        self.send_media(ObjType::Image, Self::named(data, filename), None, true).await
    }

    pub async fn send_video(&self, data: Vec<u8>, filename: Option<&str>, duration_ms: Option<u64>) -> Result<SentMedia> {
        self.send_media(ObjType::Video, Self::named(data, filename), duration_ms, true).await
    }

    pub async fn send_audio(&self, data: Vec<u8>, filename: Option<&str>, duration_ms: Option<u64>) -> Result<SentMedia> {
        let file = Self::named(data, Some(filename.unwrap_or("voice.m4a")));
        self.send_media(ObjType::Audio, file, duration_ms, true).await
    }

    pub async fn send_file(&self, data: Vec<u8>, filename: &str) -> Result<SentMedia> {
        self.send_media(ObjType::File, Self::named(data, Some(filename)), None, true).await
    }

    /// Share a contact card (`CONTACT` message).
    pub async fn send_contact(&self, mid: &str, display_name: &str) -> Result<TalkMessage> {
        self.send_message(MessageInput::contact(mid, display_name)).await
    }

    /// `updateChat` with a partial `Chat` and the `Pb1_O2` attribute name
    /// (e.g. `NAME`).
    pub async fn update_chat(&self, chat: Value, updated_attribute: &str) -> Result<Value> {
        let mut chat = chat;
        if chat.get("chatMid").is_none() {
            chat.insert("chatMid", self.mid());
        }
        let mut request =
            tv!({"updatedAttribute": updated_attribute, "reqSeq": self.client.base.get_reqseq("talk").await?});
        request.insert("chat", chat);
        let mut args = Value::object();
        args.insert("request", request);
        self.client.base.talk().call("updateChat", args).await
    }

    pub async fn update_name(&self, name: &str) -> Result<Value> {
        self.update_chat(tv!({"chatName": name}), "NAME").await
    }

    /// Turn notifications off (`true`) or on (`false`).
    pub async fn set_muted(&self, muted: bool) -> Result<Value> {
        self.update_chat(tv!({"notificationDisabled": muted}), "NOTIFICATION_SETTING").await
    }

    pub async fn mute(&self) -> Result<Value> {
        self.set_muted(true).await
    }

    pub async fn unmute(&self) -> Result<Value> {
        self.set_muted(false).await
    }

    /// Add to (`true`) or remove from (`false`) favorites.
    pub async fn set_favorite(&self, favorite: bool) -> Result<Value> {
        let ts = if favorite { now_ms() } else { 0 };
        self.update_chat(tv!({"favoriteTimestamp": ts}), "FAVORITE_TIMESTAMP").await
    }

    pub async fn favorite(&self) -> Result<Value> {
        self.set_favorite(true).await
    }

    pub async fn unfavorite(&self) -> Result<Value> {
        self.set_favorite(false).await
    }

    /// Hide (or unhide) the chat from the list (`setChatHiddenStatus`).
    /// `last_message_id` is the newest message id the hide applies to.
    pub async fn set_hidden(&self, hidden: bool, last_message_id: i64) -> Result<Value> {
        let seq = self.client.base.get_reqseq("talk").await?;
        self.client.base.talk().call("setChatHiddenStatus", hidden_status_args(seq, self.mid(), last_message_id, hidden)).await
    }

    /// Mark the chat as read up to `last_message_id` (`sendChatChecked`).
    pub async fn mark_as_read(&self, last_message_id: &str) -> Result<Value> {
        let seq = self.client.base.get_reqseq("talk").await?;
        self.client.base.talk().call("sendChatChecked", chat_marker_args(seq, self.mid(), last_message_id)).await
    }

    /// Delete the local history up to `last_message_id` (`sendChatRemoved`).
    pub async fn delete_history(&self, last_message_id: &str) -> Result<Value> {
        let seq = self.client.base.get_reqseq("talk").await?;
        self.client.base.talk().call("sendChatRemoved", chat_marker_args(seq, self.mid(), last_message_id)).await
    }

    /// Pinned announcements (`list<ChatRoomAnnouncement>`).
    pub async fn announcements(&self) -> Result<Vec<Value>> {
        let res = self.client.base.talk().call("getChatRoomAnnouncements", tv!({"chatRoomMid": self.mid()})).await?;
        Ok(list(&res))
    }

    /// Remove an announcement by its `announcementSeq`.
    pub async fn remove_announcement(&self, announcement_seq: i64) -> Result<Value> {
        let seq = self.client.base.get_reqseq("talk").await?;
        self.client
            .base
            .talk()
            .call(
                "removeChatRoomAnnouncement",
                tv!({"reqSeq": seq, "chatRoomMid": self.mid(), "announcementSeq": announcement_seq}),
            )
            .await
    }

    /// Who read what (`getMessageReadRange`): the chat's `TMessageReadRange`,
    /// if any.
    pub async fn read_range(&self) -> Result<Option<Value>> {
        let res = self
            .client
            .base
            .talk()
            .call("getMessageReadRange", tv!({"chatIds": [self.mid()], "syncReason": "INTERNAL"}))
            .await?;
        Ok(list(&res).into_iter().find(|r| r["chatId"].as_str() == Some(self.mid())))
    }

    pub async fn invite(&self, mids: &[&str]) -> Result<Value> {
        self.client
            .base
            .talk()
            .call("inviteIntoChat", tv!({"request": {"targetUserMids": mids, "chatMid": self.mid()}}))
            .await
    }

    pub async fn kick(&self, mid: &str) -> Result<Value> {
        self.client
            .base
            .talk()
            .call("deleteOtherFromChat", tv!({"request": {"targetUserMids": [mid], "chatMid": self.mid()}}))
            .await
    }

    pub async fn leave(&self) -> Result<Value> {
        self.client.base.talk().call("deleteSelfFromChat", tv!({"request": {"chatMid": self.mid()}})).await
    }

    /// The latest `limit` messages.
    pub async fn fetch_messages(&self, limit: i32) -> Result<Vec<TalkMessage>> {
        self.message_fetcher().await?.fetch(limit).await
    }

    /// A cursor over the chat history, newest first.
    ///
    /// `getMessageBoxes` only returns the first page of boxes, so a chat that
    /// is missing from it (e.g. Keep Memo) is located through its newest
    /// message (`getRecentMessagesV2`, one call), which the fetcher then
    /// returns first.
    pub async fn message_fetcher(&self) -> Result<MessageFetcher> {
        let boxes = self.client.base.talk().call("getMessageBoxes", tv!({"messageBoxListRequest": {}})).await?;
        let mid = self.mid();
        let found = list(&boxes["messageBoxes"]).into_iter().find(|b| b["id"].as_str() == Some(mid));
        if let Some(found) = found {
            return Ok(MessageFetcher::new(self.client.clone(), found));
        }
        let recent = list(&self.client.base.talk().get_recent_messages_v2(mid, 1).await?);
        if recent.is_empty() {
            return Err(Error::internal("NotFound", "Message box not found and the chat has no messages."));
        }
        let mut fetcher = MessageFetcher::new(self.client.clone(), tv!({"id": mid}));
        fetcher.advance(&recent);
        fetcher.pending = recent;
        Ok(fetcher)
    }

    /// The chat BGM, if set (`getChatRoomBGMs`).
    pub async fn bgm(&self) -> Result<Option<Value>> {
        let res = self.client.base.talk().call("getChatRoomBGMs", tv!({"chatRoomMids": [self.mid()]})).await?;
        Ok(res.get(self.mid()).cloned())
    }

    /// Set (`Some`) or clear (`None`) the chat BGM.
    pub async fn set_bgm(&self, bgm_info: Option<&str>) -> Result<Value> {
        let seq = self.client.base.get_reqseq("talk").await?;
        self.client
            .base
            .talk()
            .call(
                "updateChatRoomBGM",
                tv!({"reqSeq": seq, "chatRoomMid": self.mid(), "chatRoomBGMInfo": bgm_info.unwrap_or("")}),
            )
            .await
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Client {
    /// Chat effect definitions (`getChatEffectMetaList`) for `Pb1_Q2`
    /// categories (`BACKGROUND`, `KEYWORD`, `CONTENT_METADATA_TAG_BASED`).
    pub async fn chat_effects(&self, categories: &[&str]) -> Result<Vec<Value>> {
        let res = self.base.talk().call("getChatEffectMetaList", tv!({"categories": categories})).await?;
        Ok(list(&res))
    }
}

/// Paginates `getPreviousMessagesV2WithRequest` backwards from the message
/// box's last delivered message.
pub struct MessageFetcher {
    client: Client,
    box_id: String,
    /// `MessageBoxV2MessageId` cursor (`{deliveredTime, messageId}`).
    cursor: Value,
    /// Messages already known to be newer than `cursor`, returned first.
    pending: Vec<Value>,
}

impl MessageFetcher {
    fn new(client: Client, message_box: Value) -> Self {
        Self {
            client,
            box_id: message_box["id"].as_str().unwrap_or("").to_string(),
            cursor: message_box["lastDeliveredMessageId"].clone(),
            pending: Vec::new(),
        }
    }

    pub(crate) fn request_args(&self, limit: i32) -> Value {
        let mut request = tv!({"messageBoxId": self.box_id, "messagesCount": limit});
        request.insert("endMessageId", self.cursor.clone());
        let mut args = Value::object();
        args.insert("request", request);
        args
    }

    /// Move the cursor to the oldest message of a page; empty pages leave it
    /// unchanged. Message ids are 64-bit and kept lossless.
    pub(crate) fn advance(&mut self, page: &[Value]) {
        if let Some(last) = page.last() {
            let id = last["id"].as_i64().unwrap_or(0);
            self.cursor = tv!({"deliveredTime": last["deliveredTime"].as_i64().unwrap_or(0), "messageId": id});
        }
    }

    /// The next `limit` older messages (empty at the end of history).
    pub async fn fetch(&mut self, limit: i32) -> Result<Vec<TalkMessage>> {
        let take = self.pending.len().min(limit.max(0) as usize);
        let mut page: Vec<Value> = self.pending.drain(..take).collect();
        let rest = limit - page.len() as i32;
        if rest > 0 {
            let args = self.request_args(rest);
            let res = self.client.base.talk().call("getPreviousMessagesV2WithRequest", args).await?;
            let older = list(&res);
            self.advance(&older);
            page.extend(older);
        }
        if page.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(page.len());
        for raw in page {
            out.push(TalkMessage::from_raw(self.client.clone(), raw).await?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::BaseClient;
    use crate::device::Device;

    #[test]
    fn media_route_and_marker_args() {
        assert!(uses_e2ee_media("c-group", true));
        assert!(uses_e2ee_media("u-user", true));
        assert!(!uses_e2ee_media("r-room", true));
        assert!(!uses_e2ee_media("c-group", false));
        let a = hidden_status_args(3, "c1", 9_007_199_254_740_993, true);
        assert_eq!(a["setChatHiddenStatusRequest"]["lastMessageId"].as_i64(), Some(9_007_199_254_740_993));
        assert_eq!(a["setChatHiddenStatusRequest"]["hidden"].as_bool(), Some(true));
        let m = chat_marker_args(4, "c1", "77");
        assert_eq!((m["seq"].as_i64(), m["chatMid"].as_str(), m["lastMessageId"].as_str()), (Some(4), Some("c1"), Some("77")));
    }

    // 2^53 + 1 is not representable as an f64; the cursor must stay exact and
    // an empty page must not move it.
    #[test]
    fn fetcher_cursor_is_lossless_and_tolerates_empty_pages() {
        let client = Client::new(BaseClient::new(Device::Android).unwrap());
        let mut f = MessageFetcher::new(
            client,
            tv!({"id": "c-chat", "lastDeliveredMessageId": {"deliveredTime": 100, "messageId": 9007199254740993_i64}}),
        );
        let first = f.request_args(10);
        assert_eq!(first["request"]["endMessageId"]["messageId"].as_i64(), Some(9007199254740993));
        assert_eq!(first["request"]["messageBoxId"].as_str(), Some("c-chat"));

        f.advance(&[tv!({"id": "9007199254740993", "deliveredTime": 100, "contentType": "NONE", "text": "hi"})]);
        assert_eq!(f.request_args(10)["request"]["endMessageId"]["messageId"].as_i64(), Some(9007199254740993));
        f.advance(&[]);
        assert_eq!(f.request_args(10)["request"]["endMessageId"]["messageId"].as_i64(), Some(9007199254740993));

        f.advance(&[tv!({"id": "9223372036854775807", "deliveredTime": 5})]);
        let next = f.request_args(10);
        assert_eq!(next["request"]["endMessageId"]["messageId"].as_i64(), Some(i64::MAX));
        assert_eq!(next["request"]["endMessageId"]["deliveredTime"].as_i64(), Some(5));
    }

    // The fallback fetcher (box located via its newest message) hands that
    // message out before asking the server for older ones.
    #[tokio::test]
    async fn pending_messages_come_first_without_a_request() {
        let client = Client::new(BaseClient::new(Device::Android).unwrap());
        let newest = tv!({"id": "42", "deliveredTime": 7, "contentType": "NONE", "text": "newest"});
        let mut f = MessageFetcher::new(client, tv!({"id": "u-self"}));
        f.advance(std::slice::from_ref(&newest));
        f.pending = vec![newest];
        let page = f.fetch(1).await.unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id(), "42");
        assert!(f.pending.is_empty());
        let cursor = &f.request_args(5)["request"]["endMessageId"];
        assert_eq!((cursor["messageId"].as_i64(), cursor["deliveredTime"].as_i64()), (Some(42), Some(7)));
    }
}
