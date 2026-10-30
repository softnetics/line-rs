//! Request gate: random per-process secret plus Host/Origin checks, so other
//! local web pages (CSRF, DNS rebinding) cannot drive the dashboard.

use rand::RngCore;

/// Header the page sends the secret in (`EventSource` uses `?k=` instead).
pub const SECRET_HEADER: &str = "x-test-ui-key";

/// 32 random bytes, hex encoded.
pub fn new_secret() -> String {
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Constant-time string comparison.
pub fn secret_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `Host` must be `127.0.0.1` or `localhost`, with no port or our port.
pub fn host_ok(host: Option<&str>, port: u16) -> bool {
    let Some(host) = host else { return false };
    let (name, p) = match host.rsplit_once(':') {
        Some((n, p)) => (n, Some(p)),
        None => (host, None),
    };
    let name_ok = name.eq_ignore_ascii_case("127.0.0.1") || name.eq_ignore_ascii_case("localhost");
    let port_ok = match p {
        None => true,
        Some(p) => p.parse::<u16>().ok() == Some(port),
    };
    name_ok && port_ok
}

/// When present, `Origin` must be this server.
pub fn origin_ok(origin: Option<&str>, port: u16) -> bool {
    let Some(origin) = origin else { return true };
    let Some(rest) = origin.strip_prefix("http://") else { return false };
    host_ok(Some(rest), port)
}

/// Extract `k=` from a raw query string.
pub fn query_secret(query: Option<&str>) -> Option<&str> {
    query?.split('&').find_map(|kv| kv.strip_prefix("k="))
}

/// Full check for one request.
pub fn authorized(
    secret: &str,
    port: u16,
    host: Option<&str>,
    origin: Option<&str>,
    header_key: Option<&str>,
    query: Option<&str>,
) -> bool {
    if !host_ok(host, port) || !origin_ok(origin, port) {
        return false;
    }
    let given = header_key.or_else(|| query_secret(query));
    given.is_some_and(|g| secret_eq(g, secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "abc123";

    #[test]
    fn secret_is_random_hex() {
        let a = new_secret();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, new_secret());
    }

    #[test]
    fn hosts() {
        assert!(host_ok(Some("127.0.0.1:7878"), 7878));
        assert!(host_ok(Some("localhost:7878"), 7878));
        assert!(host_ok(Some("LOCALHOST"), 7878));
        assert!(!host_ok(Some("127.0.0.1:9999"), 7878));
        assert!(!host_ok(Some("evil.example:7878"), 7878));
        assert!(!host_ok(Some("127.0.0.1.evil.example:7878"), 7878));
        assert!(!host_ok(None, 7878));
    }

    #[test]
    fn origins() {
        assert!(origin_ok(None, 7878));
        assert!(origin_ok(Some("http://127.0.0.1:7878"), 7878));
        assert!(!origin_ok(Some("https://127.0.0.1:7878"), 7878));
        assert!(!origin_ok(Some("http://evil.example"), 7878));
        assert!(!origin_ok(Some("null"), 7878));
    }

    #[test]
    fn secret_via_header_or_query() {
        let h = Some("127.0.0.1:7878");
        assert!(authorized(S, 7878, h, None, Some(S), None));
        assert!(authorized(S, 7878, h, None, None, Some("x=1&k=abc123")));
        assert!(!authorized(S, 7878, h, None, None, None));
        assert!(!authorized(S, 7878, h, None, Some("abc124"), None));
        assert!(!authorized(S, 7878, h, None, None, Some("k=abc12")));
        // right secret, wrong host (DNS rebinding)
        assert!(!authorized(S, 7878, Some("rebind.example:7878"), None, Some(S), None));
        // right secret, foreign origin
        assert!(!authorized(S, 7878, h, Some("http://evil.example"), Some(S), None));
    }
}
