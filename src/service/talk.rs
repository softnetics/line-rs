//! `TalkService` (`/S4`): messages, chats, contacts, E2EE keys and sync.

pub mod compact;

use std::time::Duration;

use indexmap::IndexMap;

use self::compact::{
    COMPACT_E2EE_MESSAGE_ENDPOINT, COMPACT_PLAIN_MESSAGE_ENDPOINT, CompactMessageProtocolError, CompactMessageResponse,
    MSG_TYPE_E2EE, decode_compact_message_response, pack_compact_e2ee_message, pack_compact_plain_message,
};
use super::{TalkService, rename_raw};
use crate::base::ClientEvent;
use crate::e2ee::E2eeContent;
use crate::error::{Error, Result};
use crate::request::{Parse, RequestOptions};
use crate::thrift::{Schema, TType, TValue, TypeRef, Value};

/// Options for [`TalkService::sync`].
#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    /// Max events (default 100).
    pub limit: Option<i32>,
    pub revision: Option<i64>,
    pub global_rev: Option<i64>,
    pub individual_rev: Option<i64>,
    /// Defaults to the client's long-poll timeout.
    pub timeout: Option<Duration>,
}

/// Options for [`TalkService::send_message`].
#[derive(Debug, Clone, Default)]
pub struct SendMessageOptions {
    pub to: String,
    pub text: Option<String>,
    /// `ContentType` variant name or number (default `NONE`).
    pub content_type: Option<Value>,
    pub content_metadata: IndexMap<String, String>,
    /// Reply target; sets `messageRelationType = REPLY`.
    pub related_message_id: Option<String>,
    /// A `Location` struct value.
    pub location: Option<Value>,
    /// Pre-encrypted E2EE chunks.
    pub chunks: Option<Vec<Vec<u8>>>,
    /// `Some(true)` encrypts; `None` retries with E2EE when the server asks.
    pub e2ee: Option<bool>,
}

impl SendMessageOptions {
    /// A plain text message.
    pub fn text(to: impl Into<String>, text: impl Into<String>) -> Self {
        Self { to: to.into(), text: Some(text.into()), ..Default::default() }
    }
}

/// Options for [`TalkService::send_compact_message`].
#[derive(Debug, Clone, Default)]
pub struct SendCompactMessageOptions {
    pub to: String,
    pub text: Option<String>,
    /// Pre-encrypted E2EE chunks (forces the E2EE endpoint).
    pub chunks: Option<Vec<Vec<u8>>>,
    /// `Some(true)` forces E2EE; `None` falls back to E2EE on error 82/99.
    pub e2ee: Option<bool>,
}

/// Options for [`TalkService::react`].
#[derive(Debug, Clone)]
pub struct ReactOptions {
    pub id: i64,
    /// `MessageReactionType` variant name (`"NICE"`, ...) or number.
    pub reaction: Value,
    /// Defaults to the next `talk` request sequence.
    pub req_seq: Option<i64>,
}

/// Options for [`TalkService::get_chats`].
#[derive(Debug, Clone, Default)]
pub struct GetChatsOptions {
    pub chat_mids: Vec<String>,
    /// Default `true`.
    pub with_invitees: Option<bool>,
    /// Default `true`.
    pub with_members: Option<bool>,
}

/// Number of a `ContentType` given as a variant name or number.
fn content_type_number(v: &Value) -> i32 {
    match v {
        Value::String(s) => {
            Schema::line().get_enum("ContentType").and_then(|e| e.value_of(s)).or_else(|| s.parse().ok()).unwrap_or(0)
        }
        other => other.as_i64().unwrap_or(0) as i32,
    }
}

fn string_list(items: &[String]) -> TValue {
    TValue::List(TType::String, items.iter().map(|s| TValue::string(s.as_str())).collect())
}

pub(crate) fn sync_args(o: &SyncOptions) -> Value {
    crate::tv!({"request": {
        "lastRevision": o.revision.unwrap_or(0),
        "lastGlobalRevision": o.global_rev.unwrap_or(0),
        "lastIndividualRevision": o.individual_rev.unwrap_or(0),
        "count": o.limit.unwrap_or(100),
    }})
}

pub(crate) fn send_message_args(seq: i64, o: &SendMessageOptions) -> Value {
    let mut message = crate::tv!({
        "to": o.to,
        "createdTime": 0,
        "deliveredTime": 0,
        "hasContent": false,
        "sessionId": 0,
    });
    message.insert("contentType", o.content_type.clone().unwrap_or_else(|| "NONE".into()));
    message.insert(
        "contentMetadata",
        Value::Struct(o.content_metadata.iter().map(|(k, v)| (k.clone(), Value::from(v))).collect()),
    );
    message.insert("text", o.text.clone());
    message.insert("location", o.location.clone());
    message.insert("chunks", o.chunks.clone().map(|c| Value::List(c.into_iter().map(Value::Binary).collect())));
    if let Some(id) = &o.related_message_id {
        message.insert("relatedMessageId", id);
        message.insert("messageRelationType", "REPLY");
        message.insert("relatedMessageServiceCode", "TALK");
    }
    let mut args = crate::tv!({"seq": seq});
    args.insert("message", message);
    args
}

pub(crate) fn react_args(req_seq: i64, o: &ReactOptions) -> Value {
    let mut reaction_type = Value::object();
    reaction_type.insert("predefinedReactionType", o.reaction.clone());
    let mut req = crate::tv!({"reqSeq": req_seq, "messageId": o.id});
    req.insert("reactionType", reaction_type);
    let mut args = Value::object();
    args.insert("reactRequest", req);
    args
}

pub(crate) fn postback_args(message_id: &str, url: &str, chat_mid: &str, origin_mid: &str) -> Value {
    crate::tv!({"request": {"messageId": message_id, "url": url, "chatMID": chat_mid, "originMID": origin_mid}})
}

pub(crate) fn get_chats_args(o: &GetChatsOptions) -> Value {
    crate::tv!({
        "request": {
            "chatMids": o.chat_mids,
            "withInvitees": o.with_invitees.unwrap_or(true),
            "withMembers": o.with_members.unwrap_or(true),
        },
        "syncReason": "INTERNAL",
    })
}

/// Failure of a compact request: the server's protocol error (inspected
/// for fallbacks) or any other error.
enum CompactFailure {
    Protocol(CompactMessageProtocolError),
    Other(Error),
}

impl From<Error> for CompactFailure {
    fn from(e: Error) -> Self {
        CompactFailure::Other(e)
    }
}

impl From<CompactFailure> for Error {
    fn from(f: CompactFailure) -> Self {
        match f {
            CompactFailure::Protocol(e) => e.into(),
            CompactFailure::Other(e) => e,
        }
    }
}

impl TalkService<'_> {
    /// `sync` on `/SYNC4` (long poll): fetch operations since the given revisions.
    pub async fn sync(&self, options: SyncOptions) -> Result<Value> {
        let timeout = options.timeout.unwrap_or(self.client.config.long_timeout);
        self.call_with("sync", sync_args(&options), |o| RequestOptions { path: "/SYNC4", ..o }.timeout(timeout)).await
    }

    /// Send a message. With `e2ee: Some(true)` the text/location is
    /// encrypted first; with `e2ee: None` an `E2EE*` error code triggers one
    /// retry with encryption (as linejs). With `Config::e2ee_send_recovery`,
    /// messages are also recovered from E2EE send errors as the official
    /// client does (see [`crate::e2ee::send_recovery`]): re-encrypt with fresh
    /// keys, recreate the group key, or (only when `e2ee` was unset) fall back
    /// to plain, for at most [`crate::e2ee::MAX_E2EE_SEND_ATTEMPTS`] attempts.
    pub async fn send_message(&self, options: SendMessageOptions) -> Result<Value> {
        use crate::e2ee::{send_recovery, E2eeSendRecovery, MAX_E2EE_SEND_ATTEMPTS};
        let requested = options.e2ee;
        // Only plaintext we hold can be re-encrypted or resent in the clear.
        let reencryptable = options.chunks.is_none() && (options.text.is_some() || options.location.is_some());
        let mut options = options;
        let mut attempts = 0;
        loop {
            attempts += 1;
            let mut send = options.clone();
            if send.e2ee == Some(true)
                && send.chunks.is_none()
                && (send.text.as_deref().is_some_and(|t| !t.is_empty()) || send.location.is_some())
            {
                let content_type = content_type_number(send.content_type.as_ref().unwrap_or(&Value::Int(0)));
                let content = match (send.text.take(), send.location.take()) {
                    (Some(text), _) if !text.is_empty() => E2eeContent::Text(text),
                    (_, Some(location)) => E2eeContent::Location(location),
                    _ => E2eeContent::Text("invalid".into()),
                };
                send.chunks = Some(self.client.encrypt_e2ee_message(&send.to, content, content_type, 2).await?);
                send.content_metadata.insert("e2eeVersion".into(), "2".into());
                send.content_metadata.insert("contentType".into(), content_type.to_string());
                send.content_metadata.insert("e2eeMark".into(), "2".into());
            }
            let seq = self.client.get_reqseq("talk").await?;
            let err = match self.call("sendMessage", send_message_args(seq, &send)).await {
                Err(e) if attempts < MAX_E2EE_SEND_ATTEMPTS => e,
                res => return res,
            };
            let code = err.code().unwrap_or_default();
            if !self.client.config.e2ee_send_recovery {
                // linejs: retry once with encryption when `e2ee` was unset.
                if attempts == 1 && options.e2ee.is_none() && code.contains("E2EE") {
                    options.e2ee = Some(true);
                    continue;
                }
                return Err(err);
            }
            match send_recovery(code) {
                Some(E2eeSendRecovery::RetryPlain)
                    if requested.is_none() && reencryptable && options.e2ee == Some(true) =>
                {
                    options.e2ee = Some(false);
                }
                Some(r @ (E2eeSendRecovery::Reencrypt | E2eeSendRecovery::RecreateGroupKey))
                    if requested != Some(false) && options.e2ee != Some(false) && (reencryptable || options.e2ee.is_none()) =>
                {
                    self.client.prepare_e2ee_resend(&options.to, r).await?;
                    options.e2ee = Some(true);
                }
                None if options.e2ee.is_none() && code.contains("E2EE") => options.e2ee = Some(true),
                _ => return Err(err),
            }
        }
    }

    /// Send a text via the compact protocol (`/CA5`), falling back to
    /// `/ECA5` when the server requires E2EE (codes 82/99) and `e2ee` is unset.
    pub async fn send_compact_message(&self, options: SendCompactMessageOptions) -> Result<CompactMessageResponse> {
        if options.chunks.is_some() || options.e2ee == Some(true) {
            return self.send_compact_e2ee_message(&options.to, options.text.as_deref(), options.chunks).await;
        }
        let Some(text) = options.text.as_deref() else {
            return Err(Error::internal("TypeError", "sendCompactMessage requires text or chunks"));
        };
        match self.compact_plain(&options.to, text).await {
            Err(CompactFailure::Protocol(e)) if options.e2ee.is_none() && matches!(e.code, Some(82 | 99)) => {
                self.send_compact_e2ee_message(&options.to, Some(text), None).await
            }
            res => Ok(res?),
        }
    }

    /// Send plain text via `/CA5`.
    pub async fn send_compact_plain_message(&self, to: &str, text: &str) -> Result<CompactMessageResponse> {
        Ok(self.compact_plain(to, text).await?)
    }

    /// Send an E2EE text via `/ECA5`, encrypting `text` unless `chunks` are given.
    pub async fn send_compact_e2ee_message(
        &self,
        to: &str,
        text: Option<&str>,
        chunks: Option<Vec<Vec<u8>>>,
    ) -> Result<CompactMessageResponse> {
        let chunks = match (chunks, text) {
            (Some(c), _) => c,
            (None, Some(text)) => self.client.encrypt_e2ee_message(to, E2eeContent::Text(text.into()), 0, 2).await?,
            (None, None) => {
                return Err(Error::internal("TypeError", "sendCompactE2EEMessage requires text or chunks"));
            }
        };
        let seq = self.client.get_reqseq("talk").await? as i32;
        let body = pack_compact_e2ee_message(seq, to, &chunks, MSG_TYPE_E2EE)?;
        Ok(self.request_compact_message(COMPACT_E2EE_MESSAGE_ENDPOINT, seq, body).await?)
    }

    async fn compact_plain(&self, to: &str, text: &str) -> std::result::Result<CompactMessageResponse, CompactFailure> {
        let seq = self.client.get_reqseq("talk").await? as i32;
        let body = pack_compact_plain_message(seq, to, text).map_err(CompactFailure::Protocol)?;
        self.request_compact_message(COMPACT_PLAIN_MESSAGE_ENDPOINT, seq, body).await
    }

    async fn request_compact_message(
        &self,
        path: &str,
        seq: i32,
        body: Vec<u8>,
    ) -> std::result::Result<CompactMessageResponse, CompactFailure> {
        let client = self.client;
        let mut retried = false;
        loop {
            if client.is_disabled() {
                return Err(
                    Error::internal("ClientClosed", "Request aborted: client has been disabled (logged out)").into()
                );
            }
            let mut headers = client.line_headers("POST");
            headers.push(("x-lai".into(), seq.to_string()));
            client.log("compactMessageRequest", serde_json::json!({"path": path, "body": hex::encode(&body)}));
            let res = client.send_http_plain(path, "POST", headers, body.clone(), client.config.timeout).await?;
            if let Some(next) = res.header("x-line-next-access") {
                client.emit(ClientEvent::UpdateAuthToken(next.to_string()));
            }
            client.log(
                "compactMessageResponse",
                serde_json::json!({"path": path, "status": res.status, "body": hex::encode(&res.body)}),
            );
            if !(200..300).contains(&res.status) {
                return Err(Error::internal(
                    "CompactMessageRequestError",
                    format!("Compact message request failed: status={}", res.status),
                )
                .into());
            }
            match decode_compact_message_response(&res.body) {
                Err(e) if e.code == Some(119) && !retried && client.storage.get("refreshToken").await?.is_some() => {
                    retried = true;
                    client.try_refresh_token().await?;
                }
                res => return res.map_err(CompactFailure::Protocol),
            }
        }
    }

    /// Invite users into a group chat.
    pub async fn invite_into_chat(&self, chat_mid: &str, target_user_mids: &[String]) -> Result<Value> {
        self.call("inviteIntoChat", crate::tv!({"request": {"targetUserMids": target_user_mids, "chatMid": chat_mid}}))
            .await
    }

    /// Report a Flex message postback action (`sendPostback`): `url` is the
    /// action's postback URL, `origin_mid` the sender of the Flex message.
    pub async fn send_flex_postback(&self, message_id: &str, url: &str, chat_mid: &str, origin_mid: &str) -> Result<Value> {
        self.call("sendPostback", postback_args(message_id, url, chat_mid, origin_mid)).await
    }

    /// All of the user's registered E2EE public keys.
    pub async fn get_e2ee_public_keys(&self) -> Result<Value> {
        self.request("getE2EEPublicKeys", Vec::new(), Parse::Result).await
    }

    /// React to a message.
    pub async fn react(&self, options: ReactOptions) -> Result<Value> {
        let req_seq = match options.req_seq {
            Some(s) => s,
            None => self.client.get_reqseq("talk").await?,
        };
        self.call("react", react_args(req_seq, &options)).await
    }

    /// `getChats` (members and invitees included by default).
    pub async fn get_chats(&self, options: GetChatsOptions) -> Result<Value> {
        self.call("getChats", get_chats_args(&options)).await
    }

    /// A single chat (`getChats(...).chats[0]`).
    pub async fn get_chat(
        &self,
        chat_mid: &str,
        with_invitees: Option<bool>,
        with_members: Option<bool>,
    ) -> Result<Value> {
        let res = self
            .get_chats(GetChatsOptions { chat_mids: vec![chat_mid.to_string()], with_invitees, with_members })
            .await?;
        Ok(res["chats"][0].clone())
    }

    /// The N most recent messages in a message box (no IDL args struct; sent raw).
    pub async fn get_recent_messages_v2(&self, message_box_id: &str, messages_count: i32) -> Result<Value> {
        let args = vec![(2, TValue::string(message_box_id)), (3, TValue::I32(messages_count))];
        let raw = self.request("getRecentMessagesV2", args, Parse::Raw).await?;
        Ok(rename_raw(Schema::line(), &TypeRef::List(Box::new(TypeRef::Named("Message".into()))), raw))
    }

    /// Server time (epoch milliseconds).
    pub async fn get_server_time(&self) -> Result<Value> {
        self.request("getServerTime", Vec::new(), Parse::Result).await
    }

    /// A contact by mid.
    pub async fn get_contact(&self, mid: &str) -> Result<Value> {
        self.request("getContact", vec![(2, TValue::string(mid))], Parse::Struct("Contact")).await
    }

    /// Contacts by mids (`list<Contact>`).
    pub async fn get_contacts(&self, mids: &[String]) -> Result<Value> {
        self.request("getContacts", vec![(2, string_list(mids))], Parse::Result).await
    }

    /// `getContactsV2` (`GetContactsV2Response`).
    pub async fn get_contacts_v2(&self, mids: &[String]) -> Result<Value> {
        let args = vec![(1, TValue::Struct(vec![(1, string_list(mids))]))];
        self.request("getContactsV2", args, Parse::Struct("GetContactsV2Response")).await
    }

    /// `noop` (keep-alive).
    pub async fn noop(&self) -> Result<Value> {
        self.request("noop", Vec::new(), Parse::Result).await
    }

    // --- Methods recovered by static RE of LINE macOS 26.5.0 (unverified). ---

    /// Edit the text/content of a sent message (LINE/LYP Premium gated
    /// server-side). `message` is a `Message` struct (same shape as
    /// [`send_message`](Self::send_message)); `reqSeq` is filled automatically.
    pub async fn edit_message(&self, message: impl Into<Value>) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await? as i32;
        let mut req = Value::object();
        req.insert("reqSeq", seq);
        req.insert("message", message.into());
        let mut args = Value::object();
        args.insert("req", req);
        self.call("editMessage", args).await
    }

    /// Unsend a message without leaving a tombstone ("premium unsend").
    /// `reqSeq` is filled automatically.
    pub async fn silently_unsend_message(&self, message_id: &str) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await? as i32;
        let args = vec![(
            1,
            TValue::Struct(vec![(1, TValue::I32(seq)), (2, TValue::string(message_id))]),
        )];
        self.request("silentlyUnsendMessage", args, Parse::Result).await
    }

    /// Pin (`pin = true`) or unpin a chat. `reqSeq` is filled automatically.
    pub async fn update_pin_state(&self, chat_mid: &str, pin: bool) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await? as i32;
        let args = vec![(
            1,
            TValue::Struct(vec![
                (1, TValue::I32(seq)),
                (2, TValue::string(chat_mid)),
                (3, TValue::Bool(pin)),
            ]),
        )];
        self.request("updatePinState", args, Parse::Result).await
    }

    /// List pinned chats. `page_token` is `None` for the first page.
    pub async fn get_pinned_chats(&self, page_token: Option<&str>) -> Result<Value> {
        let mut req = Value::object();
        if let Some(t) = page_token {
            req.insert("pageToken", t);
        }
        let mut args = Value::object();
        args.insert("request", req);
        self.call("getPinnedChats", args).await
    }

    /// Message-edit eligibility notice for a chat (`mid`).
    pub async fn get_message_edit_notice(&self, mid: &str) -> Result<Value> {
        let args = vec![(1, TValue::Struct(vec![(1, TValue::string(mid))]))];
        self.request("getMessageEditNotice", args, Parse::Result).await
    }

    /// Fetch messages by ids. `args` is `getMessagesByIds_args`, e.g.
    /// `tv!({"messagesByIdsRequests": [{"chatMid": "c..", "messageIds":
    /// [{"deliveredTime": 0, "messageId": 123}]}]})`.
    pub async fn get_messages_by_ids(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getMessagesByIds", args).await
    }

    /// The caller's own invite ticket.
    pub async fn get_user_ticket(&self) -> Result<Value> {
        self.request("getUserTicket", Vec::new(), Parse::Result).await
    }

    /// Reissue the caller's user ticket. Returns the new ticket id (string).
    pub async fn reissue_user_ticket(
        &self,
        expiration_time: i64,
        max_use_count: i32,
    ) -> Result<Value> {
        let args = vec![(3, TValue::I64(expiration_time)), (4, TValue::I32(max_use_count))];
        self.request("reissueUserTicket", args, Parse::Result).await
    }

    /// Unregister the current user and device. Returns the success string.
    pub async fn unregister_user_and_device(&self) -> Result<Value> {
        self.request("unregisterUserAndDevice", Vec::new(), Parse::Result).await
    }
}

// <generated pass-through>
impl TalkService<'_> {
    /// `acceptChatInvitation(request: AcceptChatInvitationRequest)`.
    pub async fn accept_chat_invitation(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("acceptChatInvitation", args).await
    }

    /// `acceptChatInvitationByTicket(request: AcceptChatInvitationByTicketRequest)`.
    pub async fn accept_chat_invitation_by_ticket(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("acceptChatInvitationByTicket", args).await
    }

    /// `acquireEncryptedAccessToken(featureType: Pb1_EnumC13222w4)`.
    pub async fn acquire_encrypted_access_token(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("acquireEncryptedAccessToken", args).await
    }

    /// `addToFollowBlacklist(addToFollowBlacklistRequest: AddToFollowBlacklistRequest)`.
    pub async fn add_to_follow_blacklist(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("addToFollowBlacklist", args).await
    }

    /// `blockContact(reqSeq: i32, id: string)`.
    pub async fn block_contact(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("blockContact", args).await
    }

    /// `blockRecommendation(reqSeq: i32, targetMid: string)`.
    pub async fn block_recommendation(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("blockRecommendation", args).await
    }

    /// `bulkFollow(bulkFollowRequest: BulkFollowRequest)`.
    pub async fn bulk_follow(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("bulkFollow", args).await
    }

    /// `cancelChatInvitation(request: CancelChatInvitationRequest)`.
    pub async fn cancel_chat_invitation(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("cancelChatInvitation", args).await
    }

    /// `cancelReaction(cancelReactionRequest: CancelReactionRequest)`.
    pub async fn cancel_reaction(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("cancelReaction", args).await
    }

    /// `changeVerificationMethod(sessionId: string, method: VerificationMethod)`.
    pub async fn change_verification_method(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("changeVerificationMethod", args).await
    }

    /// `clearRingtone(oid: string)`.
    pub async fn clear_ringtone(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("clearRingtone", args).await
    }

    /// `createChat(request: CreateChatRequest)`.
    pub async fn create_chat(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createChat", args).await
    }

    /// `createChatRoomAnnouncement` (args struct `createChatRoomAnnouncement_args`).
    pub async fn create_chat_room_announcement(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createChatRoomAnnouncement", args).await
    }

    /// `createRoomV2(reqSeq: i32, contactIds: list<string>)`.
    pub async fn create_room_v2(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createRoomV2", args).await
    }

    /// `createSession(request: h80_C25643c)`.
    pub async fn create_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createSession", args).await
    }

    /// `decryptFollowEMid(eMid: string)`.
    pub async fn decrypt_follow_e_mid(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("decryptFollowEMid", args).await
    }

    /// `deleteOtherFromChat(request: DeleteOtherFromChatRequest)`.
    pub async fn delete_other_from_chat(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("deleteOtherFromChat", args).await
    }

    /// `deleteSelfFromChat(request: DeleteSelfFromChatRequest)`.
    pub async fn delete_self_from_chat(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("deleteSelfFromChat", args).await
    }

    /// `determineMediaMessageFlow(request: DetermineMediaMessageFlowRequest)`.
    pub async fn determine_media_message_flow(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("determineMediaMessageFlow", args).await
    }

    /// `fetchOperations(request: FetchOperationsRequest)`.
    pub async fn fetch_operations(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("fetchOperations", args).await
    }

    /// `findChatByTicket(request: FindChatByTicketRequest)`.
    pub async fn find_chat_by_ticket(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("findChatByTicket", args).await
    }

    /// `findContactByUserTicket(ticketIdWithTag: string)`.
    pub async fn find_contact_by_user_ticket(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("findContactByUserTicket", args).await
    }

    /// `findContactByUserid(searchId: string)`.
    pub async fn find_contact_by_userid(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("findContactByUserid", args).await
    }

    /// `findContactsByPhone(phones: set<string>)`.
    pub async fn find_contacts_by_phone(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("findContactsByPhone", args).await
    }

    /// `finishUpdateVerification(sessionId: string)`.
    pub async fn finish_update_verification(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("finishUpdateVerification", args).await
    }

    /// `follow(followRequest: FollowRequest)`.
    pub async fn follow(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("follow", args).await
    }

    /// `generateUserTicket(expirationTime: i64, maxUseCount: i32)`.
    pub async fn generate_user_ticket(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("generateUserTicket", args).await
    }

    /// `getAllChatMids(request: GetAllChatMidsRequest, syncReason: Pb1_V7)`.
    pub async fn get_all_chat_mids(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getAllChatMids", args).await
    }

    /// `getAllContactIds(syncReason: Pb1_V7)`.
    pub async fn get_all_contact_ids(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getAllContactIds", args).await
    }

    /// `getBlockedContactIds(syncReason: Pb1_V7)`.
    pub async fn get_blocked_contact_ids(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getBlockedContactIds", args).await
    }

    /// `getBlockedRecommendationIds(syncReason: Pb1_V7)`.
    pub async fn get_blocked_recommendation_ids(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getBlockedRecommendationIds", args).await
    }

    /// `getChatEffectMetaList(categories: set<Pb1_Q2>)`.
    pub async fn get_chat_effect_meta_list(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getChatEffectMetaList", args).await
    }

    /// `getChatRoomAnnouncements(chatRoomMid: string)`.
    pub async fn get_chat_room_announcements(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getChatRoomAnnouncements", args).await
    }

    /// `getChatRoomAnnouncementsBulk(chatRoomMids: list<string>, syncReason: Pb1_V7)`.
    pub async fn get_chat_room_announcements_bulk(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getChatRoomAnnouncementsBulk", args).await
    }

    /// `getChatRoomBGMs(chatRoomMids: set<string>, syncReason: Pb1_V7)`.
    pub async fn get_chat_room_bgms(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getChatRoomBGMs", args).await
    }

    /// `getConfigurations` (args struct `getConfigurations_args`).
    pub async fn get_configurations(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getConfigurations", args).await
    }

    /// `getCountries(countryGroup: Pb1_EnumC13221w3)`.
    pub async fn get_countries(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getCountries", args).await
    }

    /// `getE2EEGroupSharedKey(keyVersion: i32, chatMid: string, groupKeyId: i32)`.
    pub async fn get_e2ee_group_shared_key(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getE2EEGroupSharedKey", args).await
    }

    /// `getE2EEPublicKey(mid: string, keyVersion: i32, keyId: i32)`.
    pub async fn get_e2ee_public_key(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getE2EEPublicKey", args).await
    }

    /// `getExtendedProfile(syncReason: Pb1_V7)`.
    pub async fn get_extended_profile(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getExtendedProfile", args).await
    }

    /// `getFollowBlacklist(getFollowBlacklistRequest: GetFollowBlacklistRequest)`.
    pub async fn get_follow_blacklist(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getFollowBlacklist", args).await
    }

    /// `getFollowers(getFollowersRequest: GetFollowersRequest)`.
    pub async fn get_followers(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getFollowers", args).await
    }

    /// `getFollowings(getFollowingsRequest: GetFollowingsRequest)`.
    pub async fn get_followings(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getFollowings", args).await
    }

    /// `getFriendRequests(direction: Pb1_F4, lastSeenSeqId: i64)`.
    pub async fn get_friend_requests(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getFriendRequests", args).await
    }

    /// `getInstantNews(region: string, location: Location)`.
    pub async fn get_instant_news(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getInstantNews", args).await
    }

    /// `getLastE2EEGroupSharedKey(keyVersion: i32, chatMid: string)`.
    pub async fn get_last_e2ee_group_shared_key(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getLastE2EEGroupSharedKey", args).await
    }

    /// `getLastE2EEPublicKeys(chatMid: string)`.
    pub async fn get_last_e2ee_public_keys(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getLastE2EEPublicKeys", args).await
    }

    /// `getMessageBoxes(messageBoxListRequest: MessageBoxListRequest, syncReason: Pb1_V7)`.
    pub async fn get_message_boxes(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getMessageBoxes", args).await
    }

    /// `getMessageReadRange(chatIds: list<string>, syncReason: Pb1_V7)`.
    pub async fn get_message_read_range(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getMessageReadRange", args).await
    }

    /// `getNotificationSettings(request: GetNotificationSettingsRequest)`.
    pub async fn get_notification_settings(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getNotificationSettings", args).await
    }

    /// `getPreviousMessagesV2WithRequest(request: GetPreviousMessagesV2Request, syncReason: Pb1_V7)`.
    pub async fn get_previous_messages_v2_with_request(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPreviousMessagesV2WithRequest", args).await
    }

    /// `getProfile(syncReason: Pb1_V7)`.
    pub async fn get_profile(&self, args: impl Into<Value>) -> Result<Value> {
        self.call_with("getProfile", args, |o| o.parse(Parse::Struct("Profile"))).await
    }

    /// `getRecentFriendRequests(syncReason: Pb1_V7)`.
    pub async fn get_recent_friend_requests(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getRecentFriendRequests", args).await
    }

    /// `getRecommendationIds(syncReason: Pb1_V7)`.
    pub async fn get_recommendation_ids(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getRecommendationIds", args).await
    }

    /// `getRepairElements(request: GetRepairElementsRequest)`.
    pub async fn get_repair_elements(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getRepairElements", args).await
    }

    /// `getRoomsV2(roomIds: list<string>)`.
    pub async fn get_rooms_v2(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getRoomsV2", args).await
    }

    /// `getSettings(syncReason: Pb1_V7)`.
    pub async fn get_settings(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSettings", args).await
    }

    /// `getSettingsAttributes2(attributesToRetrieve: set<SettingsAttributeEx>)`.
    pub async fn get_settings_attributes2(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSettingsAttributes2", args).await
    }

    /// `inviteIntoRoom(reqSeq: i32, roomId: string, contactIds: list<string>)`.
    pub async fn invite_into_room(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("inviteIntoRoom", args).await
    }

    /// `isUseridAvailable(searchId: string)`.
    pub async fn is_userid_available(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("isUseridAvailable", args).await
    }

    /// `leaveRoom(reqSeq: i32, roomId: string)`.
    pub async fn leave_room(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("leaveRoom", args).await
    }

    /// `negotiateE2EEPublicKey(mid: string)`.
    pub async fn negotiate_e2ee_public_key(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("negotiateE2EEPublicKey", args).await
    }

    /// `notifyInstalled(udidHash: string, applicationTypeWithExtensions: string)`.
    pub async fn notify_installed(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("notifyInstalled", args).await
    }

    /// `notifyRegistrationComplete(udidHash: string, applicationTypeWithExtensions: string)`.
    pub async fn notify_registration_complete(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("notifyRegistrationComplete", args).await
    }

    /// `notifyUpdated(lastRev: i64, deviceInfo: DeviceInfo, udidHash: string, oldUdidHash: string)`.
    pub async fn notify_updated(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("notifyUpdated", args).await
    }

    /// `registerE2EEGroupKey` (args struct `registerE2EEGroupKey_args`).
    pub async fn register_e2ee_group_key(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("registerE2EEGroupKey", args).await
    }

    /// `registerE2EEPublicKey(reqSeq: i32, publicKey: Pb1_C13097n4)`.
    pub async fn register_e2ee_public_key(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("registerE2EEPublicKey", args).await
    }

    /// `registerUserid(reqSeq: i32, searchId: string)`.
    pub async fn register_userid(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("registerUserid", args).await
    }

    /// `reissueChatTicket(request: ReissueChatTicketRequest)`.
    pub async fn reissue_chat_ticket(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reissueChatTicket", args).await
    }

    /// `rejectChatInvitation(request: RejectChatInvitationRequest)`.
    pub async fn reject_chat_invitation(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("rejectChatInvitation", args).await
    }

    /// `removeChatRoomAnnouncement(reqSeq: i32, chatRoomMid: string, announcementSeq: i64)`.
    pub async fn remove_chat_room_announcement(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("removeChatRoomAnnouncement", args).await
    }

    /// `removeFollower(removeFollowerRequest: RemoveFollowerRequest)`.
    pub async fn remove_follower(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("removeFollower", args).await
    }

    /// `removeFriendRequest(direction: Pb1_F4, midOrEMid: string)`.
    pub async fn remove_friend_request(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("removeFriendRequest", args).await
    }

    /// `removeFromFollowBlacklist(removeFromFollowBlacklistRequest: RemoveFromFollowBlacklistRequest)`.
    pub async fn remove_from_follow_blacklist(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("removeFromFollowBlacklist", args).await
    }

    /// `reportAbuseEx(request: ReportAbuseExRequest)`.
    pub async fn report_abuse_ex(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reportAbuseEx", args).await
    }

    /// `reportDeviceState(booleanState: map<i32, bool>, stringState: map<i32, string>)`.
    pub async fn report_device_state(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reportDeviceState", args).await
    }

    /// `reportProfile(syncOpRevision: i64, profile: Profile)`.
    pub async fn report_profile(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reportProfile", args).await
    }

    /// `reportPushRecvReports(reqSeq: i32, pushRecvReports: list<PushRecvReport>)`.
    pub async fn report_push_recv_reports(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reportPushRecvReports", args).await
    }

    /// `reportSettings(syncOpRevision: i64, settings: Settings)`.
    pub async fn report_settings(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reportSettings", args).await
    }

    /// `resendPinCode(sessionId: string)`.
    pub async fn resend_pin_code(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("resendPinCode", args).await
    }

    /// `sendChatChecked(seq: i32, chatMid: string, lastMessageId: string, sessionId: byte)`.
    pub async fn send_chat_checked(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("sendChatChecked", args).await
    }

    /// `sendChatRemoved(seq: i32, chatMid: string, lastMessageId: string, sessionId: byte)`.
    pub async fn send_chat_removed(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("sendChatRemoved", args).await
    }

    /// `sendPostback(request: SendPostbackRequest)`.
    pub async fn send_postback(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("sendPostback", args).await
    }

    /// `setChatHiddenStatus(setChatHiddenStatusRequest: SetChatHiddenStatusRequest)`.
    pub async fn set_chat_hidden_status(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("setChatHiddenStatus", args).await
    }

    /// `setNotificationsEnabled(reqSeq: i32, type: MIDType, target: string, enablement: bool)`.
    pub async fn set_notifications_enabled(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("setNotificationsEnabled", args).await
    }

    /// `startUpdateVerification` (args struct `startUpdateVerification_args`).
    pub async fn start_update_verification(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("startUpdateVerification", args).await
    }

    /// `syncContacts(reqSeq: i32, localContacts: list<ContactModification>)`.
    pub async fn sync_contacts(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("syncContacts", args).await
    }

    /// `tryFriendRequest(midOrEMid: string, method: Pb1_G4, friendRequestParams: string)`.
    pub async fn try_friend_request(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("tryFriendRequest", args).await
    }

    /// `unblockContact(reqSeq: i32, id: string, reference: string)`.
    pub async fn unblock_contact(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("unblockContact", args).await
    }

    /// `unblockRecommendation(reqSeq: i32, targetMid: string)`.
    pub async fn unblock_recommendation(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("unblockRecommendation", args).await
    }

    /// `unfollow(unfollowRequest: UnfollowRequest)`.
    pub async fn unfollow(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("unfollow", args).await
    }

    /// `unsendMessage(seq: i32, messageId: string)`.
    pub async fn unsend_message(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("unsendMessage", args).await
    }

    /// `updateAndGetNearby` (args struct `updateAndGetNearby_args`).
    pub async fn update_and_get_nearby(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateAndGetNearby", args).await
    }

    /// `updateChat(request: UpdateChatRequest)`.
    pub async fn update_chat(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateChat", args).await
    }

    /// `updateChatRoomBGM(reqSeq: i32, chatRoomMid: string, chatRoomBGMInfo: string)`.
    pub async fn update_chat_room_bgm(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateChatRoomBGM", args).await
    }

    /// `updateContactSetting(reqSeq: i32, mid: string, flag: ContactSetting, value: string)`.
    pub async fn update_contact_setting(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateContactSetting", args).await
    }

    /// `updateExtendedProfileAttribute(reqSeq: i32, attr: Pb1_EnumC13180t4, extendedProfile: ExtendedProfile)`.
    pub async fn update_extended_profile_attribute(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateExtendedProfileAttribute", args).await
    }

    /// `updateNotificationToken(token: string, type: NotificationType)`.
    pub async fn update_notification_token(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateNotificationToken", args).await
    }

    /// `updateProfileAttributes(reqSeq: i32, request: UpdateProfileAttributesRequest)`.
    pub async fn update_profile_attributes(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateProfileAttributes", args).await
    }

    /// `updateSettingsAttributes2(reqSeq: i32, settings: Settings, attributesToUpdate: set<SettingsAttributeEx>)`.
    pub async fn update_settings_attributes2(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateSettingsAttributes2", args).await
    }

    /// `verifyPhoneNumber` (args struct `verifyPhoneNumber_args`).
    pub async fn verify_phone_number(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("verifyPhoneNumber", args).await
    }

    /// `verifyQrcode(verifier: string, pinCode: string)`.
    pub async fn verify_qrcode(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("verifyQrcode", args).await
    }

    /// `wakeUpLongPolling(clientRevision: i64)`.
    pub async fn wake_up_long_polling(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("wakeUpLongPolling", args).await
    }
}
// </generated pass-through>

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::test_util::{client, encode, field, path, s};

    #[test]
    fn sync_args_defaults() {
        let f = encode("sync_args", &sync_args(&SyncOptions { revision: Some(5), ..Default::default() }));
        assert_eq!(path(&f, &[1, 1]), Some(&TValue::I64(5)));
        assert_eq!(path(&f, &[1, 2]), Some(&TValue::I32(100)));
        assert_eq!(path(&f, &[1, 3]), Some(&TValue::I64(0)));
    }

    #[test]
    fn postback_args_encoding() {
        let f = encode("sendPostback_args", &postback_args("1", "https://p", "c1", "u1"));
        assert_eq!(path(&f, &[2, 1]), Some(&s("1")));
        assert_eq!(path(&f, &[2, 2]), Some(&s("https://p")));
        assert_eq!(path(&f, &[2, 3]), Some(&s("c1")));
        assert_eq!(path(&f, &[2, 4]), Some(&s("u1")));
    }

    #[test]
    fn react_args_encoding() {
        let o = ReactOptions { id: 42, reaction: "NICE".into(), req_seq: None };
        let f = encode("react_args", &react_args(7, &o));
        assert_eq!(
            f,
            vec![(
                1,
                TValue::Struct(vec![
                    (1, TValue::I32(7)),
                    (2, TValue::I64(42)),
                    (3, TValue::Struct(vec![(1, TValue::I32(2))]))
                ])
            )]
        );
        // explicit zero sequence and a message id beyond f64 precision
        let o = ReactOptions { id: 9_007_199_254_740_993, reaction: "NICE".into(), req_seq: Some(0) };
        let f = encode("react_args", &react_args(0, &o));
        assert_eq!(path(&f, &[1, 1]), Some(&TValue::I32(0)));
        assert_eq!(path(&f, &[1, 2]), Some(&TValue::I64(9_007_199_254_740_993)));
    }

    #[tokio::test]
    async fn reqseq_is_monotonic_and_persisted() {
        let c = client();
        let mut seqs = Vec::new();
        for _ in 0..5 {
            seqs.push(c.get_reqseq("talk").await.unwrap());
        }
        assert_eq!(seqs, [0, 1, 2, 3, 4]);
        assert_eq!(c.get_reqseq("square").await.unwrap(), 0);
        let stored: serde_json::Value =
            serde_json::from_str(&c.storage.get_string("reqseq").await.unwrap().unwrap()).unwrap();
        assert_eq!(stored, serde_json::json!({"talk": 5, "square": 1}));
    }

    #[test]
    fn send_message_args_plain_and_reply() {
        let mut o = SendMessageOptions::text("u0", "hi");
        let f = encode("sendMessage_args", &send_message_args(7, &o));
        assert_eq!(field(&f, 1), Some(&TValue::I32(7)));
        assert_eq!(path(&f, &[2, 2]), Some(&s("u0")));
        assert_eq!(path(&f, &[2, 10]), Some(&s("hi")));
        assert_eq!(path(&f, &[2, 15]), Some(&TValue::I32(0)));
        assert_eq!(path(&f, &[2, 21]), None);

        o.related_message_id = Some("123".into());
        o.content_metadata.insert("k".into(), "v".into());
        let f = encode("sendMessage_args", &send_message_args(8, &o));
        assert_eq!(path(&f, &[2, 21]), Some(&s("123")));
        assert_eq!(path(&f, &[2, 22]), Some(&TValue::I32(3)));
        assert!(path(&f, &[2, 24]).is_some());
        let Some(TValue::Map(_, _, entries)) = path(&f, &[2, 18]) else { panic!("metadata") };
        assert_eq!(entries, &vec![(s("k"), s("v"))]);
    }

    #[test]
    fn send_message_args_chunks() {
        let o = SendMessageOptions { to: "u0".into(), chunks: Some(vec![vec![1, 2, 3]]), ..Default::default() };
        let f = encode("sendMessage_args", &send_message_args(0, &o));
        assert_eq!(path(&f, &[2, 20]), Some(&TValue::List(TType::String, vec![TValue::Binary(vec![1, 2, 3])])));
    }

    #[test]
    fn get_chats_args_defaults() {
        let f = encode(
            "getChats_args",
            &get_chats_args(&GetChatsOptions { chat_mids: vec!["c1".into()], ..Default::default() }),
        );
        assert_eq!(path(&f, &[1, 1]), Some(&TValue::List(TType::String, vec![s("c1")])));
        assert_eq!(path(&f, &[1, 2]), Some(&TValue::Bool(true)));
        assert_eq!(path(&f, &[1, 3]), Some(&TValue::Bool(true)));
        assert_eq!(
            field(&f, 2),
            Some(&TValue::I32(Schema::line().get_enum("Pb1_V7").unwrap().value_of("INTERNAL").unwrap()))
        );
    }

    #[test]
    fn content_type_numbers() {
        assert_eq!(content_type_number(&"NONE".into()), 0);
        assert_eq!(content_type_number(&"IMAGE".into()), 1);
        assert_eq!(content_type_number(&Value::Int(7)), 7);
    }

    #[tokio::test]
    async fn send_message_rethrows_without_retry_when_disabled() {
        let c = client();
        c.disable();
        let err = c.talk().send_message(SendMessageOptions::text("u0", "hi")).await.unwrap_err();
        assert!(err.to_string().contains("ClientClosed"), "{err}");
        let err = c.talk().send_compact_plain_message(&format!("u{}", "0".repeat(32)), "hi").await.unwrap_err();
        assert!(err.to_string().contains("ClientClosed"), "{err}");
    }

    // --- RE-recovered methods: offline encode/decode against the IDL. ---

    #[test]
    fn edit_message_args_layout() {
        let f = encode(
            "editMessage_args",
            &crate::tv!({"req": {"reqSeq": 7, "message": {"to": "u1", "text": "hi", "id": "m1"}}}),
        );
        assert_eq!(path(&f, &[1, 1]), Some(&TValue::I32(7)));
        assert_eq!(path(&f, &[1, 2, 2]), Some(&s("u1"))); // Message.to (field 2)
        assert_eq!(path(&f, &[1, 2, 10]), Some(&s("hi"))); // Message.text (field 10)
    }

    #[test]
    fn get_messages_by_ids_args_layout() {
        // field id 2 at the top level (no field 1), nested MessageBoxV2MessageId
        let f = encode(
            "getMessagesByIds_args",
            &crate::tv!({"messagesByIdsRequests": [
                {"chatMid": "c1", "messageIds": [{"deliveredTime": 1, "messageId": 9_007_199_254_740_993i64}]}
            ]}),
        );
        let Some(TValue::List(TType::Struct, parts)) = field(&f, 2) else { panic!("field 2 list") };
        assert_eq!(parts[0].field(1), Some(&s("c1")));
        let Some(TValue::List(TType::Struct, ids)) = parts[0].field(2) else { panic!("messageIds") };
        assert_eq!(ids[0].field(1), Some(&TValue::I64(1)));
        assert_eq!(ids[0].field(2), Some(&TValue::I64(9_007_199_254_740_993)));
    }

    #[test]
    fn reissue_user_ticket_args_ids_3_and_4() {
        let f = encode("reissueUserTicket_args", &crate::tv!({"expirationTime": 100i64, "maxUseCount": 5}));
        assert_eq!(field(&f, 3), Some(&TValue::I64(100)));
        assert_eq!(field(&f, 4), Some(&TValue::I32(5)));
        assert_eq!(field(&f, 1), None);
    }

    #[test]
    fn get_pinned_chats_result_decodes_map_and_token() {
        // 0: GetPinnedChatsResponse { 1: map<string,i64>, 2: string }
        let success = TValue::Struct(vec![
            (1, TValue::Map(TType::String, TType::I64, vec![(s("c1"), TValue::I64(42))])),
            (2, s("next")),
        ]);
        let result = TValue::Struct(vec![(0, success)]);
        let v = crate::thrift::decode_struct(Schema::line(), Some("getPinnedChats_result"), &result);
        assert_eq!(v["success"]["pinnedChats"]["c1"].as_i64(), Some(42));
        assert_eq!(v["success"]["nextPageToken"].as_str(), Some("next"));
    }
}
