//! LEGY encrypted gateway (`gf.line.naver.jp/enc`).
//!
//! The inner request (path, access token and thrift body) is wrapped in a
//! small header block, AES-128-CBC encrypted with a per-session key and
//! authenticated with an xxhash32-based HMAC. The session key is delivered to
//! the server RSA-OAEP encrypted in the `x-lcs` header.

use std::collections::HashMap;
use std::sync::OnceLock;

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use base64::Engine;
use rand::RngCore;
use rsa::pkcs8::DecodePublicKey;

use crate::error::{Error, Result};

pub const LEGY_ENDPOINT: &str = "https://gf.line.naver.jp/enc";
const LEGY_LE: u8 = 7;
const LEGY_LAP: &str = "5";
const LEGY_LCS_PREFIX: &str = "0008";
const LEGY_IV: [u8; 16] = [78, 9, 72, 62, 56, 245, 255, 114, 128, 18, 123, 158, 251, 92, 45, 51];
const LINE_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAsMC6HAYeMq4R59e2yRw6
W1OWT2t9aepiAp4fbSCXzRj7A29BOAFAvKlzAub4oxN13Nt8dbcB+ICAufyDnN5N
d3+vXgDxEXZ/sx2/wuFbC3B3evSNKR4hKcs80suRs8aL6EeWi+bAU2oYIc78Bbqh
Nzx0WCzZSJbMBFw1VlsU/HQ/XdiUufopl5QSa0S246XXmwJmmXRO0v7bNvrxaNV0
cbviGkOvTlBt1+RerIFHMTw3SwLDnCOolTz3CuE5V2OrPZCmC0nlmPRzwUfxoxxs
/6qFdpZNoORH/s5mQenSyqPkmH8TBOlHJWPH3eN1k6aZIlK5S54mcUb/oNRRq9wD
1wIDAQAB
-----END PUBLIC KEY-----";

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// Per-client LEGY session (AES key + its RSA-wrapped form).
pub struct LegyEncryptedTransport {
    pub endpoint: String,
    aes_key: [u8; 16],
    x_lcs: OnceLock<String>,
}

/// Inner request to tunnel.
pub struct LegyRequest<'a> {
    /// Path plus query (`/S4`).
    pub path: &'a str,
    pub access_token: Option<&'a str>,
    pub body: &'a [u8],
}

/// Decrypted inner response.
#[derive(Debug)]
pub struct LegyResponse {
    /// HTTP status (from `x-lc` when it is not 200).
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl LegyEncryptedTransport {
    pub fn new(endpoint: Option<&str>) -> Self {
        let mut aes_key = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut aes_key);
        Self { endpoint: endpoint.unwrap_or(LEGY_ENDPOINT).to_string(), aes_key, x_lcs: OnceLock::new() }
    }

    /// Encrypt the inner request into the outer body.
    pub fn encrypt_request(&self, req: &LegyRequest<'_>) -> Vec<u8> {
        let mut inner = vec![("x-lpqs".to_string(), req.path.to_string())];
        if let Some(access) = req.access_token {
            inner.push(("x-lt".to_string(), super::auth_token::resolve_line_access_token(access)));
        }
        let mut plaintext = encode_legy_headers(&inner);
        plaintext.extend_from_slice(req.body);
        let payload = if LEGY_LE & 4 == 4 {
            let mut p = Vec::with_capacity(plaintext.len() + 1);
            p.push(LEGY_LE);
            p.extend_from_slice(&plaintext);
            p
        } else {
            plaintext
        };
        let mut encrypted = self.encrypt(&payload);
        if LEGY_LE & 2 == 2 {
            let mac = legy_hmac(&self.aes_key, &encrypted);
            encrypted.extend_from_slice(&mac);
        }
        encrypted
    }

    /// Decrypt an outer response body.
    pub fn decrypt_response(&self, outer_status: u16, body: &[u8]) -> Result<LegyResponse> {
        if body.is_empty() {
            return Ok(LegyResponse { status: outer_status, headers: HashMap::new(), body: Vec::new() });
        }
        let mut decrypted = self.decrypt(body)?;
        if LEGY_LE & 4 == 4 && !decrypted.is_empty() {
            decrypted.remove(0);
        }
        let (headers, data) = decode_legy_headers(&decrypted)?;
        let status = match headers.get("x-lc") {
            Some(lc) if lc != "200" => lc.parse().unwrap_or(outer_status),
            _ => outer_status,
        };
        Ok(LegyResponse { status, headers, body: data })
    }

    /// Outer request headers.
    pub fn outer_headers(&self, application: &str, user_agent: &str, method: &str) -> Vec<(&'static str, String)> {
        vec![
            ("x-line-application", application.to_string()),
            ("x-le", LEGY_LE.to_string()),
            ("x-lap", LEGY_LAP.to_string()),
            ("x-lpv", "1".to_string()),
            ("x-lcs", self.x_lcs().to_string()),
            ("user-agent", user_agent.to_string()),
            ("content-type", "application/x-thrift".to_string()),
            ("x-lal", "ja_JP".to_string()),
            ("x-lhm", method.to_string()),
            ("accept", "application/x-thrift".to_string()),
            ("accept-encoding", "gzip, deflate".to_string()),
            ("connection", "keep-alive".to_string()),
        ]
    }

    fn x_lcs(&self) -> &str {
        self.x_lcs.get_or_init(|| {
            let key = rsa::RsaPublicKey::from_public_key_pem(LINE_PUBLIC_KEY).expect("embedded LINE key is valid");
            let enc = key
                .encrypt(&mut rand::thread_rng(), rsa::Oaep::new::<sha1::Sha1>(), &self.aes_key)
                .expect("RSA-OAEP of 16 bytes cannot fail");
            format!("{LEGY_LCS_PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(enc))
        })
    }

    fn encrypt(&self, plaintext: &[u8]) -> Vec<u8> {
        Aes128CbcEnc::new(&self.aes_key.into(), &LEGY_IV.into()).encrypt_padded_vec_mut::<Pkcs7>(plaintext)
    }

    /// Mirrors linejs: responses are not block aligned (a trailer such as the
    /// 4-byte HMAC follows the ciphertext), so pad to a whole block, decrypt
    /// without padding and drop the final block.
    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let pad = 16 - ciphertext.len() % 16;
        let mut buf = ciphertext.to_vec();
        buf.resize(ciphertext.len() + pad, pad as u8);
        let mut dec = Aes128CbcDec::new(&self.aes_key.into(), &LEGY_IV.into())
            .decrypt_padded_mut::<cipher::block_padding::NoPadding>(&mut buf)
            .map_err(|e| Error::Crypto(e.to_string()))?
            .to_vec();
        dec.truncate(dec.len() - 16);
        Ok(pkcs7_unpad_lenient(dec))
    }
}

fn pkcs7_unpad_lenient(mut buf: Vec<u8>) -> Vec<u8> {
    if let Some(&size) = buf.last()
        && size > 0 && size <= 16 && (size as usize) <= buf.len() {
            buf.truncate(buf.len() - size as usize);
        }
    buf
}

pub fn encode_legy_headers(headers: &[(String, String)]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(headers.len() as u16).to_be_bytes());
    for (k, v) in headers {
        body.extend_from_slice(&(k.len() as u16).to_be_bytes());
        body.extend_from_slice(k.as_bytes());
        body.extend_from_slice(&(v.len() as u16).to_be_bytes());
        body.extend_from_slice(v.as_bytes());
    }
    let mut out = Vec::with_capacity(body.len() + 2);
    out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

pub fn decode_legy_headers(data: &[u8]) -> Result<(HashMap<String, String>, Vec<u8>)> {
    let err = || Error::Crypto("truncated LEGY header block".into());
    let mut off = 0usize;
    let u16_at = |off: &mut usize| -> Result<usize> {
        let b = data.get(*off..*off + 2).ok_or_else(err)?;
        *off += 2;
        Ok(u16::from_be_bytes([b[0], b[1]]) as usize)
    };
    let data_len = u16_at(&mut off)? + 2;
    let count = u16_at(&mut off)?;
    let mut headers = HashMap::new();
    for _ in 0..count {
        let kl = u16_at(&mut off)?;
        let k = data.get(off..off + kl).ok_or_else(err)?;
        off += kl;
        let vl = u16_at(&mut off)?;
        let v = data.get(off..off + vl).ok_or_else(err)?;
        off += vl;
        headers.insert(String::from_utf8_lossy(k).into_owned(), String::from_utf8_lossy(v).into_owned());
    }
    Ok((headers, data.get(data_len..).unwrap_or_default().to_vec()))
}

/// xxHash32.
pub fn xxhash32(data: &[u8], seed: u32) -> u32 {
    const P1: u32 = 0x9e3779b1;
    const P2: u32 = 0x85ebca77;
    const P3: u32 = 0xc2b2ae3d;
    const P4: u32 = 0x27d4eb2f;
    const P5: u32 = 0x165667b1;
    let round = |acc: u32, input: u32| acc.wrapping_add(input.wrapping_mul(P2)).rotate_left(13).wrapping_mul(P1);
    let rd = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
    let len = data.len();
    let mut off = 0;
    let mut h = if len >= 16 {
        let (mut v1, mut v2, mut v3, mut v4) =
            (seed.wrapping_add(P1).wrapping_add(P2), seed.wrapping_add(P2), seed, seed.wrapping_sub(P1));
        while off + 16 <= len {
            v1 = round(v1, rd(off));
            v2 = round(v2, rd(off + 4));
            v3 = round(v3, rd(off + 8));
            v4 = round(v4, rd(off + 12));
            off += 16;
        }
        v1.rotate_left(1).wrapping_add(v2.rotate_left(7)).wrapping_add(v3.rotate_left(12)).wrapping_add(v4.rotate_left(18))
    } else {
        seed.wrapping_add(P5)
    };
    h = h.wrapping_add(len as u32);
    while off + 4 <= len {
        h = h.wrapping_add(rd(off).wrapping_mul(P3)).rotate_left(17).wrapping_mul(P4);
        off += 4;
    }
    while off < len {
        h = h.wrapping_add((data[off] as u32).wrapping_mul(P5)).rotate_left(11).wrapping_mul(P1);
        off += 1;
    }
    h ^= h >> 15;
    h = h.wrapping_mul(P2);
    h ^= h >> 13;
    h = h.wrapping_mul(P3);
    h ^= h >> 16;
    h
}

fn legy_hmac(key: &[u8; 16], data: &[u8]) -> [u8; 4] {
    let mut ipad = Vec::with_capacity(16 + data.len());
    let mut opad = Vec::with_capacity(20);
    for &k in key {
        ipad.push(0x36 ^ k);
        opad.push(0x5c ^ k);
    }
    ipad.extend_from_slice(data);
    let inner = xxhash32(&ipad, 0).to_be_bytes();
    opad.extend_from_slice(&inner);
    xxhash32(&opad, 0).to_be_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xxhash_vectors() {
        assert_eq!(xxhash32(b"", 0), 0x02cc5d05);
        assert_eq!(xxhash32(b"a", 0), 0x550d7456);
        assert_eq!(xxhash32(b"Nobody inspects the spammish repetition", 0), 0xe2293b2f);
    }

    #[test]
    fn header_block_roundtrip() {
        let h = vec![("x-lpqs".to_string(), "/S4".to_string()), ("x-lt".to_string(), "tok".to_string())];
        let mut enc = encode_legy_headers(&h);
        enc.extend_from_slice(b"BODY");
        let (dec, body) = decode_legy_headers(&enc).unwrap();
        assert_eq!(dec["x-lpqs"], "/S4");
        assert_eq!(body, b"BODY");
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let t = LegyEncryptedTransport::new(None);
        let req = LegyRequest { path: "/S4", access_token: None, body: b"thrift" };
        let enc = t.encrypt_request(&req);
        // Server reply shape: [le byte] + header block + body, encrypted, no MAC.
        let mut inner = vec![LEGY_LE];
        inner.extend(encode_legy_headers(&[("x-lc".into(), "200".into())]));
        inner.extend_from_slice(b"reply");
        let reply = t.encrypt(&inner);
        let res = t.decrypt_response(200, &reply).unwrap();
        assert_eq!(res.body, b"reply");
        assert_eq!(res.status, 200);
        assert_eq!((enc.len() - 4) % 16, 0);
        assert!(t.x_lcs().starts_with("0008"));
    }

    #[test]
    fn decrypts_reply_with_mac_trailer() {
        let t = LegyEncryptedTransport::new(None);
        let mut inner = vec![LEGY_LE];
        inner.extend(encode_legy_headers(&[("x-lc".into(), "200".into())]));
        inner.extend_from_slice(b"reply");
        let mut reply = t.encrypt(&inner);
        let mac = legy_hmac(&t.aes_key, &reply);
        reply.extend_from_slice(&mac);
        assert_ne!(reply.len() % 16, 0);
        let res = t.decrypt_response(200, &reply).unwrap();
        assert_eq!(res.body, b"reply");
        assert_eq!(res.status, 200);
    }
}
