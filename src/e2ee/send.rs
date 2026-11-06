//! Encrypt-first sending: the per-chat Letter Sealing state and the send
//! loop that recovers from `E2EE_*` errors.
//!
//! LINE for Android keeps an `E2EEUsableInfo` per chat (`jn8/c.java`, table
//! `userinfo`): whether the chat uses Letter Sealing, the key id to encrypt
//! for, which content types may be encrypted and the spec version. It is
//! filled from `negotiateE2EEPublicKey` for a user and from the group shared
//! key (`Pb1_U3`) for a group, looked up only when unknown
//! (`xn8/g0.java#B`), and a message is encrypted first whenever the state
//! allows its content type (`vq8/b.java`, `vq8/a.java#C`). A failed send is
//! recovered by `xn8/k.java#c` and resent with the same `reqSeq`.
//!
//! [`E2eeChatState`] is that record, stored as JSON under
//! `e2eeChatState:<mid>`, and [`BaseClient::send_with_e2ee_recovery`] the
//! loop.

use std::collections::BTreeSet;
use std::future::Future;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};

use super::E2eeContent;
use super::keys::{E2ee, field, key_data_of, key_id_of, no_e2ee_key};
use super::recovery::{E2eeSendRecovery, MAX_E2EE_SEND_ATTEMPTS, send_recovery};
use crate::base::BaseClient;
use crate::error::{Error, Result};
use crate::service::talk::{SendMessageOptions, talk_message_value};
use crate::thrift::Value;

/// `ContentType` numbers encrypted when the server's answer names no
/// `allowedTypes`: text (`NONE`) and `LOCATION`, the types line-rs always
/// encrypted.
const DEFAULT_ALLOWED_TYPES: [i32; 2] = [0, 15];

/// Letter Sealing state of one chat (a user, group or room), like the
/// official client's `E2EEUsableInfo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct E2eeChatState {
    /// Messages to this chat are encrypted (for the allowed types).
    pub usable: bool,
    /// The peer's public key id (user) or the group shared key id (group,
    /// room) to encrypt for.
    pub key_id: Option<i64>,
    /// `ContentType` numbers that may be encrypted.
    pub allowed_types: BTreeSet<i32>,
    /// Letter Sealing spec version (`1`, `2`; `-1` unknown).
    pub spec_version: i32,
    /// The state was looked up (or the chat was marked plain). An unknown
    /// chat is looked up before its next send.
    pub known: bool,
}

impl Default for E2eeChatState {
    fn default() -> Self {
        Self {
            usable: false,
            key_id: None,
            allowed_types: BTreeSet::new(),
            spec_version: -1,
            known: false,
        }
    }
}

impl E2eeChatState {
    /// A chat that sends plain (after `E2EE_RETRY_PLAIN`, or when the peer
    /// has no key).
    pub fn plain() -> Self {
        Self {
            known: true,
            ..Self::default()
        }
    }

    /// Whether a message of `content_type` should be encrypted.
    pub fn allows(&self, content_type: i32) -> bool {
        self.usable
            && self.key_id.is_some()
            && self.spec_version != -1
            && self.allowed_types.contains(&content_type)
    }
}

/// `allowedTypes` (a thrift `set<i32>`), or the default when absent.
fn allowed_types_of(v: &Value) -> BTreeSet<i32> {
    match v.as_list() {
        Some(list) => list
            .iter()
            .filter_map(Value::as_i64)
            .map(|t| t as i32)
            .collect(),
        None => DEFAULT_ALLOWED_TYPES.into_iter().collect(),
    }
}

fn spec_version_of(v: &Value) -> i32 {
    v.as_i64().map_or(2, |v| v as i32)
}

/// Options for [`BaseClient::send_with_e2ee_recovery`].
#[derive(Debug, Clone, Default)]
pub struct E2eeSendOptions {
    /// Numeric `ContentType` of the message, checked against the chat's
    /// `allowedTypes`.
    pub content_type: i32,
    /// `Some(true)`: always encrypt and never fall back to plain.
    /// `Some(false)`: always plain, no E2EE recovery. `None`: encrypt when
    /// the chat's [`E2eeChatState`] allows the content type.
    pub e2ee: Option<bool>,
    /// The `reqSeq` of every attempt. Default: the next `talk` sequence,
    /// taken once per send.
    pub req_seq: Option<i64>,
}

/// One attempt of [`BaseClient::send_with_e2ee_recovery`], handed to the
/// message builder.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct E2eeSendAttempt {
    /// 1 for the first send.
    pub attempt: u32,
    /// The same for every attempt of one send.
    pub req_seq: i64,
    /// Encrypt this attempt (with `state`); otherwise send plain.
    pub encrypt: bool,
    /// The chat's state for this attempt; pass it to
    /// [`BaseClient::encrypt_e2ee_message_for`].
    pub state: E2eeChatState,
}

/// Errors from our own key material (no key, a key LINE does not have, a
/// peer without Letter Sealing): the message cannot be encrypted here, but
/// may still go plain when E2EE was not required.
fn is_local_key_error(e: &Error) -> bool {
    match e {
        Error::Internal { kind, .. } => matches!(
            kind.as_str(),
            "NoE2EEKey" | "No E2EEKey" | "Not support E2EE" | "NotAuthorized"
        ),
        Error::Crypto(_) => true,
        _ => false,
    }
}

fn is_transport_error(e: &Error) -> bool {
    matches!(e, Error::Http(_) | Error::Io(_)) || e.is_timeout()
}

fn chat_state_key(mid: &str) -> String {
    format!("e2eeChatState:{mid}")
}

impl E2ee<'_> {
    /// The stored state of `mid` (unknown when never looked up).
    pub async fn chat_state(&self, mid: &str) -> E2eeChatState {
        match self.host.storage().get(&chat_state_key(mid)).await {
            Ok(Some(serde_json::Value::String(s))) => serde_json::from_str(&s).unwrap_or_default(),
            Ok(Some(v @ serde_json::Value::Object(_))) => {
                serde_json::from_value(v).unwrap_or_default()
            }
            _ => E2eeChatState::default(),
        }
    }

    pub async fn save_chat_state(&self, mid: &str, state: &E2eeChatState) -> Result<()> {
        self.host
            .storage()
            .set(&chat_state_key(mid), serde_json::to_string(state)?.into())
            .await
    }

    pub async fn forget_chat_state(&self, mid: &str) -> Result<()> {
        self.host.storage().delete(&chat_state_key(mid)).await
    }

    /// Mark `mid` plain (`g0.z`): usable = false, known = true.
    pub async fn mark_chat_plain(&self, mid: &str) -> Result<()> {
        self.host
            .e2ee_log("e2eeChatMarkedPlain", serde_json::Value::Null);
        self.save_chat_state(mid, &E2eeChatState::plain()).await
    }

    /// Look the chat's key up from the server and store the result:
    /// `negotiateE2EEPublicKey` for a user, `getLastE2EEGroupSharedKey`
    /// (registering a key when the group has none, or none we can open) for
    /// a group or room.
    pub async fn refresh_chat_state(&self, mid: &str) -> Result<E2eeChatState> {
        match BaseClient::get_to_type(mid) {
            Some(0) => self.refresh_user_state(mid).await,
            Some(1 | 2) => self.refresh_group_state(mid).await,
            _ => Err(Error::internal("Invalid mid", mid)),
        }
    }

    /// `g0.s`: usable iff the server returns a public key.
    async fn refresh_user_state(&self, mid: &str) -> Result<E2eeChatState> {
        let res = self.host.negotiate_e2ee_public_key(mid).await?;
        let spec_version = spec_version_of(&res["specVersion"]);
        let public_key = &res["publicKey"];
        let key_id = key_id_of(&public_key["keyId"]);
        let key_data = key_data_of(public_key, mid).ok();
        let state = match (key_id, key_data) {
            (Some(id), Some(data)) if spec_version != -1 => {
                self.host
                    .storage()
                    .set(
                        &format!("e2eePublicKeys:{mid}:{id}"),
                        B64.encode(data).into(),
                    )
                    .await?;
                E2eeChatState {
                    usable: true,
                    key_id: Some(id),
                    allowed_types: allowed_types_of(&res["allowedTypes"]),
                    spec_version,
                    known: true,
                }
            }
            _ => E2eeChatState::plain(),
        };
        self.save_chat_state(mid, &state).await?;
        Ok(state)
    }

    /// `g0.l`: the group's last key; a new one when there is none or ours
    /// is not in it.
    async fn refresh_group_state(&self, chat_mid: &str) -> Result<E2eeChatState> {
        let shared = match self.host.get_last_e2ee_group_shared_key(2, chat_mid).await {
            Ok(v) => v,
            Err(e) if e.code() == Some("NOT_FOUND") => {
                return self.recreate_group_state(chat_mid).await;
            }
            Err(e) => return Err(e),
        };
        let receiver_key_id = key_id_of(&shared["receiverKeyId"]).unwrap_or_default();
        if self.get_self_key_by_key_id(receiver_key_id).await.is_none() {
            self.host.e2ee_log(
                "e2eeGroupKeyNotForOurKey",
                serde_json::json!({"receiverKeyId": receiver_key_id}),
            );
            return self.recreate_group_state(chat_mid).await;
        }
        self.store_group_state(chat_mid, &shared).await
    }

    /// `g0.b`: register a new group shared key and store it. A group that
    /// cannot use Letter Sealing (`E2EE_RETRY_PLAIN`,
    /// `E2EE_GROUP_TOO_MANY_MEMBERS`) is marked plain.
    pub async fn recreate_group_state(&self, chat_mid: &str) -> Result<E2eeChatState> {
        match self.try_register_group_key(chat_mid).await {
            Ok(shared) => self.store_group_state(chat_mid, &shared).await,
            Err(e)
                if send_recovery(&crate::client::error_code(&e))
                    == Some(E2eeSendRecovery::RetryPlain) =>
            {
                self.mark_chat_plain(chat_mid).await?;
                Ok(E2eeChatState::plain())
            }
            Err(e) => Err(e),
        }
    }

    async fn store_group_state(&self, chat_mid: &str, shared: &Value) -> Result<E2eeChatState> {
        let key = self.unwrap_group_shared_key(chat_mid, shared).await?;
        let state = E2eeChatState {
            usable: true,
            key_id: Some(key.key_id),
            allowed_types: allowed_types_of(&shared["allowedTypes"]),
            spec_version: spec_version_of(&shared["specVersion"]),
            known: true,
        };
        self.save_chat_state(chat_mid, &state).await?;
        Ok(state)
    }

    /// After `E2EE_UPDATE_SENDER_KEY`: forget which own keys were confirmed
    /// and make the newest key LINE has registered for us the current one.
    ///
    /// LINE for Android drops its key and registers a new one (`g0.a()`,
    /// `g0.x()`), but only on the device that owns the account; a secondary
    /// device (LINE for PC, this library) gets its keys from the login key
    /// chain and must not replace the account's key. So this only reloads,
    /// and fails with `NoE2EEKey` when we do not hold the newest key (log in
    /// again to receive it).
    pub async fn refresh_self_key(&self) -> Result<()> {
        self.state.confirmed_keys.lock().unwrap().clear();
        let keys = self.host.get_e2ee_public_keys().await?;
        let newest = keys
            .as_list()
            .unwrap_or_default()
            .iter()
            .filter_map(|k| key_id_of(field(k, "keyId", 2)))
            .max()
            .ok_or_else(no_e2ee_key)?;
        let key = self
            .get_self_key_by_key_id(newest)
            .await
            .ok_or_else(no_e2ee_key)?;
        self.save_self_key(&key).await
    }

    /// `g0.r` for a received or fetched message: an encrypted message in a
    /// chat marked plain makes the chat unknown again, so the next send
    /// looks it up.
    pub async fn observe_message(&self, message: &Value) {
        if message["chunks"].as_list().is_none_or(<[Value]>::is_empty) {
            return;
        }
        let from = message["from"].as_str().unwrap_or_default();
        let to = message["to"].as_str().unwrap_or_default();
        let chat = match BaseClient::get_to_type(to) {
            Some(0) if self.host.self_mid().as_deref() != Some(from) => from,
            _ => to,
        };
        let state = self.chat_state(chat).await;
        if state.known && !state.usable {
            let _ = self.forget_chat_state(chat).await;
        }
    }

    /// Encrypt or not for this attempt, and the state to use.
    async fn plan(
        &self,
        to: &str,
        content_type: i32,
        plain: bool,
        force_encrypt: bool,
        refresh: bool,
    ) -> Result<(bool, E2eeChatState)> {
        let state = self.chat_state(to).await;
        if plain {
            return Ok((false, state));
        }
        if !refresh && state.known && (state.usable || !force_encrypt) {
            return Ok((force_encrypt || state.allows(content_type), state));
        }
        match self.refresh_chat_state(to).await {
            Ok(state) => Ok((force_encrypt || state.allows(content_type), state)),
            // A failed lookup only blocks the send when E2EE is required or
            // the network is down.
            Err(e) if !force_encrypt && !is_transport_error(&e) => {
                self.host.e2ee_log(
                    "e2eeChatStateLookupFailed",
                    serde_json::json!({"code": crate::client::error_code(&e)}),
                );
                Ok((false, state))
            }
            Err(e) => Err(e),
        }
    }

    /// Build the message for one attempt. A message we cannot encrypt
    /// locally is built again after a fresh lookup, then plain unless E2EE
    /// is required.
    #[allow(clippy::too_many_arguments)]
    async fn build_attempt<B, BF>(
        &self,
        build: &mut B,
        to: &str,
        attempt: u32,
        req_seq: i64,
        mut encrypt: bool,
        mut state: E2eeChatState,
        required: bool,
        content_type: i32,
    ) -> Result<Value>
    where
        B: FnMut(E2eeSendAttempt) -> BF,
        BF: Future<Output = Result<Value>>,
    {
        let mut refreshed = false;
        loop {
            let res = build(E2eeSendAttempt {
                attempt,
                req_seq,
                encrypt,
                state: state.clone(),
            })
            .await;
            match res {
                Err(e) if encrypt && is_local_key_error(&e) => {
                    if !refreshed {
                        refreshed = true;
                        if let Ok(fresh) = self.refresh_chat_state(to).await {
                            encrypt = required || fresh.allows(content_type);
                            state = fresh;
                            continue;
                        }
                    }
                    if required {
                        return Err(e);
                    }
                    self.host
                        .e2ee_log("e2eeEncryptFailedSendingPlain", e.to_string().into());
                    encrypt = false;
                }
                res => return res,
            }
        }
    }

    /// The send loop (see [`BaseClient::send_with_e2ee_recovery`]). `send`
    /// gets the `reqSeq` and the built message for each attempt.
    pub async fn send_with_recovery<B, BF, S, SF>(
        &self,
        to: &str,
        options: &E2eeSendOptions,
        req_seq: i64,
        recover: bool,
        mut build: B,
        mut send: S,
    ) -> Result<Value>
    where
        B: FnMut(E2eeSendAttempt) -> BF,
        BF: Future<Output = Result<Value>>,
        S: FnMut(i64, Value) -> SF,
        SF: Future<Output = Result<Value>>,
    {
        let to_type = BaseClient::get_to_type(to);
        let talk_chat = matches!(to_type, Some(0..=2));
        let required = talk_chat && options.e2ee == Some(true);
        let never = !talk_chat || options.e2ee == Some(false);
        let mut plain = never;
        let mut force_encrypt = required;
        let mut refresh = false;
        let mut sender_key_reloaded = false;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let (encrypt, state) = self
                .plan(to, options.content_type, plain, force_encrypt, refresh)
                .await?;
            refresh = false;
            let message = self
                .build_attempt(
                    &mut build,
                    to,
                    attempt,
                    req_seq,
                    encrypt,
                    state,
                    required,
                    options.content_type,
                )
                .await?;
            let err = match send(req_seq, message).await {
                Ok(v) => return Ok(v),
                Err(e) => e,
            };
            if attempt >= MAX_E2EE_SEND_ATTEMPTS || never {
                return Err(err);
            }
            let code = crate::client::error_code(&err);
            if !recover {
                // linejs: one encrypted retry when `e2ee` was unset.
                if attempt == 1 && !encrypt && options.e2ee.is_none() && code.contains("E2EE") {
                    force_encrypt = true;
                    refresh = true;
                    continue;
                }
                return Err(err);
            }
            let Some(recovery) = send_recovery(&code) else {
                return Err(err);
            };
            self.host.e2ee_log(
                "e2eeSendRecovery",
                serde_json::json!({"code": code, "attempt": attempt}),
            );
            match recovery {
                E2eeSendRecovery::RetryPlain => {
                    self.mark_chat_plain(to).await?;
                    if required {
                        return Err(err);
                    }
                    plain = true;
                }
                E2eeSendRecovery::SendPlain => {
                    if required {
                        return Err(err);
                    }
                    plain = true;
                }
                E2eeSendRecovery::Reencrypt => {
                    plain = false;
                    refresh = true;
                }
                E2eeSendRecovery::RecreateGroupKey if matches!(to_type, Some(1 | 2)) => {
                    plain = false;
                    match self.recreate_group_state(to).await {
                        Ok(state) if !state.usable => {
                            if required {
                                return Err(err);
                            }
                            plain = true;
                        }
                        Ok(_) => {}
                        Err(e) if !required && is_local_key_error(&e) => plain = true,
                        Err(e) => return Err(e),
                    }
                }
                E2eeSendRecovery::RecreateGroupKey => {
                    plain = false;
                    refresh = true;
                }
                E2eeSendRecovery::RefreshSenderKey => {
                    // Reloading cannot help twice in one send.
                    if sender_key_reloaded {
                        return Err(err);
                    }
                    sender_key_reloaded = true;
                    match self.refresh_self_key().await {
                        Ok(()) => {}
                        Err(e) if !required && is_local_key_error(&e) => plain = true,
                        Err(e) => return Err(e),
                    }
                }
            }
        }
    }
}

/// Numeric `ContentType` of a variant name or number (default `NONE`).
fn content_type_of(o: &SendMessageOptions) -> i32 {
    o.content_type
        .as_ref()
        .map_or(0, |v| super::content_type_number(v) as i32)
}

/// The `Message` for one attempt of a text or location send: `text` /
/// `location` go into encrypted `chunks` when the attempt encrypts.
pub(crate) async fn talk_message_for_attempt(
    e2ee: &E2ee<'_>,
    options: &SendMessageOptions,
    attempt: E2eeSendAttempt,
) -> Result<Value> {
    let mut send = options.clone();
    if attempt.encrypt && send.chunks.is_none() {
        let content_type = content_type_of(&send);
        let content = match (
            send.text.clone().filter(|t| !t.is_empty()),
            send.location.clone(),
        ) {
            (Some(text), _) => Some(E2eeContent::Text(text)),
            (None, Some(location)) => Some(E2eeContent::Location(location)),
            (None, None) => None,
        };
        if let Some(content) = content {
            let chunks = e2ee
                .encrypt_for_state(&send.to, content, content_type, &attempt.state)
                .await?;
            let spec = attempt.state.spec_version.max(1).to_string();
            send.text = None;
            send.location = None;
            send.chunks = Some(chunks);
            send.content_metadata.insert("e2eeVersion".into(), spec);
            send.content_metadata
                .insert("contentType".into(), content_type.to_string());
            send.content_metadata.insert("e2eeMark".into(), "2".into());
        }
    }
    Ok(talk_message_value(&send))
}

impl BaseClient {
    /// The stored Letter Sealing state of a chat (user, group or room mid).
    /// Unknown until a send or [`Self::refresh_e2ee_chat_state`] looks it
    /// up.
    pub async fn e2ee_chat_state(&self, mid: &str) -> E2eeChatState {
        self.e2ee().chat_state(mid).await
    }

    /// Look a chat's state up from the server (`negotiateE2EEPublicKey` or
    /// the group's last shared key) and store it.
    pub async fn refresh_e2ee_chat_state(&self, mid: &str) -> Result<E2eeChatState> {
        self.e2ee().refresh_chat_state(mid).await
    }

    /// Forget a chat's state; the next send looks it up again.
    pub async fn forget_e2ee_chat_state(&self, mid: &str) -> Result<()> {
        self.e2ee().forget_chat_state(mid).await
    }

    /// Note a received or fetched `Message`: an encrypted one in a chat
    /// marked plain makes that chat unknown again.
    pub async fn observe_e2ee_message(&self, message: &Value) {
        self.e2ee().observe_message(message).await
    }

    /// Encrypt for `to` with the key and spec version of `state` (as given
    /// to a [`Self::send_with_e2ee_recovery`] builder). Returns the
    /// `chunks`.
    pub async fn encrypt_e2ee_message_for(
        &self,
        to: &str,
        content: E2eeContent,
        content_type: i32,
        state: &E2eeChatState,
    ) -> Result<Vec<Vec<u8>>> {
        self.e2ee()
            .encrypt_for_state(to, content, content_type, state)
            .await
    }

    /// Send a talk message the way LINE for Android does: encrypted first
    /// when the chat's [`E2eeChatState`] allows the content type (looked up
    /// once when unknown), and recovered from `E2EE_*` errors (see
    /// [`crate::e2ee::send_recovery`]) for at most
    /// [`MAX_E2EE_SEND_ATTEMPTS`] attempts, all with the same `reqSeq`.
    /// Without [`crate::base::Config::e2ee_send_recovery`] the only retry
    /// is one encrypted resend of a plain message the server refused with
    /// an `E2EE*` code.
    ///
    /// `build` makes the `Message` struct for each attempt. When
    /// [`E2eeSendAttempt::encrypt`] is set it should encrypt with
    /// [`Self::encrypt_e2ee_message_for`] and the attempt's state, and set
    /// `contentMetadata.e2eeVersion`; otherwise build it plain. A builder
    /// that fails with a local key error (`NoE2EEKey`, a self-check failure)
    /// while encrypting is called again plain unless E2EE is required.
    /// Returns the `sendMessage` result.
    pub async fn send_with_e2ee_recovery<B, BF>(
        &self,
        to: &str,
        options: E2eeSendOptions,
        build: B,
    ) -> Result<Value>
    where
        B: FnMut(E2eeSendAttempt) -> BF,
        BF: Future<Output = Result<Value>>,
    {
        self.run_with_e2ee_recovery(to, options, build, |seq, message| {
            let mut args = crate::tv!({ "seq": seq });
            args.insert("message", message);
            async move { self.talk().call("sendMessage", args).await }
        })
        .await
    }

    /// [`Self::send_with_e2ee_recovery`] with another call than
    /// `sendMessage` (e.g. `editMessage`): `send` gets the `reqSeq` and the
    /// built message.
    pub(crate) async fn run_with_e2ee_recovery<B, BF, S, SF>(
        &self,
        to: &str,
        options: E2eeSendOptions,
        build: B,
        send: S,
    ) -> Result<Value>
    where
        B: FnMut(E2eeSendAttempt) -> BF,
        BF: Future<Output = Result<Value>>,
        S: FnMut(i64, Value) -> SF,
        SF: Future<Output = Result<Value>>,
    {
        let req_seq = match options.req_seq {
            Some(seq) => seq,
            None => self.get_reqseq("talk").await?,
        };
        self.e2ee()
            .send_with_recovery(
                to,
                &options,
                req_seq,
                self.config.e2ee_send_recovery,
                build,
                send,
            )
            .await
    }

    /// The `Message` for one attempt of a [`SendMessageOptions`] send.
    pub(crate) async fn talk_message_for_attempt(
        &self,
        options: &SendMessageOptions,
        attempt: E2eeSendAttempt,
    ) -> Result<Value> {
        talk_message_for_attempt(&self.e2ee(), options, attempt).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::e2ee::E2eeState;
    use crate::e2ee::crypto;
    use crate::e2ee::keys::E2eeHost;
    use crate::storage::{MemoryStorage, Storage};
    use crate::tv;

    const ME: &str = "u-self";
    const PEER: &str = "u-them";
    const GROUP: &str = "c-group";
    const SEQ: i64 = 42;

    fn server_error(code: &str) -> Error {
        Error::Request {
            method: "x".into(),
            path: "/S4".into(),
            data: tv!({ "code": code }),
        }
    }

    fn key(id: i64, data: &[u8]) -> Value {
        let mut k = tv!({ "keyId": id });
        k.insert("keyData", Value::Binary(data.to_vec()));
        k
    }

    /// A talk service with our key 1, a peer with key 2 and a group whose
    /// shared key we created.
    struct Fake {
        storage: MemoryStorage,
        self_key: (Vec<u8>, Vec<u8>),
        peer_pub: Vec<u8>,
        calls: Mutex<Vec<String>>,
        /// `negotiateE2EEPublicKey` answer for the peer.
        negotiate: Mutex<Value>,
        /// Our key ids LINE lists (`getE2EEPublicKeys`).
        own_key_ids: Mutex<Vec<i64>>,
        last_group_key: Mutex<Option<Value>>,
        next_group_key_id: Mutex<i64>,
    }

    impl Fake {
        async fn new() -> Self {
            let self_key = crypto::generate_key_pair();
            let (_, peer_pub) = crypto::generate_key_pair();
            let fake = Fake {
                storage: MemoryStorage::new(),
                self_key,
                peer_pub,
                calls: Mutex::default(),
                negotiate: Mutex::new(Value::Null),
                own_key_ids: Mutex::new(vec![1]),
                last_group_key: Mutex::new(None),
                next_group_key_id: Mutex::new(5),
            };
            fake.allow(&[0, 15]);
            let stored = serde_json::json!({
                "keyId": 1,
                "privKey": B64.encode(&fake.self_key.0),
                "pubKey": B64.encode(&fake.self_key.1),
            })
            .to_string();
            for slot in ["e2eeKeys:1", "e2eeKeys:u-self"] {
                fake.storage.set(slot, stored.clone().into()).await.unwrap();
            }
            // We are the group key's creator.
            fake.storage
                .set(
                    "e2eePublicKeys:u-self:1",
                    B64.encode(&fake.self_key.1).into(),
                )
                .await
                .unwrap();
            let shared = fake.wrap_group_key(fake.take_group_key_id());
            *fake.last_group_key.lock().unwrap() = Some(shared);
            fake
        }

        fn allow(&self, types: &[i64]) {
            let mut res = tv!({ "specVersion": 2, "allowedTypes": types });
            res.insert("publicKey", key(2, &self.peer_pub));
            *self.negotiate.lock().unwrap() = res;
        }

        fn take_group_key_id(&self) -> i64 {
            let mut id = self.next_group_key_id.lock().unwrap();
            *id += 1;
            *id - 1
        }

        /// A `Pb1_U3` wrapping a fresh group key for us, created by us.
        fn wrap_group_key(&self, group_key_id: i64) -> Value {
            let secret = crypto::generate_shared_secret(&self.self_key.0, &self.self_key.1);
            let (aes, iv) = crypto::derive_key_iv(&secret, &[]);
            let wrapped = crypto::encrypt_aes_cbc(&aes, &iv, &crypto::random_bytes(32)).unwrap();
            let mut shared = tv!({
                "groupKeyId": group_key_id,
                "creator": ME,
                "creatorKeyId": 1,
                "receiver": ME,
                "receiverKeyId": 1,
                "allowedTypes": [0, 15],
                "specVersion": 2,
            });
            shared.insert("encryptedSharedKey", Value::Binary(wrapped));
            shared
        }

        fn record(&self, name: &str) {
            self.calls.lock().unwrap().push(name.into());
        }

        fn count(&self, name: &str) -> usize {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| *c == name)
                .count()
        }
    }

    #[async_trait]
    impl E2eeHost for Fake {
        fn storage(&self) -> &dyn Storage {
            &self.storage
        }
        fn self_mid(&self) -> Option<String> {
            Some(ME.into())
        }
        async fn get_e2ee_public_keys(&self) -> Result<Value> {
            self.record("getE2EEPublicKeys");
            let ids = self.own_key_ids.lock().unwrap().clone();
            Ok(Value::List(
                ids.into_iter()
                    .map(|id| key(id, &self.self_key.1))
                    .collect(),
            ))
        }
        async fn negotiate_e2ee_public_key(&self, _mid: &str) -> Result<Value> {
            self.record("negotiateE2EEPublicKey");
            Ok(self.negotiate.lock().unwrap().clone())
        }
        async fn get_e2ee_group_shared_key(&self, _: i32, _: &str, _: i64) -> Result<Value> {
            self.record("getE2EEGroupSharedKey");
            Err(server_error("NOT_FOUND"))
        }
        async fn get_last_e2ee_group_shared_key(&self, _: i32, _: &str) -> Result<Value> {
            self.record("getLastE2EEGroupSharedKey");
            self.last_group_key
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| server_error("NOT_FOUND"))
        }
        async fn get_last_e2ee_public_keys(&self, _: &str) -> Result<Value> {
            self.record("getLastE2EEPublicKeys");
            Ok(Value::Map(vec![(ME.into(), key(1, &self.self_key.1))]))
        }
        async fn register_e2ee_group_key(&self, _args: Value) -> Result<Value> {
            self.record("registerE2EEGroupKey");
            let shared = self.wrap_group_key(self.take_group_key_id());
            *self.last_group_key.lock().unwrap() = Some(shared.clone());
            Ok(shared)
        }
        async fn register_e2ee_public_key(&self, _: Value) -> Result<Value> {
            self.record("registerE2EEPublicKey");
            Err(server_error("NOT_ALLOWED"))
        }
    }

    struct Run {
        result: Result<Value>,
        /// `(reqSeq, Message)` of every attempt.
        sent: Vec<(i64, Value)>,
    }

    impl Run {
        fn encrypted(&self, i: usize) -> bool {
            let m = &self.sent[i].1;
            m["chunks"].as_list().is_some_and(|c| c.len() == 5) && m["text"].is_null()
        }
        fn receiver_key_id(&self, i: usize) -> i64 {
            crypto::byte2int(
                self.sent[i].1["chunks"].as_list().unwrap()[4]
                    .as_bytes()
                    .unwrap(),
            )
        }
    }

    /// Send `text` to `to`; `script` answers each attempt (`None` = ok).
    async fn send(
        host: &Fake,
        to: &str,
        e2ee: Option<bool>,
        recover: bool,
        script: &[Option<&str>],
    ) -> Run {
        let state = E2eeState::default();
        let e2ee_client = E2ee::new(host, &state);
        let mut message = SendMessageOptions::text(to, "hello");
        message.e2ee = e2ee;
        let options = E2eeSendOptions {
            content_type: 0,
            e2ee,
            req_seq: Some(SEQ),
        };
        let sent = Mutex::new(Vec::new());
        let script = Mutex::new(script.iter().copied().collect::<VecDeque<_>>());
        let (client, message) = (&e2ee_client, &message);
        let result = e2ee_client
            .send_with_recovery(
                to,
                &options,
                SEQ,
                recover,
                |attempt| talk_message_for_attempt(client, message, attempt),
                |seq, m| {
                    sent.lock().unwrap().push((seq, m));
                    let answer = script.lock().unwrap().pop_front().flatten();
                    async move {
                        match answer {
                            None => Ok(tv!({ "id": "100" })),
                            Some(code) => Err(server_error(code)),
                        }
                    }
                },
            )
            .await;
        Run {
            result,
            sent: sent.into_inner().unwrap(),
        }
    }

    async fn state_of(host: &Fake, mid: &str) -> E2eeChatState {
        E2ee::new(host, &E2eeState::default()).chat_state(mid).await
    }

    #[tokio::test]
    async fn encrypts_first_when_the_chat_uses_letter_sealing() {
        let host = Fake::new().await;
        let run = send(&host, PEER, None, true, &[None]).await;
        run.result.as_ref().unwrap();
        assert_eq!(run.sent.len(), 1);
        assert!(run.encrypted(0), "{:?}", run.sent[0].1);
        assert_eq!(run.receiver_key_id(0), 2);
        let meta = &run.sent[0].1["contentMetadata"];
        assert_eq!(meta["e2eeVersion"].as_str(), Some("2"));
        assert_eq!(meta["contentType"].as_str(), Some("0"));

        let state = state_of(&host, PEER).await;
        assert!(state.usable && state.known);
        assert_eq!(state.key_id, Some(2));
        assert_eq!(state.allowed_types, BTreeSet::from([0, 15]));

        // The state is reused: no second negotiation.
        send(&host, PEER, None, true, &[None]).await.result.unwrap();
        assert_eq!(host.count("negotiateE2EEPublicKey"), 1);
    }

    #[tokio::test]
    async fn sends_plain_when_allowed_types_exclude_the_type() {
        let host = Fake::new().await;
        host.allow(&[15]);
        let run = send(&host, PEER, None, true, &[None]).await;
        run.result.as_ref().unwrap();
        assert!(!run.encrypted(0));
        assert_eq!(run.sent[0].1["text"].as_str(), Some("hello"));
        assert!(
            run.sent[0].1["contentMetadata"]
                .get("e2eeVersion")
                .is_none()
        );
    }

    #[tokio::test]
    async fn sends_plain_without_lookup_when_e2ee_is_off() {
        let host = Fake::new().await;
        let run = send(&host, PEER, Some(false), true, &[None]).await;
        run.result.as_ref().unwrap();
        assert!(!run.encrypted(0));
        assert_eq!(host.count("negotiateE2EEPublicKey"), 0);
    }

    #[tokio::test]
    async fn retry_plain_marks_the_chat_and_resends_plain_with_the_same_req_seq() {
        let host = Fake::new().await;
        let run = send(&host, PEER, None, true, &[Some("E2EE_RETRY_PLAIN"), None]).await;
        run.result.as_ref().unwrap();
        assert_eq!(run.sent.len(), 2);
        assert!(run.encrypted(0));
        assert!(!run.encrypted(1));
        assert!(run.sent.iter().all(|(seq, _)| *seq == SEQ));
        assert_eq!(state_of(&host, PEER).await, E2eeChatState::plain());

        // The next send goes plain straight away.
        let run = send(&host, PEER, None, true, &[None]).await;
        assert!(!run.encrypted(0));
    }

    #[tokio::test]
    async fn retry_plain_fails_when_e2ee_is_required() {
        let host = Fake::new().await;
        let run = send(&host, PEER, Some(true), true, &[Some("E2EE_RETRY_PLAIN")]).await;
        assert_eq!(run.result.unwrap_err().code(), Some("E2EE_RETRY_PLAIN"));
        assert_eq!(run.sent.len(), 1);
    }

    #[tokio::test]
    async fn sender_disabled_sends_plain_without_marking_the_chat() {
        let host = Fake::new().await;
        let run = send(
            &host,
            PEER,
            None,
            true,
            &[Some("E2EE_SENDER_DISABLED"), None],
        )
        .await;
        run.result.as_ref().unwrap();
        assert!(run.encrypted(0) && !run.encrypted(1));
        assert!(state_of(&host, PEER).await.usable);
    }

    #[tokio::test]
    async fn recreate_group_key_registers_a_key_and_encrypts_with_it() {
        let host = Fake::new().await;
        let run = send(
            &host,
            GROUP,
            None,
            true,
            &[Some("E2EE_RECREATE_GROUP_KEY"), None],
        )
        .await;
        run.result.as_ref().unwrap();
        assert_eq!(run.sent.len(), 2);
        assert!(run.encrypted(0) && run.encrypted(1));
        assert_eq!(run.receiver_key_id(0), 5);
        assert_eq!(run.receiver_key_id(1), 6);
        assert_eq!(host.count("registerE2EEGroupKey"), 1);
        assert_eq!(state_of(&host, GROUP).await.key_id, Some(6));
        assert!(run.sent.iter().all(|(seq, _)| *seq == SEQ));
    }

    #[tokio::test]
    async fn group_without_a_key_gets_one() {
        let host = Fake::new().await;
        *host.last_group_key.lock().unwrap() = None;
        let run = send(&host, GROUP, None, true, &[None]).await;
        run.result.as_ref().unwrap();
        assert!(run.encrypted(0));
        assert_eq!(host.count("registerE2EEGroupKey"), 1);
    }

    #[tokio::test]
    async fn retry_encrypt_looks_the_key_up_again() {
        let host = Fake::new().await;
        let run = send(
            &host,
            PEER,
            None,
            true,
            &[Some("E2EE_UPDATE_RECEIVER_KEY"), None],
        )
        .await;
        run.result.as_ref().unwrap();
        assert!(run.encrypted(1));
        assert_eq!(host.count("negotiateE2EEPublicKey"), 2);
    }

    #[tokio::test]
    async fn stops_after_max_attempts() {
        let host = Fake::new().await;
        let run = send(&host, PEER, None, true, &[Some("E2EE_RETRY_ENCRYPT"); 5]).await;
        assert_eq!(run.result.unwrap_err().code(), Some("E2EE_RETRY_ENCRYPT"));
        assert_eq!(run.sent.len(), MAX_E2EE_SEND_ATTEMPTS as usize);
        assert!(run.sent.iter().all(|(seq, _)| *seq == SEQ));
    }

    #[tokio::test]
    async fn update_sender_key_reloads_our_key_once() {
        let host = Fake::new().await;
        let run = send(
            &host,
            PEER,
            None,
            true,
            &[Some("E2EE_UPDATE_SENDER_KEY"), None],
        )
        .await;
        run.result.as_ref().unwrap();
        assert!(run.encrypted(1));
        assert_eq!(host.count("registerE2EEPublicKey"), 0);

        let run = send(
            &host,
            PEER,
            None,
            true,
            &[
                Some("E2EE_UPDATE_SENDER_KEY"),
                Some("E2EE_UPDATE_SENDER_KEY"),
            ],
        )
        .await;
        assert_eq!(
            run.result.unwrap_err().code(),
            Some("E2EE_UPDATE_SENDER_KEY")
        );
        assert_eq!(run.sent.len(), 2);
    }

    #[tokio::test]
    async fn update_sender_key_without_the_newest_key_sends_plain() {
        let host = Fake::new().await;
        // LINE has a newer key (3) than the one we hold.
        *host.own_key_ids.lock().unwrap() = vec![1, 3];
        let run = send(
            &host,
            PEER,
            None,
            true,
            &[Some("E2EE_UPDATE_SENDER_KEY"), None],
        )
        .await;
        run.result.as_ref().unwrap();
        assert!(!run.encrypted(1));
    }

    #[tokio::test]
    async fn without_own_key_sends_plain_unless_required() {
        let host = Fake::new().await;
        for slot in ["e2eeKeys:1", "e2eeKeys:u-self"] {
            host.storage.delete(slot).await.unwrap();
        }
        *host.own_key_ids.lock().unwrap() = vec![];
        let run = send(&host, PEER, None, true, &[None]).await;
        run.result.as_ref().unwrap();
        assert!(!run.encrypted(0));

        let run = send(&host, PEER, Some(true), true, &[None]).await;
        assert!(run.result.is_err());
        assert!(run.sent.is_empty());
    }

    #[tokio::test]
    async fn without_recovery_retries_once_encrypted() {
        let host = Fake::new().await;
        host.allow(&[15]);
        let run = send(
            &host,
            PEER,
            None,
            false,
            &[Some("E2EE_RETRY_ENCRYPT"), None],
        )
        .await;
        run.result.as_ref().unwrap();
        assert!(!run.encrypted(0) && run.encrypted(1));

        // An encrypted send is not recovered.
        let host = Fake::new().await;
        let run = send(&host, PEER, None, false, &[Some("E2EE_RETRY_PLAIN"), None]).await;
        assert!(run.result.is_err());
        assert_eq!(run.sent.len(), 1);
    }

    #[tokio::test]
    async fn encrypted_message_in_a_plain_chat_makes_it_unknown() {
        let host = Fake::new().await;
        let state = E2eeState::default();
        let e2ee = E2ee::new(&host, &state);
        e2ee.mark_chat_plain(PEER).await.unwrap();
        let mut received = tv!({ "from": PEER, "to": ME });
        e2ee.observe_message(&received).await;
        assert_eq!(e2ee.chat_state(PEER).await, E2eeChatState::plain());
        received.insert("chunks", Value::List(vec![Value::Binary(vec![0; 16])]));
        e2ee.observe_message(&received).await;
        assert!(!e2ee.chat_state(PEER).await.known);
    }

    #[tokio::test]
    async fn location_is_encrypted_as_location() {
        let host = Fake::new().await;
        let state = E2eeState::default();
        let e2ee = E2ee::new(&host, &state);
        let chat = e2ee.refresh_chat_state(PEER).await.unwrap();
        let options = SendMessageOptions {
            to: PEER.into(),
            content_type: Some("LOCATION".into()),
            location: Some(tv!({ "title": "here", "latitude": 1.5, "longitude": 2.0 })),
            ..Default::default()
        };
        let attempt = E2eeSendAttempt {
            attempt: 1,
            req_seq: SEQ,
            encrypt: true,
            state: chat,
        };
        let m = talk_message_for_attempt(&e2ee, &options, attempt)
            .await
            .unwrap();
        assert!(m["location"].is_null());
        assert_eq!(m["chunks"].as_list().map(<[Value]>::len), Some(5));
        assert_eq!(m["contentMetadata"]["contentType"].as_str(), Some("15"));
    }

    #[test]
    fn state_allows_only_listed_types() {
        let state = E2eeChatState {
            usable: true,
            key_id: Some(1),
            allowed_types: BTreeSet::from([0]),
            spec_version: 2,
            known: true,
        };
        assert!(state.allows(0));
        assert!(!state.allows(7));
        assert!(!E2eeChatState::plain().allows(0));
        assert_eq!(allowed_types_of(&Value::Null), BTreeSet::from([0, 15]));
    }
}
