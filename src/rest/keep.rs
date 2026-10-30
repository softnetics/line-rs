//! LINE Keep REST API (`/kp/api/v27/keep/*.json`).
//!
//! Derived from static RE of LINE macOS 26.5.0; unverified against live
//! server. Evidence: `line::BIZ::KP::command::Keep*` vtables at
//! `0x105e370a0..0x105e38098` — slot 10 is the HTTP method (0 = GET,
//! 1 = POST), slot 16 the path, slot 22 the server type (19 → `/kp`),
//! slot 27 the query map and slot 28 the JSON body. Requests carry
//! `X-Line-ChannelToken` for [`channels::KEEP`].
//!
//! **Not reachable over plain LEGY.** A live run on 2026-10-21 (DESKTOPWIN,
//! region TH) got a bodiless `HTTP 404` from `server=legy` for every path
//! under `/kp` (`sync.json`, `init.json`, and a path that cannot exist), on
//! `legy.line-apps.com` and `ga2.line.naver.jp`, with the Keep and timeline
//! channel tokens, over HTTP/1.1. The paths match the desktop binary and
//! CHRLINE; LEGY simply does not route the prefix for this client.
//! The same account's mobile app shows Keep only as the "Keep Memo"
//! self-chat, so Keep storage looks retired in favour of Keep Memo, which
//! is an ordinary chat with your own mid (Thrift `sendMessage` etc.).
//!
//! The shapes of `contentData`, `pinInfo` and the responses are not
//! recoverable statically, so they are passed through as raw JSON.

use serde_json::{Map, Value as Json, json};

use super::{RestAuth, RestClient, RestPrefix, RestRequest, channels};
use crate::error::Result;

const AUTH: RestAuth = RestAuth::Channel(channels::KEEP);

pub const INIT: &str = "/api/v27/keep/init.json";
pub const GET: &str = "/api/v27/keep/get.json";
pub const CREATE: &str = "/api/v27/keep/create.json";
pub const UPDATE: &str = "/api/v27/keep/update.json";
pub const DELETE: &str = "/api/v27/keep/delete.json";
pub const MESSAGE_DELETE: &str = "/api/v27/keep/message/delete.json";
pub const OBS_DELETE: &str = "/api/v27/keep/obs/delete.json";
pub const SIZE: &str = "/api/v27/keep/size.json";
pub const SYNC: &str = "/api/v27/keep/sync.json";
pub const FETCH: &str = "/api/v27/keep/fetch.json";
pub const PIN: &str = "/api/v27/keep/contents/pin.json";
pub const UNPIN: &str = "/api/v27/keep/contents/unpin.json";

/// Body of `keep/create.json` (`KeepCreate`). `None` fields are omitted.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeepCreate {
    pub client_id: Option<String>,
    pub message_id: Option<String>,
    pub content_data: Option<Json>,
    pub source: Option<Json>,
    pub tags: Option<Json>,
    pub pin_info: Option<Json>,
    pub service_type: Option<String>,
}

impl KeepCreate {
    pub fn body(&self) -> Json {
        let mut m = Map::new();
        put(&mut m, "clientId", self.client_id.clone().map(Json::from));
        put(&mut m, "messageId", self.message_id.clone().map(Json::from));
        put(&mut m, "contentData", self.content_data.clone());
        put(&mut m, "source", self.source.clone());
        put(&mut m, "tags", self.tags.clone());
        put(&mut m, "pinInfo", self.pin_info.clone());
        put(&mut m, "serviceType", self.service_type.clone().map(Json::from));
        Json::Object(m)
    }
}

/// Request of `keep/update.json` (`KeepUpdate`): `force` goes in the query,
/// the rest in the body.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeepUpdate {
    pub content_id: String,
    pub content_data: Option<Json>,
    pub source: Option<Json>,
    pub tags: Option<Json>,
    pub revision: Option<i64>,
    pub force: Option<bool>,
}

impl KeepUpdate {
    pub fn body(&self) -> Json {
        let mut m = Map::new();
        m.insert("contentId".into(), self.content_id.clone().into());
        put(&mut m, "contentData", self.content_data.clone());
        put(&mut m, "source", self.source.clone());
        put(&mut m, "tags", self.tags.clone());
        put(&mut m, "revision", self.revision.map(Json::from));
        Json::Object(m)
    }
}

fn put(m: &mut Map<String, Json>, k: &str, v: Option<Json>) {
    if let Some(v) = v {
        m.insert(k.to_string(), v);
    }
}

/// Keep operations (see [`RestClient::keep`]).
#[derive(Clone, Copy)]
pub struct Keep<'a> {
    rest: &'a RestClient,
}

impl RestClient {
    pub fn keep(&self) -> Keep<'_> {
        Keep { rest: self }
    }
}

impl Keep<'_> {
    fn get(path: &str) -> RestRequest {
        RestRequest::get(RestPrefix::Keep, path, AUTH)
    }

    fn post(path: &str, body: Json) -> RestRequest {
        RestRequest::post(RestPrefix::Keep, path, AUTH).body(body)
    }

    /// `GET keep/init.json`.
    pub async fn init(&self) -> Result<Json> {
        self.rest.send(Self::get(INIT)).await
    }

    /// `GET keep/get.json?contentId=`.
    pub async fn get_content(&self, content_id: &str) -> Result<Json> {
        self.rest.send(Self::get(GET).query("contentId", content_id)).await
    }

    /// `GET keep/size.json` (storage usage).
    pub async fn size(&self) -> Result<Json> {
        self.rest.send(Self::get(SIZE)).await
    }

    /// `GET keep/sync.json?revision=&limit=`. `source` is sent as `x-source`.
    pub async fn sync(&self, revision: i64, limit: Option<i32>, source: Option<&str>) -> Result<Json> {
        let mut req = Self::get(SYNC).query("revision", revision).query_opt("limit", limit);
        if let Some(s) = source {
            req = req.header("x-source", s);
        }
        self.rest.send(req).await
    }

    /// `GET keep/fetch.json?startRevision=&endRevision=&limit=` (full sync
    /// window). `source` is sent as `x-source`.
    pub async fn fetch(
        &self,
        start_revision: i64,
        end_revision: i64,
        limit: Option<i32>,
        source: Option<&str>,
    ) -> Result<Json> {
        let mut req = Self::get(FETCH)
            .query("startRevision", start_revision)
            .query("endRevision", end_revision)
            .query_opt("limit", limit);
        if let Some(s) = source {
            req = req.header("x-source", s);
        }
        self.rest.send(req).await
    }

    /// `POST keep/create.json`.
    pub async fn create(&self, opts: KeepCreate) -> Result<Json> {
        self.rest.send(Self::post(CREATE, opts.body())).await
    }

    /// `POST keep/update.json[?force=]`.
    pub async fn update(&self, opts: KeepUpdate) -> Result<Json> {
        let req = Self::post(UPDATE, opts.body()).query_opt("force", opts.force);
        self.rest.send(req).await
    }

    /// `POST keep/delete.json` `{contentIds}`.
    pub async fn delete(&self, content_ids: &[&str]) -> Result<Json> {
        self.rest.send(Self::post(DELETE, json!({"contentIds": content_ids}))).await
    }

    /// `POST keep/message/delete.json` `{messageIds}`.
    pub async fn delete_messages(&self, message_ids: &[&str]) -> Result<Json> {
        self.rest.send(Self::post(MESSAGE_DELETE, json!({"messageIds": message_ids}))).await
    }

    /// `POST keep/obs/delete.json` `{contentData}`.
    pub async fn delete_obs(&self, content_data: Json) -> Result<Json> {
        self.rest.send(Self::post(OBS_DELETE, json!({"contentData": content_data}))).await
    }

    /// `POST keep/contents/pin.json` `{contentIds}`.
    pub async fn pin(&self, content_ids: &[&str]) -> Result<Json> {
        self.rest.send(Self::post(PIN, json!({"contentIds": content_ids}))).await
    }

    /// `POST keep/contents/unpin.json` `{contentIds}`.
    pub async fn unpin(&self, content_ids: &[&str]) -> Result<Json> {
        self.rest.send(Self::post(UNPIN, json!({"contentIds": content_ids}))).await
    }
}
