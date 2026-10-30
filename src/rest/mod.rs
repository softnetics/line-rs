//! JSON REST commands of the LINE desktop client, routed through the LEGY
//! host (`https://{endpoint}{prefix}{path}`).
//!
//! Derived from static RE of LINE macOS 26.5.0; unverified against live
//! server. The desktop app models every REST call as a command object with
//! a *server type* that selects a path prefix (the switch at `0x10146a920`)
//! and an optional channel token (`X-Line-ChannelToken`, attached by the
//! generic header builder at `0x1013d401c`). The prefixes and channel ids
//! used here are the ones observed in that binary:
//!
//! | server type | prefix | used by |
//! |---|---|---|
//! | 14 / 16 | `/ma` | `CGetUserDetail` (`userpopup/getDetail.json`) |
//! | 17 | `/ext/note/nt` | group Note commands |
//! | 18 | `/PAGEPOKER` | `CGetPageInfo` (URL preview) |
//! | 19 | `/kp` | Keep (`line::BIZ::KP::command::*`) |
//! | 24 | `/ds` | `CTranslateCommand`, `CTranslateListCommand` |
//! | 25 | `/sn` | square Note commands, announcements |
//! | 29 / 30 | `/ext/album/api`, `/ext/album/moa` | album (see [`crate::service::moa`]) |
//! | 33 | `/hm` | home profile / cover commands |
//!
//! Sub-APIs: [`RestClient::keep`], [`RestClient::note`], [`RestClient::home`],
//! [`RestClient::translate`] / [`RestClient::translate_list`].
//!
//! Keep one [`RestClient`] around: channel tokens are cached inside it.

use std::collections::HashMap;
use std::sync::Mutex;

use reqwest::Method;
use serde_json::Value as Json;

use crate::base::BaseClient;
use crate::error::{Error, Result};

pub mod home;
pub mod keep;
pub mod note;
pub mod translate;

#[cfg(test)]
pub(crate) mod tests;

const ERROR_KIND: &str = "RestError";

/// Path prefix ("server type") of a desktop REST command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RestPrefix {
    /// `/kp` — Keep (server type 19).
    Keep,
    /// `/ds` — translation (server type 24).
    Translation,
    /// `/hm` — home profile / cover (server type 33).
    Home,
    /// `/ma` — user popup (server type 16).
    HomeApi,
    /// `/ext/note/nt` — group Note (server type 17).
    Note,
    /// `/sn` — square Note and announcements (server type 25).
    SquareNote,
    /// `/PAGEPOKER` — URL preview (server type 18).
    PagePoker,
    /// `/ext/album/api` — group album (server type 29).
    AlbumApi,
    /// `/ext/album/moa` — Moa album (server type 30).
    AlbumMoa,
}

impl RestPrefix {
    pub fn as_str(self) -> &'static str {
        match self {
            RestPrefix::Keep => "/kp",
            RestPrefix::Translation => "/ds",
            RestPrefix::Home => "/hm",
            RestPrefix::HomeApi => "/ma",
            RestPrefix::Note => "/ext/note/nt",
            RestPrefix::SquareNote => "/sn",
            RestPrefix::PagePoker => "/PAGEPOKER",
            RestPrefix::AlbumApi => "/ext/album/api",
            RestPrefix::AlbumMoa => "/ext/album/moa",
        }
    }

    /// Note prefix for a home id: `/sn` for squares (`s...`), else `/ext/note/nt`
    /// (the selector at `0x1026b9d70`).
    pub fn note_for(home_id: &str) -> Self {
        if home_id.starts_with('s') { RestPrefix::SquareNote } else { RestPrefix::Note }
    }
}

/// Production channel ids observed in the desktop binary.
pub mod channels {
    /// Keep (`KEEP-DEFAULT`, selector at `0x10217e230`; beta id `1428230068`).
    pub const KEEP: &str = "1433572998";
    /// Timeline (default branch of the TL channel switch at `0x1026ba28c`).
    pub const TIMELINE: &str = "1341209950";
    /// MyHome.
    pub const MYHOME: &str = "1341209850";
    /// Album.
    pub const ALBUM: &str = "1375220249";
    /// Group note (`privnote`).
    pub const PRIVATE_NOTE: &str = "1655599932";
    /// Square note (`opennote`).
    pub const OPEN_NOTE: &str = "1657618623";
}

/// How a request authenticates (on top of `X-Line-Access`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestAuth {
    /// Only the session access token.
    Access,
    /// `X-Line-ChannelToken` issued for this channel id.
    Channel(&'static str),
}

/// One REST request.
#[derive(Debug, Clone)]
pub struct RestRequest {
    pub method: Method,
    pub prefix: RestPrefix,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Option<Json>,
    pub headers: Vec<(String, String)>,
    pub auth: RestAuth,
}

impl RestRequest {
    pub fn new(method: Method, prefix: RestPrefix, path: impl Into<String>, auth: RestAuth) -> Self {
        Self { method, prefix, path: path.into(), query: Vec::new(), body: None, headers: Vec::new(), auth }
    }

    pub fn get(prefix: RestPrefix, path: impl Into<String>, auth: RestAuth) -> Self {
        Self::new(Method::GET, prefix, path, auth)
    }

    pub fn post(prefix: RestPrefix, path: impl Into<String>, auth: RestAuth) -> Self {
        Self::new(Method::POST, prefix, path, auth)
    }

    pub fn query(mut self, k: &str, v: impl ToString) -> Self {
        self.query.push((k.to_string(), v.to_string()));
        self
    }

    pub fn query_opt(self, k: &str, v: Option<impl ToString>) -> Self {
        match v {
            Some(v) => self.query(k, v),
            None => self,
        }
    }

    pub fn body(mut self, body: Json) -> Self {
        self.body = Some(body);
        self
    }

    pub fn header(mut self, k: &str, v: impl Into<String>) -> Self {
        self.headers.push((k.to_string(), v.into()));
        self
    }

    /// Full URL under `base` (`https://host` without a trailing slash).
    pub fn url(&self, base: &str) -> Result<url::Url> {
        let raw = format!("{}{}{}", base.trim_end_matches('/'), self.prefix.as_str(), self.path);
        let mut url = url::Url::parse(&raw).map_err(|e| Error::internal(ERROR_KIND, format!("invalid url: {e}")))?;
        if !self.query.is_empty() {
            url.query_pairs_mut().extend_pairs(self.query.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        }
        Ok(url)
    }
}

/// Desktop REST client; see the [module docs](self).
pub struct RestClient {
    client: BaseClient,
    base_url: Option<String>,
    tokens: Mutex<HashMap<String, String>>,
}

impl BaseClient {
    /// A new [`RestClient`] (empty channel-token cache) for this client.
    pub fn rest(&self) -> RestClient {
        RestClient::new(self.clone())
    }
}

impl RestClient {
    pub fn new(client: BaseClient) -> Self {
        Self { client, base_url: None, tokens: Mutex::new(HashMap::new()) }
    }

    pub fn client(&self) -> &BaseClient {
        &self.client
    }

    /// Send requests to `base` (e.g. `http://127.0.0.1:8080`) instead of
    /// `https://{endpoint}`.
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url = Some(base.into());
        self
    }

    fn base(&self) -> String {
        self.base_url.clone().unwrap_or_else(|| format!("https://{}", self.client.endpoint))
    }

    /// Seed the token cache (e.g. with a token issued elsewhere).
    pub fn set_channel_token(&self, channel_id: &str, token: impl Into<String>) {
        self.tokens.lock().unwrap().insert(channel_id.to_string(), token.into());
    }

    /// Forget a cached token (after expiry).
    pub fn clear_channel_token(&self, channel_id: &str) {
        self.tokens.lock().unwrap().remove(channel_id);
    }

    /// `approveChannelAndIssueChannelToken` for `channel_id` (cached).
    pub async fn channel_token(&self, channel_id: &str) -> Result<String> {
        if let Some(t) = self.tokens.lock().unwrap().get(channel_id) {
            return Ok(t.clone());
        }
        let res = self
            .client
            .channel()
            .approve_channel_and_issue_channel_token(crate::tv!({"channelId": channel_id}))
            .await?;
        let token = match res["channelAccessToken"].as_str() {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => return Err(Error::internal(ERROR_KIND, "channelAccessToken missing from channel response")),
        };
        self.set_channel_token(channel_id, token.clone());
        Ok(token)
    }

    /// Send a request and return the JSON body (`Null` when empty). Non-2xx
    /// statuses are errors.
    pub async fn send(&self, req: RestRequest) -> Result<Json> {
        let mid = self.client.mid().ok_or_else(|| Error::internal(ERROR_KIND, "login must complete first"))?;
        let mut headers = self.client.line_headers(req.method.as_str());
        let mut set = |k: &str, v: String| {
            headers.retain(|(hk, _)| !hk.eq_ignore_ascii_case(k));
            headers.push((k.to_string(), v));
        };
        set("accept", "application/json".into());
        set("content-type", "application/json; charset=utf-8".into());
        set("X-Line-Mid", mid);
        // The desktop header builder (0x1013d401c) adds `x-lsr` whenever the
        // service region is known; region-routed APIs such as Keep need it.
        if let Some(region) = self.client.region_code() {
            set("x-lsr", region);
        }
        if let RestAuth::Channel(id) = req.auth {
            let token = self.channel_token(id).await?;
            set("X-Line-ChannelToken", token);
        }
        for (k, v) in &req.headers {
            set(k, v.clone());
        }
        let url = req.url(&self.base())?;
        // LEGY routes REST calls on `x-lhm` (already set to `req.method`): a
        // wire GET answers 404, so GETs go out as POST with `x-lhm: GET`, as
        // CHRLINE and linejs (Moa) send them.
        let wire = if req.method == Method::GET { Method::POST } else { req.method.clone() };
        let mut builder = self.client.http.request(wire, url).timeout(self.client.config.timeout);
        for (k, v) in headers {
            builder = builder.header(k, v);
        }
        if let Some(body) = &req.body {
            builder = builder.body(serde_json::to_vec(body)?);
        }
        let res = builder.send().await?;
        let status = res.status();
        // Routing headers explain a LEGY 404 better than its (often empty) body.
        let diag: Vec<String> = res
            .headers()
            .iter()
            .filter(|(k, _)| {
                let k = k.as_str();
                k.starts_with("x-l") || k == "server" || k == "content-type"
            })
            .map(|(k, v)| format!("{k}={}", v.to_str().unwrap_or("?")))
            .collect();
        let version = res.version();
        let bytes = res.bytes().await?;
        if !status.is_success() {
            let snippet: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
            let snippet = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
            let tail = if snippet.is_empty() { String::new() } else { format!(": {snippet}") };
            return Err(Error::internal(
                ERROR_KIND,
                format!(
                    "{}{} HTTP {}{tail} [{version:?}; {}]",
                    req.prefix.as_str(),
                    req.path,
                    status.as_u16(),
                    diag.join(", ")
                ),
            ));
        }
        if bytes.is_empty() {
            return Ok(Json::Null);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
}
