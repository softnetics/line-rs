//! Home profile / cover (`/hm`), user popup (`/ma`) and URL preview
//! (`/PAGEPOKER`).
//!
//! Derived from static RE of LINE macOS 26.5.0; unverified against live
//! server. Evidence: `line::BIZ::TL::command` vtables `GetHomeProfile`,
//! `UpdateHomeCover`, `DeleteHomeCover`, `DeleteGroupHomeCover` (server type
//! 33 → `/hm`), `CGetUserDetail` (16 → `/ma`) and `CGetPageInfo`
//! (18 → `/PAGEPOKER`) at `0x105e56fe0..0x105e57b20`. Method enum 4 is
//! `DELETE` (method-name switch at `0x10120e0a0`).
//!
//! **Auth.** `/hm` requests carry the Timeline channel token and `/ma`
//! the MyHome one, as LINE for Android picks them per routing prefix
//! (`ul6/a.java`: `MYHOME` `/mh` and `HOMEAPI` `/ma` use `ul6/h.c()`,
//! 1341209850; every other prefix, `MYHOME_RENEWAL` `/hm` included, falls
//! to `ul6/h.f()`, 1341209950). `/PAGEPOKER` goes through the app's plain
//! LEGY client with the session token only (`sa0/i`, `sa0/g`).

use reqwest::Method;
use serde_json::{Map, Value as Json};

use super::{RestAuth, RestClient, RestPrefix, RestRequest, channels};
use crate::error::{Error, Result};

/// `/hm` (`MYHOME_RENEWAL`).
const AUTH: RestAuth = RestAuth::Channel(channels::TIMELINE);
/// `/ma` (`HOMEAPI`).
const MYHOME_AUTH: RestAuth = RestAuth::Channel(channels::MYHOME);

pub const HOME_PROFILE: &str = "/api/v1/home/profile";
pub const GROUP_PROFILE: &str = "/api/v1/home/groupprofile";
pub const HOME_COVER: &str = "/api/v1/home/cover";
pub const USER_DETAIL: &str = "/api/v1/userpopup/getDetail.json";
pub const PAGE_INFO: &str = "/api/v2/pageinfo/get.json";

/// Options of [`Home::profile`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HomeProfileQuery {
    /// User or group (`c...`) mid; groups use `home/groupprofile`.
    pub home_id: String,
    pub profile_id: Option<String>,
    pub story_version: Option<String>,
    pub style_media_version: Option<String>,
}

/// Body of `POST home/cover` (`UpdateHomeCover`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdateHomeCover {
    pub home_id: String,
    pub profile_id: Option<String>,
    /// OBS object id of the uploaded cover image.
    pub cover_object_id: Option<String>,
    pub video_cover_object_id: Option<String>,
    pub story_share: Option<bool>,
}

impl UpdateHomeCover {
    pub fn body(&self) -> Json {
        let mut m = Map::new();
        m.insert("homeId".into(), self.home_id.clone().into());
        let opt = [
            ("profileId", self.profile_id.clone().map(Json::from)),
            (
                "coverObjectId",
                self.cover_object_id.clone().map(Json::from),
            ),
            (
                "videoCoverObjectId",
                self.video_cover_object_id.clone().map(Json::from),
            ),
            ("storyShare", self.story_share.map(Json::from)),
        ];
        m.extend(
            opt.into_iter()
                .filter_map(|(k, v)| v.map(|v| (k.to_string(), v))),
        );
        Json::Object(m)
    }
}

/// Options of [`Home::page_info`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageInfoQuery {
    pub url: String,
    /// Who asks (default `TALK_ANDROID`, LINE for Android's chat: `ws6/a`).
    pub caller: Option<String>,
    /// Locale such as `th_TH`, sent as `Accept-Language: th-TH`.
    pub lang: Option<String>,
    /// Ask LINE to check the page against its phishing list.
    pub phishing_check: Option<bool>,
}

impl PageInfoQuery {
    /// `{url, caller, unescape:"true", phishingCheck}` (`ys6/c.java`).
    pub fn body(&self) -> Json {
        serde_json::json!({
            "url": self.url,
            "caller": self.caller.as_deref().unwrap_or(PAGE_INFO_CALLER),
            "unescape": "true",
            "phishingCheck": self.phishing_check.unwrap_or(false),
        })
    }
}

/// `caller` of LINE for Android's chat link previews (`ws6/a.TALK`).
pub const PAGE_INFO_CALLER: &str = "TALK_ANDROID";

/// Host of a preview image given as a path (`ia0/k.CDN_OBS`; the
/// concrete host comes from server config, so this one is unverified).
pub const PREVIEW_IMAGE_CDN: &str = "https://obs.line-scdn.net/";

/// A link preview, as LINE for Android reads it (`xs6/a.java`).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PagePreview {
    /// The page's (`url`, else `ogUrl`).
    pub url: String,
    pub title: String,
    pub summary: String,
    pub domain: String,
    /// Full image URL: the OBS object (`obs.cdn` + `/` + `obs.hash`),
    /// else `image` on the OBS CDN.
    pub image: Option<String>,
    pub image_width: u32,
    pub image_height: u32,
    pub is_video: bool,
    pub service_name: String,
}

impl PagePreview {
    /// Read the `result` object of a `pageinfo/get.json` answer.
    pub fn from_result(r: &Json) -> Self {
        let text = |k: &str| r[k].as_str().unwrap_or_default().trim().to_string();
        let either = |a: &str, b: &str| {
            let v = text(a);
            if v.is_empty() { text(b) } else { v }
        };
        let obs = &r["obs"];
        let cdn = obs["cdn"].as_str().unwrap_or_default();
        let hash = obs["hash"].as_str().unwrap_or_default();
        let image = if !cdn.is_empty() && !hash.is_empty() {
            Some(format!(
                "{}/{}",
                cdn.trim_end_matches('/'),
                hash.trim_start_matches('/')
            ))
        } else {
            match text("image") {
                i if i.is_empty() => None,
                i if i.starts_with("http://") || i.starts_with("https://") => Some(i),
                i => Some(format!("{PREVIEW_IMAGE_CDN}{}", i.trim_start_matches('/'))),
            }
        };
        let num = |k: &str| r[k].as_u64().map(|n| n as u32).unwrap_or_default();
        PagePreview {
            url: either("url", "ogUrl"),
            title: text("title"),
            summary: text("summary"),
            domain: text("domain"),
            image,
            image_width: num("image_width"),
            image_height: num("image_height"),
            is_video: r["isvideo"].as_bool().unwrap_or(false),
            service_name: text("service_name"),
        }
    }
}

/// Home operations (see [`RestClient::home`]).
#[derive(Clone, Copy)]
pub struct Home<'a> {
    rest: &'a RestClient,
}

impl RestClient {
    pub fn home(&self) -> Home<'_> {
        Home { rest: self }
    }
}

impl Home<'_> {
    /// `GET /hm/api/v1/home/profile` (or `home/groupprofile` for `c...`
    /// homes, as the path builder at `0x10275d1cc` does).
    pub async fn profile(&self, q: HomeProfileQuery) -> Result<Json> {
        let path = if q.home_id.starts_with('c') {
            GROUP_PROFILE
        } else {
            HOME_PROFILE
        };
        let req = RestRequest::get(RestPrefix::Home, path, AUTH)
            .query("homeId", &q.home_id)
            .query_opt("profileId", q.profile_id)
            .query_opt("storyVersion", q.story_version)
            .query_opt("styleMediaVersion", q.style_media_version);
        self.rest.send(req).await
    }

    /// `POST /hm/api/v1/home/cover`.
    pub async fn update_cover(&self, opts: UpdateHomeCover) -> Result<Json> {
        self.rest
            .send(RestRequest::post(RestPrefix::Home, HOME_COVER, AUTH).body(opts.body()))
            .await
    }

    /// `DELETE /hm/api/v1/home/cover` `{homeId, profileId}`.
    pub async fn delete_cover(&self, home_id: &str, profile_id: Option<&str>) -> Result<Json> {
        let mut body = Map::new();
        body.insert("homeId".into(), home_id.into());
        if let Some(p) = profile_id {
            body.insert("profileId".into(), p.into());
        }
        let req = RestRequest::new(Method::DELETE, RestPrefix::Home, HOME_COVER, AUTH)
            .body(Json::Object(body));
        self.rest.send(req).await
    }

    /// `DELETE /hm/api/v1/home/groupprofile?homeId` (group cover).
    pub async fn delete_group_cover(&self, home_id: &str) -> Result<Json> {
        let req = RestRequest::new(Method::DELETE, RestPrefix::Home, GROUP_PROFILE, AUTH)
            .query("homeId", home_id);
        self.rest.send(req).await
    }

    /// `GET /ma/api/v1/userpopup/getDetail.json?userMid`.
    pub async fn user_detail(&self, user_mid: &str) -> Result<Json> {
        self.rest
            .send(
                RestRequest::get(RestPrefix::HomeApi, USER_DETAIL, MYHOME_AUTH)
                    .query("userMid", user_mid),
            )
            .await
    }

    /// `POST /PAGEPOKER/api/v2/pageinfo/get.json` with the JSON body of
    /// [`PageInfoQuery::body`] and `Accept-Language`, as LINE for Android
    /// asks for a link preview (`ys6/c.java`). LINE's server fetches the
    /// page; the site never sees the client. Authenticated by the session
    /// token alone (`SimpleLegyClient`, access MANDATORY). Returns the raw
    /// answer; see [`page_preview`](Self::page_preview).
    pub async fn page_info(&self, q: PageInfoQuery) -> Result<Json> {
        self.rest.send(page_info_request(&q)).await
    }

    /// [`page_info`](Self::page_info), read into a [`PagePreview`]. A
    /// refusal is a `PagePokerError` whose [`Error::http_status`] is the
    /// code LINE gave (`ys6/b.java`: the body's `code`, else the HTTP
    /// status): 451 on LINE's block list, 452 on its phishing list, 400,
    /// 403, 404, 422, 500 (`ws6/d.java`).
    ///
    /// [`Error::http_status`]: crate::Error::http_status
    pub async fn page_preview(&self, q: PageInfoQuery) -> Result<PagePreview> {
        let reply = self.rest.send_raw(&page_info_request(&q)).await?;
        let body: Json = serde_json::from_slice(&reply.body).unwrap_or(Json::Null);
        if !reply.status.is_success() {
            let code = body["code"]
                .as_u64()
                .unwrap_or(u64::from(reply.status.as_u16()));
            return Err(Error::internal(
                "PagePokerError",
                format!("pageinfo: HTTP {code}"),
            ));
        }
        match &body["result"] {
            r @ Json::Object(_) => Ok(PagePreview::from_result(r)),
            _ => Err(Error::internal("PagePokerError", "pageinfo: no result")),
        }
    }
}

fn page_info_request(q: &PageInfoQuery) -> RestRequest {
    let mut req =
        RestRequest::post(RestPrefix::PagePoker, PAGE_INFO, RestAuth::Access).body(q.body());
    if let Some(lang) = &q.lang {
        req = req.header("accept-language", lang.replace('_', "-"));
    }
    req
}
