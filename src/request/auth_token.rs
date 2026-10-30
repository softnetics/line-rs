//! Auth token parsing and classification (JWT v3 tokens, primary tokens,
//! raw auth keys).

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AuthTokenCredential {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expire: Option<i64>,
}

/// Parse a token given as a plain string, two newline-separated JWTs
/// (access + refresh), or JSON (`{accessToken, refreshToken, expire}` or a
/// `tokenV3IssueResult` wrapper).
pub fn parse_auth_token_input(input: &str) -> Result<AuthTokenCredential> {
    let value = input.trim();
    if value.is_empty() {
        return Ok(AuthTokenCredential { access_token: String::new(), ..Default::default() });
    }
    let lines: Vec<&str> = value.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.len() >= 2 && is_jwt(lines[0]) && is_jwt(lines[1]) {
        return Ok(AuthTokenCredential {
            access_token: lines[0].to_string(),
            refresh_token: Some(lines[1].to_string()),
            expire: None,
        });
    }
    if value.starts_with('{') {
        let parsed: serde_json::Value = serde_json::from_str(value)?;
        let s = |v: &serde_json::Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let nested = &parsed["tokenV3IssueResult"];
        let access_token = s(&parsed["accessToken"])
            .or_else(|| s(&parsed["authToken"]))
            .or_else(|| s(&nested["accessToken"]))
            .ok_or_else(|| Error::internal("InvalidToken", "auth token JSON must contain accessToken"))?;
        let refresh_token = s(&parsed["refreshToken"]).or_else(|| s(&nested["refreshToken"]));
        let expire = parsed["expire"].as_i64();
        return Ok(AuthTokenCredential { access_token, refresh_token, expire });
    }
    Ok(AuthTokenCredential { access_token: value.to_string(), ..Default::default() })
}

/// The value sent as `x-lt` inside LEGY encrypted requests.
pub fn resolve_line_access_token(token: &str) -> String {
    let value = token.trim();
    let Some(colon) = value.find(':') else { return value.to_string() };
    let payload = &value[colon + 1..];
    if is_jwt(payload) || is_primary_access_token(value) {
        return if is_jwt(payload) { payload.to_string() } else { value.to_string() };
    }
    if looks_like_auth_key(value) {
        return create_primary_access_token(value, now_ms());
    }
    value.to_string()
}

pub fn should_use_legy_encrypted_access(token: Option<&str>) -> bool {
    let Some(token) = token else { return false };
    let value = token.trim();
    if is_jwt(value) {
        return true;
    }
    let Some(colon) = value.find(':') else { return false };
    let payload = &value[colon + 1..];
    is_jwt(payload) || is_primary_access_token(value) || looks_like_auth_key(value)
}

fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s.trim_end_matches('=').replace('+', "-").replace('/', "_")).ok()
}

pub fn is_jwt(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    let parse = |s: &str| -> Option<serde_json::Value> { serde_json::from_slice(&b64url_decode(s)?).ok() };
    let (Some(header), Some(payload)) = (parse(parts[0]), parse(parts[1])) else { return false };
    header["alg"].is_string()
        && ["ver", "scp", "exp", "iat", "sub", "iss", "aud"].iter().any(|k| payload.get(*k).is_some())
}

pub fn is_primary_access_token(value: &str) -> bool {
    let Some(colon) = value.find(':') else { return false };
    let first = value[colon + 1..].split('.').next().unwrap_or("");
    lenient_b64(first).map(|b| String::from_utf8_lossy(&b).starts_with("iat:")).unwrap_or(false)
}

/// Build a primary access token (`mid:base64("iat: ...\n").:hmac`) from an
/// auth key (`mid:base64key`).
pub fn create_primary_access_token(auth_key: &str, now_ms: i64) -> String {
    let Some(colon) = auth_key.find(':') else { return auth_key.to_string() };
    let mid = &auth_key[..colon];
    let key = lenient_b64(&auth_key[colon + 1..]).unwrap_or_default();
    let iat = format!("{}.", STANDARD.encode(format!("iat: {}\n", (now_ms / 1000) * 60)));
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(&key).expect("hmac accepts any key length");
    mac.update(iat.as_bytes());
    let digest = STANDARD.encode(mac.finalize().into_bytes());
    format!("{mid}:{iat}.{digest}")
}

fn looks_like_auth_key(value: &str) -> bool {
    let Some(colon) = value.find(':') else { return false };
    let mid = &value[..colon];
    let payload = &value[colon + 1..];
    let mut chars = mid.chars();
    let mid_ok = mid.len() == 33
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_hexdigit());
    mid_ok && lenient_b64(payload).is_some_and(|b| !b.is_empty())
}

/// Node's `Buffer.from(s, "base64")` is lenient; approximate that.
fn lenient_b64(s: &str) -> Option<Vec<u8>> {
    let cleaned: String = s.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '-' | '_')).collect();
    let std = cleaned.replace('-', "+").replace('_', "/");
    base64::engine::general_purpose::STANDARD_NO_PAD.decode(std.trim_end_matches('=')).ok().or_else(|| {
        // drop a dangling partial quantum like Node does
        let n = std.len() - std.len() % 4;
        base64::engine::general_purpose::STANDARD_NO_PAD.decode(&std[..n]).ok()
    })
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt() -> String {
        let h = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256"}"#);
        let p = URL_SAFE_NO_PAD.encode(br#"{"exp":1}"#);
        format!("{h}.{p}.sig")
    }

    #[test]
    fn parses_inputs() {
        let j = jwt();
        let c = parse_auth_token_input(&format!("{j}\n{j}")).unwrap();
        assert_eq!(c.refresh_token.as_deref(), Some(j.as_str()));
        let c = parse_auth_token_input(r#"{"tokenV3IssueResult":{"accessToken":"a","refreshToken":"r"},"expire":5}"#)
            .unwrap();
        assert_eq!(c, AuthTokenCredential { access_token: "a".into(), refresh_token: Some("r".into()), expire: Some(5) });
        assert_eq!(parse_auth_token_input(" tok ").unwrap().access_token, "tok");
    }

    #[test]
    fn classification() {
        let j = jwt();
        assert!(is_jwt(&j));
        assert!(should_use_legy_encrypted_access(Some(&j)));
        assert!(!should_use_legy_encrypted_access(Some("legacyopaque")));
        let key = format!("u{}:{}", "a".repeat(32), STANDARD.encode(b"secretkey"));
        assert!(looks_like_auth_key(&key));
        let primary = create_primary_access_token(&key, 1_700_000_000_000);
        assert!(is_primary_access_token(&primary));
        assert_eq!(resolve_line_access_token(&primary), primary);
        assert_eq!(resolve_line_access_token(&format!("u1:{j}")), j);
    }
}
