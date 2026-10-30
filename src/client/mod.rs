//! High-level client (linejs `client/`): [`Client`] wraps a logged-in
//! [`BaseClient`] and hands out message / chat / user / square objects.
//!
//! ```no_run
//! # async fn demo() -> line_rs::Result<()> {
//! use line_rs::client::{login_with_auth_token, ClientEvent, InitOptions, ListenOptions};
//! use line_rs::Device;
//!
//! let client = login_with_auth_token("token", InitOptions::new(Device::Android)).await?;
//! let mut events = client.subscribe();
//! let _handle = client.listen(ListenOptions::default());
//! while let Ok(event) = events.recv().await {
//!     if let ClientEvent::Message(m) = event {
//!         if m.text() == "!ping" {
//!             m.reply("pong").await?;
//!         }
//!     }
//! }
//! # Ok(()) }
//! ```

mod chat;
mod listen;
pub mod liff;
mod login;
pub mod message;
mod profile;
mod square;
mod user;
pub mod voom;

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use tokio::sync::broadcast;

pub use chat::{Chat, MessageFetcher, SentMedia};
pub use listen::{ListenHandle, ListenOptions, ReactionEvent, TalkEvent};
pub use login::{create_base_client, login_with_auth_token, login_with_password, login_with_qr, InitOptions};
pub use message::{
    build_text_decorations, AppLinkInfo, CallInfo, ChatEventInfo, Decoration, Emoji, GiftInfo, MentionTarget,
    MessageInput, Mid, PostNotificationInfo, ReactionType, SquareMessage, SquareThreadMessage, TalkMessage,
};
pub use profile::{MyProfileUpdate, ProfileAttribute};
pub use square::{Square, SquareChat, SquareChatEvent};
pub use user::User;
pub use voom::{Voom, VoomChannel, VoomRestOptions, VoomRestResponse, VoomRouting};

use crate::base::BaseClient;
use crate::service::talk::compact::CompactMessageResponse;
use crate::service::talk::SendCompactMessageOptions;
use crate::e2ee::E2eeContent;
use crate::error::{Error, Result};
use crate::request::{Parse, RequestOptions};
use crate::thrift::{Protocol, TType, TValue, Value};
use crate::tv;
use message::types::enum_value;
use message::utils::metadata_value;

/// Events emitted by [`Client`] while [listening](Client::listen).
#[derive(Debug, Clone)]
pub enum ClientEvent {
    /// A talk message was sent or received (`SEND_MESSAGE` /
    /// `RECEIVE_MESSAGE`), already decrypted.
    Message(TalkMessage),
    /// A talk message was edited (`EDIT_MESSAGE` / `NOTIFIED_EDIT_MESSAGE`).
    /// It keeps the id of the original message.
    MessageEdit(TalkMessage),
    /// Every talk `Operation`.
    Event(Value),
    /// A typed view of a common talk `Operation` (read receipts,
    /// reactions, unsend, calls, group membership, contacts), emitted right
    /// after its [`Event`](Self::Event).
    Talk(TalkEvent),
    /// A square `NOTIFICATION_MESSAGE`.
    SquareMessage(SquareMessage),
    /// Every `SquareEvent`.
    SquareEvent(Value),
}

/// The high-level LINE client. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    pub base: BaseClient,
    inner: Arc<Inner>,
}

struct Inner {
    events: broadcast::Sender<ClientEvent>,
    /// Default LIFF id used by [`Client::liff`].
    liff_id: Mutex<String>,
    /// LIFF access tokens by chat mid.
    liff_tokens: Mutex<HashMap<String, String>>,
    /// VOOM channel tokens by channel id.
    voom_tokens: Mutex<HashMap<String, String>>,
}

impl Client {
    /// Wrap a (logged-in) base client.
    pub fn new(base: BaseClient) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            base,
            inner: Arc::new(Inner {
                events,
                liff_id: Mutex::new(liff::DEFAULT_LIFF_ID.to_string()),
                liff_tokens: Mutex::default(),
                voom_tokens: Mutex::default(),
            }),
        }
    }

    /// Subscribe to [`ClientEvent`]s.
    pub fn subscribe(&self) -> broadcast::Receiver<ClientEvent> {
        self.inner.events.subscribe()
    }

    pub(crate) fn emit(&self, event: ClientEvent) {
        let _ = self.inner.events.send(event);
    }

    /// The current auth token.
    pub fn auth_token(&self) -> Option<String> {
        self.base.auth_token()
    }

    /// The logged-in user's mid.
    pub fn mid(&self) -> Option<String> {
        self.base.mid()
    }

    // -----------------------------------------------------------------------
    // Sending
    // -----------------------------------------------------------------------

    /// Send a talk message to `to` (user, group or room); port of linejs
    /// `talk.sendMessage`. With `e2ee: Some(true)` text/location is encrypted;
    /// with `None` a server E2EE demand triggers one encrypted retry.
    pub async fn send_message(&self, to: &str, input: impl Into<MessageInput>) -> Result<TalkMessage> {
        let mut sent = self.send_message_raw(to, input.into()).await?;
        sent.insert("to", to);
        match TalkMessage::from_raw(self.clone(), sent.clone()).await {
            Ok(m) => Ok(m),
            Err(e) => {
                self.base.log("decryptE2EEMessageError", serde_json::json!({"error": e.to_string()}));
                Ok(TalkMessage::new(self.clone(), sent))
            }
        }
    }

    async fn send_message_raw(&self, to: &str, input: MessageInput) -> Result<Value> {
        let mut input = input;
        loop {
            let original = input.clone();
            let mut input_now = input.clone();
            let content_type = input_now.content_type.clone().unwrap_or_else(|| "NONE".into());
            if input_now.e2ee == Some(true)
                && input_now.chunks.is_none()
                && (input_now.text.is_some() || input_now.location.is_some())
            {
                let ct = enum_value(&Value::String(content_type.clone()), "ContentType").unwrap_or(0);
                let payload = match (input_now.text.take().filter(|t| !t.is_empty()), input_now.location.take()) {
                    (Some(t), _) => E2eeContent::Text(t),
                    (None, Some(loc)) => E2eeContent::Location(loc),
                    (None, None) => E2eeContent::Text("invalid".into()),
                };
                let chunks = self.base.encrypt_e2ee_message(to, payload, ct, 2).await?;
                input_now.content_metadata.insert("e2eeVersion".into(), "2".into());
                input_now.content_metadata.insert("contentType".into(), ct.to_string());
                input_now.content_metadata.insert("e2eeMark".into(), "2".into());
                input_now.chunks = Some(chunks);
            }
            let mut message = tv!({
                "to": to,
                "createdTime": 0,
                "deliveredTime": 0,
                "hasContent": false,
                "contentType": content_type,
                "sessionId": 0,
                "text": input_now.text,
                "relatedMessageId": input_now.related_message_id,
            });
            message.insert("contentMetadata", metadata_value(&input_now.content_metadata));
            if let Some(loc) = input_now.location {
                message.insert("location", loc);
            }
            if let Some(chunks) = input_now.chunks {
                message.insert("chunks", Value::List(chunks.into_iter().map(Value::Binary).collect()));
            }
            if input_now.related_message_id.is_some() {
                message.insert("messageRelationType", "REPLY");
                message.insert("relatedMessageServiceCode", "TALK");
            }
            let mut args = tv!({"seq": self.base.get_reqseq("talk").await?});
            args.insert("message", message);
            match self.base.talk().call("sendMessage", args).await {
                Err(e) if original.e2ee.is_none() && error_code(&e).contains("E2EE") => {
                    input = original;
                    input.e2ee = Some(true);
                }
                other => return other,
            }
        }
    }

    /// Send a message to a square chat (`SquareService.sendMessage`).
    /// Returns the `SendMessageResponse`.
    pub async fn send_square_message(&self, square_chat_mid: &str, input: impl Into<MessageInput>) -> Result<Value> {
        let input = input.into();
        let content_type: Value = input.content_type.map(Value::String).unwrap_or(Value::Int(0));
        let mut message = tv!({"to": square_chat_mid, "text": input.text});
        message.insert("contentType", content_type);
        message.insert("contentMetadata", metadata_value(&input.content_metadata));
        if let Some(loc) = input.location {
            message.insert("location", loc);
        }
        if let Some(rel) = input.related_message_id {
            message.insert("relatedMessageId", rel);
            message.insert("relatedMessageServiceCode", "SQUARE");
            message.insert("messageRelationType", "REPLY");
        }
        let mut square_message = tv!({"squareMessageRevision": 4});
        square_message.insert("message", message);
        let mut request = tv!({"reqSeq": self.base.get_reqseq("sq").await?, "squareChatMid": square_chat_mid});
        request.insert("squareMessage", square_message);
        let mut args = Value::object();
        args.insert("request", request);
        self.base.square().call("sendMessage", args).await
    }

    /// Send a compact talk message through `/CA5` or `/ECA5` (falls back to
    /// E2EE when the server requires it).
    pub async fn send_compact_message(&self, to: &str, text: &str) -> Result<CompactMessageResponse> {
        let options = SendCompactMessageOptions { to: to.to_string(), text: Some(text.to_string()), ..Default::default() };
        self.base.talk().send_compact_message(options).await
    }

    // -----------------------------------------------------------------------
    // Fetching
    // -----------------------------------------------------------------------

    /// All group chats the user joined.
    pub async fn fetch_joined_chats(&self) -> Result<Vec<Chat>> {
        let joined = self
            .base
            .talk()
            .call("getAllChatMids", tv!({"request": {"withMemberChats": true}, "syncReason": "INTERNAL"}))
            .await?;
        let mids: Vec<Value> = joined["memberChatMids"].as_list().unwrap_or_default().to_vec();
        if mids.is_empty() {
            return Ok(Vec::new());
        }
        let res = self.get_chats(Value::List(mids)).await?;
        Ok(list(&res["chats"]).into_iter().map(|raw| Chat::new(self.clone(), raw)).collect())
    }

    async fn get_chats(&self, mids: Value) -> Result<Value> {
        let mut request = tv!({"withInvitees": true, "withMembers": true});
        request.insert("chatMids", mids);
        let mut args = tv!({"syncReason": "INTERNAL"});
        args.insert("request", request);
        self.base.talk().call("getChats", args).await
    }

    /// All friends. Tries `getContactsV3`, then `getContactsV2`, then one
    /// `getContactsV3` per mid (skipping failures) when the server answers
    /// "API method not capable".
    pub async fn fetch_users(&self) -> Result<Vec<User>> {
        let ids = self.base.relation().call("getUserFriendIds", tv!({"request": {"blockStatus": "ALL"}})).await?;
        let mids: Vec<String> = list(&ids["userFriendMids"]).iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        let raws = fetch_users_with(
            mids,
            |mids| Box::pin(self.get_contacts_v3(mids)),
            |mids| Box::pin(self.get_contacts_v2(mids)),
            |mid| Box::pin(async move { Ok(self.get_user(&mid).await?.raw) }),
        )
        .await?;
        Ok(raws.into_iter().map(|raw| User::new(self.clone(), raw)).collect())
    }

    async fn get_contacts_v3(&self, mids: Vec<String>) -> Result<Value> {
        let targets: Vec<serde_json::Value> = mids.iter().map(|m| serde_json::json!({"targetUserMid": m})).collect();
        self.base
            .relation()
            .call("getContactsV3", tv!({"request": {"targetUsers": targets, "syncReason": "AUTO_REPAIR"}}))
            .await
    }

    /// `getContactsV2` has no `_args` struct in the IDL; encode by hand like
    /// linejs (`[[12, 1, [[15, 1, [11, mids]]]]]`).
    async fn get_contacts_v2(&self, mids: Vec<String>) -> Result<Value> {
        let list = TValue::List(TType::String, mids.into_iter().map(TValue::string).collect());
        let args = vec![(1, TValue::Struct(vec![(1, list)]))];
        let opts = RequestOptions::new("/S4", Protocol::Compact).parse(Parse::Struct("GetContactsV2Response"));
        self.base.request(args, "getContactsV2", opts).await
    }

    /// All squares (OpenChats) the user joined.
    pub async fn fetch_joined_squares(&self) -> Result<Vec<Square>> {
        let res = continue_request(|token| async move {
            self.base
                .square()
                .call("getJoinedSquares", tv!({"request": {"limit": 100, "continuationToken": token}}))
                .await
        })
        .await?;
        Ok(list(&res["squares"]).into_iter().map(|raw| Square::new(self.clone(), raw)).collect())
    }

    /// Square chats found in the latest `fetchMyEvents` page
    /// (`notifiedCreateSquareChatMember` events).
    pub async fn fetch_joined_square_chats(&self) -> Result<Vec<SquareChat>> {
        let res = self.base.square().call("fetchMyEvents", tv!({"request": {"limit": 200}})).await?;
        Ok(list(&res["events"])
            .into_iter()
            .filter_map(|e| e["payload"].get("notifiedCreateSquareChatMember").map(|n| n["chat"].clone()))
            .map(|raw| SquareChat::new(self.clone(), raw))
            .collect())
    }

    /// A user by mid (`getContactsV3`).
    pub async fn get_user(&self, mid: &str) -> Result<User> {
        let res = self.get_contacts_v3(vec![mid.to_string()]).await?;
        Ok(User::new(self.clone(), res["responses"][0].clone()))
    }

    /// A group chat by mid (with members and invitees).
    pub async fn get_chat(&self, chat_mid: &str) -> Result<Chat> {
        let res = self.get_chats(tv!([chat_mid])).await?;
        let raw = res["chats"][0].clone();
        if raw.is_null() {
            return Err(Error::internal("NotFound", format!("chat {chat_mid} not found")));
        }
        Ok(Chat::new(self.clone(), raw))
    }

    pub async fn get_square(&self, square_mid: &str) -> Result<Square> {
        let res = self.base.square().call("getSquare", tv!({"request": {"mid": square_mid}})).await?;
        Ok(Square::new(self.clone(), res["square"].clone()))
    }

    pub async fn get_square_chat(&self, square_chat_mid: &str) -> Result<SquareChat> {
        let res = self.base.square().call("getSquareChat", tv!({"request": {"squareChatMid": square_chat_mid}})).await?;
        Ok(SquareChat::new(self.clone(), res["squareChat"].clone()))
    }
}

/// Elements of a list value (owned).
pub(crate) fn list(v: &Value) -> Vec<Value> {
    v.as_list().map(<[Value]>::to_vec).unwrap_or_default()
}

/// The server `code` of an error as text (enum name or number).
pub(crate) fn error_code(e: &Error) -> String {
    match e {
        Error::Request { data, .. } => match &data["code"] {
            Value::Null => String::new(),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        },
        _ => String::new(),
    }
}

fn is_api_not_capable(e: &Error) -> bool {
    e.to_string().to_ascii_lowercase().contains("api method not capable")
}

type FetchFn<'a, A> = Box<dyn FnMut(A) -> BoxFuture<'a, Result<Value>> + Send + 'a>;

/// The V3 → V2 → per-mid fallback of [`Client::fetch_users`], over injected
/// calls so the policy is testable. Returns `GetContactV3Response`-shaped raws.
pub(crate) async fn fetch_users_with<'a>(
    mids: Vec<String>,
    v3: impl FnMut(Vec<String>) -> BoxFuture<'a, Result<Value>> + Send + 'a,
    v2: impl FnMut(Vec<String>) -> BoxFuture<'a, Result<Value>> + Send + 'a,
    one: impl FnMut(String) -> BoxFuture<'a, Result<Value>> + Send + 'a,
) -> Result<Vec<Value>> {
    let (mut v3, mut v2, mut one): (FetchFn<'a, Vec<String>>, FetchFn<'a, Vec<String>>, FetchFn<'a, String>) =
        (Box::new(v3), Box::new(v2), Box::new(one));
    if mids.is_empty() {
        return Ok(Vec::new());
    }
    match v3(mids.clone()).await {
        Ok(res) => return Ok(list(&res["responses"])),
        Err(e) if !is_api_not_capable(&e) => return Err(e),
        Err(_) => {}
    }
    match v2(mids.clone()).await {
        Ok(res) => {
            let out: Vec<Value> = res["contacts"]
                .entries()
                .into_iter()
                .map(|(mid, entry)| {
                    let mut raw = if entry.as_struct().is_some() { entry.clone() } else { Value::object() };
                    raw.insert("targetUserMid", mid);
                    raw
                })
                .collect();
            if !out.is_empty() {
                return Ok(out);
            }
        }
        Err(e) if !is_api_not_capable(&e) => return Err(e),
        Err(_) => {}
    }
    let mut out = Vec::new();
    for mid in mids {
        if let Ok(raw) = one(mid).await {
            out.push(raw);
        }
    }
    Ok(out)
}

/// Repeat a paginated call while it returns a `continuationToken`, merging
/// the pages (port of linejs `continueRequest`: lists of later pages are
/// prepended, nested structs merged, scalars overwritten).
pub(crate) async fn continue_request<F, Fut>(mut handler: F) -> Result<Value>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    let mut sum: Option<Value> = None;
    let mut token: Option<String> = None;
    loop {
        let res = handler(token.take()).await?;
        let next = res["continuationToken"].as_str().filter(|t| !t.is_empty()).map(str::to_string);
        match &mut sum {
            None => sum = Some(res),
            Some(s) => object_sum(s, res),
        }
        match next {
            Some(t) => token = Some(t),
            None => return Ok(sum.unwrap_or_default()),
        }
    }
}

fn object_sum(base: &mut Value, add: Value) {
    let entries: Vec<(Value, Value)> = match add {
        Value::Struct(m) => m.into_iter().map(|(k, v)| (Value::String(k), v)).collect(),
        Value::Map(m) => m,
        _ => return,
    };
    for (k, v) in entries {
        let Some(key) = k.key_string() else { continue };
        if v.is_null() {
            continue;
        }
        let existing = match base {
            Value::Struct(m) => m.get_mut(&key),
            Value::Map(m) => m.iter_mut().find(|(ek, _)| ek.key_string().as_deref() == Some(&key)).map(|(_, v)| v),
            _ => return,
        };
        match (existing, v) {
            (Some(Value::List(old)), Value::List(mut new)) => {
                new.append(old);
                *old = new;
            }
            (Some(old @ (Value::Struct(_) | Value::Map(_))), new @ (Value::Struct(_) | Value::Map(_))) => {
                object_sum(old, new)
            }
            (Some(old), new) if !old.is_null() => *old = new,
            (_, new) => match base {
                Value::Struct(m) => {
                    m.insert(key, new);
                }
                Value::Map(m) => {
                    m.retain(|(ek, _)| ek.key_string().as_deref() != Some(&key));
                    m.push((k, new));
                }
                _ => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn not_capable() -> Error {
        Error::internal("RequestError", "API method not capable: 'getContactsV3'")
    }

    async fn run(
        mids: &[&str],
        v3: impl Fn() -> Result<Value> + Send + Sync,
        v2: impl Fn() -> Result<Value> + Send + Sync,
    ) -> Result<Vec<Value>> {
        let mids = mids.iter().map(|s| s.to_string()).collect();
        let (v3, v2) = (&v3, &v2);
        fetch_users_with(
            mids,
            move |_| Box::pin(async move { v3() }),
            move |_| Box::pin(async move { v2() }),
            |mid| Box::pin(async move { if mid == "u-bad" { Err(not_capable()) } else { Ok(tv!({"targetUserMid": mid})) } }),
        )
        .await
    }

    #[tokio::test]
    async fn fetch_users_empty_friend_list() {
        let users = run(&[], || panic!("no call"), || panic!("no call")).await.unwrap();
        assert!(users.is_empty());
    }

    #[tokio::test]
    async fn fetch_users_v3_path() {
        let users = run(
            &["u-a", "u-b"],
            || Ok(tv!({"responses": [{"targetUserMid": "u-a", "displayName": "Alice"}, {"targetUserMid": "u-b"}]})),
            || panic!("no v2"),
        )
        .await
        .unwrap();
        let mids: Vec<_> = users.iter().map(|u| u["targetUserMid"].as_str().unwrap()).collect();
        assert_eq!(mids, ["u-a", "u-b"]);
    }

    #[tokio::test]
    async fn fetch_users_v2_fallback() {
        let users = run(
            &["u-x"],
            || Err(not_capable()),
            || Ok(tv!({"contacts": {"u-x": {"displayName": "Xeno", "userStatus": 0}}})),
        )
        .await
        .unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0]["targetUserMid"].as_str(), Some("u-x"));
        assert_eq!(users[0]["displayName"].as_str(), Some("Xeno"));
    }

    #[tokio::test]
    async fn fetch_users_per_mid_fallback_skips_failures() {
        let users = run(&["u-a", "u-bad"], || Err(not_capable()), || Err(not_capable())).await.unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0]["targetUserMid"].as_str(), Some("u-a"));
    }

    #[tokio::test]
    async fn fetch_users_rethrows_other_errors() {
        let err = run(&["u-z"], || Err(Error::internal("X", "AUTHENTICATION_FAILED")), || panic!("no v2"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("AUTHENTICATION_FAILED"));
    }

    /// Every key of `v` must name a field of `ty` (recursively), so a typo
    /// cannot be dropped silently by the encoder.
    fn check_fields(path: &str, ty: &crate::thrift::TypeRef, v: &Value) {
        use crate::thrift::schema::Named;
        use crate::thrift::{Schema, TypeRef};
        match (ty, v) {
            (TypeRef::Named(name), Value::Struct(m)) => {
                let Some(Named::Struct(def)) = Schema::line().resolve(name) else { return };
                for (k, fv) in m {
                    let f = def.field_by_name(k).unwrap_or_else(|| panic!("{path}: {name} has no field `{k}`"));
                    check_fields(&format!("{path}.{k}"), &f.ty, fv);
                }
            }
            (TypeRef::List(inner) | TypeRef::Set(inner), Value::List(items)) => {
                for it in items {
                    check_fields(path, inner, it);
                }
            }
            _ => {}
        }
    }

    fn check(method: &str, square: bool, args: Value) {
        let name = if square { format!("SquareService_{method}_args") } else { format!("{method}_args") };
        let ty = crate::thrift::TypeRef::Named(name.clone());
        check_fields(&name, &ty, &args);
        crate::thrift::encode_struct(crate::thrift::Schema::line(), &name, &args).unwrap();
    }

    #[test]
    fn request_shapes_match_idl() {
        let message = tv!({"to": "u", "createdTime": 0, "deliveredTime": 0, "hasContent": false, "contentType": "NONE",
            "sessionId": 0, "text": "t", "relatedMessageId": "1", "contentMetadata": {"a": "b"}, "chunks": ["x"],
            "messageRelationType": "REPLY", "relatedMessageServiceCode": "TALK", "location": {"title": "x"}});
        let mut send = tv!({"seq": 1});
        send.insert("message", message);
        check("sendMessage", false, send);
        check("react", false, tv!({"reactRequest": {"reqSeq": 1, "messageId": "1",
            "reactionType": {"predefinedReactionType": "NICE"}}}));
        check("sendChatChecked", false, tv!({"chatMid": "c", "lastMessageId": "1", "seq": 1}));
        check("createChatRoomAnnouncement", false, tv!({"chatRoomMid": "c", "type": "MESSAGE",
            "contents": {"text": "t", "link": "l"}}));
        check("unsendMessage", false, tv!({"messageId": "1"}));
        check("getAllChatMids", false, tv!({"request": {"withMemberChats": true}, "syncReason": "INTERNAL"}));
        check("getChats", false, tv!({"request": {"withInvitees": true, "withMembers": true, "chatMids": ["c"]},
            "syncReason": "INTERNAL"}));
        check("getMessageBoxes", false, tv!({"messageBoxListRequest": {}}));
        check("getPreviousMessagesV2WithRequest", false, tv!({"request": {"messageBoxId": "c", "messagesCount": 1,
            "endMessageId": {"deliveredTime": 1, "messageId": 2}}}));
        check("updateChat", false, tv!({"request": {"updatedAttribute": "NAME", "reqSeq": 1,
            "chat": {"chatMid": "c", "chatName": "n"}}}));
        check("inviteIntoChat", false, tv!({"request": {"targetUserMids": ["u"], "chatMid": "c"}}));
        check("deleteOtherFromChat", false, tv!({"request": {"targetUserMids": ["u"], "chatMid": "c"}}));
        check("deleteSelfFromChat", false, tv!({"request": {"chatMid": "c"}}));
        check("getChatRoomBGMs", false, tv!({"chatRoomMids": ["c"]}));
        check("updateChatRoomBGM", false, tv!({"reqSeq": 1, "chatRoomMid": "c", "chatRoomBGMInfo": ""}));
        check("updateContactSetting", false, tv!({"reqSeq": 1, "mid": "u", "flag": "CONTACT_SETTING_FAVORITE",
            "value": "true"}));
        check("updateChat", false, tv!({"request": {"updatedAttribute": "NOTIFICATION_SETTING", "reqSeq": 1,
            "chat": {"chatMid": "c", "notificationDisabled": true}}}));
        check("updateChat", false, tv!({"request": {"updatedAttribute": "FAVORITE_TIMESTAMP", "reqSeq": 1,
            "chat": {"chatMid": "c", "favoriteTimestamp": 1}}}));
        check("setChatHiddenStatus", false, chat::hidden_status_args(1, "c", 9_007_199_254_740_993, true));
        check("sendChatChecked", false, chat::chat_marker_args(1, "c", "1"));
        check("sendChatRemoved", false, chat::chat_marker_args(1, "c", "1"));
        check("getChatRoomAnnouncements", false, tv!({"chatRoomMid": "c"}));
        check("removeChatRoomAnnouncement", false, tv!({"reqSeq": 1, "chatRoomMid": "c", "announcementSeq": 2}));
        check("getMessageReadRange", false, tv!({"chatIds": ["c"], "syncReason": "INTERNAL"}));
        check("getChatEffectMetaList", false, tv!({"categories": ["BACKGROUND", "KEYWORD"]}));
        check("cancelReaction", false, tv!({"cancelReactionRequest": {"reqSeq": 1, "messageId": 2}}));
        check("sendPostback", false, crate::service::talk::postback_args("1", "u", "c", "o"));
        check("getProfile", false, tv!({"syncReason": "INTERNAL"}));
        check("getUserFriendIds", false, tv!({"request": {"blockStatus": "ALL"}}));
        check("getContactsV3", false, tv!({"request": {"targetUsers": [{"targetUserMid": "u"}],
            "syncReason": "AUTO_REPAIR"}}));
        check("getContactCalendarEvents", false, tv!({"request": {"targetUsers": [{"targetUserMid": "u"}]}}));
        check("issueChannelToken", false, tv!({"channelId": "1"}));
        check("sync", false, tv!({"request": {"lastRevision": 1, "lastGlobalRevision": 1,
            "lastIndividualRevision": 1, "count": 100}}));

        check("sendMessage", true, tv!({"request": {"reqSeq": 1, "squareChatMid": "s", "squareMessage": {
            "squareMessageRevision": 4, "message": {"to": "s", "text": "t", "contentType": 0,
            "relatedMessageId": "1", "relatedMessageServiceCode": "SQUARE", "messageRelationType": "REPLY"}}}}));
        check("reactToMessage", true, tv!({"request": {"reqSeq": 0, "reactionType": 2, "messageId": "1",
            "squareChatMid": "s", "threadMid": "t"}}));
        check("markAsRead", true, tv!({"request": {"messageId": "1", "squareChatMid": "s", "threadMid": "t"}}));
        check("createSquareChatAnnouncement", true, tv!({"createSquareChatAnnouncementRequest": {
            "reqSeq": 0, "squareChatMid": "s", "squareChatAnnouncement": {"announcementSeq": 0,
            "contents": {"textMessageAnnouncementContents": {"senderSquareMemberMid": "m", "messageId": "1",
            "text": "t"}}, "createdAt": 0, "type": 0}}}));
        check("unsendMessage", true, tv!({"request": {"messageId": "1", "squareChatMid": "s", "threadMid": "t"}}));
        check("destroyMessage", true, tv!({"request": {"messageId": "1", "squareChatMid": "s", "threadMid": "t"}}));
        check("getSquareChat", true, tv!({"request": {"squareChatMid": "s"}}));
        check("getSquare", true, tv!({"request": {"mid": "s"}}));
        check("getJoinedSquares", true, tv!({"request": {"limit": 100, "continuationToken": "t"}}));
        check("fetchMyEvents", true, tv!({"request": {"limit": 100, "syncToken": "t", "continuationToken": "c"}}));
        check("updateSquare", true, tv!({"request": {"updatedAttrs": ["NAME"], "square": {"mid": "s", "name": "n"}}}));
        check("updateSquareChat", true, tv!({"request": {"updatedAttrs": ["NAME"],
            "squareChat": {"squareChatMid": "s", "name": "n"}}}));
        check("getSquareChatMembers", true, tv!({"request": {"continuationToken": "t", "squareChatMid": "s",
            "limit": 100}}));
        check("fetchSquareChatEvents", true, tv!({"request": {"limit": 100, "squareChatMid": "s", "syncToken": "t"}}));
        check("sendSquareThreadMessage", true, tv!({"request": {"reqSeq": 1, "chatMid": "s", "threadMid": "t",
            "threadMessage": {"message": {"to": "s", "text": "t", "contentType": "NONE", "toType": "SQUARE_THREAD",
            "contentMetadata": {}, "relatedMessageId": "1", "relatedMessageServiceCode": "SQUARE",
            "messageRelationType": "REPLY"}}}}));
    }

    #[tokio::test]
    async fn continue_request_merges_pages() {
        let pages = [
            tv!({"squares": [{"mid": "s1"}], "members": {"s1": "m1"}, "continuationToken": "t1"}),
            tv!({"squares": [{"mid": "s2"}], "members": {"s2": "m2"}}),
        ];
        let mut seen = Vec::new();
        let res = continue_request(|token| {
            seen.push(token.clone());
            let page = pages[seen.len() - 1].clone();
            async move { Ok(page) }
        })
        .await
        .unwrap();
        assert_eq!(seen, [None, Some("t1".to_string())]);
        let mids: Vec<_> = list(&res["squares"]).iter().map(|s| s["mid"].as_str().unwrap().to_string()).collect();
        assert_eq!(mids, ["s2", "s1"]);
        assert_eq!(res["members"]["s2"].as_str(), Some("m2"));
        assert_eq!(res["members"]["s1"].as_str(), Some("m1"));
    }
}
