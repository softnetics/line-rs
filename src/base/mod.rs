//! [`BaseClient`]: configuration, credentials, storage and events shared by
//! every service, plus the thrift request pipeline.

mod events;
mod session;

pub use events::ClientEvent;
pub use session::{LEGY_BACKUP_HOST, LEGY_HOST, LogoutReason, ProxyConfig};
pub(crate) use session::now_epoch_sec as session_now;

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use tokio::sync::{broadcast, Mutex};

use crate::device::{Device, DeviceDetails};
use crate::e2ee::E2eeState;
use crate::error::{Error, Result};
use crate::polling::SyncData;
use crate::request::legy::LegyEncryptedTransport;
use crate::storage::{MemoryStorage, Storage};
use crate::thrift::Value;

/// Whether requests go through the LEGY encrypted gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LegyMode {
    /// Encrypt talk-path requests when the token is a modern JWT/primary/auth
    /// key token; keep legacy opaque tokens on the plain endpoint.
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Request timeout (default 30s).
    pub timeout: Duration,
    /// Long-poll timeout (default 180s).
    pub long_timeout: Duration,
    /// Refresh a v3 access token before sending a request once the stored
    /// `expire` (issue time + `durationUntilRefreshInSec`) is within
    /// `refresh_margin` (default on).
    pub proactive_refresh: bool,
    /// How long before `expire` to refresh (default 60s).
    pub refresh_margin: Duration,
    /// Call `reportRefreshedAccessToken` after a refresh (best effort,
    /// default on).
    pub report_refreshed_token: bool,
    /// Adopt the token in an `X-Line-Next-Access` response header (default
    /// on).
    pub apply_next_access: bool,
    /// Recover from E2EE `sendMessage` errors the way the macOS client does
    /// (re-encrypt, recreate group key, plain fallback; see
    /// [`crate::e2ee::send_recovery`]). Off by default: linejs only retries
    /// once with encryption when `e2ee` was unset.
    pub e2ee_send_recovery: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            long_timeout: Duration::from_secs(180),
            // Off by default to match linejs, which only refreshes after
            // MUST_REFRESH_V3_TOKEN and never adopts X-Line-Next-Access.
            proactive_refresh: false,
            refresh_margin: Duration::from_secs(60),
            report_refreshed_token: false,
            apply_next_access: false,
            e2ee_send_recovery: false,
        }
    }
}

/// Builder for [`BaseClient`].
pub struct ClientBuilder {
    device: Device,
    version: Option<String>,
    endpoint: Option<String>,
    backup_endpoints: Option<Vec<String>>,
    proxy: Option<ProxyConfig>,
    storage: Option<Arc<dyn Storage>>,
    http: Option<reqwest::Client>,
    legy_mode: LegyMode,
    legy_endpoint: Option<String>,
    config: Config,
}

impl ClientBuilder {
    /// LINE app version to emulate.
    pub fn version(mut self, v: impl Into<String>) -> Self {
        self.version = Some(v.into());
        self
    }
    /// API host (default `legy.line-apps.com`).
    pub fn endpoint(mut self, host: impl Into<String>) -> Self {
        self.endpoint = Some(host.into());
        self
    }
    /// Hosts to fail over to when the API host cannot be reached. Default:
    /// `legy-backup.line-apps.com` with the default endpoint, none with a
    /// custom one. Pass an empty list to disable failover.
    pub fn backup_endpoints<I: IntoIterator<Item = S>, S: Into<String>>(mut self, hosts: I) -> Self {
        self.backup_endpoints = Some(hosts.into_iter().map(Into::into).collect());
        self
    }
    /// Route all traffic through an HTTP or SOCKS proxy. Cannot be combined
    /// with [`http_client`](Self::http_client) (configure the proxy on that
    /// client instead).
    pub fn proxy(mut self, proxy: ProxyConfig) -> Self {
        self.proxy = Some(proxy);
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
    /// Use a custom HTTP client (proxy, custom TLS, ...).
    pub fn http_client(mut self, c: reqwest::Client) -> Self {
        self.http = Some(c);
        self
    }
    pub fn legy(mut self, mode: LegyMode) -> Self {
        self.legy_mode = mode;
        self
    }
    pub fn legy_endpoint(mut self, url: impl Into<String>) -> Self {
        self.legy_endpoint = Some(url.into());
        self
    }
    pub fn config(mut self, c: Config) -> Self {
        self.config = c;
        self
    }

    pub fn build(self) -> Result<BaseClient> {
        // linejs sends everything over HTTP/1.1 (`createNodeFetch(false)`)
        // and only PUSH over HTTP/2; LEGY answers some REST paths (e.g.
        // `/kp`) differently over h2. A custom client is used for both.
        let (http, push_http) = match (self.http, &self.proxy) {
            (Some(_), Some(_)) => {
                return Err(Error::internal("ConfigError", "proxy cannot be combined with a custom http client"));
            }
            (Some(h), None) => (h.clone(), h),
            (None, proxy) => {
                let builder = || -> Result<reqwest::ClientBuilder> {
                    let mut b = reqwest::Client::builder().connect_timeout(self.config.timeout);
                    if let Some(p) = proxy {
                        b = b.proxy(p.to_reqwest()?);
                    }
                    Ok(b)
                };
                (builder()?.http1_only().build()?, builder()?.build()?)
            }
        };
        let backup_endpoints = match (self.backup_endpoints, &self.endpoint) {
            (Some(b), _) => b,
            (None, None) => vec![LEGY_BACKUP_HOST.to_string()],
            (None, Some(_)) => Vec::new(),
        };
        let (events, _) = broadcast::channel(256);
        Ok(BaseClient(Arc::new(Inner {
            device_details: self.device.details(self.version.as_deref()),
            device: self.device,
            endpoint: self.endpoint.unwrap_or_else(|| LEGY_HOST.into()),
            backup_endpoints,
            active_endpoint: AtomicUsize::new(0),
            proxy: self.proxy,
            storage: self.storage.unwrap_or_else(|| Arc::new(MemoryStorage::new())),
            http,
            push_http,
            legy_mode: self.legy_mode,
            legy_endpoint: self.legy_endpoint,
            config: self.config,
            auth_token: RwLock::new(None),
            profile: RwLock::new(None),
            events,
            reqseq: Mutex::new(None),
            legy_transport: OnceLock::new(),
            disabled: AtomicBool::new(false),
            kicked_out: RwLock::new(None),
            token_expire: RwLock::new(None),
            refresh_lock: Mutex::new(()),
            e2ee: E2eeState::default(),
            sync: Mutex::new(SyncData::default()),
            push: crate::push::PushState::default(),
        })))
    }
}

/// The low-level LINE client. Cheap to clone (shared state behind an `Arc`).
#[derive(Clone)]
pub struct BaseClient(Arc<Inner>);

pub struct Inner {
    pub device: Device,
    pub device_details: DeviceDetails,
    /// Primary API host. Requests go to [`BaseClient::current_endpoint`],
    /// which moves to a backup host after a connection failure.
    pub endpoint: String,
    /// Hosts tried after `endpoint`, in order.
    pub backup_endpoints: Vec<String>,
    active_endpoint: AtomicUsize,
    pub proxy: Option<ProxyConfig>,
    pub storage: Arc<dyn Storage>,
    /// HTTP/1.1-only client for Thrift and REST calls (as linejs).
    pub http: reqwest::Client,
    /// Client for PUSH (`/PUSH/1/subs`), which needs HTTP/2.
    pub push_http: reqwest::Client,
    pub legy_mode: LegyMode,
    pub legy_endpoint: Option<String>,
    pub config: Config,
    auth_token: RwLock<Option<String>>,
    profile: RwLock<Option<Value>>,
    events: broadcast::Sender<ClientEvent>,
    reqseq: Mutex<Option<HashMap<String, i64>>>,
    legy_transport: OnceLock<LegyEncryptedTransport>,
    disabled: AtomicBool,
    kicked_out: RwLock<Option<LogoutReason>>,
    /// Cached `expire` (`None` = not loaded from storage yet).
    token_expire: RwLock<Option<Option<i64>>>,
    pub(crate) refresh_lock: Mutex<()>,
    pub(crate) e2ee: E2eeState,
    pub(crate) sync: Mutex<SyncData>,
    pub(crate) push: crate::push::PushState,
}

impl Deref for BaseClient {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl BaseClient {
    pub fn builder(device: Device) -> ClientBuilder {
        ClientBuilder {
            device,
            version: None,
            endpoint: None,
            backup_endpoints: None,
            proxy: None,
            storage: None,
            http: None,
            legy_mode: LegyMode::Auto,
            legy_endpoint: None,
            config: Config::default(),
        }
    }

    pub fn new(device: Device) -> Result<Self> {
        Self::builder(device).build()
    }

    pub fn auth_token(&self) -> Option<String> {
        self.auth_token.read().unwrap().clone()
    }

    pub fn set_auth_token(&self, token: Option<String>) {
        *self.auth_token.write().unwrap() = token;
    }

    /// Profile of the logged-in user (set by login).
    pub fn profile(&self) -> Option<Value> {
        self.profile.read().unwrap().clone()
    }

    pub fn set_profile(&self, profile: Option<Value>) {
        *self.profile.write().unwrap() = profile;
    }

    /// The logged-in user's mid.
    pub fn mid(&self) -> Option<String> {
        self.profile().and_then(|p| p["mid"].as_str().map(str::to_string))
    }

    /// The account's service region (`Profile.regionCode`, e.g. `TH`).
    pub fn region_code(&self) -> Option<String> {
        self.profile().and_then(|p| p["regionCode"].as_str().filter(|s| !s.is_empty()).map(str::to_string))
    }

    /// Subscribe to client events. Subscribe before calling login to see
    /// `QrCall`/`PinCall`.
    pub fn subscribe(&self) -> broadcast::Receiver<ClientEvent> {
        self.events.subscribe()
    }

    pub fn emit(&self, event: ClientEvent) {
        let _ = self.events.send(event);
    }

    pub(crate) fn log(&self, kind: &str, data: serde_json::Value) {
        tracing::debug!(kind, %data);
        self.emit(ClientEvent::Log { kind: kind.to_string(), data });
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Relaxed)
    }

    /// Disable the client: subsequent requests fail with `ClientClosed`.
    pub fn disable(&self) {
        self.disabled.store(true, Ordering::Relaxed);
    }

    /// Why the server ended this session, if it did.
    pub fn kicked_out(&self) -> Option<LogoutReason> {
        self.kicked_out.read().unwrap().clone()
    }

    /// End the session: drop the token, disable the client and emit
    /// `KickedOut(reason)` then `End`. Only the first call has an effect.
    pub fn kick_out(&self, reason: LogoutReason) {
        {
            let mut k = self.kicked_out.write().unwrap();
            if k.is_some() {
                return;
            }
            *k = Some(reason.clone());
        }
        tracing::warn!(%reason, "session ended by server");
        self.set_auth_token(None);
        self.disable();
        self.emit(ClientEvent::KickedOut(reason));
        self.emit(ClientEvent::End(self.profile()));
    }

    /// Host requests currently go to (primary or a backup after failover).
    pub fn current_endpoint(&self) -> &str {
        let i = self.active_endpoint.load(Ordering::Relaxed);
        if i == 0 { &self.endpoint } else { self.backup_endpoints.get(i - 1).map_or(&self.endpoint, String::as_str) }
    }

    /// Number of hosts (primary + backups).
    pub(crate) fn endpoint_count(&self) -> usize {
        1 + self.backup_endpoints.len()
    }

    /// Move to the host after `failed` (by index) unless another request
    /// already did. Returns the index now active.
    pub(crate) fn fail_over_from(&self, failed: usize) -> usize {
        let next = (failed + 1) % self.endpoint_count();
        match self.active_endpoint.compare_exchange(failed, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => {
                tracing::warn!(from = failed, to = next, host = self.current_endpoint(), "LEGY host failover");
                self.log("Failover", serde_json::json!({"host": self.current_endpoint()}));
                next
            }
            Err(now) => now,
        }
    }

    pub(crate) fn active_endpoint_index(&self) -> usize {
        self.active_endpoint.load(Ordering::Relaxed)
    }

    /// When the access token should be refreshed (Unix seconds), from the
    /// cached or stored `expire`.
    pub async fn token_expire(&self) -> Result<Option<i64>> {
        if let Some(cached) = *self.token_expire.read().unwrap() {
            return Ok(cached);
        }
        let stored = self.storage.get("expire").await?.and_then(|v| match v {
            serde_json::Value::Number(n) => n.as_i64(),
            serde_json::Value::String(s) => s.parse().ok(),
            _ => None,
        });
        *self.token_expire.write().unwrap() = Some(stored);
        Ok(stored)
    }

    /// Persist `expire` (Unix seconds) and update the cache.
    pub async fn set_token_expire(&self, expire: i64) -> Result<()> {
        *self.token_expire.write().unwrap() = Some(Some(expire));
        self.storage.set("expire", expire.into()).await
    }

    pub(crate) fn legy_transport(&self) -> &LegyEncryptedTransport {
        self.legy_transport.get_or_init(|| LegyEncryptedTransport::new(self.legy_endpoint.as_deref()))
    }

    /// `toType` for a mid (`u` user, `r` room, `c` chat, `s` square, ...).
    pub fn get_to_type(mid: &str) -> Option<i32> {
        Some(match mid.chars().next()? {
            'u' => 0,
            'r' => 1,
            'c' => 2,
            's' => 3,
            'm' => 4,
            'p' => 5,
            'v' => 6,
            't' => 7,
            _ => return None,
        })
    }

    /// Next request sequence number for `name` (persisted in storage).
    pub async fn get_reqseq(&self, name: &str) -> Result<i64> {
        let mut guard = self.reqseq.lock().await;
        if guard.is_none() {
            let stored = self.storage.get("reqseq").await?;
            let map = match stored {
                Some(serde_json::Value::String(s)) => serde_json::from_str(&s).unwrap_or_default(),
                Some(v @ serde_json::Value::Object(_)) => serde_json::from_value(v).unwrap_or_default(),
                _ => HashMap::new(),
            };
            *guard = Some(map);
        }
        let map = guard.as_mut().unwrap();
        let entry = map.entry(name.to_string()).or_insert(0);
        let seq = *entry;
        *entry += 1;
        let snapshot = serde_json::to_string(map)?;
        self.storage.set("reqseq", snapshot.into()).await?;
        Ok(seq)
    }

    pub(crate) fn require_token(&self) -> Result<String> {
        self.auth_token().ok_or_else(|| Error::internal("NotAuthorized", "try login first"))
    }
}
