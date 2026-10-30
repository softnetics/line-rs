//! Moa: the LINE Album REST API (JSON over HTTPS through the LEGY host).
//!
//! Unlike the thrift services, every Moa request carries an
//! `X-Line-ChannelToken` issued for [`MOA_CHANNEL_ID`] by
//! `ChannelService.approveChannelAndIssueChannelToken`.
//!
//! **Token cache.** [`BaseClient::moa`] hands out a short-lived borrowing
//! struct, so the channel token is memoised in the client's
//! [`Storage`](crate::storage::Storage) under [`CHANNEL_TOKEN_KEY`], tagged
//! with the access token, mid and endpoint it was issued for; a change of any
//! of them re-issues it. Issuance is serialised by a process-wide lock so
//! concurrent callers share one approval. Call
//! [`MoaService::clear_album_channel_token`] after the token expires or is
//! revoked.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::base::BaseClient;
use crate::error::{Error, Result};

/// Channel id of the album (Moa) subsystem.
pub const MOA_CHANNEL_ID: &str = "1375220249";
/// Storage key of the memoised channel token.
pub const CHANNEL_TOKEN_KEY: &str = "moaChannelToken";

const LEGY_MOA_PREFIX: &str = "/ext/album";
const ERROR_KIND: &str = "MoaError";

static ISSUE_LOCK: Mutex<()> = Mutex::const_new(());

/// An album.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MoaAlbum {
    pub album_id: serde_json::Value,
    pub chat_id: String,
    pub title: Option<String>,
    pub photo_count: Option<i64>,
    pub create_time: Option<i64>,
    pub update_time: Option<i64>,
}

/// One page of albums.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MoaAlbumsResult {
    pub albums: Vec<MoaAlbum>,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub has_more: Option<bool>,
}

/// `/moa/v2/albums` response.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MoaAlbumsResponse {
    pub code: Option<i64>,
    pub message: Option<String>,
    pub result: Option<MoaAlbumsResult>,
}

/// OBS location of a photo.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObsResourceId {
    pub oid: Option<String>,
    pub svc: Option<String>,
    pub sid: Option<String>,
}

/// A photo or video in an album.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AlbumPhoto {
    pub id: Option<serde_json::Value>,
    pub photo_id: Option<serde_json::Value>,
    pub oid: Option<String>,
    pub obs_resource_id: Option<ObsResourceId>,
    pub shot_time: Option<i64>,
    pub create_user_mid: Option<String>,
    pub owner_mid: Option<String>,
    pub resource_type: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

/// One page of photos.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AlbumPhotosResult {
    pub photos: Vec<AlbumPhoto>,
    pub next_cursor: Option<String>,
}

/// `/api/v6/albums/{id}/photos` response.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlbumPhotosResponse {
    pub code: Option<i64>,
    pub message: Option<String>,
    pub result: Option<AlbumPhotosResult>,
}

/// Options for [`MoaService::get_albums`].
#[derive(Debug, Clone, Default)]
pub struct GetAlbumsOptions {
    pub cursor: Option<String>,
    /// Default `createTimeDesc`.
    pub order_by: Option<String>,
    pub include: Option<String>,
}

/// Options for [`MoaService::get_photos`].
#[derive(Debug, Clone, Default)]
pub struct GetPhotosOptions {
    pub chat_id: String,
    pub album_id: String,
    pub cursor: Option<String>,
    /// Default 100.
    pub page_size: Option<u32>,
    /// Default `createTimeDesc`.
    pub order_by: Option<String>,
    /// Default `all`.
    pub include: Option<String>,
    pub filter_type: Option<String>,
    pub target_user: Option<String>,
}

/// Options for [`MoaService::download_photo`].
#[derive(Debug, Clone, Default)]
pub struct DownloadPhotoOptions {
    pub chat_id: String,
    pub album_id: String,
    pub oid: String,
    /// `album/a` (still image, default) or `album/v` (video).
    pub prefix: Option<String>,
}

/// `encodeURIComponent`.
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `https://{endpoint}/ext/album{path_suffix}?k=v&...`; `None` values are dropped.
pub fn build_moa_url(endpoint: &str, path_suffix: &str, params: &[(&str, Option<String>)]) -> String {
    build_moa_url_with_base(&format!("https://{endpoint}"), path_suffix, params)
}

/// [`build_moa_url`] under an explicit `scheme://host` base.
pub fn build_moa_url_with_base(base: &str, path_suffix: &str, params: &[(&str, Option<String>)]) -> String {
    let base = format!("{}{LEGY_MOA_PREFIX}{path_suffix}", base.trim_end_matches('/'));
    let query: Vec<String> = params
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| format!("{}={}", encode_uri_component(k), encode_uri_component(v))))
        .collect();
    if query.is_empty() { base } else { format!("{base}?{}", query.join("&")) }
}

fn encode_path_id(id: &str) -> Result<String> {
    if id.is_empty() || id == "." || id == ".." {
        return Err(Error::internal(ERROR_KIND, "album and object IDs must be nonempty path segments"));
    }
    Ok(encode_uri_component(id))
}

fn header_set(headers: &mut Vec<(String, String)>, k: &str, v: impl Into<String>) {
    headers.retain(|(hk, _)| !hk.eq_ignore_ascii_case(k));
    headers.push((k.to_string(), v.into()));
}

/// Client for the album REST API; see the [module docs](self).
#[derive(Clone, Copy)]
pub struct MoaService<'a> {
    pub client: &'a BaseClient,
    base_url: Option<&'a str>,
}

impl BaseClient {
    /// The album (Moa) REST API.
    pub fn moa(&self) -> MoaService<'_> {
        MoaService { client: self, base_url: None }
    }
}

impl<'a> MoaService<'a> {
    /// Send requests to `base` (e.g. `http://127.0.0.1:8080`) instead of
    /// `https://{endpoint}`.
    pub fn with_base_url(mut self, base: &'a str) -> Self {
        self.base_url = Some(base);
        self
    }

    fn url(&self, path_suffix: &str, params: &[(&str, Option<String>)]) -> String {
        match self.base_url {
            Some(base) => build_moa_url_with_base(base, path_suffix, params),
            None => build_moa_url(&self.client.endpoint, path_suffix, params),
        }
    }

    /// Forget the memoised channel token (after expiry or revocation).
    pub async fn clear_album_channel_token(&self) -> Result<()> {
        self.client.storage.delete(CHANNEL_TOKEN_KEY).await
    }

    /// Issue (and memoise) the album channel token.
    pub async fn get_album_channel_token(&self) -> Result<String> {
        let (Some(auth_token), Some(mid)) = (self.client.auth_token(), self.client.mid()) else {
            return Err(Error::internal(ERROR_KIND, "login must complete before calling Moa"));
        };
        let endpoint = self.client.endpoint.as_str();
        let _guard = ISSUE_LOCK.lock().await;
        if let Some(cached) = self.client.storage.get(CHANNEL_TOKEN_KEY).await?
            && cached["authToken"] == auth_token.as_str()
            && cached["mid"] == mid.as_str()
            && cached["endpoint"] == endpoint
            && let Some(v) = cached["value"].as_str()
        {
            return Ok(v.to_string());
        }
        let res = self
            .client
            .channel()
            .approve_channel_and_issue_channel_token(crate::tv!({"channelId": MOA_CHANNEL_ID}))
            .await?;
        let token = match res["channelAccessToken"].as_str() {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => return Err(Error::internal(ERROR_KIND, "channelAccessToken missing from channel response")),
        };
        let entry = serde_json::json!({"authToken": auth_token, "mid": mid, "endpoint": endpoint, "value": token});
        self.client.storage.set(CHANNEL_TOKEN_KEY, entry).await?;
        Ok(token)
    }

    async fn fetch<T: DeserializeOwned>(
        &self,
        path_suffix: &str,
        params: &[(&str, Option<String>)],
        extra_headers: &[(&str, &str)],
    ) -> Result<T> {
        // linejs POSTs these with `x-lhm: GET`; LEGY routes on `x-lhm`.
        self.request_lhm(reqwest::Method::POST, "GET", path_suffix, params, extra_headers, None).await
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path_suffix: &str,
        params: &[(&str, Option<String>)],
        extra_headers: &[(&str, &str)],
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let lhm = method.as_str().to_string();
        self.request_lhm(method, &lhm, path_suffix, params, extra_headers, body).await
    }

    async fn request_lhm<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        lhm: &str,
        path_suffix: &str,
        params: &[(&str, Option<String>)],
        extra_headers: &[(&str, &str)],
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let Some(mid) = self.client.mid() else {
            return Err(Error::internal(
                ERROR_KIND,
                "client.profile.mid not populated — login must complete before calling Moa",
            ));
        };
        let token = self.get_album_channel_token().await?;
        // Moa is JSON REST: the thrift accept/content-type headers are rejected
        // with code 102001.
        let mut headers = self.client.line_headers(lhm);
        header_set(&mut headers, "accept", "application/json");
        header_set(&mut headers, "content-type", "application/json; charset=UTF-8");
        header_set(&mut headers, "X-Line-ChannelToken", token);
        header_set(&mut headers, "X-Line-Mid", mid);
        for (k, v) in extra_headers {
            header_set(&mut headers, k, *v);
        }
        let url = self.url(path_suffix, params);
        let payload = match &body {
            Some(b) => serde_json::to_vec(b)?,
            None => Vec::new(),
        };
        let mut req = self.client.http.request(method, url).timeout(self.client.config.timeout).body(payload);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let res = req.send().await?;
        let status = res.status();
        if !status.is_success() {
            return Err(Error::internal(ERROR_KIND, format!("Moa {path_suffix} HTTP {}", status.as_u16())));
        }
        let bytes = res.bytes().await?;
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| Error::internal(ERROR_KIND, "Moa returned invalid JSON"))?;
        check_moa_response(path_suffix, &json)?;
        Ok(serde_json::from_value(json)?)
    }

    /// The caller's albums, one page at a time (pass the previous `cursor`).
    pub async fn get_albums(&self, options: GetAlbumsOptions) -> Result<MoaAlbumsResponse> {
        let params = [
            ("cursor", Some(options.cursor.unwrap_or_default())),
            ("orderBy", Some(options.order_by.unwrap_or_else(|| "createTimeDesc".into()))),
            ("include", Some(options.include.unwrap_or_default())),
        ];
        self.fetch("/moa/v2/albums", &params, &[]).await
    }

    /// Photos of an album in `chat_id`, one page at a time.
    pub async fn get_photos(&self, options: GetPhotosOptions) -> Result<AlbumPhotosResponse> {
        let path = format!("/api/v6/albums/{}/photos", encode_path_id(&options.album_id)?);
        let params = [
            ("cursor", Some(options.cursor.unwrap_or_default())),
            ("pageSize", Some(options.page_size.unwrap_or(100).to_string())),
            ("orderBy", Some(options.order_by.unwrap_or_else(|| "createTimeDesc".into()))),
            ("include", Some(options.include.unwrap_or_else(|| "all".into()))),
            ("filterType", Some(options.filter_type.unwrap_or_default())),
            ("targetUser", options.target_user),
        ];
        self.fetch(&path, &params, &[("X-Line-Chat-Id", &options.chat_id)]).await
    }

    /// Download the original bytes of an album photo (or video with
    /// `prefix = "album/v"`).
    pub async fn download_photo(&self, options: DownloadPhotoOptions) -> Result<Vec<u8>> {
        let prefix = options.prefix.as_deref().unwrap_or("album/a");
        if prefix != "album/a" && prefix != "album/v" {
            return Err(Error::internal(ERROR_KIND, "unsupported album resource prefix"));
        }
        let oid = encode_path_id(&options.oid)?;
        let token = self.get_album_channel_token().await?;
        let mut headers = self.client.line_headers("GET");
        header_set(&mut headers, "X-Line-ChannelToken", token);
        header_set(&mut headers, "X-Line-Album", options.album_id.as_str());
        header_set(&mut headers, "X-Line-Mid", options.chat_id.as_str());
        let url = format!("https://{}/oa/r/{prefix}/{oid}", self.client.endpoint);
        let mut req = self.client.http.post(url).timeout(self.client.config.timeout).body(Vec::new());
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let res = req.send().await?;
        if !res.status().is_success() {
            return Err(Error::internal(ERROR_KIND, format!("OBS {} HTTP {}", options.oid, res.status().as_u16())));
        }
        Ok(res.bytes().await?.to_vec())
    }

    // --- group album writes ------------------------------------------------
    //
    // Derived from static RE of LINE macOS 26.5.0; unverified against live
    // server. Evidence: `line::album::*` command vtables at
    // `0x105e17700..0x105e18c00` (server type 29 → `/ext/album/api`, method,
    // path pieces from the string block at `0x105820745`, query/body keys,
    // `x-line-chat-id` header). The schema of `photos` entries and the
    // `likeType` values were not recovered and are passed through.

    async fn album_call(
        &self,
        method: reqwest::Method,
        path: String,
        params: &[(&str, Option<String>)],
        chat_id: &str,
        extra: Option<(&str, &str)>,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let mut headers = vec![("x-line-chat-id", chat_id)];
        headers.extend(extra);
        self.request(method, &path, params, &headers, body).await
    }

    fn album_path(album_id: &str, rest: &str) -> Result<String> {
        Ok(format!("/api/v6/albums/{}{rest}", encode_path_id(album_id)?))
    }

    fn photo_path(album_id: &str, photo_id: &str, rest: &str) -> Result<String> {
        Self::album_path(album_id, &format!("/photos/{}{rest}", encode_path_id(photo_id)?))
    }

    /// `POST /ext/album/api/v6/albums/create[?modifyDuplicateTitle=]` `{title}`.
    pub async fn create_album(
        &self,
        chat_id: &str,
        title: &str,
        modify_duplicate_title: Option<bool>,
    ) -> Result<serde_json::Value> {
        let params = [("modifyDuplicateTitle", modify_duplicate_title.map(|b| b.to_string()))];
        let body = serde_json::json!({"title": title});
        self.album_call(reqwest::Method::POST, "/api/v6/albums/create".into(), &params, chat_id, None, Some(body)).await
    }

    /// `POST .../albums/{albumId}/update` `{title}`.
    pub async fn update_album_title(&self, chat_id: &str, album_id: &str, title: &str) -> Result<serde_json::Value> {
        let path = Self::album_path(album_id, "/update")?;
        let body = serde_json::json!({"title": title});
        self.album_call(reqwest::Method::POST, path, &[], chat_id, None, Some(body)).await
    }

    /// `POST .../albums/{albumId}/delete`.
    pub async fn delete_album(&self, chat_id: &str, album_id: &str) -> Result<serde_json::Value> {
        let path = Self::album_path(album_id, "/delete")?;
        self.album_call(reqwest::Method::POST, path, &[], chat_id, None, None).await
    }

    /// `POST .../albums/{albumId}/share` (share into the chat).
    pub async fn share_album(&self, chat_id: &str, album_id: &str) -> Result<serde_json::Value> {
        let path = Self::album_path(album_id, "/share")?;
        self.album_call(reqwest::Method::POST, path, &[], chat_id, None, None).await
    }

    /// `POST .../albums/{albumId}/photos/create` `{photos}` — register
    /// already-uploaded OBS objects; `photos` entries are sent verbatim.
    pub async fn create_photos(
        &self,
        chat_id: &str,
        album_id: &str,
        photos: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let path = Self::album_path(album_id, "/photos/create")?;
        let body = serde_json::json!({"photos": photos});
        self.album_call(reqwest::Method::POST, path, &[], chat_id, None, Some(body)).await
    }

    /// `POST .../albums/{albumId}/photos/delete` `{photoIds}`.
    pub async fn delete_photos(
        &self,
        chat_id: &str,
        album_id: &str,
        photo_ids: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let path = Self::album_path(album_id, "/photos/delete")?;
        let body = serde_json::json!({"photoIds": photo_ids});
        self.album_call(reqwest::Method::POST, path, &[], chat_id, None, Some(body)).await
    }

    /// `POST .../albums/{albumId}/photos/{photoId}/likes/create` `{likeType}`.
    /// `referrer` is sent as `x-line-album-referrer`.
    pub async fn create_photo_like(
        &self,
        chat_id: &str,
        album_id: &str,
        photo_id: &str,
        like_type: &str,
        referrer: Option<&str>,
    ) -> Result<serde_json::Value> {
        let path = Self::photo_path(album_id, photo_id, "/likes/create")?;
        let extra = referrer.map(|r| ("x-line-album-referrer", r));
        let body = serde_json::json!({"likeType": like_type});
        self.album_call(reqwest::Method::POST, path, &[], chat_id, extra, Some(body)).await
    }

    /// `POST .../albums/{albumId}/photos/{photoId}/likes/delete`.
    pub async fn delete_photo_like(&self, chat_id: &str, album_id: &str, photo_id: &str) -> Result<serde_json::Value> {
        let path = Self::photo_path(album_id, photo_id, "/likes/delete")?;
        self.album_call(reqwest::Method::POST, path, &[], chat_id, None, None).await
    }

    /// `GET .../albums/{albumId}/photos/{photoId}/likes?cursor&pageSize`.
    pub async fn get_photo_likes(
        &self,
        chat_id: &str,
        album_id: &str,
        photo_id: &str,
        cursor: Option<String>,
        page_size: Option<u32>,
    ) -> Result<serde_json::Value> {
        let path = Self::photo_path(album_id, photo_id, "/likes")?;
        let params = [("cursor", cursor), ("pageSize", page_size.map(|n| n.to_string()))];
        self.album_call(reqwest::Method::GET, path, &params, chat_id, None, None).await
    }
}

/// Reject non-object bodies and non-zero `code`s.
fn check_moa_response(path_suffix: &str, json: &serde_json::Value) -> Result<()> {
    let Some(obj) = json.as_object() else {
        return Err(Error::internal(ERROR_KIND, "Moa returned an invalid response object"));
    };
    match obj.get("code") {
        None | Some(serde_json::Value::Null) => Ok(()),
        Some(code) if code.as_i64() == Some(0) => Ok(()),
        Some(code) => {
            let message = obj.get("message").and_then(|m| m.as_str()).unwrap_or("undefined");
            Err(Error::internal(ERROR_KIND, format!("Moa {path_suffix} code={code} message={message}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::test_util::client;

    fn some(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn url_prefix_and_params() {
        assert_eq!(
            build_moa_url("legy.line-apps.com", "/moa/v2/albums", &[]),
            "https://legy.line-apps.com/ext/album/moa/v2/albums"
        );
        assert_eq!(
            build_moa_url(
                "legy.line-apps.com",
                "/moa/v2/albums",
                &[("cursor", some("")), ("orderBy", some("createTimeDesc")), ("include", some(""))]
            ),
            "https://legy.line-apps.com/ext/album/moa/v2/albums?cursor=&orderBy=createTimeDesc&include="
        );
        assert_eq!(
            build_moa_url(
                "legy.line-apps.com",
                "/api/v6/albums/42/photos",
                &[("cursor", some("")), ("pageSize", some("100")), ("filterType", some("")), ("targetUser", None)]
            ),
            "https://legy.line-apps.com/ext/album/api/v6/albums/42/photos?cursor=&pageSize=100&filterType="
        );
        assert_eq!(
            build_moa_url("legy.line-apps.com", "/moa/v2/albums", &[("cursor", some("abc/def="))]),
            "https://legy.line-apps.com/ext/album/moa/v2/albums?cursor=abc%2Fdef%3D"
        );
        assert_eq!(
            build_moa_url("legy-proxy.example.com", "/moa/v2/albums", &[]),
            "https://legy-proxy.example.com/ext/album/moa/v2/albums"
        );
    }

    #[test]
    fn path_ids() {
        assert_eq!(encode_path_id("a/b?").unwrap(), "a%2Fb%3F");
        assert_eq!(encode_path_id("photo/?#").unwrap(), "photo%2F%3F%23");
        for bad in ["", ".", ".."] {
            assert!(encode_path_id(bad).is_err());
        }
    }

    #[test]
    fn response_validation() {
        assert!(check_moa_response("/x", &serde_json::json!({"code": 0, "result": {"albums": []}})).is_ok());
        assert!(check_moa_response("/x", &serde_json::json!({"result": {}})).is_ok());
        let err = check_moa_response("/x", &serde_json::json!({"code": 102001, "message": "error"})).unwrap_err();
        assert!(err.to_string().contains("code=102001"), "{err}");
        for bad in [serde_json::Value::Null, serde_json::json!([])] {
            let err = check_moa_response("/x", &bad).unwrap_err();
            assert!(err.to_string().contains("invalid response object"), "{err}");
        }
        let parsed: MoaAlbumsResponse =
            serde_json::from_value(serde_json::json!({"code": 0, "result": {"albums": []}})).unwrap();
        assert_eq!(parsed.result.unwrap().albums, Vec::new());
    }

    #[tokio::test]
    async fn requires_login_and_valid_ids_before_any_request() {
        let c = client();
        let err = c.moa().get_albums(GetAlbumsOptions::default()).await.unwrap_err();
        assert!(err.to_string().contains("login"), "{err}");
        c.set_profile(Some(crate::tv!({"mid": "u-self"})));
        let opts =
            DownloadPhotoOptions { chat_id: "c-chat".into(), album_id: "1".into(), oid: "photo".into(), prefix: None };
        let err = c.moa().download_photo(opts.clone()).await.unwrap_err();
        assert!(err.to_string().contains("login"), "{err}");

        c.set_auth_token(Some("access-token".into()));
        let err = c
            .moa()
            .get_photos(GetPhotosOptions { chat_id: "c".into(), album_id: "..".into(), ..Default::default() })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("MoaError"), "{err}");
        let err = c.moa().download_photo(DownloadPhotoOptions { oid: "..".into(), ..opts.clone() }).await.unwrap_err();
        assert!(err.to_string().contains("MoaError"), "{err}");
        let err = c.moa().download_photo(DownloadPhotoOptions { prefix: some("../other"), ..opts }).await.unwrap_err();
        assert!(err.to_string().contains("unsupported album resource prefix"), "{err}");
    }

    #[tokio::test]
    async fn album_writes() {
        let ok = (200, r#"{"code":0,"result":{}}"#);
        let (base, seen) = crate::rest::tests::serve(vec![ok, ok, ok, ok, (200, r#"{"code":1,"message":"x"}"#)]).await;
        let c = client();
        c.set_auth_token(Some("access-token".into()));
        c.set_profile(Some(crate::tv!({"mid": "u-self"})));
        let entry = serde_json::json!({
            "authToken": "access-token", "mid": "u-self", "endpoint": c.endpoint, "value": "album-token",
        });
        c.storage.set(CHANNEL_TOKEN_KEY, entry).await.unwrap();
        let moa = c.moa().with_base_url(&base);
        moa.create_album("c-chat", "trip", Some(true)).await.unwrap();
        moa.delete_photos("c-chat", "9", vec![serde_json::json!(1), serde_json::json!(2)]).await.unwrap();
        moa.create_photo_like("c-chat", "9", "p/1", "1003", Some("chat")).await.unwrap();
        moa.get_photo_likes("c-chat", "9", "p1", None, Some(20)).await.unwrap();
        let err = moa.delete_album("c-chat", "9").await.unwrap_err();
        assert!(err.to_string().contains("code=1"), "{err}");
        assert!(moa.delete_album("c-chat", "..").await.is_err());

        let s = seen.lock().unwrap();
        assert_eq!(
            (s[0].method.as_str(), s[0].target.as_str()),
            ("POST", "/ext/album/api/v6/albums/create?modifyDuplicateTitle=true")
        );
        assert_eq!(s[0].json(), serde_json::json!({"title": "trip"}));
        assert_eq!(s[0].header("x-line-chat-id"), Some("c-chat"));
        assert_eq!(s[0].header("x-line-channeltoken"), Some("album-token"));
        assert_eq!(s[1].target, "/ext/album/api/v6/albums/9/photos/delete");
        assert_eq!(s[1].json(), serde_json::json!({"photoIds": [1, 2]}));
        assert_eq!(s[2].target, "/ext/album/api/v6/albums/9/photos/p%2F1/likes/create");
        assert_eq!(s[2].header("x-line-album-referrer"), Some("chat"));
        assert_eq!((s[3].method.as_str(), s[3].target.as_str()), ("GET", "/ext/album/api/v6/albums/9/photos/p1/likes?pageSize=20"));
        assert_eq!(s[4].target, "/ext/album/api/v6/albums/9/delete");
        assert!(s[4].body.is_empty());
    }

    #[tokio::test]
    async fn album_list_posts_with_lhm_get_like_linejs() {
        let (base, seen) = crate::rest::tests::serve(vec![(200, r#"{"code":0,"result":{}}"#)]).await;
        let c = client();
        c.set_auth_token(Some("access-token".into()));
        c.set_profile(Some(crate::tv!({"mid": "u-self"})));
        let entry = serde_json::json!({
            "authToken": "access-token", "mid": "u-self", "endpoint": c.endpoint, "value": "album-token",
        });
        c.storage.set(CHANNEL_TOKEN_KEY, entry).await.unwrap();
        c.moa().with_base_url(&base).get_albums(GetAlbumsOptions::default()).await.unwrap();
        let s = seen.lock().unwrap();
        assert_eq!(s[0].method.as_str(), "POST");
        assert_eq!(s[0].header("x-lhm"), Some("GET"));
    }

    #[tokio::test]
    async fn cached_token_is_scoped_to_session() {
        let c = client();
        c.set_auth_token(Some("access-token".into()));
        c.set_profile(Some(crate::tv!({"mid": "u-self"})));
        let entry = serde_json::json!({
            "authToken": "access-token", "mid": "u-self", "endpoint": c.endpoint, "value": "channel-1",
        });
        c.storage.set(CHANNEL_TOKEN_KEY, entry).await.unwrap();
        assert_eq!(c.moa().get_album_channel_token().await.unwrap(), "channel-1");
        c.moa().clear_album_channel_token().await.unwrap();
        assert!(c.storage.get(CHANNEL_TOKEN_KEY).await.unwrap().is_none());
    }
}
