//! [`Chat`]: a talk group chat (linejs `features/chat`).

use super::message::{MessageInput, TalkMessage};
use super::{Client, list};
use crate::error::{Error, Result};
pub use crate::obs::SentMedia;
use crate::obs::{MediaOptions, ObjType, ObsFile};
use crate::thrift::Value;
use crate::tv;

/// `setChatHiddenStatus` arguments.
pub(crate) fn hidden_status_args(
    req_seq: i64,
    chat_mid: &str,
    last_message_id: i64,
    hidden: bool,
) -> Value {
    tv!({"setChatHiddenStatusRequest": {
        "reqSeq": req_seq, "chatMid": chat_mid, "lastMessageId": last_message_id, "hidden": hidden,
    }})
}

/// `sendChatChecked` / `sendChatRemoved` arguments.
pub(crate) fn chat_marker_args(seq: i64, chat_mid: &str, last_message_id: &str) -> Value {
    tv!({"seq": seq, "chatMid": chat_mid, "lastMessageId": last_message_id, "sessionId": 0})
}

/// The link an announcement of a message points at, as LINE for Android
/// builds it (`qk8/a.java`).
pub fn announcement_link(chat_mid: &str, message_id: &str) -> String {
    format!("line://nv/chatMsg?chatId={chat_mid}&messageId={message_id}")
}

/// `createChatRoomAnnouncement` arguments for announcing a message: type
/// `MESSAGE`, `displayFields` 5 and a link to the message, as `sk8/a.java`
/// and `ts8/r6.java` set them.
pub(crate) fn announce_args(req_seq: i64, chat_mid: &str, message_id: &str, text: &str) -> Value {
    tv!({
        "reqSeq": req_seq,
        "chatRoomMid": chat_mid,
        "type": "MESSAGE",
        "contents": {
            "displayFields": 5,
            "text": text,
            "link": announcement_link(chat_mid, message_id),
        },
    })
}

/// A group write request (`inviteIntoChat`, `deleteOtherFromChat`,
/// `cancelChatInvitation`, `acceptChatInvitation`, `rejectChatInvitation`,
/// `deleteSelfFromChat`): LINE for Android sends a `reqSeq` with each
/// (`wf3/e.java`). `targets` is left out when `None`.
pub(crate) fn group_request_args(req_seq: i64, chat_mid: &str, targets: Option<&[&str]>) -> Value {
    let mut request = tv!({"reqSeq": req_seq, "chatMid": chat_mid});
    if let Some(t) = targets {
        request.insert("targetUserMids", mid_list(t));
    }
    let mut args = Value::object();
    args.insert("request", request);
    args
}

/// `GetMessageRequestBoxListResponse.chatMids`: chat mid → time.
pub(crate) fn request_box_entries(res: &Value) -> Vec<(String, i64)> {
    res["chatMids"]
        .entries()
        .into_iter()
        .map(|(mid, time)| (mid, time.as_i64().unwrap_or(0)))
        .collect()
}

fn mid_list(mids: &[&str]) -> Value {
    Value::List(mids.iter().map(|m| Value::from(*m)).collect())
}

/// `createChat` arguments for a group (`type` GROUP) with `name` and the
/// users to invite.
pub(crate) fn create_group_args(req_seq: i64, name: &str, targets: &[&str]) -> Value {
    let mut request = tv!({"reqSeq": req_seq, "type": "GROUP", "name": name});
    request.insert("targetUserMids", mid_list(targets));
    let mut args = Value::object();
    args.insert("request", request);
    args
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
    pub async fn send_compact_message(
        &self,
        text: &str,
    ) -> Result<crate::service::talk::compact::CompactMessageResponse> {
        self.client.send_compact_message(self.mid(), text).await
    }

    /// Send media (image / gif / video / audio / file). With `e2ee` the
    /// chat's flow decides, as in the official app: LINE says per chat
    /// whether media goes encrypted (Letter Sealing) or plain; rooms always
    /// go plain. Without `e2ee` it is uploaded in "reqseq" mode so the
    /// server creates the message. `duration_ms` is the audio/video length.
    /// See [`send_media_with`](Self::send_media_with) for previews and
    /// image sizes.
    pub async fn send_media(
        &self,
        o_type: ObjType,
        file: ObsFile,
        duration_ms: Option<u64>,
        e2ee: bool,
    ) -> Result<SentMedia> {
        let opts = MediaOptions {
            duration_ms,
            e2ee: (!e2ee).then_some(false),
            ..Default::default()
        };
        self.send_media_with(o_type, file, opts).await
    }

    /// Send media with a preview, duration, size or flow choice:
    ///
    /// ```no_run
    /// # async fn f(chat: line_rs::client::Chat, mp4: Vec<u8>, jpeg: Vec<u8>) -> line_rs::Result<()> {
    /// use line_rs::obs::{MediaOptions, ObjType, ObsFile};
    /// let opts = MediaOptions::new().preview(jpeg).duration_ms(4200);
    /// chat.send_media_with(ObjType::Video, ObsFile::new(mp4).name("clip.mp4"), opts)
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub async fn send_media_with(
        &self,
        o_type: ObjType,
        file: ObsFile,
        opts: MediaOptions,
    ) -> Result<SentMedia> {
        self.client
            .base
            .obs()
            .send_media(self.mid(), o_type, file, opts)
            .await
    }

    /// Send several images as one photo group, as the app does when you
    /// pick more than one (see [`Obs::send_image_group`](crate::obs::Obs::send_image_group)).
    /// One result per image, in order.
    pub async fn send_image_group(
        &self,
        images: Vec<(ObsFile, MediaOptions)>,
    ) -> Vec<Result<SentMedia>> {
        self.client
            .base
            .obs()
            .send_image_group(self.mid(), images)
            .await
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
        self.send_media(ObjType::Image, Self::named(data, filename), None, true)
            .await
    }

    pub async fn send_video(
        &self,
        data: Vec<u8>,
        filename: Option<&str>,
        duration_ms: Option<u64>,
    ) -> Result<SentMedia> {
        self.send_media(
            ObjType::Video,
            Self::named(data, filename),
            duration_ms,
            true,
        )
        .await
    }

    pub async fn send_audio(
        &self,
        data: Vec<u8>,
        filename: Option<&str>,
        duration_ms: Option<u64>,
    ) -> Result<SentMedia> {
        let file = Self::named(data, Some(filename.unwrap_or("voice.m4a")));
        self.send_media(ObjType::Audio, file, duration_ms, true)
            .await
    }

    pub async fn send_file(&self, data: Vec<u8>, filename: &str) -> Result<SentMedia> {
        self.send_media(ObjType::File, Self::named(data, Some(filename)), None, true)
            .await
    }

    /// Share a contact card (`CONTACT` message).
    pub async fn send_contact(&self, mid: &str, display_name: &str) -> Result<TalkMessage> {
        self.send_message(MessageInput::contact(mid, display_name))
            .await
    }

    /// `updateChat` with a partial `Chat` and the `Pb1_O2` attribute name
    /// (e.g. `NAME`).
    pub async fn update_chat(&self, chat: Value, updated_attribute: &str) -> Result<Value> {
        let mut chat = chat;
        if chat.get("chatMid").is_none() {
            chat.insert("chatMid", self.mid());
        }
        let mut request = tv!({"updatedAttribute": updated_attribute, "reqSeq": self.client.base.get_reqseq("talk").await?});
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
        self.update_chat(tv!({"notificationDisabled": muted}), "NOTIFICATION_SETTING")
            .await
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
        self.update_chat(tv!({"favoriteTimestamp": ts}), "FAVORITE_TIMESTAMP")
            .await
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
        self.client
            .base
            .talk()
            .call(
                "setChatHiddenStatus",
                hidden_status_args(seq, self.mid(), last_message_id, hidden),
            )
            .await
    }

    /// Mark the chat as read up to `last_message_id` (`sendChatChecked`).
    pub async fn mark_as_read(&self, last_message_id: &str) -> Result<Value> {
        let seq = self.client.base.get_reqseq("talk").await?;
        self.client
            .base
            .talk()
            .call(
                "sendChatChecked",
                chat_marker_args(seq, self.mid(), last_message_id),
            )
            .await
    }

    /// Delete the local history up to `last_message_id` (`sendChatRemoved`).
    pub async fn delete_history(&self, last_message_id: &str) -> Result<Value> {
        let seq = self.client.base.get_reqseq("talk").await?;
        self.client
            .base
            .talk()
            .call(
                "sendChatRemoved",
                chat_marker_args(seq, self.mid(), last_message_id),
            )
            .await
    }

    /// Pinned announcements (`list<ChatRoomAnnouncement>`).
    pub async fn announcements(&self) -> Result<Vec<Value>> {
        let res = self
            .client
            .base
            .talk()
            .call("getChatRoomAnnouncements", tv!({"chatRoomMid": self.mid()}))
            .await?;
        Ok(list(&res))
    }

    /// Announce a message: pin `text` at the top of the chat with a link
    /// back to `message_id`, as LINE's apps do on a long-pressed message.
    pub async fn announce(&self, message_id: &str, text: &str) -> Result<Value> {
        let seq = self.client.base.get_reqseq("talk").await?;
        self.client
            .base
            .talk()
            .call(
                "createChatRoomAnnouncement",
                announce_args(seq, self.mid(), message_id, text),
            )
            .await
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
            .call(
                "getMessageReadRange",
                tv!({"chatIds": [self.mid()], "syncReason": "INTERNAL"}),
            )
            .await?;
        Ok(list(&res)
            .into_iter()
            .find(|r| r["chatId"].as_str() == Some(self.mid())))
    }

    async fn group_call(&self, method: &str, targets: Option<&[&str]>) -> Result<Value> {
        let seq = self.client.base.get_reqseq("talk").await?;
        self.client
            .base
            .talk()
            .call(method, group_request_args(seq, self.mid(), targets))
            .await
    }

    /// Invite users into the group (`inviteIntoChat`).
    pub async fn invite(&self, mids: &[&str]) -> Result<Value> {
        self.group_call("inviteIntoChat", Some(mids)).await
    }

    /// Remove a member from the group (`deleteOtherFromChat`).
    pub async fn kick(&self, mid: &str) -> Result<Value> {
        self.kick_many(&[mid]).await
    }

    /// Remove several members at once (`deleteOtherFromChat`).
    pub async fn kick_many(&self, mids: &[&str]) -> Result<Value> {
        self.group_call("deleteOtherFromChat", Some(mids)).await
    }

    /// Withdraw invitations that were not accepted yet
    /// (`cancelChatInvitation`).
    pub async fn cancel_invitation(&self, mids: &[&str]) -> Result<Value> {
        self.group_call("cancelChatInvitation", Some(mids)).await
    }

    /// Join a group we were invited to (`acceptChatInvitation`).
    pub async fn accept_invitation(&self) -> Result<Value> {
        self.group_call("acceptChatInvitation", None).await
    }

    /// Decline an invitation (`rejectChatInvitation`).
    pub async fn reject_invitation(&self) -> Result<Value> {
        self.group_call("rejectChatInvitation", None).await
    }

    /// Leave the group (`deleteSelfFromChat` with only `reqSeq` and
    /// `chatMid`, as the Android app sends it).
    pub async fn leave(&self) -> Result<Value> {
        self.group_call("deleteSelfFromChat", None).await
    }

    /// Whether we are only invited (we are in `inviteeMids`, not
    /// `memberMids`). Needs the chat fetched with invitees.
    pub fn is_invitation(&self, my_mid: &str) -> bool {
        let extra = &self.raw["extra"]["groupExtra"];
        extra["memberMids"].get(my_mid).is_none() && extra["inviteeMids"].get(my_mid).is_some()
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
        let boxes = self
            .client
            .base
            .talk()
            .call("getMessageBoxes", tv!({"messageBoxListRequest": {}}))
            .await?;
        let mid = self.mid();
        let found = list(&boxes["messageBoxes"])
            .into_iter()
            .find(|b| b["id"].as_str() == Some(mid));
        if let Some(found) = found {
            return Ok(MessageFetcher::new(self.client.clone(), found));
        }
        let recent = list(
            &self
                .client
                .base
                .talk()
                .get_recent_messages_v2(mid, 1)
                .await?,
        );
        if recent.is_empty() {
            return Err(Error::internal(
                "NotFound",
                "Message box not found and the chat has no messages.",
            ));
        }
        let mut fetcher = MessageFetcher::new(self.client.clone(), tv!({"id": mid}));
        fetcher.advance(&recent);
        fetcher.pending = recent;
        Ok(fetcher)
    }

    /// The chat BGM, if set (`getChatRoomBGMs`).
    pub async fn bgm(&self) -> Result<Option<Value>> {
        let res = self
            .client
            .base
            .talk()
            .call("getChatRoomBGMs", tv!({"chatRoomMids": [self.mid()]}))
            .await?;
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Client {
    /// Create a group named `name` and invite `mids` to it
    /// (`createChat` with type GROUP).
    pub async fn create_group(&self, name: &str, mids: &[&str]) -> Result<Chat> {
        let seq = self.base.get_reqseq("talk").await?;
        let res = self
            .base
            .talk()
            .call("createChat", create_group_args(seq, name, mids))
            .await?;
        Ok(Chat::new(self.clone(), res["chat"].clone()))
    }

    /// Chats in the message request box (messages from people who are not
    /// friends), with the time of each one's latest request: every page of
    /// `getMessageRequestBoxList` for PEER and GROUP chats, as the app's
    /// full sync reads them (`zd2/l.java`; the server refuses ROOM with
    /// "unexpected chat type"). LINE's chat list leaves
    /// these chats out until the user accepts them
    /// (`updateChatStatusBulk` with `trusted`).
    pub async fn message_request_chats(&self) -> Result<Vec<(String, i64)>> {
        let talk = self.base.talk();
        let mut out = Vec::new();
        for kind in ["PEER", "GROUP"] {
            let mut token: Option<String> = None;
            for _ in 0..50 {
                let res = talk
                    .get_message_request_box_list(kind, token.as_deref())
                    .await?;
                out.extend(request_box_entries(&res));
                match res["nextPageToken"].as_str().filter(|t| !t.is_empty()) {
                    Some(next) if token.as_deref() != Some(next) => token = Some(next.into()),
                    _ => break,
                }
            }
        }
        Ok(out)
    }

    /// Send a `linepostback://` URL to a chat's bot without a message, as a
    /// rich-menu tap does (see [`postback`](crate::client::postback)).
    pub async fn send_postback(&self, chat_mid: &str, url: &str) -> Result<Value> {
        self.base
            .talk()
            .send_postback_url(url, chat_mid, None, None)
            .await
    }

    /// Chat effect definitions (`getChatEffectMetaList`) for `Pb1_Q2`
    /// categories (`BACKGROUND`, `KEYWORD`, `CONTENT_METADATA_TAG_BASED`).
    pub async fn chat_effects(&self, categories: &[&str]) -> Result<Vec<Value>> {
        let res = self
            .base
            .talk()
            .call("getChatEffectMetaList", tv!({"categories": categories}))
            .await?;
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
            let res = self
                .client
                .base
                .talk()
                .call("getPreviousMessagesV2WithRequest", args)
                .await?;
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
    use crate::thrift::TValue;

    #[test]
    fn marker_args() {
        let a = hidden_status_args(3, "c1", 9_007_199_254_740_993, true);
        assert_eq!(
            a["setChatHiddenStatusRequest"]["lastMessageId"].as_i64(),
            Some(9_007_199_254_740_993)
        );
        assert_eq!(
            a["setChatHiddenStatusRequest"]["hidden"].as_bool(),
            Some(true)
        );
        let m = chat_marker_args(4, "c1", "77");
        assert_eq!(
            (
                m["seq"].as_i64(),
                m["chatMid"].as_str(),
                m["lastMessageId"].as_str()
            ),
            (Some(4), Some("c1"), Some("77"))
        );
    }

    #[test]
    fn announce_args_match_the_app() {
        use crate::service::test_util::{encode, path, s};
        let a = encode(
            "createChatRoomAnnouncement_args",
            &announce_args(7, "c1", "9007199254740993", "hi"),
        );
        assert_eq!(path(&a, &[1]), Some(&TValue::I32(7)));
        assert_eq!(path(&a, &[2]), Some(&s("c1")));
        assert_eq!(path(&a, &[3]), Some(&TValue::I32(0)));
        assert_eq!(path(&a, &[4, 1]), Some(&TValue::I32(5)));
        assert_eq!(path(&a, &[4, 2]), Some(&s("hi")));
        assert_eq!(
            path(&a, &[4, 3]),
            Some(&s("line://nv/chatMsg?chatId=c1&messageId=9007199254740993"))
        );
    }

    #[test]
    fn group_writes_carry_req_seq() {
        use crate::service::test_util::{encode, path, s};
        for method in [
            "inviteIntoChat",
            "deleteOtherFromChat",
            "cancelChatInvitation",
        ] {
            let a = encode(
                &format!("{method}_args"),
                &group_request_args(3, "c1", Some(&["u1", "u2"])),
            );
            assert_eq!(path(&a, &[1, 1]), Some(&TValue::I32(3)), "{method}");
            assert_eq!(path(&a, &[1, 2]), Some(&s("c1")));
            assert!(
                matches!(path(&a, &[1, 3]), Some(TValue::Set(_, v)) if v.len() == 2),
                "{method}"
            );
        }
        for method in [
            "acceptChatInvitation",
            "rejectChatInvitation",
            "deleteSelfFromChat",
        ] {
            let a = encode(
                &format!("{method}_args"),
                &group_request_args(4, "c1", None),
            );
            assert_eq!(path(&a, &[1, 1]), Some(&TValue::I32(4)), "{method}");
            assert_eq!(path(&a, &[1, 3]), None);
        }
        let c = encode("createChat_args", &create_group_args(5, "team", &["u1"]));
        assert_eq!(path(&c, &[1, 2]), Some(&TValue::I32(0)));
        assert_eq!(path(&c, &[1, 3]), Some(&s("team")));
        assert!(matches!(path(&c, &[1, 4]), Some(TValue::Set(_, v)) if v.len() == 1));
    }

    #[test]
    fn request_box_maps_chats_to_times() {
        use crate::thrift::{Schema, TType, decode_struct};
        let reply = TValue::Struct(vec![(
            1,
            TValue::Map(
                TType::String,
                TType::I64,
                vec![(TValue::string("u1"), TValue::I64(5))],
            ),
        )]);
        let v = decode_struct(
            Schema::line(),
            Some("GetMessageRequestBoxListResponse"),
            &reply,
        );
        assert_eq!(request_box_entries(&v), [("u1".to_string(), 5)]);
    }

    #[test]
    fn invitation_is_read_from_group_extra() {
        let client = Client::new(BaseClient::new(Device::Android).unwrap());
        let invited = Chat::new(
            client.clone(),
            tv!({"chatMid": "c1", "extra": {"groupExtra": {"memberMids": {"u2": 1}, "inviteeMids": {"u1": 2}}}}),
        );
        assert!(invited.is_invitation("u1"));
        assert!(!invited.is_invitation("u2"));
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
        assert_eq!(
            first["request"]["endMessageId"]["messageId"].as_i64(),
            Some(9007199254740993)
        );
        assert_eq!(first["request"]["messageBoxId"].as_str(), Some("c-chat"));

        f.advance(&[tv!({"id": "9007199254740993", "deliveredTime": 100, "contentType": "NONE", "text": "hi"})]);
        assert_eq!(
            f.request_args(10)["request"]["endMessageId"]["messageId"].as_i64(),
            Some(9007199254740993)
        );
        f.advance(&[]);
        assert_eq!(
            f.request_args(10)["request"]["endMessageId"]["messageId"].as_i64(),
            Some(9007199254740993)
        );

        f.advance(&[tv!({"id": "9223372036854775807", "deliveredTime": 5})]);
        let next = f.request_args(10);
        assert_eq!(
            next["request"]["endMessageId"]["messageId"].as_i64(),
            Some(i64::MAX)
        );
        assert_eq!(
            next["request"]["endMessageId"]["deliveredTime"].as_i64(),
            Some(5)
        );
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
        assert_eq!(
            (
                cursor["messageId"].as_i64(),
                cursor["deliveredTime"].as_i64()
            ),
            (Some(42), Some(7))
        );
    }
}
