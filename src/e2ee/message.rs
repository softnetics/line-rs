//! Message encryption and decryption.
//!
//! An E2EE message carries five `chunks`: `[salt(16), payload, nonce(12),
//! senderKeyId(i32 BE), receiverKeyId(i32 BE)]`. The payload is JSON
//! (`{"text": ...}`, `{"location": {...}}` or content-specific metadata)
//! encrypted with AES-256-GCM (spec v2, AAD-bound to the envelope) or
//! AES-256-CBC (spec v1). The ECDH pair is our key and the peer's public key
//! for 1:1 chats, or the group shared key and the sender's public key for
//! groups and rooms.

use super::crypto;
use super::keys::{E2ee, key_data_of, no_e2ee_key};
use super::E2eeContent;
use crate::error::{Error, Result};
use crate::thrift::{Schema, Value};

const LOCATION: i64 = 15;

/// Numeric `ContentType` of a decoded message (enum name or number).
pub fn content_type_number(v: &Value) -> i64 {
    match v {
        Value::String(name) => Schema::line()
            .get_enum("ContentType")
            .and_then(|e| e.value_of(name))
            .map(i64::from)
            .or_else(|| name.parse().ok())
            .unwrap_or(0),
        other => other.as_i64().unwrap_or(0),
    }
}

fn is_user_to_type(v: &Value) -> bool {
    v.as_str() == Some("USER") || matches!(v, Value::Int(0))
}

fn malformed() -> Error {
    Error::Crypto("E2EE message has malformed chunks".into())
}

/// The five envelope chunks as bytes.
fn chunks_of(message: &Value) -> Result<Vec<Vec<u8>>> {
    let chunks: Vec<Vec<u8>> = message["chunks"]
        .as_list()
        .unwrap_or_default()
        .iter()
        .map(|c| c.as_bytes().map(<[u8]>::to_vec).ok_or_else(malformed))
        .collect::<Result<_>>()?;
    if chunks.len() < 5 {
        return Err(malformed());
    }
    Ok(chunks)
}

/// Encrypt a JSON payload into the chunk layout.
#[allow(clippy::too_many_arguments)]
pub fn encrypt_payload(
    sender_key_id: i64,
    receiver_key_id: i64,
    shared_secret: &[u8],
    spec_version: i64,
    payload: &serde_json::Value,
    to: &str,
    from: &str,
    content_type: i64,
) -> Result<Vec<Vec<u8>>> {
    let salt = crypto::random_bytes(16);
    let gcm_key = crypto::sha256_sum(&[shared_secret, &salt, b"Key"]);
    let aad = crypto::generate_aad(to, from, sender_key_id, receiver_key_id, spec_version, content_type);
    let nonce = crypto::random_bytes(12);
    let data = serde_json::to_vec(payload)?;
    let enc = crypto::encrypt_aes_gcm(&gcm_key, &nonce, &data, &aad)?;
    Ok(vec![
        salt,
        enc,
        nonce,
        crypto::int_bytes(sender_key_id).to_vec(),
        crypto::int_bytes(receiver_key_id).to_vec(),
    ])
}

/// Spec v1: AES-256-CBC keyed by `sha256(secret ‖ salt ‖ "Key")`.
pub fn decrypt_message_v1(chunks: &[Vec<u8>], private_key: &[u8], public_key: &[u8]) -> Result<serde_json::Value> {
    let secret = crypto::generate_shared_secret(private_key, public_key);
    let (key, iv) = crypto::derive_key_iv(&secret, &chunks[0]);
    let plain = crypto::decrypt_aes_cbc(&key, &iv, &chunks[1], true)
        .or_else(|_| crypto::decrypt_aes_cbc(&key, &iv, &chunks[1], false))?;
    Ok(serde_json::from_slice(&plain)?)
}

/// Spec v2: AES-256-GCM with the envelope as AAD.
pub fn decrypt_message_v2(
    to: &str,
    from: &str,
    chunks: &[Vec<u8>],
    private_key: &[u8],
    public_key: &[u8],
    spec_version: i64,
    content_type: i64,
) -> Result<serde_json::Value> {
    let secret = crypto::generate_shared_secret(private_key, public_key);
    let gcm_key = crypto::sha256_sum(&[&secret, &chunks[0], b"Key"]);
    let aad = crypto::generate_aad(
        to,
        from,
        crypto::byte2int(&chunks[3]),
        crypto::byte2int(&chunks[4]),
        spec_version,
        content_type,
    );
    let plain = crypto::decrypt_aes_gcm(&gcm_key, &chunks[2], &chunks[1], &aad)?;
    Ok(serde_json::from_slice(&plain)?)
}

/// The ECDH inputs selected for a message, before decryption.
pub(crate) struct MessageKeys {
    pub private_key: Vec<u8>,
    pub public_key: Vec<u8>,
}

impl E2ee<'_> {
    pub async fn encrypt_message(
        &self,
        to: &str,
        content: E2eeContent,
        content_type: i32,
        spec_version: i32,
    ) -> Result<Vec<Vec<u8>>> {
        let from = self.host.self_mid().ok_or_else(|| Error::internal("NotAuthorized", "profile is not loaded"))?;
        let self_key = self.get_self_key(&from).await?;
        let to_type = crate::base::BaseClient::get_to_type(to);
        if !matches!(to_type, Some(0..=2)) {
            return Err(Error::internal("Invalid mid", to));
        }
        let sender_key_id = self_key.key_id.ok_or_else(no_e2ee_key)?;
        let mut spec_version = spec_version as i64;
        let (receiver_key_id, secret) = if to_type == Some(0) {
            let res = self.host.negotiate_e2ee_public_key(to).await?;
            spec_version = res["specVersion"].as_i64().unwrap_or(spec_version);
            if spec_version == -1 {
                return Err(Error::internal("Not support E2EE", to));
            }
            let pk = &res["publicKey"];
            let secret = crypto::generate_shared_secret(&self_key.private_key(), key_data_of(pk, to)?);
            (super::keys::key_id_of(&pk["keyId"]).unwrap_or_default(), secret)
        } else {
            let group = self.get_group_key(to, None).await?;
            (group.key_id, crypto::generate_shared_secret(&group.private_key(), &self_key.public_key()))
        };
        let content_type = content_type as i64;
        let (payload, aad_type) = match content {
            E2eeContent::Location(loc) if content_type == LOCATION => {
                (serde_json::json!({"location": loc.to_json()}), LOCATION)
            }
            E2eeContent::Location(loc) => (loc.to_json(), content_type),
            // Text is always bound as content type 0, as linejs does.
            E2eeContent::Text(text) => (serde_json::json!({"text": text}), 0),
            E2eeContent::Json(data) => (data, content_type),
        };
        encrypt_payload(sender_key_id, receiver_key_id, &secret, spec_version, &payload, to, &from, aad_type)
    }

    /// Pick our private key and the peer public key for a received or sent
    /// message. For 1:1 chats our generation is pinned by the envelope key id
    /// (falling back to the current key); groups use the shared key of the
    /// generation named by `receiverKeyId`.
    pub(crate) async fn message_keys(&self, message: &Value, chunks: &[Vec<u8>]) -> Result<MessageKeys> {
        let mid = self.host.self_mid().unwrap_or_default();
        let from = message["from"].as_str().unwrap_or_default();
        let to = message["to"].as_str().unwrap_or_default();
        let is_self = from == mid;
        let sender_key_id = crypto::byte2int(&chunks[3]);
        let receiver_key_id = crypto::byte2int(&chunks[4]);

        if is_user_to_type(&message["toType"]) {
            let self_key_id = if is_self { sender_key_id } else { receiver_key_id };
            let self_key = match self.get_self_key_by_key_id(self_key_id).await {
                Some(k) => k,
                None => self.get_self_key(&mid).await?,
            };
            let public_key = if is_self {
                self.get_user_public_key(to, Some(receiver_key_id)).await?
            } else {
                self.get_user_public_key(from, Some(sender_key_id)).await?
            };
            Ok(MessageKeys { private_key: self_key.private_key(), public_key })
        } else {
            let self_key = self.get_self_key(&mid).await?;
            let group = self.get_group_key(to, Some(receiver_key_id)).await?;
            let public_key = if is_self {
                self_key.public_key()
            } else {
                self.get_user_public_key(from, Some(sender_key_id)).await?
            };
            Ok(MessageKeys { private_key: group.private_key(), public_key })
        }
    }

    /// Decrypt the payload of any E2EE message to its JSON object.
    pub async fn decrypt_payload(&self, message: &Value) -> Result<serde_json::Value> {
        let chunks = chunks_of(message)?;
        let keys = self.message_keys(message, &chunks).await?;
        let spec = message["contentMetadata"]["e2eeVersion"].as_str().filter(|s| !s.is_empty()).unwrap_or("2");
        if spec == "2" {
            decrypt_message_v2(
                message["to"].as_str().unwrap_or_default(),
                message["from"].as_str().unwrap_or_default(),
                &chunks,
                &keys.private_key,
                &keys.public_key,
                2,
                content_type_number(&message["contentType"]),
            )
        } else {
            decrypt_message_v1(&chunks, &keys.private_key, &keys.public_key)
        }
    }

    /// Text and metadata of an E2EE text message. Non-string metadata values
    /// are JSON-encoded.
    pub async fn decrypt_text(&self, message: &Value) -> Result<(String, Vec<(String, String)>)> {
        let payload = self.decrypt_payload(message).await?;
        let text = payload["text"].as_str().unwrap_or_default().to_string();
        let meta = payload
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(k, _)| k.as_str() != "text")
            .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())))
            .collect();
        Ok((text, meta))
    }

    pub async fn decrypt_location(&self, message: &Value) -> Result<Value> {
        let payload = self.decrypt_payload(message).await?;
        Ok(Value::from(payload["location"].clone()))
    }

    /// Fill `text`/`contentMetadata` (NONE) or `location` (LOCATION) of an
    /// E2EE message; anything else is returned unchanged.
    pub async fn decrypt_message(&self, mut message: Value) -> Result<Value> {
        if message["chunks"].as_list().is_none_or(<[Value]>::is_empty) {
            return Ok(message);
        }
        match content_type_number(&message["contentType"]) {
            0 => {
                let (text, meta) = self.decrypt_text(&message).await?;
                let mut entries: Vec<(Value, Value)> = message["contentMetadata"]
                    .entries()
                    .into_iter()
                    .filter(|(k, _)| !meta.iter().any(|(m, _)| m == k))
                    .map(|(k, v)| (Value::String(k), v.clone()))
                    .collect();
                entries.extend(meta.into_iter().map(|(k, v)| (Value::String(k), Value::String(v))));
                message.insert("text", text);
                message.insert("contentMetadata", Value::Map(entries));
            }
            LOCATION => {
                let location = self.decrypt_location(&message).await?;
                message.insert("location", location);
            }
            _ => {}
        }
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;

    use super::*;
    use crate::e2ee::E2eeState;
    use crate::e2ee::keys::tests::FakeHost;
    use crate::storage::Storage;
    use crate::tv;

    fn b64(b: &[u8]) -> String {
        B64.encode(b)
    }

    fn message(from: &str, to: &str, content_type: &str, sender: i64, receiver: i64) -> Value {
        let mut m = tv!({"from": from, "to": to, "toType": "USER", "contentType": content_type});
        m.insert("contentMetadata", Value::Map(vec![("e2eeVersion".into(), "2".into())]));
        let chunks = vec![
            Value::Binary(vec![0; 16]),
            Value::Binary(vec![0; 64]),
            Value::Binary(vec![0; 12]),
            Value::Binary(crypto::int_bytes(sender).to_vec()),
            Value::Binary(crypto::int_bytes(receiver).to_vec()),
        ];
        m.insert("chunks", Value::List(chunks));
        m
    }

    fn key_json(private: &[u8]) -> String {
        serde_json::json!({"privKey": b64(private), "pubKey": b64(&crypto::public_key_of(private))}).to_string()
    }

    /// Storage with self keys 9/11 (or 7/11), the latest alias, and peer keys.
    async fn fixture(by_id: &[(i64, u8)], latest: u8) -> FakeHost {
        let host = FakeHost::new();
        for &(id, fill) in by_id {
            host.set(&format!("e2eeKeys:{id}"), key_json(&[fill; 32])).await;
        }
        host.set("e2eeKeys:u-self", key_json(&[latest; 32])).await;
        for (id, fill) in [(7, 0x70u8), (11, 0x71)] {
            host.set(&format!("e2eePublicKeys:u-them:{id}"), b64(&[fill; 32])).await;
        }
        host
    }

    async fn keys_for(host: &FakeHost, m: &Value) -> MessageKeys {
        let state = E2eeState::default();
        let chunks = chunks_of(m).unwrap();
        E2ee::new(host, &state).message_keys(m, &chunks).await.unwrap()
    }

    // --- key_selection.test.ts / location_decrypt.test.ts -------------------

    #[tokio::test]
    async fn received_message_pins_self_key_by_receiver_key_id() {
        let host = fixture(&[(9, 1), (11, 2)], 2).await;
        for ct in ["IMAGE", "LOCATION"] {
            let keys = keys_for(&host, &message("u-them", "u-self", ct, 7, 9)).await;
            assert_eq!(keys.private_key, [1; 32]);
            // The sender's public key for senderKeyId, not ours.
            assert_eq!(keys.public_key, [0x70; 32]);
        }
    }

    #[tokio::test]
    async fn sent_message_pins_self_key_by_sender_key_id() {
        let host = fixture(&[(7, 3), (11, 4)], 4).await;
        let keys = keys_for(&host, &message("u-self", "u-them", "LOCATION", 7, 11)).await;
        assert_eq!(keys.private_key, [3; 32]);
        // Self-sent: the recipient's public key for receiverKeyId.
        assert_eq!(keys.public_key, [0x71; 32]);
    }

    #[tokio::test]
    async fn missing_key_id_falls_back_to_latest_key() {
        let host = fixture(&[], 5).await;
        let keys = keys_for(&host, &message("u-them", "u-self", "IMAGE", 7, 9)).await;
        assert_eq!(keys.private_key, [5; 32]);
    }

    #[test]
    fn location_aad_uses_numeric_content_type() {
        assert_eq!(content_type_number(&Value::from("LOCATION")), 15);
        assert_eq!(content_type_number(&Value::from("NONE")), 0);
        assert_eq!(content_type_number(&Value::Int(7)), 7);
    }

    // --- end-to-end round trips ---------------------------------------------

    /// Self key 1 for `u-self`, peer `u-them` with key 2.
    async fn pair() -> (FakeHost, Vec<u8>) {
        let mut host = FakeHost::new();
        let (self_priv, self_pub) = crypto::generate_key_pair();
        let (peer_priv, peer_pub) = crypto::generate_key_pair();
        let key = serde_json::json!({"keyId": 1, "privKey": b64(&self_priv), "pubKey": b64(&self_pub)}).to_string();
        host.set("e2eeKeys:1", key.clone()).await;
        host.set("e2eeKeys:u-self", key).await;
        host.negotiate.insert("u-them".into(), (2, peer_pub));
        (host, peer_priv)
    }

    #[tokio::test]
    async fn text_round_trip_through_own_sent_message() {
        let (host, _) = pair().await;
        let state = E2eeState::default();
        let e2ee = E2ee::new(&host, &state);
        let chunks = e2ee.encrypt_message("u-them", E2eeContent::Text("hello".into()), 0, 2).await.unwrap();
        assert_eq!(chunks.len(), 5);
        assert_eq!(crypto::byte2int(&chunks[3]), 1);
        assert_eq!(crypto::byte2int(&chunks[4]), 2);

        let mut m = message("u-self", "u-them", "NONE", 1, 2);
        m.insert("chunks", Value::List(chunks.into_iter().map(Value::Binary).collect()));
        let out = e2ee.decrypt_message(m).await.unwrap();
        assert_eq!(out["text"].as_str(), Some("hello"));
        assert_eq!(out["contentMetadata"]["e2eeVersion"].as_str(), Some("2"));
    }

    #[tokio::test]
    async fn location_round_trip_from_peer() {
        let (host, peer_priv) = pair().await;
        let self_key = host.storage.get_string("e2eeKeys:1").await.unwrap().unwrap();
        let self_key: serde_json::Value = serde_json::from_str(&self_key).unwrap();
        let self_pub = crypto::decode_base64(self_key["pubKey"].as_str().unwrap());
        // The peer encrypts to us with its key 2 → our key 1.
        let secret = crypto::generate_shared_secret(&peer_priv, &self_pub);
        let loc = serde_json::json!({"location": {"title": "here", "latitude": 1.5, "longitude": 2.0}});
        let chunks = encrypt_payload(2, 1, &secret, 2, &loc, "u-self", "u-them", LOCATION).unwrap();

        let mut m = message("u-them", "u-self", "LOCATION", 2, 1);
        m.insert("chunks", Value::List(chunks.into_iter().map(Value::Binary).collect()));
        let state = E2eeState::default();
        let out = E2ee::new(&host, &state).decrypt_message(m).await.unwrap();
        assert_eq!(out["location"]["title"].as_str(), Some("here"));
        assert_eq!(out["location"]["latitude"].as_f64(), Some(1.5));
    }

    #[tokio::test]
    async fn v1_payload_decrypts() {
        let (a, ap) = crypto::generate_key_pair();
        let (b, bp) = crypto::generate_key_pair();
        let salt = vec![9u8; 16];
        let (key, iv) = crypto::derive_key_iv(&crypto::generate_shared_secret(&a, &bp), &salt);
        let enc = crypto::encrypt_aes_cbc(&key, &iv, br#"{"text":"v1"}"#).unwrap();
        let chunks = vec![salt, enc, vec![0; 12], vec![0; 4], vec![0; 4]];
        assert_eq!(decrypt_message_v1(&chunks, &b, &ap).unwrap()["text"], "v1");
    }

    #[tokio::test]
    async fn non_e2ee_message_passes_through() {
        let host = FakeHost::new();
        let state = E2eeState::default();
        let m = tv!({"from": "u-them", "to": "u-self", "text": "plain", "contentType": "NONE"});
        assert_eq!(E2ee::new(&host, &state).decrypt_message(m.clone()).await.unwrap(), m);
    }
}
