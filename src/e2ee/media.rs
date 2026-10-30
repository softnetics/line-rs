//! E2EE media integrity, spec v1 and v2 ("chunked HMAC").
//!
//! Both versions encrypt the file the same way (see
//! [`crypto::derive_key_material`]: HKDF "FileEncryption", AES-256-CTR) and
//! append a 32-byte HMAC-SHA256 under the derived MAC key. They differ in
//! what that HMAC covers:
//!
//! * **v1**: `HMAC(macKey, ciphertext)`.
//! * **v2**: the ciphertext is split into 128 KiB chunks (the last one may
//!   be shorter) and the HMAC covers the concatenated SHA-256 digests of the
//!   chunks: `HMAC(macKey, SHA256(c0) ‖ SHA256(c1) ‖ …)`. Each chunk can
//!   then be checked on its own against a separate *hash object*
//!   `SHA256(c0) ‖ SHA256(c1) ‖ … ‖ HMAC`, stored next to the media as
//!   `<oid>__ud-hash`, so a download can be validated while it streams.
//!
//! Derived from static RE of LINE macOS 26.5.0 (`CommLib::E2EEHMACHelper`,
//! `E2EEChunkedHMACValidator`, `E2EEChunkValidator`, the `FileEncryption`
//! encryptor and `FileTransManager`'s `__ud-hash` download); unverified
//! against live server.
//!
//! [`crypto::derive_key_material`]: super::crypto::derive_key_material

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use super::crypto::{aes_ctr, derive_key_material, random_bytes};
use crate::error::{Error, Result};

/// Size of a v2 ciphertext chunk (`0x20000` in the client).
pub const MEDIA_CHUNK_SIZE: usize = 128 * 1024;
/// Length of the trailing HMAC-SHA256 and of each chunk digest.
pub const MEDIA_HMAC_LEN: usize = 32;

type HmacSha256 = Hmac<Sha256>;

fn media_error(message: impl Into<String>) -> Error {
    Error::internal("E2EEMediaError", message)
}

fn hmac(mac_key: &[u8]) -> HmacSha256 {
    <HmacSha256 as Mac>::new_from_slice(mac_key).expect("HMAC accepts any key length")
}

/// Which integrity scheme protects an E2EE media file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaHmacVersion {
    /// HMAC over the whole ciphertext.
    #[default]
    V1,
    /// HMAC over the SHA-256 digests of 128 KiB ciphertext chunks.
    V2,
}

/// SHA-256 digest of every [`MEDIA_CHUNK_SIZE`] chunk of `ciphertext`.
pub fn chunk_hashes(ciphertext: &[u8]) -> Vec<[u8; 32]> {
    ciphertext.chunks(MEDIA_CHUNK_SIZE).map(|c| Sha256::digest(c).into()).collect()
}

/// The v2 trailer: `HMAC(macKey, SHA256(c0) ‖ SHA256(c1) ‖ …)`.
pub fn chunked_hmac(mac_key: &[u8], ciphertext: &[u8]) -> Vec<u8> {
    let mut mac = hmac(mac_key);
    for h in chunk_hashes(ciphertext) {
        mac.update(&h);
    }
    mac.finalize().into_bytes().to_vec()
}

/// An encrypted media file and, for v2, its `__ud-hash` object.
#[derive(Debug, Clone)]
pub struct EncryptedMedia {
    /// Base64 key material (the `keyMaterial` of the message payload).
    pub key_material: String,
    /// `ciphertext ‖ HMAC`.
    pub data: Vec<u8>,
    /// v2 only: `SHA256(c0) ‖ … ‖ HMAC` (the same HMAC as `data`'s trailer).
    pub hash_object: Option<Vec<u8>>,
}

/// Encrypt `raw` with `key_material` (random 32 bytes when `None`) and
/// protect it with the given HMAC scheme.
pub fn encrypt_media(raw: &[u8], key_material: Option<&[u8]>, version: MediaHmacVersion) -> EncryptedMedia {
    let key_material = key_material.map(<[u8]>::to_vec).unwrap_or_else(|| random_bytes(32));
    let keys = derive_key_material(&key_material);
    let mut data = aes_ctr(&keys.enc_key, &keys.nonce, raw).expect("derived key/nonce sizes are valid");
    let hash_object = match version {
        MediaHmacVersion::V1 => {
            let mut mac = hmac(&keys.mac_key);
            mac.update(&data);
            data.extend_from_slice(&mac.finalize().into_bytes());
            None
        }
        MediaHmacVersion::V2 => {
            let hashes = chunk_hashes(&data);
            let mut mac = hmac(&keys.mac_key);
            let mut object = Vec::with_capacity((hashes.len() + 1) * MEDIA_HMAC_LEN);
            for h in &hashes {
                mac.update(h);
                object.extend_from_slice(h);
            }
            let tag = mac.finalize().into_bytes();
            data.extend_from_slice(&tag);
            object.extend_from_slice(&tag);
            Some(object)
        }
    };
    EncryptedMedia { key_material: B64.encode(key_material), data, hash_object }
}

/// Check the trailing HMAC of `data` (`ciphertext ‖ HMAC`) under either
/// scheme and report which one matched. v1 is tried first; both are keyed,
/// so a forgery cannot pass as the other version.
pub fn verify_media(data: &[u8], mac_key: &[u8]) -> Result<MediaHmacVersion> {
    if data.len() < MEDIA_HMAC_LEN {
        return Err(media_error(format!("encrypted data too short ({} bytes) to contain HMAC", data.len())));
    }
    let (enc, tag) = data.split_at(data.len() - MEDIA_HMAC_LEN);
    let mut v1 = hmac(mac_key);
    v1.update(enc);
    if v1.verify_slice(tag).is_ok() {
        return Ok(MediaHmacVersion::V1);
    }
    let mut v2 = hmac(mac_key);
    for c in enc.chunks(MEDIA_CHUNK_SIZE) {
        v2.update(&Sha256::digest(c));
    }
    v2.verify_slice(tag)
        .map(|_| MediaHmacVersion::V2)
        .map_err(|_| media_error("HMAC verification failed: ciphertext was tampered with or keyMaterial is wrong"))
}

/// Verify (v1 or v2) and decrypt `ciphertext ‖ HMAC`.
pub fn decrypt_media(data: &[u8], key_material: &[u8]) -> Result<(Vec<u8>, MediaHmacVersion)> {
    let keys = derive_key_material(key_material);
    let version = verify_media(data, &keys.mac_key)?;
    let plain = aes_ctr(&keys.enc_key, &keys.nonce, &data[..data.len() - MEDIA_HMAC_LEN])?;
    Ok((plain, version))
}

/// Streaming v2 check of a downloaded file (`E2EEChunkedHMACValidator`):
/// feed the raw bytes in any split, then [`finish`](Self::finish). The last
/// 32 bytes seen are held back as the expected HMAC.
pub struct ChunkedHmacValidator {
    mac: HmacSha256,
    pending: Vec<u8>,
    tail: Vec<u8>,
}

impl ChunkedHmacValidator {
    pub fn new(mac_key: &[u8]) -> Self {
        Self { mac: hmac(mac_key), pending: Vec::new(), tail: Vec::new() }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.tail.extend_from_slice(data);
        if self.tail.len() > MEDIA_HMAC_LEN {
            let body = self.tail.len() - MEDIA_HMAC_LEN;
            self.pending.extend(self.tail.drain(..body));
        }
        while self.pending.len() >= MEDIA_CHUNK_SIZE {
            self.mac.update(&Sha256::digest(&self.pending[..MEDIA_CHUNK_SIZE]));
            self.pending.drain(..MEDIA_CHUNK_SIZE);
        }
    }

    pub fn finish(mut self) -> Result<()> {
        if self.tail.len() < MEDIA_HMAC_LEN {
            return Err(media_error("encrypted data too short to contain HMAC"));
        }
        if !self.pending.is_empty() {
            self.mac.update(&Sha256::digest(&self.pending));
        }
        self.mac.verify_slice(&self.tail).map_err(|_| media_error("chunked HMAC verification failed"))
    }
}

/// A validated `__ud-hash` object: the per-chunk digests of a v2 file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkHashes {
    hashes: Vec<[u8; 32]>,
    hmac: Vec<u8>,
}

impl ChunkHashes {
    /// Parse `SHA256(c0) ‖ … ‖ HMAC` and check its HMAC
    /// (`E2EEHMACHelper` over everything but the last 32 bytes).
    pub fn parse(object: &[u8], mac_key: &[u8]) -> Result<Self> {
        if object.len() < MEDIA_HMAC_LEN || !object.len().is_multiple_of(MEDIA_HMAC_LEN) {
            return Err(media_error(format!("invalid chunk hash object length {}", object.len())));
        }
        let (body, tag) = object.split_at(object.len() - MEDIA_HMAC_LEN);
        let mut mac = hmac(mac_key);
        mac.update(body);
        mac.verify_slice(tag).map_err(|_| media_error("chunk hash object HMAC verification failed"))?;
        let hashes = body.chunks(MEDIA_HMAC_LEN).map(|h| h.try_into().expect("exact 32-byte chunks")).collect();
        Ok(Self { hashes, hmac: tag.to_vec() })
    }

    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }

    /// The HMAC that also ends the media file.
    pub fn hmac(&self) -> &[u8] {
        &self.hmac
    }

    /// Whether ciphertext chunk `index` matches its recorded digest.
    pub fn check_chunk(&self, index: usize, chunk: &[u8]) -> bool {
        self.hashes.get(index).is_some_and(|h| Sha256::digest(chunk).as_slice() == h)
    }

    /// Validate a whole downloaded file (`ciphertext ‖ HMAC`) chunk by chunk.
    pub fn check_file(&self, data: &[u8]) -> Result<()> {
        if data.len() < MEDIA_HMAC_LEN || data[data.len() - MEDIA_HMAC_LEN..] != self.hmac[..] {
            return Err(media_error("media HMAC does not match the chunk hash object"));
        }
        let body = &data[..data.len() - MEDIA_HMAC_LEN];
        if body.chunks(MEDIA_CHUNK_SIZE).count() != self.hashes.len() {
            return Err(media_error("chunk count does not match the chunk hash object"));
        }
        match body.chunks(MEDIA_CHUNK_SIZE).enumerate().find(|(i, c)| !self.check_chunk(*i, c)) {
            Some((i, _)) => Err(media_error(format!("chunk {i} failed validation"))),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 31 + 7) as u8).collect()
    }

    #[test]
    fn v1_and_v2_round_trip_and_detect() {
        for n in [0, 1, 1000, MEDIA_CHUNK_SIZE - 1, MEDIA_CHUNK_SIZE, MEDIA_CHUNK_SIZE + 1, 3 * MEDIA_CHUNK_SIZE + 5] {
            let raw = sample(n);
            for version in [MediaHmacVersion::V1, MediaHmacVersion::V2] {
                let enc = encrypt_media(&raw, Some(&[9; 32]), version);
                assert_eq!(enc.data.len(), n + 32);
                let (plain, got) = decrypt_media(&enc.data, &[9; 32]).unwrap();
                assert_eq!(plain, raw);
                // Up to one chunk the two schemes only coincide on empty input.
                let expect = if n == 0 { MediaHmacVersion::V1 } else { version };
                assert_eq!(got, expect, "n={n}");
                assert_eq!(enc.hash_object.is_some(), version == MediaHmacVersion::V2);
            }
        }
    }

    #[test]
    fn v2_layout_vector() {
        // Body is the same CTR stream as v1; only the trailer differs.
        let km = [1u8; 32];
        let raw = sample(MEDIA_CHUNK_SIZE + 10);
        let v1 = encrypt_media(&raw, Some(&km), MediaHmacVersion::V1);
        let v2 = encrypt_media(&raw, Some(&km), MediaHmacVersion::V2);
        let body = &v2.data[..raw.len()];
        assert_eq!(body, &v1.data[..raw.len()]);
        let keys = derive_key_material(&km);
        let mut expect = hmac(&keys.mac_key);
        expect.update(&Sha256::digest(&body[..MEDIA_CHUNK_SIZE]));
        expect.update(&Sha256::digest(&body[MEDIA_CHUNK_SIZE..]));
        let tag = expect.finalize().into_bytes();
        assert_eq!(&v2.data[raw.len()..], tag.as_slice());
        assert_eq!(chunked_hmac(&keys.mac_key, body), tag.to_vec());

        let object = v2.hash_object.unwrap();
        assert_eq!(object.len(), 3 * 32);
        assert_eq!(&object[..32], Sha256::digest(&body[..MEDIA_CHUNK_SIZE]).as_slice());
        assert_eq!(&object[64..], tag.as_slice());
    }

    #[test]
    fn known_vector_small() {
        // keyMaterial = 32×0x01, data = 40×0x07; the CTR body matches the
        // node/WebCrypto vector in obs::crypto, the v2 trailer is one digest.
        let enc = encrypt_media(&[7u8; 40], Some(&[1u8; 32]), MediaHmacVersion::V2);
        assert_eq!(
            hex::encode(&enc.data[..40]),
            "b2fbb80fb2b115e55fd1c44075de7d169f8ffb93980194527a41e4e99d7b8a41f80f4ed43145559f"
        );
        let keys = derive_key_material(&[1u8; 32]);
        let mut mac = hmac(&keys.mac_key);
        mac.update(&Sha256::digest(&enc.data[..40]));
        assert_eq!(&enc.data[40..], mac.finalize().into_bytes().as_slice());
    }

    #[test]
    fn tamper_and_wrong_key_fail() {
        let raw = sample(2 * MEDIA_CHUNK_SIZE + 3);
        let enc = encrypt_media(&raw, Some(&[2; 32]), MediaHmacVersion::V2);
        let mut bad = enc.data.clone();
        bad[MEDIA_CHUNK_SIZE + 1] ^= 1;
        let err = decrypt_media(&bad, &[2; 32]).unwrap_err();
        assert!(err.to_string().contains("HMAC verification failed"));
        assert!(decrypt_media(&enc.data, &[3; 32]).is_err());
        assert!(decrypt_media(&[0; 5], &[2; 32]).unwrap_err().to_string().contains("too short"));
    }

    #[test]
    fn streaming_validator_any_split() {
        let raw = sample(2 * MEDIA_CHUNK_SIZE + 77);
        let km = [4u8; 32];
        let enc = encrypt_media(&raw, Some(&km), MediaHmacVersion::V2);
        let mac_key = derive_key_material(&km).mac_key;
        for step in [1usize, 31, 32, 33, 4096, MEDIA_CHUNK_SIZE, enc.data.len()] {
            let mut v = ChunkedHmacValidator::new(&mac_key);
            for part in enc.data.chunks(step.max(1)) {
                v.update(part);
            }
            v.finish().unwrap();
        }
        let mut bad = enc.data.clone();
        bad[10] ^= 0x80;
        let mut v = ChunkedHmacValidator::new(&mac_key);
        v.update(&bad);
        assert!(v.finish().is_err());
        let mut v = ChunkedHmacValidator::new(&mac_key);
        v.update(&[0; 10]);
        assert!(v.finish().is_err());
        // A v1 file is rejected by the chunked validator.
        let v1 = encrypt_media(&raw, Some(&km), MediaHmacVersion::V1);
        let mut v = ChunkedHmacValidator::new(&mac_key);
        v.update(&v1.data);
        assert!(v.finish().is_err());
    }

    #[test]
    fn chunk_hash_object_validation() {
        let raw = sample(3 * MEDIA_CHUNK_SIZE + 1);
        let km = [5u8; 32];
        let enc = encrypt_media(&raw, Some(&km), MediaHmacVersion::V2);
        let mac_key = derive_key_material(&km).mac_key;
        let object = enc.hash_object.clone().unwrap();
        let hashes = ChunkHashes::parse(&object, &mac_key).unwrap();
        assert_eq!(hashes.len(), 4);
        assert_eq!(hashes.hmac(), &enc.data[enc.data.len() - 32..]);
        assert!(hashes.check_chunk(0, &enc.data[..MEDIA_CHUNK_SIZE]));
        assert!(!hashes.check_chunk(1, &enc.data[..MEDIA_CHUNK_SIZE]));
        assert!(!hashes.check_chunk(9, &[]));
        hashes.check_file(&enc.data).unwrap();

        let mut bad = enc.data.clone();
        bad[2 * MEDIA_CHUNK_SIZE] ^= 1;
        assert!(hashes.check_file(&bad).unwrap_err().to_string().contains("chunk 2"));

        let mut bad_object = object.clone();
        bad_object[0] ^= 1;
        assert!(ChunkHashes::parse(&bad_object, &mac_key).is_err());
        assert!(ChunkHashes::parse(&object[..40], &mac_key).is_err());
        assert!(ChunkHashes::parse(&object, &[0; 32]).is_err());
    }
}
