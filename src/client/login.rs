//! Async constructors that build a [`BaseClient`], log in and return a
//! [`Client`] (linejs `client/login.ts`).

use std::sync::Arc;

use super::Client;
use crate::base::{BaseClient, ClientEvent as BaseEvent, Config, LegyMode, ProxyConfig};
use crate::device::Device;
use crate::error::Result;
use crate::login::LoginOption;
use crate::storage::Storage;

/// Options for creating the underlying [`BaseClient`].
#[derive(Clone)]
pub struct InitOptions {
    pub device: Device,
    /// LINE app version to emulate.
    pub version: Option<String>,
    /// API host (default `legy.line-apps.com`).
    pub endpoint: Option<String>,
    /// Hosts to fail over to (default: `legy-backup.line-apps.com` when
    /// `endpoint` is unset; empty disables failover).
    pub backup_endpoints: Option<Vec<String>>,
    /// HTTP / SOCKS proxy for all traffic.
    pub proxy: Option<ProxyConfig>,
    /// Timeouts and token lifecycle settings.
    pub config: Option<Config>,
    /// Default: in-memory storage.
    pub storage: Option<Arc<dyn Storage>>,
    /// Custom HTTP client (proxy, TLS, ...).
    pub http: Option<reqwest::Client>,
    pub legy: LegyMode,
    pub legy_endpoint: Option<String>,
}

impl InitOptions {
    pub fn new(device: Device) -> Self {
        Self {
            device,
            version: None,
            endpoint: None,
            backup_endpoints: None,
            proxy: None,
            config: None,
            storage: None,
            http: None,
            legy: LegyMode::Auto,
            legy_endpoint: None,
        }
    }
    pub fn version(mut self, v: impl Into<String>) -> Self {
        self.version = Some(v.into());
        self
    }
    pub fn endpoint(mut self, host: impl Into<String>) -> Self {
        self.endpoint = Some(host.into());
        self
    }
    pub fn backup_endpoints<I: IntoIterator<Item = S>, S: Into<String>>(mut self, hosts: I) -> Self {
        self.backup_endpoints = Some(hosts.into_iter().map(Into::into).collect());
        self
    }
    /// Route all traffic through a proxy, e.g.
    /// `ProxyConfig::socks5("127.0.0.1:1080")`.
    pub fn proxy(mut self, proxy: ProxyConfig) -> Self {
        self.proxy = Some(proxy);
        self
    }
    pub fn config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }
    pub fn storage(mut self, s: impl Storage + 'static) -> Self {
        self.storage = Some(Arc::new(s));
        self
    }
    pub fn storage_arc(mut self, s: Arc<dyn Storage>) -> Self {
        self.storage = Some(s);
        self
    }
    pub fn http_client(mut self, c: reqwest::Client) -> Self {
        self.http = Some(c);
        self
    }
    pub fn legy(mut self, mode: LegyMode) -> Self {
        self.legy = mode;
        self
    }
    pub fn legy_endpoint(mut self, url: impl Into<String>) -> Self {
        self.legy_endpoint = Some(url.into());
        self
    }
}

/// Build a [`BaseClient`] from [`InitOptions`].
pub fn create_base_client(init: InitOptions) -> Result<BaseClient> {
    let mut b = BaseClient::builder(init.device).legy(init.legy);
    if let Some(v) = init.version {
        b = b.version(v);
    }
    if let Some(e) = init.endpoint {
        b = b.endpoint(e);
    }
    if let Some(hosts) = init.backup_endpoints {
        b = b.backup_endpoints(hosts);
    }
    if let Some(p) = init.proxy {
        b = b.proxy(p);
    }
    if let Some(c) = init.config {
        b = b.config(c);
    }
    if let Some(s) = init.storage {
        b = b.storage_arc(s);
    }
    if let Some(h) = init.http {
        b = b.http_client(h);
    }
    if let Some(e) = init.legy_endpoint {
        b = b.legy_endpoint(e);
    }
    b.build()
}

/// Run `login` while forwarding QR / PIN events to the callbacks.
async fn login_forwarding(
    base: &BaseClient,
    option: LoginOption,
    on_qr_url: Option<Box<dyn Fn(String) + Send + Sync>>,
    on_pincode: Box<dyn Fn(String) + Send + Sync>,
) -> Result<()> {
    let mut rx = base.subscribe();
    let forward = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(BaseEvent::QrCall(url)) => {
                    if let Some(f) = &on_qr_url {
                        f(url)
                    }
                }
                Ok(BaseEvent::PinCall(pin)) => on_pincode(pin),
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
    });
    let res = base.login(option).await;
    forward.abort();
    res
}

/// Log in with a QR code: `on_qr_url` receives the URL to render as a QR
/// code, `on_pincode` the PIN to enter on the phone.
pub async fn login_with_qr(
    init: InitOptions,
    on_qr_url: impl Fn(String) + Send + Sync + 'static,
    on_pincode: impl Fn(String) + Send + Sync + 'static,
) -> Result<Client> {
    let base = create_base_client(init)?;
    login_forwarding(&base, LoginOption::qr(), Some(Box::new(on_qr_url)), Box::new(on_pincode)).await?;
    Ok(Client::new(base))
}

/// Log in with email (or phone number, e.g. `+66812345678`) and password.
/// `pincode` (6 digits) defaults to
/// `114514`; `on_pincode` receives the PIN to confirm on the phone.
pub async fn login_with_password(
    init: InitOptions,
    email: &str,
    password: &str,
    pincode: Option<&str>,
    on_pincode: impl Fn(String) + Send + Sync + 'static,
) -> Result<Client> {
    let base = create_base_client(init)?;
    let mut option = LoginOption::password(email, password);
    if let LoginOption::Password { pincode: p, .. } = &mut option {
        *p = pincode.map(str::to_string);
    }
    login_forwarding(&base, option, None, Box::new(on_pincode)).await?;
    Ok(Client::new(base))
}

/// Log in with an existing token: a plain access token, `access\nrefresh`,
/// or JSON `{accessToken, refreshToken, expire}` (V3 credentials).
pub async fn login_with_auth_token(auth_token: &str, init: InitOptions) -> Result<Client> {
    let base = create_base_client(init)?;
    base.login(LoginOption::token(auth_token)).await?;
    Ok(Client::new(base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemoryStorage;
    use base64::Engine;

    #[test]
    fn passes_version_and_endpoint_to_base_client() {
        let base = create_base_client(InitOptions::new(Device::Android).version("26.6.2").endpoint("example.invalid"))
            .unwrap();
        assert_eq!(base.endpoint, "example.invalid");
        assert_eq!(base.device_details.application(), "ANDROID\t26.6.2\tAndroid OS\t16");
        // A custom endpoint has no implicit backup host.
        assert!(base.backup_endpoints.is_empty());
    }

    #[test]
    fn default_endpoint_has_backup_and_proxy_is_applied() {
        let base = create_base_client(
            InitOptions::new(Device::DesktopMac).proxy(ProxyConfig::socks5("127.0.0.1:1080").auth("u", "p")),
        )
        .unwrap();
        assert_eq!(base.current_endpoint(), "legy.line-apps.com");
        assert_eq!(base.backup_endpoints, vec!["legy-backup.line-apps.com".to_string()]);
        assert!(base.proxy.is_some());
        // A proxy cannot be combined with a caller-built HTTP client.
        let both = InitOptions::new(Device::DesktopMac)
            .proxy(ProxyConfig::http("127.0.0.1:3128"))
            .http_client(reqwest::Client::new());
        assert!(create_base_client(both).is_err());
    }

    fn jwt(payload: serde_json::Value) -> String {
        let b = |v: &serde_json::Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
        format!("{}.{}.sig", b(&serde_json::json!({"alg": "none", "typ": "JWT"})), b(&payload))
    }

    // Offline: the profile fetch fails (connection refused), but the V3
    // credentials must already be applied and the refresh token stored.
    #[tokio::test]
    async fn accepts_v3_credentials() {
        let storage = Arc::new(MemoryStorage::new());
        let access = jwt(serde_json::json!({"ver": "3.1", "scp": "LINE_AUTH"}));
        let refresh = jwt(serde_json::json!({"ver": "3.1", "scp": "LINE_AUTH_REFRESH"}));
        let init = InitOptions::new(Device::Android)
            .endpoint("127.0.0.1:9")
            .legy(LegyMode::Never)
            .storage_arc(storage.clone());
        let token = serde_json::json!({"accessToken": access, "refreshToken": refresh}).to_string();
        assert!(login_with_auth_token(&token, init).await.is_err());
        assert_eq!(storage.get_string("refreshToken").await.unwrap().as_deref(), Some(refresh.as_str()));
    }
}
