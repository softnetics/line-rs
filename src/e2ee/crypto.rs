//! Letter Sealing primitives: curve25519 ECDH, the SHA-256 based key/IV
//! derivation, AES-CBC/ECB/GCM wrappers, message AAD, login key chains and
//! the E2EE-next file encryption ("key material").
//!
//! Everything here is pure and synchronous; key lookup lives in `keys.rs`.

use aes::cipher::block_padding::{NoPadding, Pkcs7};
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyInit, KeyIvInit, StreamCipher};
use aes_gcm::aead::{Aead, Payload};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::thrift::{read_struct, Protocol, TValue};

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;
/// WebCrypto `AES-CTR` with `length: 32` increments only the low 32 bits.
type Aes256Ctr32 = ctr::Ctr32BE<aes::Aes256>;

fn no_e2ee_key(message: &str) -> Error {
    Error::internal("NoE2EEKey", message)
}

pub fn sha256(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

/// SHA-256 over the concatenation of `parts` (linejs `getSHA256Sum`).
pub fn sha256_sum(parts: &[&[u8]]) -> Vec<u8> {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().to_vec()
}

/// Fold a buffer in half by XOR-ing its two halves.
pub fn xor(buf: &[u8]) -> Vec<u8> {
    let half = buf.len() / 2;
    (0..half).map(|i| buf[i] ^ buf[half + i]).collect()
}

fn key32(bytes: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 32];
    let n = bytes.len().min(32);
    k[..n].copy_from_slice(&bytes[..n]);
    k
}

/// X25519 shared secret on raw 32-byte keys. Both sides are clamped/masked
/// exactly like curve25519-js `sharedKey`.
pub fn generate_shared_secret(private_key: &[u8], public_key: &[u8]) -> Vec<u8> {
    x25519_dalek::x25519(key32(private_key), key32(public_key)).to_vec()
}

/// The curve25519 public key for `private_key` (`nacl.scalarMult.base`).
pub fn public_key_of(private_key: &[u8]) -> Vec<u8> {
    x25519_dalek::x25519(key32(private_key), x25519_dalek::X25519_BASEPOINT_BYTES).to_vec()
}

/// A fresh `(private, public)` key pair (`nacl.box.keyPair`).
pub fn generate_key_pair() -> (Vec<u8>, Vec<u8>) {
    let private = random_bytes(32);
    let public = public_key_of(&private);
    (private, public)
}

/// Whether `public_key` is the curve25519 public key of `private_key`.
pub fn verify_key_pair(private_key: &[u8], public_key: &[u8]) -> bool {
    private_key.len() == 32 && public_key_of(private_key) == public_key
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut b);
    b
}

/// AES key and IV for a shared secret: `sha256(secret ‖ salt ‖ "Key")` and
/// the folded `sha256(secret ‖ salt ‖ "IV")`. `salt` is empty for key
/// wrapping and the message salt for v1 messages.
pub fn derive_key_iv(shared_secret: &[u8], salt: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let key = sha256_sum(&[shared_secret, salt, b"Key"]);
    let iv = xor(&sha256_sum(&[shared_secret, salt, b"IV"]));
    (key, iv)
}

/// AES-ECB without padding. The key must be 16, 24 or 32 bytes; a trailing
/// partial block is zero-filled.
///
/// # Panics
/// On any other key length.
pub fn encrypt_aes_ecb(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut buf = data.to_vec();
    buf.resize(data.len().div_ceil(16) * 16, 0);
    let len = buf.len();
    macro_rules! run {
        ($aes:ty) => {
            ecb::Encryptor::<$aes>::new_from_slice(key)
                .expect("key length checked")
                .encrypt_padded_mut::<NoPadding>(&mut buf, len)
                .expect("block aligned")
                .to_vec()
        };
    }
    match key.len() {
        16 => run!(aes::Aes128),
        24 => run!(aes::Aes192),
        32 => run!(aes::Aes256),
        n => panic!("encrypt_aes_ecb: invalid AES key length {n}"),
    }
}

/// AES-256-CBC with PKCS#7 padding.
pub fn encrypt_aes_cbc(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let enc = Aes256CbcEnc::new_from_slices(key, iv).map_err(|e| Error::Crypto(format!("aes-256-cbc: {e}")))?;
    Ok(enc.encrypt_padded_vec_mut::<Pkcs7>(data))
}

/// AES-256-CBC decryption; `padded` strips PKCS#7 padding.
pub fn decrypt_aes_cbc(key: &[u8], iv: &[u8], data: &[u8], padded: bool) -> Result<Vec<u8>> {
    let dec = Aes256CbcDec::new_from_slices(key, iv).map_err(|e| Error::Crypto(format!("aes-256-cbc: {e}")))?;
    let res = if padded {
        dec.decrypt_padded_vec_mut::<Pkcs7>(data)
    } else {
        dec.decrypt_padded_vec_mut::<NoPadding>(data)
    };
    res.map_err(|_| Error::Crypto("aes-256-cbc: bad decrypt".into()))
}

/// AES-256-GCM; returns `ciphertext ‖ tag`.
pub fn encrypt_aes_gcm(key: &[u8], nonce: &[u8], data: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(key).map_err(|e| Error::Crypto(format!("aes-256-gcm: {e}")))?;
    if nonce.len() != 12 {
        return Err(Error::Crypto(format!("aes-256-gcm: nonce must be 12 bytes, got {}", nonce.len())));
    }
    cipher
        .encrypt(aes_gcm::Nonce::from_slice(nonce), Payload { msg: data, aad })
        .map_err(|_| Error::Crypto("aes-256-gcm: encryption failed".into()))
}

/// AES-256-GCM over `ciphertext ‖ tag`.
pub fn decrypt_aes_gcm(key: &[u8], nonce: &[u8], data: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(key).map_err(|e| Error::Crypto(format!("aes-256-gcm: {e}")))?;
    if nonce.len() != 12 || data.len() < 16 {
        return Err(Error::Crypto("aes-256-gcm: malformed nonce or ciphertext".into()));
    }
    cipher
        .decrypt(aes_gcm::Nonce::from_slice(nonce), Payload { msg: data, aad })
        .map_err(|_| Error::Crypto("aes-256-gcm: unable to authenticate data".into()))
}

/// Big-endian `i32` (linejs `getIntBytes`).
pub fn int_bytes(i: i64) -> [u8; 4] {
    (i as i32).to_be_bytes()
}

/// Big-endian unsigned integer of any length (linejs `byte2int`).
pub fn byte2int(b: &[u8]) -> i64 {
    b.iter().fold(0i64, |acc, &x| acc.wrapping_mul(256).wrapping_add(x as i64))
}

/// Message AAD: `to ‖ from ‖ senderKeyId ‖ receiverKeyId ‖ specVersion ‖ contentType`.
pub fn generate_aad(
    to: &str,
    from: &str,
    sender_key_id: i64,
    receiver_key_id: i64,
    spec_version: i64,
    content_type: i64,
) -> Vec<u8> {
    let mut aad = Vec::with_capacity(to.len() + from.len() + 16);
    aad.extend_from_slice(to.as_bytes());
    aad.extend_from_slice(from.as_bytes());
    for i in [sender_key_id, receiver_key_id, spec_version, content_type] {
        aad.extend_from_slice(&int_bytes(i));
    }
    aad
}

/// One generation of the account key, as found in a login key chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChainEntry {
    pub key_id: i64,
    pub private_key: Vec<u8>,
    pub public_key: Vec<u8>,
}

/// Decrypt and validate every entry of a login key chain (a compact-thrift
/// struct whose field 1 is a list of `{2: keyId, 4: pubKey, 5: privKey}`).
pub fn decrypt_key_chain_entries(
    public_key: &[u8],
    private_key: &[u8],
    encrypted_key_chain: &[u8],
) -> Result<Vec<KeyChainEntry>> {
    let shared = generate_shared_secret(private_key, public_key);
    let (key, iv) = derive_key_iv(&shared, &[]);
    // linejs decrypts without unpadding; the thrift reader stops at STOP.
    let plain = decrypt_aes_cbc(&key, &iv, encrypted_key_chain, false)?;
    let malformed = || no_e2ee_key("Login keychain is empty or malformed");
    let chain = read_struct(&plain, Protocol::Compact).map_err(|_| malformed())?;
    let entries = match chain.field(1) {
        Some(TValue::List(_, items) | TValue::Set(_, items)) if !items.is_empty() => items,
        _ => return Err(malformed()),
    };
    let mut seen = std::collections::HashSet::new();
    entries
        .iter()
        .map(|entry| {
            let key_id = match entry.field(2) {
                Some(v @ (TValue::Byte(_) | TValue::I16(_) | TValue::I32(_) | TValue::I64(_))) => v.as_i64().unwrap(),
                _ => return Err(no_e2ee_key("Login keychain contains an invalid or duplicate keyId")),
            };
            if !seen.insert(key_id) {
                return Err(no_e2ee_key("Login keychain contains an invalid or duplicate keyId"));
            }
            let bytes = |fid| match entry.field(fid) {
                Some(TValue::Binary(b)) => b.clone(),
                _ => Vec::new(),
            };
            let (public_key, private_key) = (bytes(4), bytes(5));
            if private_key.len() != 32 || public_key.len() != 32 || !verify_key_pair(&private_key, &public_key) {
                return Err(no_e2ee_key("Login keychain contains an invalid key pair"));
            }
            Ok(KeyChainEntry { key_id, private_key, public_key })
        })
        .collect()
}

/// Decrypt a login key chain and pick `(private, public)` for `key_id`.
/// Without a key id the chain must hold exactly one entry.
pub fn decrypt_key_chain(
    public_key: &[u8],
    private_key: &[u8],
    encrypted_key_chain: &[u8],
    key_id: Option<i64>,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut keys = decrypt_key_chain_entries(public_key, private_key, encrypted_key_chain)?;
    let idx = match key_id {
        None if keys.len() == 1 => Some(0),
        None => None,
        Some(id) => keys.iter().position(|k| k.key_id == id),
    };
    let entry = idx
        .map(|i| keys.swap_remove(i))
        .ok_or_else(|| no_e2ee_key("A matching keyId is required for the login keychain"))?;
    Ok((entry.private_key, entry.public_key))
}

/// The device secret answered to `confirmE2EELogin`:
/// `AES-ECB(sha256(shared ‖ "Key"), xor(sha256(encryptedKeyChain)))`.
pub fn encrypt_device_secret(public_key: &[u8], private_key: &[u8], encrypted_key_chain: &[u8]) -> Vec<u8> {
    let shared = generate_shared_secret(private_key, public_key);
    let key = sha256_sum(&[&shared, b"Key"]);
    encrypt_aes_ecb(&key, &xor(&sha256(encrypted_key_chain)))
}

// ---------------------------------------------------------------------------
// E2EE next: file encryption by key material
// ---------------------------------------------------------------------------

/// Keys derived from a file's key material.
pub struct DerivedKeys {
    pub enc_key: Vec<u8>,
    pub mac_key: Vec<u8>,
    /// 12 derived bytes followed by a zero 32-bit block counter.
    pub nonce: Vec<u8>,
}

/// HKDF-SHA256(keyMaterial, salt = "", info = "FileEncryption", 76 bytes).
pub fn derive_key_material(key_material: &[u8]) -> DerivedKeys {
    let mut okm = [0u8; 76];
    hkdf::Hkdf::<Sha256>::new(Some(&[]), key_material)
        .expand(b"FileEncryption", &mut okm)
        .expect("76 bytes is a valid HKDF-SHA256 length");
    let mut nonce = okm[64..76].to_vec();
    nonce.extend_from_slice(&[0; 4]);
    DerivedKeys { enc_key: okm[..32].to_vec(), mac_key: okm[32..64].to_vec(), nonce }
}

/// AES-256-CTR (32-bit counter) with a 16-byte initial counter block.
pub fn aes_ctr(key: &[u8], nonce: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let mut c = Aes256Ctr32::new_from_slices(key, nonce).map_err(|e| Error::Crypto(format!("aes-256-ctr: {e}")))?;
    let mut buf = data.to_vec();
    c.apply_keystream(&mut buf);
    Ok(buf)
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Encrypt a file for E2EE media upload. A random 32-byte key material is
/// generated when none is given. Returns `(base64 key material,
/// ciphertext ‖ HMAC-SHA256)`.
pub fn encrypt_by_key_material(data: &[u8], key_material: Option<&[u8]>) -> (String, Vec<u8>) {
    let key_material = key_material.map(<[u8]>::to_vec).unwrap_or_else(|| random_bytes(32));
    let keys = derive_key_material(&key_material);
    let mut out = aes_ctr(&keys.enc_key, &keys.nonce, data).expect("derived key/nonce sizes are valid");
    let sign = hmac_sha256(&keys.mac_key, &out);
    out.extend_from_slice(&sign);
    (B64.encode(key_material), out)
}

/// Verify the trailing HMAC (spec v1 whole-file or v2 chunked, see
/// [`super::media`]) and decrypt a downloaded E2EE file.
pub fn decrypt_by_key_material(data: &[u8], key_material: &[u8]) -> Result<Vec<u8>> {
    super::media::decrypt_media(data, key_material).map(|(plain, _)| plain)
}

/// [`decrypt_by_key_material`] with the key material as base64 (as found in
/// message `contentMetadata`).
pub fn decrypt_by_key_material_b64(data: &[u8], key_material: &str) -> Result<Vec<u8>> {
    decrypt_by_key_material(data, &decode_base64(key_material))
}

// ---------------------------------------------------------------------------
// AES-GCM-SIV (secure QR login)
// ---------------------------------------------------------------------------

/// AES-GCM-SIV (RFC 8452) decryption of `ciphertext ‖ tag`; the key is 16 or
/// 32 bytes.
pub fn decrypt_aes_gcm_siv(key: &[u8], nonce: &[u8], data: &[u8], aad: Option<&[u8]>) -> Result<Vec<u8>> {
    use aes_gcm_siv::{Aes128GcmSiv, Aes256GcmSiv, Nonce};
    if nonce.len() != 12 {
        return Err(Error::Crypto(format!("aes-gcm-siv: nonce must be 12 bytes, got {}", nonce.len())));
    }
    let payload = Payload { msg: data, aad: aad.unwrap_or_default() };
    let nonce = Nonce::from_slice(nonce);
    let res = match key.len() {
        16 => Aes128GcmSiv::new_from_slice(key).map(|c| c.decrypt(nonce, payload)),
        32 => Aes256GcmSiv::new_from_slice(key).map(|c| c.decrypt(nonce, payload)),
        n => return Err(Error::Crypto(format!("aes-gcm-siv: invalid key length {n}"))),
    };
    res.map_err(|e| Error::Crypto(format!("aes-gcm-siv: {e}")))?
        .map_err(|_| Error::Crypto("aes-gcm-siv: unable to authenticate data".into()))
}

/// Decrypt the `<12-byte nonce><ciphertext ‖ tag>` QR identifier of the
/// secure-login handshake, keyed by the X25519 shared secret.
pub fn decrypt_encrypted_qr_identifier(encrypted: &[u8], private_key: &[u8], public_key: &[u8]) -> Result<Vec<u8>> {
    if encrypted.len() < 12 {
        return Err(Error::Crypto("encrypted QR identifier is too short".into()));
    }
    let shared = generate_shared_secret(private_key, public_key);
    let (nonce, data) = encrypted.split_at(12);
    decrypt_aes_gcm_siv(&shared, nonce, data, None)
}

/// Lenient base64 decoding in the spirit of Node's `Buffer.from(s, "base64")`:
/// accepts the URL-safe alphabet, missing padding and embedded whitespace.
pub fn decode_base64(s: &str) -> Vec<u8> {
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    const LENIENT: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_end_matches('=');
    LENIENT.decode(cleaned).unwrap_or_default()
}

/// `encodeURIComponent` for base64 text.
pub(crate) fn uri_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::thrift::write_struct;

    #[test]
    fn aes_gcm_siv_rfc8452_vector() {
        // RFC 8452 §C.2: AES-256-GCM-SIV with a one-byte AAD.
        let key = hex::decode("0100000000000000000000000000000000000000000000000000000000000000").unwrap();
        let nonce = hex::decode("030000000000000000000000").unwrap();
        let ct = hex::decode("1de22967237a813291213f267e3b452f02d01ae33e4ec854").unwrap();
        let pt = decrypt_aes_gcm_siv(&key, &nonce, &ct, Some(&[1])).unwrap();
        assert_eq!(hex::encode(pt), "0200000000000000");
        assert!(decrypt_aes_gcm_siv(&key, &nonce, &ct, None).is_err());
    }

    #[test]
    fn key_material_round_trip_and_hmac() {
        let raw = b"secret media payload";
        let (km, enc) = encrypt_by_key_material(raw, None);
        assert_eq!(enc.len(), raw.len() + 32);
        assert_eq!(decrypt_by_key_material_b64(&enc, &km).unwrap(), raw);

        let mut tampered = enc.clone();
        tampered[0] ^= 1;
        let err = decrypt_by_key_material_b64(&tampered, &km).unwrap_err();
        assert!(err.to_string().contains("HMAC verification failed"));

        let err = decrypt_by_key_material(&enc, &[7; 32]).unwrap_err();
        assert!(err.to_string().contains("HMAC verification failed"));

        let err = decrypt_by_key_material(&[0; 10], &[0; 32]).unwrap_err();
        assert!(err.to_string().contains("too short"));
    }

    #[test]
    fn key_material_matches_webcrypto_layout() {
        // Fixed key material: the ciphertext body is plain CTR under the
        // HKDF-derived key with the 12-byte nonce + zero counter.
        let km = [3u8; 32];
        let (_, enc) = encrypt_by_key_material(b"abc", Some(&km));
        let keys = derive_key_material(&km);
        assert_eq!(keys.nonce.len(), 16);
        assert_eq!(&keys.nonce[12..], &[0; 4]);
        assert_eq!(aes_ctr(&keys.enc_key, &keys.nonce, &enc[..3]).unwrap(), b"abc");
        assert_eq!(&enc[3..], hmac_sha256(&keys.mac_key, &enc[..3]).as_slice());
    }

    #[test]
    fn shared_secret_is_symmetric() {
        let (a, ap) = generate_key_pair();
        let (b, bp) = generate_key_pair();
        assert_eq!(generate_shared_secret(&a, &bp), generate_shared_secret(&b, &ap));
        assert!(verify_key_pair(&a, &ap));
        assert!(!verify_key_pair(&a, &bp));
    }

    #[test]
    fn aad_and_int_helpers() {
        let aad = generate_aad("t", "f", 1, 258, 2, 15);
        assert_eq!(aad, [b't', b'f', 0, 0, 0, 1, 0, 0, 1, 2, 0, 0, 0, 2, 0, 0, 0, 15]);
        assert_eq!(byte2int(&int_bytes(123456)), 123456);
        assert_eq!(xor(&[1, 2, 3, 4]), [2, 6]);
        assert_eq!(uri_encode("a+b/c="), "a%2Bb%2Fc%3D");
        assert_eq!(decode_base64("AQID"), [1, 2, 3]);
        assert_eq!(decode_base64("AQI"), [1, 2]);
    }

    /// Build an encrypted login key chain like the server does.
    pub(crate) fn key_chain_fixture(ids: &[i64], corrupt: bool) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<KeyChainEntry>) {
        let keys: Vec<KeyChainEntry> = ids
            .iter()
            .enumerate()
            .map(|(i, &key_id)| {
                let private_key = vec![i as u8 + 1; 32];
                KeyChainEntry { key_id, public_key: public_key_of(&private_key), private_key }
            })
            .collect();
        let entries = keys
            .iter()
            .enumerate()
            .map(|(i, k)| {
                TValue::Struct(vec![
                    (1, TValue::I32(1)),
                    (2, TValue::I32(k.key_id as i32)),
                    (4, TValue::Binary(if corrupt && i == 0 { vec![0; 32] } else { k.public_key.clone() })),
                    (5, TValue::Binary(k.private_key.clone())),
                ])
            })
            .collect();
        let chain = [(1, TValue::List(crate::thrift::TType::Struct, entries))];
        let plain = write_struct(&chain, Protocol::Compact).unwrap();
        let secret = vec![99u8; 32];
        let public = public_key_of(&[100u8; 32]);
        let (key, iv) = derive_key_iv(&generate_shared_secret(&secret, &public), &[]);
        let encrypted = encrypt_aes_cbc(&key, &iv, &plain).unwrap();
        (secret, public, encrypted, keys)
    }

    #[test]
    fn decrypt_key_chain_by_id_and_ambiguity() {
        let (secret, public, encrypted, keys) = key_chain_fixture(&[10, 20, 30, 40, 50, 60, 70, 80], false);
        let (priv_key, pub_key) = decrypt_key_chain(&public, &secret, &encrypted, Some(80)).unwrap();
        assert_eq!(priv_key, keys[7].private_key);
        assert_eq!(pub_key, keys[7].public_key);
        assert!(decrypt_key_chain(&public, &secret, &encrypted, None).is_err());

        let (secret, public, encrypted, keys) = key_chain_fixture(&[80], false);
        assert_eq!(decrypt_key_chain(&public, &secret, &encrypted, None).unwrap().0, keys[0].private_key);
    }

    #[test]
    fn key_chain_validation() {
        let (secret, public, encrypted, _) = key_chain_fixture(&[10, 80], true);
        let err = decrypt_key_chain_entries(&public, &secret, &encrypted).unwrap_err();
        assert!(err.to_string().contains("invalid key pair"));
        let (secret, public, encrypted, _) = key_chain_fixture(&[80, 80], false);
        let err = decrypt_key_chain_entries(&public, &secret, &encrypted).unwrap_err();
        assert!(err.to_string().contains("duplicate keyId"));
    }

    #[test]
    fn device_secret_is_one_block() {
        let (secret, public, encrypted, _) = key_chain_fixture(&[1], false);
        let ds = encrypt_device_secret(&public, &secret, &encrypted);
        assert_eq!(ds.len(), 16);
        let key = sha256_sum(&[&generate_shared_secret(&secret, &public), b"Key"]);
        assert_eq!(ds, encrypt_aes_ecb(&key, &xor(&sha256(&encrypted))));
    }
}
