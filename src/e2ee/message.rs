//! Message encryption and decryption.
//!
//! An E2EE message carries five `chunks`: `[salt(16), payload, nonce(12),
//! senderKeyId(i32 BE), receiverKeyId(i32 BE)]`. The payload is JSON
//! (`{"text": ...}`, `{"location": {...}}` or content-specific metadata)
//! encrypted with AES-256-GCM (spec v2, AAD-bound to the envelope) or
//! AES-256-CBC (spec v1). The ECDH pair is our key and the peer's public key
//! for 1:1 chats, or the group shared key and the sender's public key for
//! groups and rooms.

use super::E2eeContent;
use super::crypto;
use super::keys::{E2ee, no_e2ee_key};
use super::send::E2eeChatState;
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
    let aad = crypto::generate_aad(
        to,
        from,
        sender_key_id,
        receiver_key_id,
        spec_version,
        content_type,
    );
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

/// Spec v1: `[salt(8), ciphertext, mac, senderKeyId, receiverKeyId]`,
/// AES-256-CBC (PKCS#7) keyed by `sha256(secret ‖ salt ‖ "Key")` with the
/// folded `sha256(secret ‖ salt ‖ "IV")` as IV. `mac` must be
/// `AES-ECB(key, fold(sha256(ciphertext)))`, checked before decrypting.
pub fn decrypt_message_v1(
    chunks: &[Vec<u8>],
    private_key: &[u8],
    public_key: &[u8],
) -> Result<serde_json::Value> {
    if chunks.len() < 5 {
        return Err(malformed());
    }
    let secret = crypto::generate_shared_secret(private_key, public_key);
    let (key, iv) = crypto::derive_key_iv(&secret, &chunks[0]);
    let expected = crypto::encrypt_aes_ecb(&key, &crypto::xor(&crypto::sha256(&chunks[1])));
    if chunks[2] != expected {
        return Err(Error::Crypto(format!(
            "E2EE v1 message MAC does not match ({} byte MAC)",
            chunks[2].len()
        )));
    }
    let plain = crypto::decrypt_aes_cbc(&key, &iv, &chunks[1], true)?;
    Ok(serde_json::from_slice(&plain)?)
}

/// Encrypt a JSON payload in the spec v1 layout (tests and old peers).
#[cfg(test)]
pub(crate) fn encrypt_payload_v1(
    sender_key_id: i64,
    receiver_key_id: i64,
    shared_secret: &[u8],
    payload: &serde_json::Value,
) -> Result<Vec<Vec<u8>>> {
    let salt = crypto::random_bytes(8);
    let (key, iv) = crypto::derive_key_iv(shared_secret, &salt);
    let ct = crypto::encrypt_aes_cbc(&key, &iv, &serde_json::to_vec(payload)?)?;
    let mac = crypto::encrypt_aes_ecb(&key, &crypto::xor(&crypto::sha256(&ct)));
    Ok(vec![
        salt,
        ct,
        mac,
        crypto::int_bytes(sender_key_id).to_vec(),
        crypto::int_bytes(receiver_key_id).to_vec(),
    ])
}

/// The Letter Sealing spec of a message from `contentMetadata.e2eeVersion`:
/// 1 or 2. Without the key the message is taken as v2, as before; any
/// other value is an error and the message stays undecrypted.
pub fn spec_version_of(message: &Value) -> Result<i64> {
    let v = &message["contentMetadata"]["e2eeVersion"];
    let parsed = match v {
        Value::Null => return Ok(2),
        Value::String(s) if s.is_empty() => return Ok(2),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        other => other.as_i64(),
    };
    match parsed {
        Some(n @ (1 | 2)) => Ok(n),
        _ => Err(Error::Crypto(format!(
            "unsupported e2eeVersion {}",
            v.to_json()
        ))),
    }
}

/// Whether `message` is a sealed text or location message that was not
/// decrypted: it carries `chunks` and `e2eeVersion` but no `text` (NONE)
/// or `location` (LOCATION). Other content types keep their chunks after
/// [`decrypt_message`](super::BaseClient::decrypt_e2ee_message) and are
/// opened on demand, so they never count.
pub fn is_undecrypted(message: &Value) -> bool {
    if message["contentMetadata"].get("e2eeVersion").is_none()
        || message["chunks"].as_list().is_none_or(<[Value]>::is_empty)
    {
        return false;
    }
    match content_type_number(&message["contentType"]) {
        0 => message["text"].as_str().is_none_or(str::is_empty),
        LOCATION => message["location"].is_null(),
        _ => false,
    }
}

/// What a log may say about an E2EE message without its content: id, chat
/// kind (`u`, `c` or `r`, from `to`), content type, `e2eeVersion`, the key
/// ids in chunks 3 and 4 and every chunk's length.
pub fn describe_e2ee_message(message: &Value) -> serde_json::Value {
    let chunks: Vec<Vec<u8>> = message["chunks"]
        .as_list()
        .unwrap_or_default()
        .iter()
        .map(|c| c.as_bytes().map(<[u8]>::to_vec).unwrap_or_default())
        .collect();
    let key_id = |i: usize| {
        chunks
            .get(i)
            .filter(|c| c.len() == 4)
            .map(|c| crypto::byte2int(c) as i32)
    };
    let to = message["to"].as_str().unwrap_or_default();
    serde_json::json!({
        "id": message["id"].as_str().map(str::to_string).or_else(|| message["id"].as_i64().map(|i| i.to_string())),
        "chatKind": to.get(..1).unwrap_or(""),
        "contentType": content_type_number(&message["contentType"]),
        "e2eeVersion": message["contentMetadata"]["e2eeVersion"].to_json(),
        "senderKeyId": key_id(3),
        "receiverKeyId": key_id(4),
        "chunkLengths": chunks.iter().map(Vec::len).collect::<Vec<_>>(),
    })
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
    /// Encrypt for `to` after looking its key up again (negotiate or the
    /// group's last key); the lookup is stored as the chat's
    /// [`E2eeChatState`]. `spec_version` is used when the server names none.
    pub async fn encrypt_message(
        &self,
        to: &str,
        content: E2eeContent,
        content_type: i32,
        spec_version: i32,
    ) -> Result<Vec<Vec<u8>>> {
        if !matches!(crate::base::BaseClient::get_to_type(to), Some(0..=2)) {
            return Err(Error::internal("Invalid mid", to));
        }
        let mut state = self.refresh_chat_state(to).await?;
        if state.spec_version <= 0 && state.usable {
            state.spec_version = spec_version;
        }
        self.encrypt_for_state(to, content, content_type, &state)
            .await
    }

    /// Encrypt for `to` with the key named by `state` (the peer's key id for
    /// a user, the group key id for a group or room) and its spec version.
    pub async fn encrypt_for_state(
        &self,
        to: &str,
        content: E2eeContent,
        content_type: i32,
        state: &E2eeChatState,
    ) -> Result<Vec<Vec<u8>>> {
        let from = self
            .host
            .self_mid()
            .ok_or_else(|| Error::internal("NotAuthorized", "profile is not loaded"))?;
        let to_type = crate::base::BaseClient::get_to_type(to);
        if !matches!(to_type, Some(0..=2)) {
            return Err(Error::internal("Invalid mid", to));
        }
        let (true, Some(receiver_key_id)) = (state.usable, state.key_id) else {
            return Err(Error::internal("Not support E2EE", to));
        };
        let spec_version = if state.spec_version > 0 {
            state.spec_version as i64
        } else {
            2
        };
        let self_key = self.get_self_key(&from).await?;
        let sender_key_id = self_key.key_id.ok_or_else(no_e2ee_key)?;
        let secret = if to_type == Some(0) {
            let peer = self.get_user_public_key(to, Some(receiver_key_id)).await?;
            crypto::generate_shared_secret(&self_key.private_key(), &peer)
        } else {
            let group = self.get_group_key(to, Some(receiver_key_id)).await?;
            crypto::generate_shared_secret(&group.private_key(), &self_key.public_key())
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
        let chunks = encrypt_payload(
            sender_key_id,
            receiver_key_id,
            &secret,
            spec_version,
            &payload,
            to,
            &from,
            aad_type,
        )?;
        self.check_sealed(
            to,
            &from,
            &self_key,
            &chunks,
            &payload,
            aad_type,
            to_type != Some(0),
        )
        .await?;
        Ok(chunks)
    }

    /// Open `chunks` the way their recipients will before they are sent, so
    /// a message nobody can read never leaves: recipients derive the secret
    /// from the public key LINE has registered for our key id, and from the
    /// group key of the generation named in the envelope, not from what this
    /// device has stored.
    #[allow(clippy::too_many_arguments)]
    async fn check_sealed(
        &self,
        to: &str,
        from: &str,
        self_key: &super::keys::E2eeKey,
        chunks: &[Vec<u8>],
        payload: &serde_json::Value,
        content_type: i64,
        group: bool,
    ) -> Result<()> {
        let fail =
            |why: String| Error::Crypto(format!("E2EE self-check failed, message not sent: {why}"));
        let sender_key_id = crypto::byte2int(&chunks[3]);
        let private_key = self_key.private_key();
        let confirmed = self
            .state
            .confirmed_keys
            .lock()
            .unwrap()
            .contains(&sender_key_id);
        if !confirmed {
            match self
                .check_stored_key_against_server(&sender_key_id.to_string(), &private_key)
                .await
            {
                Some(true) => {
                    self.state
                        .confirmed_keys
                        .lock()
                        .unwrap()
                        .insert(sender_key_id);
                }
                Some(false) => {
                    return Err(fail(format!(
                        "our key {sender_key_id} does not match the one LINE has"
                    )));
                }
                None => {
                    return Err(fail(format!(
                        "LINE did not confirm our key {sender_key_id}"
                    )));
                }
            }
        }
        // Select keys exactly as decrypting the sent message will.
        let mut sent = crate::tv!({"from": from, "to": to});
        sent.insert("toType", if group { 1 } else { 0 });
        let keys = self
            .message_keys(&sent, chunks)
            .await
            .map_err(|e| fail(e.to_string()))?;
        let (ecdh_private, ecdh_public) = (keys.private_key, keys.public_key);
        let spec = 2;
        let opened = decrypt_message_v2(
            to,
            from,
            chunks,
            &ecdh_private,
            &ecdh_public,
            spec,
            content_type,
        )
        .map_err(|e| fail(e.to_string()))?;
        if &opened != payload {
            return Err(fail("the payload came back different".into()));
        }
        Ok(())
    }

    /// Pick the ECDH inputs for a received or sent message strictly by the
    /// key ids in its envelope. A key that is not
    /// available is an `E2EEKeyMissing` error, never another generation:
    /// a guessed key only fails later as "unable to authenticate data".
    ///
    /// | message        | private key                | public key               |
    /// |----------------|----------------------------|--------------------------|
    /// | 1:1 received   | ours, `receiverKeyId`      | sender's, `senderKeyId`  |
    /// | 1:1 sent       | ours, `senderKeyId`        | peer's, `receiverKeyId`  |
    /// | group received | group, `receiverKeyId`     | sender's, `senderKeyId`  |
    /// | group sent     | group, `receiverKeyId`     | ours, `senderKeyId`      |
    pub(crate) async fn message_keys(
        &self,
        message: &Value,
        chunks: &[Vec<u8>],
    ) -> Result<MessageKeys> {
        let mid = self.host.self_mid().unwrap_or_default();
        let from = message["from"].as_str().unwrap_or_default();
        let to = message["to"].as_str().unwrap_or_default();
        let is_self = !mid.is_empty() && from == mid;
        let sender_key_id = crypto::byte2int(&chunks[3]);
        let receiver_key_id = crypto::byte2int(&chunks[4]);

        if is_user_to_type(&message["toType"]) {
            let (self_key_id, peer, peer_key_id) = if is_self {
                (sender_key_id, to, receiver_key_id)
            } else {
                (receiver_key_id, from, sender_key_id)
            };
            let self_key = self.own_key(self_key_id).await?;
            let public_key = self.get_user_public_key(peer, Some(peer_key_id)).await?;
            Ok(MessageKeys {
                private_key: self_key.private_key(),
                public_key,
            })
        } else {
            let group = self.get_group_key(to, Some(receiver_key_id)).await?;
            let public_key = if is_self {
                self.own_key(sender_key_id).await?.public_key()
            } else {
                self.get_user_public_key(from, Some(sender_key_id)).await?
            };
            Ok(MessageKeys {
                private_key: group.private_key(),
                public_key,
            })
        }
    }

    /// Decrypt the payload of any E2EE message to its JSON object.
    pub async fn decrypt_payload(&self, message: &Value) -> Result<serde_json::Value> {
        let spec = spec_version_of(message)?;
        let chunks = chunks_of(message)?;
        let keys = self.message_keys(message, &chunks).await?;
        if spec == 2 {
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
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| v.to_string()),
                )
            })
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
                entries.extend(
                    meta.into_iter()
                        .map(|(k, v)| (Value::String(k), Value::String(v))),
                );
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
        m.insert(
            "contentMetadata",
            Value::Map(vec![("e2eeVersion".into(), "2".into())]),
        );
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
        serde_json::json!({"privKey": b64(private), "pubKey": b64(&crypto::public_key_of(private))})
            .to_string()
    }

    /// Storage with self keys 9/11 (or 7/11), the latest alias, and peer keys.
    async fn fixture(by_id: &[(i64, u8)], latest: u8) -> FakeHost {
        let host = FakeHost::new();
        for &(id, fill) in by_id {
            host.set(&format!("e2eeKeys:{id}"), key_json(&[fill; 32]))
                .await;
        }
        host.set("e2eeKeys:u-self", key_json(&[latest; 32])).await;
        for (id, fill) in [(7, 0x70u8), (11, 0x71)] {
            host.set(&format!("e2eePublicKeys:u-them:{id}"), b64(&[fill; 32]))
                .await;
        }
        host
    }

    async fn keys_for(host: &FakeHost, m: &Value) -> MessageKeys {
        let state = E2eeState::default();
        let chunks = chunks_of(m).unwrap();
        E2ee::new(host, &state)
            .message_keys(m, &chunks)
            .await
            .unwrap()
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

    async fn keys_err(host: &FakeHost, m: &Value) -> Error {
        let state = E2eeState::default();
        let chunks = chunks_of(m).unwrap();
        match E2ee::new(host, &state).message_keys(m, &chunks).await {
            Ok(_) => panic!("keys were found"),
            Err(e) => e,
        }
    }

    fn is_missing(e: &Error) -> bool {
        matches!(e, Error::Internal { kind, .. } if kind == "E2EEKeyMissing")
    }

    #[tokio::test]
    async fn received_message_without_our_key_is_missing_not_latest() {
        // We hold key 11 and the current pointer, but not key 9.
        let host = fixture(&[(11, 2)], 5).await;
        let err = keys_err(&host, &message("u-them", "u-self", "NONE", 7, 9)).await;
        assert!(is_missing(&err), "{err}");
    }

    #[tokio::test]
    async fn sent_message_without_our_key_is_missing_not_latest() {
        let host = fixture(&[(11, 2)], 5).await;
        let err = keys_err(&host, &message("u-self", "u-them", "NONE", 7, 11)).await;
        assert!(is_missing(&err), "{err}");
    }

    fn group_message(from: &str, sender: i64, receiver: i64) -> Value {
        let mut m = message(from, "c-group", "NONE", sender, receiver);
        m.insert("toType", "GROUP");
        m
    }

    /// `fixture` plus cached group keys 7 (fill 0x07) and 9 (fill 0x09, also
    /// the "last" slot).
    async fn group_fixture(by_id: &[(i64, u8)], latest: u8) -> FakeHost {
        let host = fixture(by_id, latest).await;
        for id in [7u8, 9] {
            let key = serde_json::json!({"privKey": b64(&[id; 32]), "keyId": id}).to_string();
            host.set(&format!("e2eeGroupKeys:c-group:{id}"), key.clone())
                .await;
            host.set("e2eeGroupKeys:c-group", key).await;
        }
        host
    }

    #[tokio::test]
    async fn received_group_message_uses_group_key_and_sender_key_by_id() {
        let host = group_fixture(&[], 5).await;
        let keys = keys_for(&host, &group_message("u-them", 11, 7)).await;
        assert_eq!(keys.private_key, [7; 32]);
        assert_eq!(keys.public_key, [0x71; 32]);
        assert!(host.calls().is_empty());
    }

    #[tokio::test]
    async fn group_message_with_unknown_group_key_is_missing_not_last() {
        // Group key 8 is neither cached nor served; the last key 9 must not
        // be used instead.
        let host = group_fixture(&[], 5).await;
        let err = keys_err(&host, &group_message("u-them", 7, 8)).await;
        assert!(is_missing(&err), "{err}");
        let names: Vec<_> = host.calls().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["getE2EEGroupSharedKey"]);
    }

    #[tokio::test]
    async fn own_group_message_uses_public_key_of_sender_key_id() {
        // Sent with our older key 3; the current key is another one.
        let host = group_fixture(&[(3, 0x33), (4, 0x44)], 0x44).await;
        let keys = keys_for(&host, &group_message("u-self", 3, 9)).await;
        assert_eq!(keys.private_key, [9; 32]);
        assert_eq!(keys.public_key, crypto::public_key_of(&[0x33; 32]));
    }

    #[tokio::test]
    async fn own_group_message_without_sender_key_is_missing() {
        let host = group_fixture(&[(4, 0x44)], 0x44).await;
        let err = keys_err(&host, &group_message("u-self", 3, 9)).await;
        assert!(is_missing(&err), "{err}");
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
        let key =
            serde_json::json!({"keyId": 1, "privKey": b64(&self_priv), "pubKey": b64(&self_pub)})
                .to_string();
        host.set("e2eeKeys:1", key.clone()).await;
        host.set("e2eeKeys:u-self", key).await;
        host.negotiate
            .insert("u-them".into(), (2, peer_pub.clone()));
        host.peer_keys.insert(("u-them".into(), 2), peer_pub);
        host.public_keys = Some(vec![registered(1, &self_pub)]);
        (host, peer_priv)
    }

    /// A `getE2EEPublicKeys` entry: what LINE has registered for our key id.
    fn registered(key_id: i64, public_key: &[u8]) -> Value {
        let mut raw = tv!({"2": key_id});
        raw.insert("4", Value::Binary(public_key.to_vec()));
        raw
    }

    #[tokio::test]
    async fn send_refuses_a_key_line_does_not_have() {
        let (mut host, _) = pair().await;
        // LINE has another public key for our key id 1, so recipients would
        // derive a different secret and could not open the message.
        let (_, other_pub) = crypto::generate_key_pair();
        host.public_keys = Some(vec![registered(1, &other_pub)]);
        let state = E2eeState::default();
        let err = E2ee::new(&host, &state)
            .encrypt_message("u-them", E2eeContent::Text("hi".into()), 0, 2)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("does not match"), "{err}");
    }

    #[tokio::test]
    async fn send_refuses_when_line_does_not_list_our_key() {
        let (mut host, _) = pair().await;
        host.public_keys = Some(vec![]);
        let state = E2eeState::default();
        let err = E2ee::new(&host, &state)
            .encrypt_message("u-them", E2eeContent::Text("hi".into()), 0, 2)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("did not confirm"), "{err}");
    }

    #[tokio::test]
    async fn confirmed_key_is_checked_with_line_once() {
        let (host, _) = pair().await;
        let state = E2eeState::default();
        let e2ee = E2ee::new(&host, &state);
        for _ in 0..3 {
            e2ee.encrypt_message("u-them", E2eeContent::Text("hi".into()), 0, 2)
                .await
                .unwrap();
        }
        let asked = host
            .calls()
            .iter()
            .filter(|(name, _)| name.contains("ublicKeys"))
            .count();
        assert_eq!(asked, 1);
    }

    #[tokio::test]
    async fn text_round_trip_through_own_sent_message() {
        let (host, _) = pair().await;
        let state = E2eeState::default();
        let e2ee = E2ee::new(&host, &state);
        let chunks = e2ee
            .encrypt_message("u-them", E2eeContent::Text("hello".into()), 0, 2)
            .await
            .unwrap();
        assert_eq!(chunks.len(), 5);
        assert_eq!(crypto::byte2int(&chunks[3]), 1);
        assert_eq!(crypto::byte2int(&chunks[4]), 2);

        let mut m = message("u-self", "u-them", "NONE", 1, 2);
        m.insert(
            "chunks",
            Value::List(chunks.into_iter().map(Value::Binary).collect()),
        );
        let out = e2ee.decrypt_message(m).await.unwrap();
        assert_eq!(out["text"].as_str(), Some("hello"));
        assert_eq!(out["contentMetadata"]["e2eeVersion"].as_str(), Some("2"));
    }

    #[tokio::test]
    async fn location_round_trip_from_peer() {
        let (host, peer_priv) = pair().await;
        let self_key = host
            .storage
            .get_string("e2eeKeys:1")
            .await
            .unwrap()
            .unwrap();
        let self_key: serde_json::Value = serde_json::from_str(&self_key).unwrap();
        let self_pub = crypto::decode_base64(self_key["pubKey"].as_str().unwrap());
        // The peer encrypts to us with its key 2 → our key 1.
        let secret = crypto::generate_shared_secret(&peer_priv, &self_pub);
        let loc =
            serde_json::json!({"location": {"title": "here", "latitude": 1.5, "longitude": 2.0}});
        let chunks = encrypt_payload(2, 1, &secret, 2, &loc, "u-self", "u-them", LOCATION).unwrap();

        let mut m = message("u-them", "u-self", "LOCATION", 2, 1);
        m.insert(
            "chunks",
            Value::List(chunks.into_iter().map(Value::Binary).collect()),
        );
        let state = E2eeState::default();
        let out = E2ee::new(&host, &state).decrypt_message(m).await.unwrap();
        assert_eq!(out["location"]["title"].as_str(), Some("here"));
        assert_eq!(out["location"]["latitude"].as_f64(), Some(1.5));
    }

    /// The secret a peer (key 2) shares with our key 1 in [`pair`].
    async fn peer_secret(host: &FakeHost, peer_priv: &[u8]) -> Vec<u8> {
        let self_key = host
            .storage
            .get_string("e2eeKeys:1")
            .await
            .unwrap()
            .unwrap();
        let self_key: serde_json::Value = serde_json::from_str(&self_key).unwrap();
        let self_pub = crypto::decode_base64(self_key["pubKey"].as_str().unwrap());
        crypto::generate_shared_secret(peer_priv, &self_pub)
    }

    fn from_peer(chunks: Vec<Vec<u8>>, version: &str) -> Value {
        let mut m = message("u-them", "u-self", "NONE", 2, 1);
        m.insert(
            "contentMetadata",
            Value::Map(vec![("e2eeVersion".into(), version.into())]),
        );
        m.insert(
            "chunks",
            Value::List(chunks.into_iter().map(Value::Binary).collect()),
        );
        m
    }

    #[tokio::test]
    async fn v1_message_is_read_with_its_mac() {
        let (host, peer_priv) = pair().await;
        let secret = peer_secret(&host, &peer_priv).await;
        let chunks = encrypt_payload_v1(2, 1, &secret, &serde_json::json!({"text": "v1"})).unwrap();
        assert_eq!((chunks[0].len(), chunks[2].len()), (8, 16));
        let state = E2eeState::default();
        let e2ee = E2ee::new(&host, &state);
        let out = e2ee
            .decrypt_message(from_peer(chunks.clone(), "1"))
            .await
            .unwrap();
        assert_eq!(out["text"].as_str(), Some("v1"));

        // A v1 message read as v2 takes the 16-byte MAC for the nonce.
        let err = e2ee
            .decrypt_message(from_peer(chunks.clone(), "2"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unable to authenticate"), "{err}");

        let mut bad = chunks;
        bad[2][0] ^= 1;
        let err = e2ee.decrypt_message(from_peer(bad, "1")).await.unwrap_err();
        assert!(err.to_string().contains("MAC does not match"), "{err}");
    }

    #[tokio::test]
    async fn v2_nonce_of_another_length_opens_like_java_gcm() {
        use aes_gcm::aead::{Aead, KeyInit};
        let (host, peer_priv) = pair().await;
        let secret = peer_secret(&host, &peer_priv).await;
        let salt = vec![5u8; 16];
        let key = crypto::sha256_sum(&[&secret, &salt, b"Key"]);
        let nonce = vec![6u8; 16];
        let aad = crypto::generate_aad("u-self", "u-them", 2, 1, 2, 0);
        let ct = aes_gcm::AesGcm::<aes::Aes256, aes_gcm::aead::consts::U16>::new_from_slice(&key)
            .unwrap()
            .encrypt(
                aes_gcm::Nonce::<aes_gcm::aead::consts::U16>::from_slice(&nonce),
                aes_gcm::aead::Payload {
                    msg: br#"{"text":"long nonce"}"#,
                    aad: &aad,
                },
            )
            .unwrap();
        let chunks = vec![
            salt,
            ct,
            nonce,
            crypto::int_bytes(2).to_vec(),
            crypto::int_bytes(1).to_vec(),
        ];
        let state = E2eeState::default();
        let out = E2ee::new(&host, &state)
            .decrypt_message(from_peer(chunks, "2"))
            .await
            .unwrap();
        assert_eq!(out["text"].as_str(), Some("long nonce"));
    }

    #[tokio::test]
    async fn malformed_v2_chunks_say_what_is_wrong() {
        let (host, _) = pair().await;
        let state = E2eeState::default();
        let e2ee = E2ee::new(&host, &state);
        let ids = || vec![crypto::int_bytes(2).to_vec(), crypto::int_bytes(1).to_vec()];
        let short = [vec![vec![0; 16], vec![0; 8], vec![0; 12]], ids()].concat();
        let err = e2ee
            .decrypt_message(from_peer(short, "2"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("ciphertext is 8 bytes"), "{err}");
        let no_nonce = [vec![vec![0; 16], vec![0; 32], vec![]], ids()].concat();
        let err = e2ee
            .decrypt_message(from_peer(no_nonce, "2"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("nonce is 0 bytes"), "{err}");
    }

    #[tokio::test]
    async fn unknown_spec_version_is_refused_without_key_lookups() {
        let host = FakeHost::new();
        let state = E2eeState::default();
        let mut m = from_peer(
            vec![
                vec![0; 16],
                vec![0; 32],
                vec![0; 12],
                vec![0, 0, 0, 2],
                vec![0, 0, 0, 1],
            ],
            "3",
        );
        let err = E2ee::new(&host, &state)
            .decrypt_message(m.clone())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unsupported e2eeVersion"), "{err}");
        assert!(host.calls().is_empty());
        m.insert("contentMetadata", Value::Map(vec![]));
        assert_eq!(spec_version_of(&m).unwrap(), 2);
    }

    #[tokio::test]
    async fn undecrypted_is_told_apart_from_decrypted() {
        let (host, peer_priv) = pair().await;
        let secret = peer_secret(&host, &peer_priv).await;
        let chunks = encrypt_payload(
            2,
            1,
            &secret,
            2,
            &serde_json::json!({"text": "hi"}),
            "u-self",
            "u-them",
            0,
        )
        .unwrap();
        let m = from_peer(chunks, "2");
        assert!(is_undecrypted(&m));
        let state = E2eeState::default();
        let out = E2ee::new(&host, &state).decrypt_message(m).await.unwrap();
        assert!(!is_undecrypted(&out));
        let mut image = from_peer(vec![vec![0; 4]; 5], "2");
        image.insert("contentType", "IMAGE");
        assert!(!is_undecrypted(&image));
        assert!(!is_undecrypted(&tv!({"text": "", "contentType": "NONE"})));
    }

    #[test]
    fn describe_names_ids_and_lengths_only() {
        let mut m = from_peer(
            vec![
                vec![0; 16],
                vec![0; 40],
                vec![0; 12],
                vec![0, 0, 0, 2],
                vec![0, 0, 1, 1],
            ],
            "2",
        );
        m.insert("id", "123");
        m.insert("to", "c-group");
        let d = describe_e2ee_message(&m);
        assert_eq!(
            d,
            serde_json::json!({
                "id": "123", "chatKind": "c", "contentType": 0, "e2eeVersion": "2",
                "senderKeyId": 2, "receiverKeyId": 257, "chunkLengths": [16, 40, 12, 4, 4]
            })
        );
    }

    #[tokio::test]
    async fn non_e2ee_message_passes_through() {
        let host = FakeHost::new();
        let state = E2eeState::default();
        let m = tv!({"from": "u-them", "to": "u-self", "text": "plain", "contentType": "NONE"});
        assert_eq!(
            E2ee::new(&host, &state)
                .decrypt_message(m.clone())
                .await
                .unwrap(),
            m
        );
    }
}
