//! VOOM / MyHome / Note / Album REST helpers (linejs `features/voom.ts`):
//! routing prefixes on `gw.line.naver.jp` and `X-Line-ChannelToken` auth.

use serde::Deserialize;

use super::Client;
use crate::error::{Error, Result};
use crate::tv;

/// LINE channels with their ids (smali `t98.a$b`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VoomChannel {
    Timeline,
    Home,
    Home26,
    Note,
    SquareNote,
    Album,
}

impl VoomChannel {
    pub fn id(self) -> &'static str {
        match self {
            VoomChannel::Timeline => "1341209950",
            VoomChannel::Home => "1341209850",
            VoomChannel::Home26 => "2007835442",
            VoomChannel::Note => "1655599932",
            VoomChannel::SquareNote => "1657618623",
            VoomChannel::Album => "1375220249",
        }
    }
}

/// Gateway routing prefix per MH-family service (smali `ps5/j`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoomRouting {
    #[default]
    Myhome,
    MyhomeRenewal,
    Timeline,
    TimelineGateway,
    Note,
    HomeApi,
    SquareNote,
    Album,
    Story,
    SocialNotification,
    Translation,
}

impl VoomRouting {
    pub fn prefix(self) -> &'static str {
        match self {
            VoomRouting::Myhome => "/mh",
            VoomRouting::MyhomeRenewal => "/hm",
            VoomRouting::Timeline => "/tl",
            VoomRouting::TimelineGateway => "/ext/timeline/tlgw",
            VoomRouting::Note => "/ext/note/nt",
            VoomRouting::HomeApi => "/ma",
            VoomRouting::SquareNote => "/sn",
            VoomRouting::Album => "/ext/album",
            VoomRouting::Story => "/st",
            VoomRouting::SocialNotification => "/eg",
            VoomRouting::Translation => "/ds",
        }
    }
}

/// Curated MH-family REST paths (LINE Android 26.6.2).
///
/// The macOS desktop client (static RE of LINE macOS 26.5.0; unverified
/// against live server) sends the Note post/comment/like commands to
/// [`VoomRouting::Note`] (`/ext/note/nt`) for groups and
/// [`VoomRouting::SquareNote`] (`/sn`) for squares, never `/mh`;
/// announcements always go to `/sn`. Typed wrappers live in
/// [`crate::rest::note`] (and Keep/home/translation in [`crate::rest`]).
pub mod endpoints {
    pub const FEED: &str = "/api/v57/post/list.json";
    pub const CREATE_POST: &str = "/api/v57/post/create.json";
    pub const UPDATE_POST: &str = "/api/v57/post/update.json";
    pub const DELETE_POST: &str = "/api/v57/post/delete.json";
    pub const GET_POST: &str = "/api/v57/post/get.json";
    pub const SHARE_POST: &str = "/api/v57/post/share.json";
    pub const SEND_POST_TO_TALK: &str = "/api/v57/post/sendPostToTalk.json";
    pub const GET_SHARE_LINK: &str = "/api/v57/post/getShareLink.json";
    pub const REPORT_POST: &str = "/api/v57/post/report.json";
    pub const CREATE_COMMENT: &str = "/api/v57/comment/create.json";
    pub const DELETE_COMMENT: &str = "/api/v57/comment/delete.json";
    pub const GET_COMMENT: &str = "/api/v57/comment/get.json";
    pub const LIST_COMMENTS: &str = "/api/v57/comment/getList.json";
    pub const REPORT_COMMENT: &str = "/api/v57/comment/report.json";
    pub const CREATE_LIKE: &str = "/api/v57/like/create.json";
    pub const CANCEL_LIKE: &str = "/api/v57/like/cancel.json";
    pub const GET_LIKE: &str = "/api/v57/like/get.json";
    pub const LIST_LIKES: &str = "/api/v57/like/getList.json";
    pub const HASHTAG_POSTS: &str = "/api/v57/hashtag/posts.json";
    pub const HASHTAG_SEARCH: &str = "/api/v57/hashtag/search.json";
    pub const HASHTAG_SUGGEST_POPULAR: &str = "/api/v57/hashtag/suggest/popular.json";
    pub const GROUP_HOME_INIT: &str = "/api/v57/grouphome/init.json";
    pub const TIMELINE_STATUS: &str = "/api/v57/timeline/tab/status.json";
    pub const TIMELINE_CONTENTS: &str = "/api/v57/timeline/tab/contents.json";
    pub const HOME_PROFILE: &str = "/api/v1/home/profile.json";
    pub const HOME_COVER: &str = "/api/v1/home/cover.json";
}

/// A VOOM REST request.
#[derive(Debug, Clone, Default)]
pub struct VoomRestOptions {
    /// Path under the routing prefix, e.g. `/api/v57/post/list.json`.
    pub path: String,
    pub routing: VoomRouting,
    /// Defaults to `POST` with a body, else `GET`.
    pub method: Option<String>,
    pub body: Option<serde_json::Value>,
    /// Channel token; without it `X-Line-Access` is sent.
    pub channel_token: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    /// Host override (default `gw.line.naver.jp`).
    pub host: Option<String>,
}

impl VoomRestOptions {
    pub fn new(routing: VoomRouting, path: impl Into<String>) -> Self {
        Self { routing, path: path.into(), ..Default::default() }
    }

    pub(crate) fn url(&self) -> String {
        let host = self.host.as_deref().unwrap_or("gw.line.naver.jp");
        let path = if self.path.starts_with('/') { self.path.clone() } else { format!("/{}", self.path) };
        format!("https://{host}{}{path}", self.routing.prefix())
    }

    pub(crate) fn method(&self) -> &str {
        self.method.as_deref().unwrap_or(if self.body.is_some() { "POST" } else { "GET" })
    }
}

/// The common `{code, message, result}` envelope.
#[derive(Debug, Clone, Deserialize)]
pub struct VoomRestResponse {
    pub code: i64,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
}

impl Client {
    /// Raw VOOM REST call.
    pub async fn voom_rest(&self, opts: VoomRestOptions) -> Result<VoomRestResponse> {
        let base = &self.base;
        let method = reqwest::Method::from_bytes(opts.method().as_bytes())
            .map_err(|_| Error::internal("VoomError", format!("bad method {}", opts.method())))?;
        let mut req = base
            .http
            .request(method, opts.url())
            .header("accept", "application/json")
            .header("X-Line-Application", base.device_details.application())
            .header("user-agent", base.device_details.user_agent())
            .header("X-Line-Mid", base.mid().unwrap_or_default())
            .header("x-lal", "ja-JP")
            .header("X-Line-BDBTemplateVersion", "v1");
        req = match &opts.channel_token {
            Some(t) => req.header("X-Line-ChannelToken", t),
            None => req.header("X-Line-Access", base.auth_token().unwrap_or_default()),
        };
        for (k, v) in &opts.extra_headers {
            req = req.header(k, v);
        }
        if let Some(body) = &opts.body {
            req = req.header("content-type", "application/json").body(serde_json::to_string(body)?);
        }
        let res = req.send().await?;
        let status = res.status();
        let text = res.text().await?;
        if text.is_empty() {
            return Ok(VoomRestResponse {
                code: status.as_u16() as i64,
                message: status.canonical_reason().map(str::to_string),
                result: None,
            });
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// VOOM helpers with cached channel tokens.
    pub fn voom(&self) -> Voom<'_> {
        Voom { client: self }
    }

    /// `issueChannelToken` for a channel id; returns the token.
    pub async fn channel_token(&self, channel_id: &str) -> Result<String> {
        let res = self.base.channel().call("issueChannelToken", tv!({"channelId": channel_id})).await?;
        res["token"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::internal("VoomError", "issueChannelToken returned no token"))
    }
}

/// VOOM operations bound to a [`Client`] (see [`Client::voom`]).
#[derive(Clone, Copy)]
pub struct Voom<'a> {
    client: &'a Client,
}

impl Voom<'_> {
    /// Channel token (cached per channel).
    pub async fn token(&self, channel: VoomChannel) -> Result<String> {
        if let Some(t) = self.client.inner.voom_tokens.lock().unwrap().get(channel.id()) {
            return Ok(t.clone());
        }
        let t = self.client.channel_token(channel.id()).await?;
        self.client.inner.voom_tokens.lock().unwrap().insert(channel.id().to_string(), t.clone());
        Ok(t)
    }

    /// REST call with an automatically minted channel token.
    pub async fn call(&self, channel: VoomChannel, mut opts: VoomRestOptions) -> Result<VoomRestResponse> {
        opts.channel_token = Some(self.token(channel).await?);
        self.client.voom_rest(opts).await
    }

    /// `GET /mh/api/v57/post/list.json` — the VOOM feed (defaults 10 / 2).
    pub async fn feed(&self, post_limit: Option<u32>, following_max_page: Option<u32>) -> Result<VoomRestResponse> {
        let path = format!(
            "{}?postLimit={}&followingMaxPage={}",
            endpoints::FEED,
            post_limit.unwrap_or(10),
            following_max_page.unwrap_or(2)
        );
        self.call(VoomChannel::Timeline, VoomRestOptions::new(VoomRouting::Myhome, path)).await
    }

    /// `GET /tl/api/v57/timeline/tab/status.json`.
    pub async fn timeline_status(&self) -> Result<VoomRestResponse> {
        self.call(VoomChannel::Timeline, VoomRestOptions::new(VoomRouting::Timeline, endpoints::TIMELINE_STATUS)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_ids_match_smali() {
        assert_eq!(VoomChannel::Timeline.id(), "1341209950");
        assert_eq!(VoomChannel::Home.id(), "1341209850");
        assert_eq!(VoomChannel::Note.id(), "1655599932");
        assert_eq!(VoomChannel::SquareNote.id(), "1657618623");
    }

    #[test]
    fn url_and_method() {
        let mut o = VoomRestOptions::new(VoomRouting::Timeline, "api/x");
        assert_eq!(o.url(), "https://gw.line.naver.jp/tl/api/x");
        assert_eq!(o.method(), "GET");
        o.body = Some(serde_json::json!({}));
        o.host = Some("h.test".into());
        assert_eq!(o.method(), "POST");
        assert_eq!(o.url(), "https://h.test/tl/api/x");
        assert_eq!(VoomRestOptions::new(VoomRouting::default(), "/a").url(), "https://gw.line.naver.jp/mh/a");
    }
}
