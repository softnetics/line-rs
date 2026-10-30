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
//! **Auth.** Same caveat as [`super::note`]: the per-command channel was not
//! recovered, so the MyHome channel token is used.

use reqwest::Method;
use serde_json::{Map, Value as Json};

use super::{RestAuth, RestClient, RestPrefix, RestRequest, channels};
use crate::error::Result;

const AUTH: RestAuth = RestAuth::Channel(channels::MYHOME);

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
            ("coverObjectId", self.cover_object_id.clone().map(Json::from)),
            ("videoCoverObjectId", self.video_cover_object_id.clone().map(Json::from)),
            ("storyShare", self.story_share.map(Json::from)),
        ];
        m.extend(opt.into_iter().filter_map(|(k, v)| v.map(|v| (k.to_string(), v))));
        Json::Object(m)
    }
}

/// Options of [`Home::page_info`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageInfoQuery {
    pub url: String,
    pub caller: Option<String>,
    pub lang: Option<String>,
    pub phishing_check: Option<bool>,
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
        let path = if q.home_id.starts_with('c') { GROUP_PROFILE } else { HOME_PROFILE };
        let req = RestRequest::get(RestPrefix::Home, path, AUTH)
            .query("homeId", &q.home_id)
            .query_opt("profileId", q.profile_id)
            .query_opt("storyVersion", q.story_version)
            .query_opt("styleMediaVersion", q.style_media_version);
        self.rest.send(req).await
    }

    /// `POST /hm/api/v1/home/cover`.
    pub async fn update_cover(&self, opts: UpdateHomeCover) -> Result<Json> {
        self.rest.send(RestRequest::post(RestPrefix::Home, HOME_COVER, AUTH).body(opts.body())).await
    }

    /// `DELETE /hm/api/v1/home/cover` `{homeId, profileId}`.
    pub async fn delete_cover(&self, home_id: &str, profile_id: Option<&str>) -> Result<Json> {
        let mut body = Map::new();
        body.insert("homeId".into(), home_id.into());
        if let Some(p) = profile_id {
            body.insert("profileId".into(), p.into());
        }
        let req = RestRequest::new(Method::DELETE, RestPrefix::Home, HOME_COVER, AUTH).body(Json::Object(body));
        self.rest.send(req).await
    }

    /// `DELETE /hm/api/v1/home/groupprofile?homeId` (group cover).
    pub async fn delete_group_cover(&self, home_id: &str) -> Result<Json> {
        let req = RestRequest::new(Method::DELETE, RestPrefix::Home, GROUP_PROFILE, AUTH).query("homeId", home_id);
        self.rest.send(req).await
    }

    /// `GET /ma/api/v1/userpopup/getDetail.json?userMid`.
    pub async fn user_detail(&self, user_mid: &str) -> Result<Json> {
        self.rest.send(RestRequest::get(RestPrefix::HomeApi, USER_DETAIL, AUTH).query("userMid", user_mid)).await
    }

    /// `GET /PAGEPOKER/api/v2/pageinfo/get.json?url&caller&lang&phishingCheck`
    /// — URL preview.
    pub async fn page_info(&self, q: PageInfoQuery) -> Result<Json> {
        let req = RestRequest::get(RestPrefix::PagePoker, PAGE_INFO, AUTH)
            .query("url", &q.url)
            .query_opt("caller", q.caller)
            .query_opt("lang", q.lang)
            .query_opt("phishingCheck", q.phishing_check);
        self.rest.send(req).await
    }
}
