//! Key management: the account's own key generations, peers' public keys
//! and group shared keys, cached in [`Storage`] under the same keys and JSON
//! formats as linejs:
//!
//! * `e2eeKeys:<keyId>` / `e2eeKeys:<mid>` — JSON [`E2eeKey`] (the mid slot
//!   aliases the current generation),
//! * `e2eePublicKeys:<mid>:<keyId>` — a peer's base64 public key,
//! * `e2eeGroupKeys:<chatMid>:<groupKeyId>` / `e2eeGroupKeys:<chatMid>` —
//!   JSON [`GroupKey`] (the unsuffixed slot holds the most recent one).

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Deserializer, Serialize};

use super::crypto::{self, decode_base64};
use super::E2eeState;
use crate::base::BaseClient;
use crate::error::{Error, Result};
use crate::request::RequestOptions;
use crate::storage::Storage;
use crate::thrift::{Protocol, Value};

/// One of the account's own key generations (`e2eeKeys:*`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct E2eeKey {
    #[serde(default, deserialize_with = "lenient_key_id", skip_serializing_if = "Option::is_none")]
    pub key_id: Option<i64>,
    /// Base64 curve25519 private key.
    pub priv_key: String,
    /// Base64 curve25519 public key.
    pub pub_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e2ee_version: Option<serde_json::Value>,
}

impl E2eeKey {
    pub fn private_key(&self) -> Vec<u8> {
        decode_base64(&self.priv_key)
    }
    pub fn public_key(&self) -> Vec<u8> {
        decode_base64(&self.pub_key)
    }
    fn is_complete(&self) -> bool {
        !self.priv_key.is_empty() && !self.pub_key.is_empty()
    }
}

/// A group's shared key for one generation (`e2eeGroupKeys:*`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupKey {
    /// Base64 shared private key.
    pub priv_key: String,
    #[serde(deserialize_with = "required_key_id")]
    pub key_id: i64,
}

impl GroupKey {
    pub fn private_key(&self) -> Vec<u8> {
        decode_base64(&self.priv_key)
    }
}

/// Key ids are numbers in linejs storage but arrive as strings from login
/// metadata; accept both.
fn lenient_key_id<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<i64>, D::Error> {
    Ok(key_id_of(&serde_json::Value::deserialize(d)?.into()))
}

fn required_key_id<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<i64, D::Error> {
    lenient_key_id(d)?.ok_or_else(|| serde::de::Error::custom("keyId is not a number"))
}

/// A key id held in a thrift/JSON value (integer or numeric string).
pub(crate) fn key_id_of(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        Value::Double(d) if d.fract() == 0.0 => Some(*d as i64),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// The 32-byte curve25519 public key in a `keyData` field. A missing or short
/// key would be zero-padded by `generate_shared_secret` into a secret anyone
/// can compute, so it is an error here.
pub(crate) fn key_data_of<'v>(key: &'v Value, owner: &str) -> Result<&'v [u8]> {
    match key["keyData"].as_bytes() {
        Some(data) if data.len() == 32 => Ok(data),
        _ => Err(Error::internal("NoE2EEKey", format!("no valid public key for {owner}"))),
    }
}

/// A field that schema decoding names but raw decoding numbers.
pub(crate) fn field<'v>(v: &'v Value, name: &str, id: u16) -> &'v Value {
    match v.get(name) {
        Some(x) => x,
        None => &v[id.to_string().as_str()],
    }
}

pub(crate) fn no_e2ee_key() -> Error {
    Error::internal(
        "NoE2EEKey",
        "E2EE Key has not been saved, try register `saveE2EESelfKeyDataByKeyId` or use E2EE Login",
    )
}

/// What key management needs from the client: storage, identity and the
/// talk service calls. [`BaseClient`] implements it; tests use a fake.
#[async_trait]
pub(crate) trait E2eeHost: Send + Sync {
    fn storage(&self) -> &dyn Storage;
    fn self_mid(&self) -> Option<String>;
    fn e2ee_log(&self, _kind: &str, _data: serde_json::Value) {}

    async fn get_e2ee_public_keys(&self) -> Result<Value>;
    async fn negotiate_e2ee_public_key(&self, mid: &str) -> Result<Value>;
    async fn get_e2ee_group_shared_key(&self, key_version: i32, chat_mid: &str, group_key_id: i64) -> Result<Value>;
    async fn get_last_e2ee_group_shared_key(&self, key_version: i32, chat_mid: &str) -> Result<Value>;
    async fn get_last_e2ee_public_keys(&self, chat_mid: &str) -> Result<Value>;
    async fn register_e2ee_group_key(&self, args: Value) -> Result<Value>;
    async fn register_e2ee_public_key(&self, public_key: Value) -> Result<Value>;
}

#[async_trait]
impl E2eeHost for BaseClient {
    fn storage(&self) -> &dyn Storage {
        &*self.storage
    }
    fn self_mid(&self) -> Option<String> {
        self.mid()
    }
    fn e2ee_log(&self, kind: &str, data: serde_json::Value) {
        self.log("e2ee", serde_json::json!({"type": kind, "message": data}));
    }

    async fn get_e2ee_public_keys(&self) -> Result<Value> {
        // No `getE2EEPublicKeys_args` in the IDL: the call takes no arguments.
        self.request(vec![], "getE2EEPublicKeys", RequestOptions::new(self.talk().path, Protocol::Compact)).await
    }
    async fn negotiate_e2ee_public_key(&self, mid: &str) -> Result<Value> {
        self.talk().call("negotiateE2EEPublicKey", crate::tv!({"mid": mid})).await
    }
    async fn get_e2ee_group_shared_key(&self, key_version: i32, chat_mid: &str, group_key_id: i64) -> Result<Value> {
        let args = crate::tv!({"keyVersion": key_version, "chatMid": chat_mid, "groupKeyId": group_key_id});
        self.talk().call("getE2EEGroupSharedKey", args).await
    }
    async fn get_last_e2ee_group_shared_key(&self, key_version: i32, chat_mid: &str) -> Result<Value> {
        let args = crate::tv!({"keyVersion": key_version, "chatMid": chat_mid});
        self.talk().call("getLastE2EEGroupSharedKey", args).await
    }
    async fn get_last_e2ee_public_keys(&self, chat_mid: &str) -> Result<Value> {
        self.talk().call("getLastE2EEPublicKeys", crate::tv!({"chatMid": chat_mid})).await
    }
    async fn register_e2ee_group_key(&self, args: Value) -> Result<Value> {
        self.talk().call("registerE2EEGroupKey", args).await
    }
    async fn register_e2ee_public_key(&self, public_key: Value) -> Result<Value> {
        let mut args = Value::object();
        args.insert("publicKey", public_key);
        self.talk().call("registerE2EEPublicKey", args).await
    }
}

/// E2EE logic bound to a host and its caches.
pub(crate) struct E2ee<'a> {
    pub host: &'a dyn E2eeHost,
    pub state: &'a E2eeState,
}

impl<'a> E2ee<'a> {
    pub fn new(host: &'a dyn E2eeHost, state: &'a E2eeState) -> Self {
        Self { host, state }
    }

    fn log(&self, kind: &str, data: serde_json::Value) {
        self.host.e2ee_log(kind, data);
    }

    /// Read a JSON document that linejs stores as a stringified JSON value.
    async fn get_json<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        match self.host.storage().get(key).await.ok()?? {
            serde_json::Value::String(s) => serde_json::from_str(&s).ok(),
            v @ serde_json::Value::Object(_) => serde_json::from_value(v).ok(),
            _ => None,
        }
    }

    async fn set_json<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.host.storage().set(key, serde_json::to_string(value)?.into()).await
    }

    pub async fn get_self_key_by_key_id(&self, key_id: impl std::fmt::Display) -> Option<E2eeKey> {
        self.get_json::<E2eeKey>(&format!("e2eeKeys:{key_id}")).await.filter(E2eeKey::is_complete)
    }

    pub async fn save_self_key_by_key_id(&self, key_id: impl std::fmt::Display, key: &E2eeKey) -> Result<()> {
        self.set_json(&format!("e2eeKeys:{key_id}"), key).await
    }

    /// Store `key` as the current generation (`e2eeKeys:<own mid>`).
    pub async fn save_self_key(&self, key: &E2eeKey) -> Result<()> {
        match self.host.self_mid() {
            Some(mid) => self.save_self_key_by_key_id(mid, key).await,
            None => Ok(()),
        }
    }

    /// The current own key: the `e2eeKeys:<mid>` alias (refreshed through its
    /// per-id slot), else the first server-registered generation we hold.
    pub async fn get_self_key(&self, mid: &str) -> Result<E2eeKey> {
        if let Some(key) = self.get_self_key_by_key_id(mid).await {
            // A new login can repair a mislabelled alias without deleting storage.
            if let Some(id) = key.key_id
                && let Some(refreshed) = self.get_self_key_by_key_id(id).await
            {
                return Ok(refreshed);
            }
            return Ok(key);
        }
        let keys = self.host.get_e2ee_public_keys().await?;
        for k in keys.as_list().unwrap_or_default() {
            let Some(id) = key_id_of(field(k, "keyId", 2)) else { continue };
            if let Some(key) = self.get_self_key_by_key_id(id).await {
                self.save_self_key(&key).await?;
                return Ok(key);
            }
        }
        Err(no_e2ee_key())
    }

    /// Compare a private key with the public key the server has for `key_id`.
    /// `None` when the server is unreachable or does not list the id.
    pub async fn check_stored_key_against_server(&self, key_id: &str, private_key: &[u8]) -> Option<bool> {
        let keys = match self.host.get_e2ee_public_keys().await {
            Ok(k) => k,
            Err(e) => {
                self.log("verifyStoredKeyAgainstServerFailed", e.to_string().into());
                return None;
            }
        };
        for k in keys.as_list().unwrap_or_default() {
            let id = field(k, "keyId", 2);
            if key_id_of(id).map(|i| i.to_string()).or_else(|| id.as_str().map(str::to_string)).as_deref()
                != Some(key_id)
            {
                continue;
            }
            let Some(data) = field(k, "keyData", 4).as_bytes() else { continue };
            return Some(crypto::verify_key_pair(private_key, data));
        }
        None
    }

    /// A peer's public key for `key_id` (or its current one), negotiated and
    /// cached per `(mid, keyId)` — key ids are small per-account counters, so
    /// the mid must be part of the cache key.
    pub async fn get_user_public_key(&self, mid: &str, key_id: Option<i64>) -> Result<Vec<u8>> {
        if let Some(id) = key_id
            && let Ok(Some(serde_json::Value::String(cached))) =
                self.host.storage().get(&format!("e2eePublicKeys:{mid}:{id}")).await
            && !cached.is_empty()
        {
            return Ok(decode_base64(&cached));
        }
        let res = self.host.negotiate_e2ee_public_key(mid).await?;
        if res["specVersion"].as_i64() == Some(-1) {
            return Err(Error::internal("Not support E2EE", mid));
        }
        let public_key = &res["publicKey"];
        let receiver_key_id = key_id_of(&public_key["keyId"]);
        if key_id.is_some() && key_id != receiver_key_id {
            return Err(Error::internal(
                "No E2EEKey",
                format!(
                    "E2EE key id {} not found on {mid}, key id should be {}",
                    key_id.unwrap_or_default(),
                    receiver_key_id.map(|i| i.to_string()).unwrap_or_default()
                ),
            ));
        }
        let data = public_key["keyData"].as_bytes().unwrap_or_default().to_vec();
        let cache_id = receiver_key_id.map(|i| i.to_string()).unwrap_or_else(|| "undefined".into());
        self.host.storage().set(&format!("e2eePublicKeys:{mid}:{cache_id}"), B64.encode(&data).into()).await?;
        Ok(data)
    }

    async fn cached_group_key(&self, chat_mid: &str, key_id: i64) -> Option<GroupKey> {
        // The per-id slot is the real cache; the unsuffixed one is read for
        // storages written by earlier linejs versions.
        for slot in [format!("e2eeGroupKeys:{chat_mid}:{key_id}"), format!("e2eeGroupKeys:{chat_mid}")] {
            let Some(key) = self.get_json::<GroupKey>(&slot).await else { continue };
            if key.key_id == key_id {
                return Some(key);
            }
            self.log("getE2EELocalPublicKeykeyIdMismatch", chat_mid.into());
        }
        None
    }

    /// A group's shared key for generation `key_id` (from a message
    /// envelope), or the current one. Membership changes rotate the key, so a
    /// message must be decrypted with the generation it names; when the
    /// server no longer serves it the last key is the best guess. A group
    /// without any key gets one registered.
    pub async fn get_group_key(&self, chat_mid: &str, key_id: Option<i64>) -> Result<GroupKey> {
        if let Some(id) = key_id
            && let Some(cached) = self.cached_group_key(chat_mid, id).await
        {
            return Ok(cached);
        }
        let mut shared = None;
        if let Some(id) = key_id {
            match self.host.get_e2ee_group_shared_key(2, chat_mid, id).await {
                Ok(v) => shared = Some(v),
                Err(e) if e.code() == Some("NOT_FOUND") => {
                    self.log("getE2EELocalPublicKeyGroupKeyIdNotFound", chat_mid.into());
                }
                Err(e) => return Err(e),
            }
        }
        let shared = match shared {
            Some(s) => s,
            None => match self.host.get_last_e2ee_group_shared_key(2, chat_mid).await {
                Ok(v) => v,
                Err(e) if e.code() == Some("NOT_FOUND") => self.try_register_group_key(chat_mid).await?,
                Err(e) => return Err(e),
            },
        };
        self.unwrap_group_shared_key(chat_mid, &shared).await
    }

    /// ECDH against the key creator's public key, then AES-256-CBC over the
    /// wrapped shared key; the result is cached under both group slots.
    async fn unwrap_group_shared_key(&self, chat_mid: &str, shared: &Value) -> Result<GroupKey> {
        let group_key_id = key_id_of(&shared["groupKeyId"])
            .ok_or_else(|| Error::internal("NoE2EEKey", format!("no group key for {chat_mid}")))?;
        let creator = shared["creator"].as_str().unwrap_or_default();
        let creator_key_id = key_id_of(&shared["creatorKeyId"]);
        let receiver_key_id = key_id_of(&shared["receiverKeyId"]).unwrap_or_default();
        let encrypted = shared["encryptedSharedKey"].as_bytes().unwrap_or_default();

        let self_key = self.get_self_key_by_key_id(receiver_key_id).await.ok_or_else(no_e2ee_key)?;
        let creator_key = self.get_user_public_key(creator, creator_key_id).await?;
        let secret = crypto::generate_shared_secret(&self_key.private_key(), &creator_key);
        let (key, iv) = crypto::derive_key_iv(&secret, &[]);
        let plain = crypto::decrypt_aes_cbc(&key, &iv, encrypted, true)?;

        let group_key = GroupKey { priv_key: B64.encode(plain), key_id: group_key_id };
        self.set_json(&format!("e2eeGroupKeys:{chat_mid}:{group_key_id}"), &group_key).await?;
        self.set_json(&format!("e2eeGroupKeys:{chat_mid}"), &group_key).await?;
        Ok(group_key)
    }

    /// Create a group shared key, wrap it for every member's current public
    /// key and register it. Returns the server's `Pb1_U3` for us.
    pub async fn try_register_group_key(&self, chat_mid: &str) -> Result<Value> {
        let public_keys = self.host.get_last_e2ee_public_keys(chat_mid).await?;
        let mid = self.host.self_mid().ok_or_else(|| Error::internal("NotAuthorized", "profile is not loaded"))?;
        let self_key_id = key_id_of(&public_keys[mid.as_str()]["keyId"]).ok_or_else(no_e2ee_key)?;
        let self_key = self.get_self_key_by_key_id(self_key_id).await.ok_or_else(no_e2ee_key)?;
        let self_private = self_key.private_key();
        let group_private = crypto::random_bytes(32);

        let (mut members, mut key_ids, mut wrapped) = (Vec::new(), Vec::new(), Vec::new());
        for (member, key) in public_keys.entries() {
            let secret = crypto::generate_shared_secret(&self_private, key_data_of(key, &member)?);
            let (aes_key, iv) = crypto::derive_key_iv(&secret, &[]);
            wrapped.push(Value::Binary(crypto::encrypt_aes_cbc(&aes_key, &iv, &group_private)?));
            key_ids.push(Value::Int(key_id_of(&key["keyId"]).unwrap_or_default()));
            members.push(Value::String(member));
        }
        let mut args = crate::tv!({"keyVersion": 1, "chatMid": chat_mid});
        args.insert("members", Value::List(members));
        args.insert("keyIds", Value::List(key_ids));
        args.insert("encryptedSharedKeys", Value::List(wrapped));
        self.host.register_e2ee_group_key(args).await
    }

    /// Generate a key pair and register its public key; the new generation is
    /// stored by id and as the current key.
    pub async fn register_key_pair(&self) -> Result<E2eeKey> {
        let (private, public) = crypto::generate_key_pair();
        let mut pk = crate::tv!({"version": 1, "keyId": 0, "createdTime": 0});
        pk.insert("keyData", Value::Binary(public.clone()));
        let registered = self.host.register_e2ee_public_key(pk).await?;
        let key_id = key_id_of(&registered["keyId"]);
        let key = E2eeKey {
            key_id,
            priv_key: B64.encode(&private),
            pub_key: B64.encode(&public),
            e2ee_version: Some(1.into()),
        };
        let id = key_id.map(|i| i.to_string()).unwrap_or_else(|| "undefined".into());
        self.save_self_key_by_key_id(&id, &key).await?;
        self.save_self_key(&key).await?;
        self.log("registerE2EEKeyPairSuccess", serde_json::json!({"keyId": key_id}));
        Ok(key)
    }

    /// Decode the login key chain (`encryptedKeyChain`, `publicKey`, `keyId`,
    /// `e2eeVersion`). Every generation is stored under its own id; the
    /// requested one is remembered for [`Self::verify_login_key`].
    pub async fn decode_key_v1(&self, data: &Value, secret: &[u8]) -> Result<Option<(E2eeKey, Vec<u8>, Vec<u8>)>> {
        let parsed;
        let data = match data {
            // ForSecure logins hand the e2ee info over as a JSON string.
            Value::String(s) => match serde_json::from_str::<serde_json::Value>(s) {
                Ok(v) => {
                    parsed = Value::from(v);
                    &parsed
                }
                Err(_) => return Ok(None),
            },
            v => v,
        };
        let bytes = |v: &Value| match v {
            Value::String(s) => decode_base64(s),
            Value::Binary(b) => b.clone(),
            _ => Vec::new(),
        };
        let chain = bytes(&data["encryptedKeyChain"]);
        if chain.is_empty() {
            return Ok(None);
        }
        let key_id_value = &data["keyId"];
        let key_id = key_id_of(key_id_value)
            .map(|i| i.to_string())
            .or_else(|| key_id_value.as_str().map(str::to_string))
            .unwrap_or_default();
        let public_key = bytes(&data["publicKey"]);
        let e2ee_version = Some(data["e2eeVersion"].to_json()).filter(|v| !v.is_null());

        let keys = crypto::decrypt_key_chain_entries(&public_key, secret, &chain)?;
        let selected = keys
            .iter()
            .find(|k| k.key_id.to_string() == key_id)
            .ok_or_else(|| Error::internal("NoE2EEKey", "Requested keyId is absent from the login keychain"))?
            .clone();
        // Server verification is deferred until the login token is installed.
        *self.state.login_key.lock().unwrap() = Some((key_id.clone(), selected.private_key.clone()));
        for k in &keys {
            let stored = E2eeKey {
                key_id: Some(k.key_id),
                priv_key: B64.encode(&k.private_key),
                pub_key: B64.encode(&k.public_key),
                e2ee_version: e2ee_version.clone(),
            };
            self.save_self_key_by_key_id(k.key_id, &stored).await?;
        }
        let key = E2eeKey {
            key_id: key_id.parse().ok(),
            priv_key: B64.encode(&selected.private_key),
            pub_key: B64.encode(&selected.public_key),
            e2ee_version,
        };
        Ok(Some((key, selected.private_key, selected.public_key)))
    }

    /// Check the key chosen at login against the server. Fails only on a
    /// definite mismatch, and never rotates keys.
    pub async fn verify_login_key(&self) -> Result<()> {
        let Some((key_id, private_key)) = self.state.login_key.lock().unwrap().clone() else { return Ok(()) };
        if self.check_stored_key_against_server(&key_id, &private_key).await == Some(false) {
            return Err(Error::internal(
                "E2EEKeyMismatch",
                "Login key does not match the server-registered public key; no replacement key was registered",
            ));
        }
        *self.state.login_key.lock().unwrap() = None;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;
    use crate::storage::MemoryStorage;
    use crate::tv;

    /// A scripted talk service recording every call.
    #[derive(Default)]
    pub(crate) struct FakeHost {
        pub storage: MemoryStorage,
        pub mid: Option<String>,
        pub calls: Mutex<Vec<(String, Value)>>,
        /// `getE2EEPublicKeys` result; `None` simulates an offline server.
        pub public_keys: Option<Vec<Value>>,
        /// `negotiateE2EEPublicKey`: mid → (keyId, keyData).
        pub negotiate: HashMap<String, (i64, Vec<u8>)>,
        /// `getE2EEGroupSharedKey` answers by group key id.
        pub group_keys: HashMap<i64, Value>,
        pub last_group_key_id: Option<i64>,
        pub register_key_id: i64,
    }

    impl FakeHost {
        pub fn new() -> Self {
            Self { mid: Some("u-self".into()), public_keys: Some(vec![]), ..Default::default() }
        }
        pub fn calls(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().clone()
        }
        fn record(&self, name: &str, args: Value) {
            self.calls.lock().unwrap().push((name.into(), args));
        }
        pub async fn get(&self, key: &str) -> Option<serde_json::Value> {
            self.storage.get(key).await.unwrap()
        }
        pub async fn set(&self, key: &str, v: impl Into<serde_json::Value>) {
            self.storage.set(key, v.into()).await.unwrap()
        }
    }

    fn not_found() -> Error {
        Error::Request { method: "x".into(), path: "/S4".into(), data: tv!({"code": "NOT_FOUND"}) }
    }

    #[async_trait]
    impl E2eeHost for FakeHost {
        fn storage(&self) -> &dyn Storage {
            &self.storage
        }
        fn self_mid(&self) -> Option<String> {
            self.mid.clone()
        }
        async fn get_e2ee_public_keys(&self) -> Result<Value> {
            self.record("getE2EEPublicKeys", Value::Null);
            self.public_keys.clone().map(Value::List).ok_or_else(|| Error::internal("Offline", "offline"))
        }
        async fn negotiate_e2ee_public_key(&self, mid: &str) -> Result<Value> {
            self.record("negotiateE2EEPublicKey", tv!({"mid": mid}));
            let (key_id, data) = self.negotiate.get(mid).cloned().ok_or_else(not_found)?;
            let mut pk = tv!({"keyId": key_id});
            pk.insert("keyData", Value::Binary(data));
            let mut res = tv!({"specVersion": 2});
            res.insert("publicKey", pk);
            Ok(res)
        }
        async fn get_e2ee_group_shared_key(&self, key_version: i32, chat_mid: &str, id: i64) -> Result<Value> {
            self.record(
                "getE2EEGroupSharedKey",
                tv!({"keyVersion": key_version, "chatMid": chat_mid, "groupKeyId": id}),
            );
            self.group_keys.get(&id).cloned().ok_or_else(not_found)
        }
        async fn get_last_e2ee_group_shared_key(&self, key_version: i32, chat_mid: &str) -> Result<Value> {
            self.record("getLastE2EEGroupSharedKey", tv!({"keyVersion": key_version, "chatMid": chat_mid}));
            self.last_group_key_id.and_then(|id| self.group_keys.get(&id).cloned()).ok_or_else(not_found)
        }
        async fn get_last_e2ee_public_keys(&self, chat_mid: &str) -> Result<Value> {
            self.record("getLastE2EEPublicKeys", tv!({"chatMid": chat_mid}));
            Err(not_found())
        }
        async fn register_e2ee_group_key(&self, args: Value) -> Result<Value> {
            self.record("registerE2EEGroupKey", args);
            Err(not_found())
        }
        async fn register_e2ee_public_key(&self, public_key: Value) -> Result<Value> {
            self.record("registerE2EEPublicKey", public_key);
            Ok(tv!({"keyId": self.register_key_id}))
        }
    }

    fn b64(b: &[u8]) -> String {
        B64.encode(b)
    }

    // --- publickey_cache.test.ts -------------------------------------------

    #[test]
    fn key_data_must_be_32_bytes() {
        let mut key = Value::object();
        assert!(key_data_of(&key, "u-peer").is_err());
        key.insert("keyData", Value::Binary(vec![9; 31]));
        assert!(key_data_of(&key, "u-peer").is_err());
        key.insert("keyData", Value::Binary(vec![9; 32]));
        assert_eq!(key_data_of(&key, "u-peer").unwrap(), &[9u8; 32][..]);
    }

    #[tokio::test]
    async fn public_key_cache_is_per_mid() {
        let mut host = FakeHost::new();
        host.negotiate.insert("u-a".into(), (1, b"pub-of-u-a".to_vec()));
        host.negotiate.insert("u-b".into(), (1, b"pub-of-u-b".to_vec()));
        let state = E2eeState::default();
        let e2ee = E2ee::new(&host, &state);

        assert_eq!(e2ee.get_user_public_key("u-a", Some(1)).await.unwrap(), b"pub-of-u-a");
        // Same keyId on another contact must not be served A's cached key.
        assert_eq!(e2ee.get_user_public_key("u-b", Some(1)).await.unwrap(), b"pub-of-u-b");
        assert_eq!(e2ee.get_user_public_key("u-a", Some(1)).await.unwrap(), b"pub-of-u-a");
        let negotiated: Vec<_> =
            host.calls().into_iter().map(|(_, a)| a["mid"].as_str().unwrap().to_string()).collect();
        assert_eq!(negotiated, ["u-a", "u-b"]);
        assert_eq!(host.get("e2eePublicKeys:u-a:1").await, Some(b64(b"pub-of-u-a").into()));
    }

    #[tokio::test]
    async fn public_key_without_key_id_accepts_negotiated_key() {
        let mut host = FakeHost::new();
        host.negotiate.insert("u-a".into(), (1, b"pub-of-u-a".to_vec()));
        let state = E2eeState::default();
        let e2ee = E2ee::new(&host, &state);
        assert_eq!(e2ee.get_user_public_key("u-a", None).await.unwrap(), b"pub-of-u-a");
        assert!(e2ee.get_user_public_key("u-a", Some(5)).await.is_err());
    }

    // --- register_keypair.test.ts ------------------------------------------

    #[tokio::test]
    async fn register_key_pair_sends_raw_public_key() {
        let host = FakeHost { register_key_id: 7, ..FakeHost::new() };
        let state = E2eeState::default();
        let key = E2ee::new(&host, &state).register_key_pair().await.unwrap();
        let calls = host.calls();
        let sent = calls[0].1["keyData"].as_bytes().unwrap().to_vec();
        assert_eq!(sent.len(), 32);
        assert_eq!(key.key_id, Some(7));
        assert_eq!(key.public_key(), sent);
        assert!(host.get("e2eeKeys:7").await.is_some());
        assert!(host.get("e2eeKeys:u-self").await.is_some());
    }

    // --- group_key_selection.test.ts ---------------------------------------

    const GROUP: &str = "c-group";
    const CREATOR: &str = "u-creator";

    fn shared_key_material(id: i64) -> Vec<u8> {
        vec![id as u8; 32]
    }

    fn expected(id: i64) -> GroupKey {
        GroupKey { priv_key: b64(&shared_key_material(id)), key_id: id }
    }

    async fn group_fixture(served: &[i64], last: i64) -> FakeHost {
        let mut host = FakeHost::new();
        let (self_priv, self_pub) = crypto::generate_key_pair();
        let (_, creator_pub) = crypto::generate_key_pair();
        let (aes_key, iv) = crypto::derive_key_iv(&crypto::generate_shared_secret(&self_priv, &creator_pub), &[]);
        for &id in served {
            let mut v = tv!({
                "keyVersion": 2, "groupKeyId": id, "creator": CREATOR, "creatorKeyId": 3,
                "receiver": "u-self", "receiverKeyId": 5, "specVersion": 2
            });
            v.insert("encryptedSharedKey", crypto::encrypt_aes_cbc(&aes_key, &iv, &shared_key_material(id)).unwrap());
            host.group_keys.insert(id, v);
        }
        host.last_group_key_id = Some(last);
        let self_key = serde_json::json!({"privKey": b64(&self_priv), "pubKey": b64(&self_pub)});
        host.set("e2eeKeys:5", self_key.to_string()).await;
        // Pre-seed the creator's public key so the unwrap needs no extra RPC.
        host.set(&format!("e2eePublicKeys:{CREATOR}:3"), b64(&creator_pub)).await;
        host
    }

    fn by_id(id: i64) -> (String, Value) {
        ("getE2EEGroupSharedKey".into(), tv!({"keyVersion": 2, "chatMid": GROUP, "groupKeyId": id}))
    }

    fn last() -> (String, Value) {
        ("getLastE2EEGroupSharedKey".into(), tv!({"keyVersion": 2, "chatMid": GROUP}))
    }

    async fn cached(host: &FakeHost, key: &str) -> Option<GroupKey> {
        match host.get(key).await? {
            serde_json::Value::String(s) => serde_json::from_str(&s).ok(),
            _ => None,
        }
    }

    #[tokio::test]
    async fn group_key_cached_generation_needs_no_rpc() {
        for slot in [format!("e2eeGroupKeys:{GROUP}:7"), format!("e2eeGroupKeys:{GROUP}")] {
            let host = group_fixture(&[7], 7).await;
            host.set(&slot, serde_json::to_string(&expected(7)).unwrap()).await;
            let state = E2eeState::default();
            let key = E2ee::new(&host, &state).get_group_key(GROUP, Some(7)).await.unwrap();
            assert_eq!(key, expected(7));
            assert!(host.calls().is_empty());
        }
    }

    #[tokio::test]
    async fn group_key_requested_generation_is_fetched_by_id() {
        let host = group_fixture(&[7, 9], 9).await;
        host.set(&format!("e2eeGroupKeys:{GROUP}"), serde_json::to_string(&expected(9)).unwrap()).await;
        let state = E2eeState::default();
        let key = E2ee::new(&host, &state).get_group_key(GROUP, Some(7)).await.unwrap();
        assert_eq!(host.calls(), [by_id(7)]);
        assert_eq!(key, expected(7));
        assert_eq!(cached(&host, &format!("e2eeGroupKeys:{GROUP}:7")).await, Some(key.clone()));
        assert_eq!(cached(&host, &format!("e2eeGroupKeys:{GROUP}")).await, Some(key));
    }

    #[tokio::test]
    async fn group_key_omitted_id_asks_for_last_key() {
        let host = group_fixture(&[9], 9).await;
        let state = E2eeState::default();
        let key = E2ee::new(&host, &state).get_group_key(GROUP, None).await.unwrap();
        assert_eq!(host.calls(), [last()]);
        assert_eq!(key, expected(9));
        assert_eq!(cached(&host, &format!("e2eeGroupKeys:{GROUP}:9")).await, Some(key));
    }

    #[tokio::test]
    async fn group_key_generations_are_cached_side_by_side() {
        let host = group_fixture(&[7, 9], 9).await;
        let state = E2eeState::default();
        let e2ee = E2ee::new(&host, &state);
        let older = e2ee.get_group_key(GROUP, Some(7)).await.unwrap();
        let newer = e2ee.get_group_key(GROUP, Some(9)).await.unwrap();
        let again = e2ee.get_group_key(GROUP, Some(7)).await.unwrap();
        assert_eq!(older, expected(7));
        assert_eq!(newer, expected(9));
        assert_eq!(again, older);
        assert_eq!(host.calls().len(), 2);
    }

    #[tokio::test]
    async fn group_key_generation_zero_is_fetched_by_id() {
        let host = group_fixture(&[0, 9], 9).await;
        let state = E2eeState::default();
        let key = E2ee::new(&host, &state).get_group_key(GROUP, Some(0)).await.unwrap();
        assert_eq!(host.calls(), [by_id(0)]);
        assert_eq!(key, expected(0));
    }

    #[tokio::test]
    async fn group_key_unserved_generation_falls_back_to_last() {
        for id in [0, 7] {
            let host = group_fixture(&[9], 9).await;
            let state = E2eeState::default();
            let key = E2ee::new(&host, &state).get_group_key(GROUP, Some(id)).await.unwrap();
            assert_eq!(host.calls(), [by_id(id), last()]);
            assert_eq!(key, expected(9));
            assert_eq!(cached(&host, &format!("e2eeGroupKeys:{GROUP}:9")).await, Some(key));
        }
    }

    // --- login_keychain.test.ts --------------------------------------------

    async fn login_fixture(ids: &[i64], corrupt: bool) -> (FakeHost, Vec<u8>, Value, Vec<crypto::KeyChainEntry>) {
        let (secret, public, encrypted, keys) = crypto::tests::key_chain_fixture(ids, corrupt);
        let data = tv!({
            "keyId": "80",
            "publicKey": b64(&public),
            "encryptedKeyChain": b64(&encrypted),
            "e2eeVersion": 2
        });
        (FakeHost::new(), secret, data, keys)
    }

    async fn saved_count(host: &FakeHost) -> usize {
        host.storage.get_all().await.unwrap().len()
    }

    #[tokio::test]
    async fn login_key_chain_selects_requested_entry_and_keeps_generations() {
        let (host, secret, data, keys) = login_fixture(&[10, 20, 30, 40, 50, 60, 70, 80], false).await;
        let state = E2eeState::default();
        let (_, priv_key, pub_key) = E2ee::new(&host, &state).decode_key_v1(&data, &secret).await.unwrap().unwrap();
        assert_eq!(priv_key, keys[7].private_key);
        assert_eq!(pub_key, keys[7].public_key);
        assert_eq!(saved_count(&host).await, 8);
        for k in &keys {
            let stored: E2eeKey =
                serde_json::from_str(host.get(&format!("e2eeKeys:{}", k.key_id)).await.unwrap().as_str().unwrap())
                    .unwrap();
            assert_eq!(stored.key_id, Some(k.key_id));
            assert_eq!(stored.priv_key, b64(&k.private_key));
        }
        assert!(host.calls().is_empty(), "server verification must be deferred");
    }

    #[tokio::test]
    async fn login_key_chain_failures_save_nothing() {
        let state = E2eeState::default();
        let (host, secret, mut data, _) = login_fixture(&[10, 80], false).await;
        data.insert("keyId", 999);
        let err = E2ee::new(&host, &state).decode_key_v1(&data, &secret).await.unwrap_err();
        assert!(err.to_string().contains("Requested keyId"));
        assert_eq!(saved_count(&host).await, 0);

        let (host, secret, data, _) = login_fixture(&[10, 80], true).await;
        let err = E2ee::new(&host, &state).decode_key_v1(&data, &secret).await.unwrap_err();
        assert!(err.to_string().contains("invalid key pair"));
        assert_eq!(saved_count(&host).await, 0);

        let (host, secret, data, _) = login_fixture(&[80, 80], false).await;
        let err = E2ee::new(&host, &state).decode_key_v1(&data, &secret).await.unwrap_err();
        assert!(err.to_string().contains("duplicate keyId"));
        assert_eq!(saved_count(&host).await, 0);
    }

    #[tokio::test]
    async fn login_key_mismatch_is_reported_without_rotation() {
        let (mut host, secret, data, _) = login_fixture(&[10, 80], false).await;
        host.public_keys = Some(vec![tv!({"keyId": 80, "keyData": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="})]);
        let state = E2eeState::default();
        let e2ee = E2ee::new(&host, &state);
        e2ee.decode_key_v1(&data, &secret).await.unwrap();
        assert!(host.calls().is_empty());
        let err = e2ee.verify_login_key().await.unwrap_err();
        assert!(err.to_string().contains("no replacement key was registered"));
        assert_eq!(host.calls().len(), 1);
    }

    #[tokio::test]
    async fn login_key_match_is_verified() {
        let (mut host, secret, data, keys) = login_fixture(&[10, 80], false).await;
        // Raw-decoded shape: numeric field ids.
        let mut raw = tv!({"2": 80});
        raw.insert("4", Value::Binary(keys[1].public_key.clone()));
        host.public_keys = Some(vec![raw]);
        let state = E2eeState::default();
        let e2ee = E2ee::new(&host, &state);
        e2ee.decode_key_v1(&data, &secret).await.unwrap();
        e2ee.verify_login_key().await.unwrap();
        assert!(state.login_key.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn unavailable_server_verification_is_unknown() {
        let (_, _, _, keys) = login_fixture(&[10, 80], false).await;
        let host = FakeHost { public_keys: None, ..FakeHost::new() };
        let state = E2eeState::default();
        assert_eq!(E2ee::new(&host, &state).check_stored_key_against_server("80", &keys[1].private_key).await, None);
    }

    #[tokio::test]
    async fn relogin_repairs_stale_mid_alias() {
        let (host, secret, data, keys) = login_fixture(&[10, 20, 30, 40, 50, 60, 70, 80], false).await;
        let stale = serde_json::json!({
            "keyId": 80, "privKey": b64(&keys[0].private_key), "pubKey": b64(&keys[0].public_key)
        });
        host.set("e2eeKeys:u-self", stale.to_string()).await;
        let state = E2eeState::default();
        let e2ee = E2ee::new(&host, &state);
        e2ee.decode_key_v1(&data, &secret).await.unwrap();
        assert_eq!(e2ee.get_self_key("u-self").await.unwrap().priv_key, b64(&keys[7].private_key));
    }

    #[tokio::test]
    async fn qr_login_map_shape_is_accepted() {
        // SQR responses carry e2ee info as map<string, string>.
        let (host, secret, data, keys) = login_fixture(&[80], false).await;
        let as_text = |v: &Value| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_json().to_string());
        let map = Value::Map(data.entries().into_iter().map(|(k, v)| (k.into(), as_text(v).into())).collect());
        let state = E2eeState::default();
        let (key, _, _) = E2ee::new(&host, &state).decode_key_v1(&map, &secret).await.unwrap().unwrap();
        assert_eq!(key.key_id, Some(80));
        assert_eq!(key.priv_key, b64(&keys[0].private_key));
    }
}
