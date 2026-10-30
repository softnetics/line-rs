//! "E2EE Next" file encryption used by OBS media upload/download (uploads
//! use the v1 whole-file HMAC; downloads accept v1 and v2 chunked HMAC, see
//! [`crate::e2ee::media`]). The
//! primitives live in [`crate::e2ee::crypto`]; this module keeps the
//! OBS-facing [`EncryptedFile`] shape.

pub use crate::e2ee::crypto::{decrypt_by_key_material, decrypt_by_key_material_b64, derive_key_material};

/// Ciphertext together with the (base64) key material that opens it.
#[derive(Debug, Clone)]
pub struct EncryptedFile {
    pub key_material: String,
    pub encrypted_data: Vec<u8>,
}

/// Encrypt `raw` (random 32-byte key material unless given); the output is
/// `ciphertext ‖ HMAC-SHA256`.
pub fn encrypt_by_key_material(raw: &[u8], key_material: Option<&[u8]>) -> EncryptedFile {
    let (key_material, encrypted_data) = crate::e2ee::crypto::encrypt_by_key_material(raw, key_material);
    EncryptedFile { key_material, encrypted_data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_hmac() {
        let raw: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        let enc = encrypt_by_key_material(&raw, None);
        assert_eq!(enc.encrypted_data.len(), raw.len() + 32);
        assert_eq!(decrypt_by_key_material_b64(&enc.encrypted_data, &enc.key_material).unwrap(), raw);

        let mut tampered = enc.encrypted_data.clone();
        tampered[0] ^= 1;
        let err = decrypt_by_key_material_b64(&tampered, &enc.key_material).unwrap_err();
        assert!(err.to_string().contains("HMAC verification failed"));

        assert!(decrypt_by_key_material(&enc.encrypted_data, &[7u8; 32]).is_err());
        let err = decrypt_by_key_material(&[0u8; 10], &[0u8; 32]).unwrap_err();
        assert!(err.to_string().contains("too short (10 bytes)"));
    }

    #[test]
    fn downloads_accept_chunked_hmac_v2() {
        use crate::e2ee::media::{encrypt_media, MediaHmacVersion, MEDIA_CHUNK_SIZE};
        let raw: Vec<u8> = (0..MEDIA_CHUNK_SIZE + 9).map(|i| i as u8).collect();
        let enc = encrypt_media(&raw, None, MediaHmacVersion::V2);
        assert_eq!(decrypt_by_key_material_b64(&enc.data, &enc.key_material).unwrap(), raw);
    }

    #[test]
    fn same_key_material_is_deterministic() {
        let key = [1u8; 32];
        let a = encrypt_by_key_material(b"hello", Some(&key));
        let b = encrypt_by_key_material(b"hello", Some(&key));
        assert_eq!(a.encrypted_data, b.encrypted_data);
        assert_eq!(a.key_material, "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=");
    }

    #[test]
    fn matches_node_vector() {
        // node: hkdfSync + subtle AES-CTR(length 32) + HMAC, keyMaterial = 32×0x01, data = 40×0x07.
        let enc = encrypt_by_key_material(&[7u8; 40], Some(&[1u8; 32]));
        assert_eq!(
            hex::encode(enc.encrypted_data),
            "b2fbb80fb2b115e55fd1c44075de7d169f8ffb93980194527a41e4e99d7b8a41f80f4ed43145559f\
             3ec92c9db2365f64e1809b75cc0f99301c2500e88969330e10a105b59518d428"
        );
    }
}
