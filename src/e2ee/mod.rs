//! E2EE (Letter Sealing): key management and message encryption, ported from
//! linejs `base/e2ee/mod.ts`.
//!
//! * [`crypto`] — pure primitives (ECDH, key derivation, AES modes, login key
//!   chains, media "key material" encryption, AES-GCM-SIV).
//! * `keys` — own/peer/group keys, cached in the client [`Storage`] with
//!   linejs-compatible keys (see [`E2eeKey`], [`GroupKey`]).
//! * `message` — `chunks` encryption/decryption for text, location and data
//!   messages.
//! * [`media`] — media integrity: v1 whole-file HMAC and v2 chunked HMAC
//!   with `__ud-hash` chunk digests.
//! * `recovery` — what to do when `sendMessage` fails with an `E2EE_*` code.
//! * `exchange` — `requestE2EEKeyExchange` / `respondE2EEKeyExchange` /
//!   `removeE2EEPublicKey` and E2EE operation classification.
//!
//! [`Storage`]: crate::storage::Storage

pub mod crypto;
mod exchange;
mod keys;
pub mod media;
mod message;
mod recovery;

use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

pub use crypto::{
    byte2int, decode_base64, decrypt_aes_gcm_siv, decrypt_by_key_material, decrypt_by_key_material_b64,
    decrypt_encrypted_qr_identifier, decrypt_key_chain, decrypt_key_chain_entries, derive_key_iv,
    encrypt_aes_ecb, encrypt_by_key_material, generate_aad, generate_key_pair, generate_shared_secret,
    int_bytes, public_key_of, sha256, sha256_sum, verify_key_pair, xor, KeyChainEntry,
};
pub use exchange::{E2eeOpKind, E2eeOperation};
pub use keys::{E2eeKey, GroupKey};
pub use media::{decrypt_media, encrypt_media, ChunkHashes, ChunkedHmacValidator, EncryptedMedia, MediaHmacVersion};
pub use recovery::{send_recovery, E2eeSendRecovery, MAX_E2EE_SEND_ATTEMPTS};
pub use message::{content_type_number, decrypt_message_v1, decrypt_message_v2, encrypt_payload};

use crate::base::BaseClient;
use crate::error::Result;
use crate::thrift::Value;
use keys::E2ee;

/// Plaintext payload for [`BaseClient::encrypt_e2ee_message`].
#[derive(Debug, Clone)]
pub enum E2eeContent {
    Text(String),
    /// A `Location` struct value.
    Location(Value),
    /// Arbitrary JSON payload (non-text content types).
    Json(serde_json::Value),
}

/// In-memory E2EE state attached to a [`BaseClient`]. Keys themselves live
/// in storage.
#[derive(Default)]
pub struct E2eeState {
    /// `(keyId, private key)` chosen at login, verified against the server by
    /// [`BaseClient::verify_login_key`] once the token is installed.
    login_key: Mutex<Option<(String, Vec<u8>)>>,
}

impl BaseClient {
    fn e2ee(&self) -> E2ee<'_> {
        E2ee::new(self, &self.e2ee)
    }

    /// Create an ephemeral curve25519 secret for SQR / email login.
    /// Returns `(private_key, suffix)` where `suffix` is the URL fragment
    /// (`?secret=...&e2eeVersion=1`) or, with `pk_only`, the base64 public key.
    pub fn create_sqr_secret(&self, pk_only: bool) -> (Vec<u8>, String) {
        let (private, public) = generate_key_pair();
        let public = B64.encode(public);
        if pk_only {
            (private, public)
        } else {
            (private, format!("?secret={}&e2eeVersion=1", crypto::uri_encode(&public)))
        }
    }

    /// Decode the key chain handed over at login (`keyId`, `publicKey`,
    /// `encryptedKeyChain`, `e2eeVersion`; base64 strings or raw bytes, as a
    /// struct, a `map<string,string>` or a JSON string). Stores every key
    /// generation and returns the selected one as
    /// `{keyId, privKey, pubKey, e2eeVersion}` (keys as binary), or `None`
    /// when `data` holds no key chain.
    pub async fn decode_e2ee_key_v1(&self, data: &Value, secret: &[u8]) -> Result<Option<Value>> {
        let Some((key, private, public)) = self.e2ee().decode_key_v1(data, secret).await? else { return Ok(None) };
        let mut out = Value::object();
        out.insert("keyId", key.key_id);
        out.insert("privKey", private);
        out.insert("pubKey", public);
        out.insert("e2eeVersion", key.e2ee_version.map(Value::from));
        Ok(Some(out))
    }

    /// Register a fresh key pair. Like linejs, a failure is logged and
    /// swallowed; use [`Self::try_register_e2ee_key_pair`] to see it.
    pub async fn register_e2ee_key_pair(&self) -> Result<()> {
        if let Err(e) = self.try_register_e2ee_key_pair().await {
            self.log("e2ee", serde_json::json!({"type": "registerE2EEKeyPairFailed", "message": e.to_string()}));
        }
        Ok(())
    }

    /// Generate and register a key pair, storing it as the current key.
    pub async fn try_register_e2ee_key_pair(&self) -> Result<E2eeKey> {
        self.e2ee().register_key_pair().await
    }

    /// Verify the key selected at login against the server-registered public
    /// key. Fails with `E2EEKeyMismatch` only on a definite mismatch.
    pub async fn verify_login_key(&self) -> Result<()> {
        self.e2ee().verify_login_key().await
    }

    /// Whether the server's public key for `key_id` matches `private_key`
    /// (`false` when unknown or unreachable).
    pub async fn verify_stored_key_against_server(&self, key_id: i64, private_key: &[u8]) -> bool {
        self.e2ee().check_stored_key_against_server(&key_id.to_string(), private_key).await == Some(true)
    }

    /// Device secret for `confirmE2EELogin` (see [`crypto::encrypt_device_secret`]).
    pub fn encrypt_device_secret(
        &self,
        public_key: &[u8],
        private_key: &[u8],
        encrypted_key_chain: &[u8],
    ) -> Result<Vec<u8>> {
        Ok(crypto::encrypt_device_secret(public_key, private_key, encrypted_key_chain))
    }

    /// The current own key (`e2eeKeys:<mid>`), recovered through the server's
    /// key list when the alias is missing.
    pub async fn get_e2ee_self_key_data(&self, mid: &str) -> Result<E2eeKey> {
        self.e2ee().get_self_key(mid).await
    }

    pub async fn get_e2ee_self_key_data_by_key_id(&self, key_id: i64) -> Option<E2eeKey> {
        self.e2ee().get_self_key_by_key_id(key_id).await
    }

    pub async fn save_e2ee_self_key_data_by_key_id(&self, key_id: i64, key: &E2eeKey) -> Result<()> {
        self.e2ee().save_self_key_by_key_id(key_id, key).await
    }

    /// Store `key` as the current own key.
    pub async fn save_e2ee_self_key_data(&self, key: &E2eeKey) -> Result<()> {
        self.e2ee().save_self_key(key).await
    }

    /// A user's public key for `key_id` (or the current one), cached per mid.
    pub async fn get_e2ee_local_public_key(&self, mid: &str, key_id: Option<i64>) -> Result<Vec<u8>> {
        self.e2ee().get_user_public_key(mid, key_id).await
    }

    /// A group's shared key for generation `key_id` (or the current one).
    pub async fn get_e2ee_group_key(&self, chat_mid: &str, key_id: Option<i64>) -> Result<GroupKey> {
        self.e2ee().get_group_key(chat_mid, key_id).await
    }

    /// Create and register a shared key for a group that has none.
    pub async fn try_register_e2ee_group_key(&self, chat_mid: &str) -> Result<Value> {
        self.e2ee().try_register_group_key(chat_mid).await
    }

    /// Encrypt a message for `to` (user, group or room). Returns the
    /// `chunks` to put in the `Message`. For users the negotiated spec
    /// version overrides `spec_version`.
    pub async fn encrypt_e2ee_message(
        &self,
        to: &str,
        content: E2eeContent,
        content_type: i32,
        spec_version: i32,
    ) -> Result<Vec<Vec<u8>>> {
        self.e2ee().encrypt_message(to, content, content_type, spec_version).await
    }

    /// Decrypt a `Message` (as decoded from thrift) if it is E2EE; returns the
    /// message with `text` / `location` / `contentMetadata` filled in.
    /// Non-E2EE messages are returned unchanged.
    pub async fn decrypt_e2ee_message(&self, message: Value) -> Result<Value> {
        self.e2ee().decrypt_message(message).await
    }

    /// `(text, extra metadata)` of an E2EE text message.
    pub async fn decrypt_e2ee_text_message(&self, message: &Value) -> Result<(String, Vec<(String, String)>)> {
        self.e2ee().decrypt_text(message).await
    }

    /// The `Location` of an E2EE location message.
    pub async fn decrypt_e2ee_location_message(&self, message: &Value) -> Result<Value> {
        self.e2ee().decrypt_location(message).await
    }

    /// The decrypted JSON payload of any E2EE message (e.g. media
    /// `keyMaterial`/`fileName` for images and files).
    pub async fn decrypt_e2ee_data_message(&self, message: &Value) -> Result<serde_json::Value> {
        self.e2ee().decrypt_payload(message).await
    }
}
