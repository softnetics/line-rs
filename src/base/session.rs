//! Session lifecycle and transport settings: kick-out reasons, proxy
//! configuration and the LEGY host list used for failover.

use std::fmt;

use crate::error::{Error, Result};

/// Primary LEGY host.
pub const LEGY_HOST: &str = "legy.line-apps.com";

/// Backup LEGY host (priority 2 in the bundled connection info).
///
/// Derived from static RE of LINE macOS 26.5.0; unverified against live
/// server. The embedded conn-info JSON lists `legy.line-apps.com:443`
/// (priority 1) and `legy-backup.line-apps.com:443` (priority 2), both
/// `http2` over TLS, with `"failover_threshold": "3"`.
pub const LEGY_BACKUP_HOST: &str = "legy-backup.line-apps.com";

/// Why the server ended this session.
///
/// The names after `NotAuthorizedDevice` are derived from static RE of LINE
/// macOS 26.5.0; unverified against live server. They sit next to the
/// `SessionManager` exception handling (`_exType`, `_exCode`) in the binary;
/// which response field carries them is not known, so detection only looks
/// at the exception `code` and `reason` (see [`LogoutReason::from_error`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LogoutReason {
    /// `TalkException.code = NOT_AUTHORIZED_DEVICE` (in the IDL).
    NotAuthorizedDevice,
    AuthenticationDivestedByOtherDevice,
    Expired,
    LogOut,
    Revoke,
    DeviceLose,
    IdentifyModified,
    V3TokenClientLoggedOut,
    DeletedAccount,
    /// Ended locally by [`BaseClient::kick_out`](super::BaseClient::kick_out)
    /// with a custom reason.
    Other(String),
}

impl LogoutReason {
    /// Wire name (`V3_TOKEN_CLIENT_LOGGED_OUT`, ...).
    pub fn as_str(&self) -> &str {
        match self {
            LogoutReason::NotAuthorizedDevice => "NOT_AUTHORIZED_DEVICE",
            LogoutReason::AuthenticationDivestedByOtherDevice => "AUTHENTICATION_DIVESTED_BY_OTHER_DEVICE",
            LogoutReason::Expired => "EXPIRED",
            LogoutReason::LogOut => "LOG_OUT",
            LogoutReason::Revoke => "REVOKE",
            LogoutReason::DeviceLose => "DEVICE_LOSE",
            LogoutReason::IdentifyModified => "IDENTIFY_MODIFIED",
            LogoutReason::V3TokenClientLoggedOut => "V3_TOKEN_CLIENT_LOGGED_OUT",
            LogoutReason::DeletedAccount => "DELETED_ACCOUNT",
            LogoutReason::Other(s) => s,
        }
    }

    /// Parse a wire name. Unknown names give `None`.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "NOT_AUTHORIZED_DEVICE" => LogoutReason::NotAuthorizedDevice,
            "AUTHENTICATION_DIVESTED_BY_OTHER_DEVICE" => LogoutReason::AuthenticationDivestedByOtherDevice,
            "EXPIRED" => LogoutReason::Expired,
            "LOG_OUT" => LogoutReason::LogOut,
            "REVOKE" => LogoutReason::Revoke,
            "DEVICE_LOSE" => LogoutReason::DeviceLose,
            "IDENTIFY_MODIFIED" => LogoutReason::IdentifyModified,
            "V3_TOKEN_CLIENT_LOGGED_OUT" => LogoutReason::V3TokenClientLoggedOut,
            "DELETED_ACCOUNT" => LogoutReason::DeletedAccount,
            _ => return None,
        })
    }

    /// Whether a server error means this session is gone.
    ///
    /// Only names that cannot be confused with unrelated failures are
    /// matched (`EXPIRED`, `LOG_OUT` and `REVOKE` are too generic: an
    /// expired invitation ticket must not end the session).
    pub fn from_error(e: &Error) -> Option<Self> {
        let Error::Request { data, .. } = e else { return None };
        [data["code"].as_str(), data["reason"].as_str()].into_iter().flatten().find_map(|name| {
            Self::from_name(name).filter(|r| {
                matches!(
                    r,
                    LogoutReason::NotAuthorizedDevice
                        | LogoutReason::AuthenticationDivestedByOtherDevice
                        | LogoutReason::V3TokenClientLoggedOut
                        | LogoutReason::DeviceLose
                        | LogoutReason::IdentifyModified
                        | LogoutReason::DeletedAccount
                )
            })
        })
    }
}

impl fmt::Display for LogoutReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// HTTP or SOCKS proxy for every request of a client (API, LEGY gateway,
/// push, OBS).
///
/// The LINE desktop app supports HTTP, SOCKS4 and SOCKS5 proxies with an
/// optional user/password; here the scheme of `url` selects the kind:
/// `http://`, `https://`, `socks4://`, `socks4a://`, `socks5://` or
/// `socks5h://` (DNS through the proxy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ProxyConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), username: None, password: None }
    }

    /// `socks5h://host:port` (hostnames resolved by the proxy).
    pub fn socks5(host_port: impl fmt::Display) -> Self {
        Self::new(format!("socks5h://{host_port}"))
    }

    /// `http://host:port`.
    pub fn http(host_port: impl fmt::Display) -> Self {
        Self::new(format!("http://{host_port}"))
    }

    pub fn auth(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }

    fn is_socks(&self) -> bool {
        self.url.to_ascii_lowercase().starts_with("socks")
    }

    /// The proxy URL with credentials embedded (how reqwest takes SOCKS
    /// credentials).
    fn url_with_credentials(&self) -> Result<String> {
        let mut u = url::Url::parse(&self.url).map_err(|e| Error::internal("ProxyConfig", e.to_string()))?;
        if let Some(user) = &self.username {
            u.set_username(user).map_err(|_| Error::internal("ProxyConfig", "cannot set proxy username"))?;
            u.set_password(self.password.as_deref())
                .map_err(|_| Error::internal("ProxyConfig", "cannot set proxy password"))?;
        }
        Ok(u.to_string())
    }

    /// Build the reqwest proxy.
    pub fn to_reqwest(&self) -> Result<reqwest::Proxy> {
        if self.is_socks() {
            return Ok(reqwest::Proxy::all(self.url_with_credentials()?)?);
        }
        let mut p = reqwest::Proxy::all(&self.url)?;
        if let Some(user) = &self.username {
            p = p.basic_auth(user, self.password.as_deref().unwrap_or(""));
        }
        Ok(p)
    }
}

/// Current Unix time in seconds.
pub(crate) fn now_epoch_sec() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn talk_err(code: &str, reason: &str) -> Error {
        Error::Request {
            method: "getProfile".into(),
            path: "/S4".into(),
            data: crate::tv!({"code": code, "reason": reason}),
        }
    }

    #[test]
    fn logout_reason_detection() {
        assert_eq!(
            LogoutReason::from_error(&talk_err("NOT_AUTHORIZED_DEVICE", "")),
            Some(LogoutReason::NotAuthorizedDevice)
        );
        assert_eq!(
            LogoutReason::from_error(&talk_err("AUTHENTICATION_FAILED", "V3_TOKEN_CLIENT_LOGGED_OUT")),
            Some(LogoutReason::V3TokenClientLoggedOut)
        );
        assert_eq!(
            LogoutReason::from_error(&talk_err("AUTHENTICATION_DIVESTED_BY_OTHER_DEVICE", "")),
            Some(LogoutReason::AuthenticationDivestedByOtherDevice)
        );
        // Generic names are not treated as a kick-out.
        assert_eq!(LogoutReason::from_error(&talk_err("NOT_FOUND", "EXPIRED")), None);
        assert_eq!(LogoutReason::from_error(&talk_err("MUST_REFRESH_V3_TOKEN", "")), None);
        assert_eq!(LogoutReason::from_error(&Error::internal("x", "y")), None);
        let numeric = Error::Request { method: "m".into(), path: "/S4".into(), data: crate::tv!({"code": 8}) };
        assert_eq!(LogoutReason::from_error(&numeric), None);
        assert_eq!(LogoutReason::from_name("DEVICE_LOSE").unwrap().to_string(), "DEVICE_LOSE");
    }

    #[test]
    fn proxy_config() {
        let p = ProxyConfig::socks5("127.0.0.1:1080").auth("user", "p@ss");
        assert_eq!(p.url_with_credentials().unwrap(), "socks5h://user:p%40ss@127.0.0.1:1080");
        assert!(p.to_reqwest().is_ok());
        assert!(ProxyConfig::http("proxy.local:3128").auth("a", "b").to_reqwest().is_ok());
        assert!(ProxyConfig::new("not a url").to_reqwest().is_err());
    }
}
