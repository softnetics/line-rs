use rsa::{BigUint, Pkcs1v15Encrypt, RsaPublicKey};

use crate::error::{Error, Result};

/// RSA PKCS#1 v1.5 encrypt `message` with the hex modulus/exponent from
/// `getRSAKeyInfo`; returns lowercase hex (node-bignumber compatible).
pub fn rsa_encrypt_credentials(message: &[u8], n_hex: &str, e_hex: &str) -> Result<String> {
    let parse = |h: &str| {
        BigUint::parse_bytes(h.as_bytes(), 16).ok_or_else(|| Error::Crypto(format!("invalid RSA hex value `{h}`")))
    };
    let key = RsaPublicKey::new(parse(n_hex)?, parse(e_hex)?).map_err(|e| Error::Crypto(e.to_string()))?;
    let enc = key
        .encrypt(&mut rand::thread_rng(), Pkcs1v15Encrypt, message)
        .map_err(|e| Error::Crypto(e.to_string()))?;
    Ok(hex::encode(enc))
}
