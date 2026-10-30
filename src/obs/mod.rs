//! LINE Object Storage (OBS): media upload/download for talk and square
//! messages, plain and E2EE (linejs `base/obs`).
//!
//! Reach it through [`BaseClient::obs`]:
//!
//! ```no_run
//! # async fn f(client: line_rs::BaseClient) -> line_rs::Result<()> {
//! use line_rs::obs::{ObjType, ObsFile};
//! let png = std::fs::read("cat.png")?;
//! let up = client.obs().upload_obj_talk("c...", ObjType::Image, ObsFile::new(png).mime("image/png"), None, None).await?;
//! let file = client.obs().download_message_data("123", false, false).await?;
//! # let _ = (up, file); Ok(()) }
//! ```

pub mod crypto;
mod e2ee;
pub mod mime;

pub use e2ee::{E2eeMediaUpload, e2ee_content_metadata, e2ee_duration};

use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};

use crate::base::BaseClient;
use crate::error::{Error, Result};

/// Default OBS host.
pub const OBS_PREFIX: &str = "https://obs.line-apps.com/";

/// Media kind of an uploaded object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ObjType {
    #[default]
    Image,
    Gif,
    Video,
    Audio,
    File,
}

impl ObjType {
    pub fn as_str(self) -> &'static str {
        match self {
            ObjType::Image => "image",
            ObjType::Gif => "gif",
            ObjType::Video => "video",
            ObjType::Audio => "audio",
            ObjType::File => "file",
        }
    }
}

impl std::fmt::Display for ObjType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// File contents plus optional name and MIME type (the `File`/`Blob` of linejs).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObsFile {
    pub data: Bytes,
    pub name: Option<String>,
    pub mime: Option<String>,
}

impl ObsFile {
    pub fn new(data: impl Into<Bytes>) -> Self {
        Self { data: data.into(), name: None, mime: None }
    }
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
    pub fn mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = Some(mime.into());
        self
    }
    pub fn len(&self) -> usize {
        self.data.len()
    }
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    /// Extension from the name, falling back to the MIME table.
    pub fn extension(&self) -> Option<String> {
        self.name
            .as_deref()
            .and_then(|n| n.rsplit('.').next())
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .or_else(|| self.mime.as_deref().and_then(mime::extension).map(str::to_string))
    }
}

/// Result of an upload: object id/hash from `x-obs-oid` / `x-obs-hash`.
#[derive(Debug, Clone)]
pub struct ObsUpload {
    pub obj_id: String,
    pub obj_hash: String,
    pub headers: HeaderMap,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ObsImageDetails {
    #[serde(deserialize_with = "lenient::string")]
    pub format: String,
    #[serde(deserialize_with = "lenient::int")]
    pub height: i64,
    #[serde(deserialize_with = "lenient::int")]
    pub width: i64,
    #[serde(deserialize_with = "lenient::string")]
    pub signature: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ObsMediaDetails {
    #[serde(deserialize_with = "lenient::int")]
    pub size: i64,
    #[serde(deserialize_with = "lenient::int")]
    pub duration_millis: i64,
    #[serde(deserialize_with = "lenient::opt_int")]
    pub height: Option<i64>,
    #[serde(deserialize_with = "lenient::opt_int")]
    pub width: Option<i64>,
    #[serde(deserialize_with = "lenient::string")]
    pub format: String,
    #[serde(deserialize_with = "lenient::string")]
    pub status: String,
}

/// `object_info.obs` response.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ObsMetadata {
    #[serde(deserialize_with = "lenient::string")]
    pub status: String,
    #[serde(deserialize_with = "lenient::string")]
    pub name: String,
    #[serde(deserialize_with = "lenient::string")]
    pub mime: String,
    #[serde(rename = "type")]
    #[serde(deserialize_with = "lenient::string")]
    pub kind: String,
    #[serde(deserialize_with = "lenient::string")]
    pub hash: String,
    #[serde(deserialize_with = "lenient::string")]
    pub cksum: String,
    /// Number or numeric string depending on the object.
    pub size: serde_json::Value,
    #[serde(deserialize_with = "lenient::int")]
    pub ctime_millis: i64,
    pub image_details: Option<ObsImageDetails>,
    pub video_mp4_details: Option<ObsMediaDetails>,
    pub audio_m4a_details: Option<ObsMediaDetails>,
    #[serde(deserialize_with = "lenient::string")]
    pub svc: String,
    #[serde(deserialize_with = "lenient::int")]
    pub offset: i64,
    #[serde(deserialize_with = "lenient::string")]
    pub ctime: String,
    #[serde(deserialize_with = "lenient::string")]
    pub oid: String,
    #[serde(deserialize_with = "lenient::string")]
    pub userid: String,
    #[serde(deserialize_with = "lenient::string")]
    pub sid: String,
    /// Fields not modelled above.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Lenient field parsers: `object_info.obs` sends `null` or numeric strings
/// in places; linejs reads the reply untyped, so a surprise there must not
/// fail the whole download.
mod lenient {
    use serde::{Deserialize, Deserializer};
    use serde_json::Value;

    pub fn string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        Ok(match Value::deserialize(d)? {
            Value::String(s) => s,
            Value::Null => String::new(),
            other => other.to_string(),
        })
    }

    pub fn opt_int<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
        Ok(match Value::deserialize(d)? {
            Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        })
    }

    pub fn int<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        Ok(opt_int(d)?.unwrap_or_default())
    }
}

/// Options for [`Obs::upload_object_for_service`].
#[derive(Debug, Clone, Default)]
pub struct ObjectUpload {
    pub data: Bytes,
    pub o_type: ObjType,
    /// Path below `/r/` (default `myhome/h`).
    pub obs_path: Option<String>,
    /// Extra `X-Obs-Params` entries; these override `type`/`ver`/`name`.
    pub params: Vec<(String, String)>,
    /// Object name (default: a random UUID).
    pub filename: Option<String>,
    pub add_headers: Vec<(String, String)>,
}

/// OBS operations bound to a client.
#[derive(Clone)]
pub struct Obs<'a> {
    pub client: &'a BaseClient,
    prefix: String,
}

impl BaseClient {
    /// LINE Object Storage.
    pub fn obs(&self) -> Obs<'_> {
        Obs { client: self, prefix: OBS_PREFIX.to_string() }
    }
}

/// A failed download carries only the status: obs error pages embed signed urls.
fn ensure_ok(status: reqwest::StatusCode, what: &str) -> Result<()> {
    if status.is_success() {
        Ok(())
    } else {
        Err(Error::internal("ObsError", format!("{what}: HTTP {}", status.as_u16())))
    }
}

fn to_header_map(headers: impl IntoIterator<Item = (String, String)>) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for (k, v) in headers {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| Error::internal("ObsError", format!("invalid header name `{k}`: {e}")))?;
        let value =
            HeaderValue::from_str(&v).map_err(|e| Error::internal("ObsError", format!("invalid header `{k}`: {e}")))?;
        map.insert(name, value);
    }
    Ok(map)
}

/// Merge `overrides` into `base`, keeping the original position of keys
/// that already exist (object-spread semantics).
fn merge_params(base: &mut Vec<(String, String)>, overrides: impl IntoIterator<Item = (String, String)>) {
    for (k, v) in overrides {
        match base.iter_mut().find(|(bk, _)| *bk == k) {
            Some(slot) => slot.1 = v,
            None => base.push((k, v)),
        }
    }
}

/// Base64 JSON object for the `X-Obs-Params` header.
pub fn obs_params_header(params: &[(String, String)]) -> String {
    use base64::Engine;
    let body: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{}:{}", serde_json::Value::from(k.as_str()), serde_json::Value::from(v.as_str())))
        .collect();
    base64::engine::general_purpose::STANDARD.encode(format!("{{{}}}", body.join(",")))
}

fn header_str(headers: &HeaderMap, name: &str) -> String {
    headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string()
}

impl<'a> Obs<'a> {
    /// Use another OBS host (must end with `/`).
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// URL of a message's media (`/preview` for the thumbnail).
    pub fn message_data_url(&self, message_id: &str, preview: bool, square: bool) -> String {
        format!(
            "{}r/{}/m/{message_id}{}",
            self.prefix,
            if square { "g2" } else { "talk" },
            if preview { "/preview" } else { "" }
        )
    }

    /// URL of a message's `object_info.obs`.
    pub fn message_metadata_url(&self, message_id: &str, square: bool) -> String {
        format!("{}r/{}/m/{message_id}/object_info.obs", self.prefix, if square { "g2" } else { "talk" })
    }

    fn read_headers(&self) -> Result<HeaderMap> {
        let token = self
            .client
            .auth_token()
            .ok_or_else(|| Error::internal("Not setup yet", "Please call 'login()' first"))?;
        to_header_map([
            ("accept".to_string(), "application/json, text/plain, */*".to_string()),
            ("x-line-application".to_string(), self.client.device_details.application()),
            ("x-line-access".to_string(), token),
        ])
    }

    /// Download a (non-E2EE) message's media; the name comes from its metadata.
    pub async fn download_message_data(&self, message_id: &str, preview: bool, square: bool) -> Result<ObsFile> {
        let headers = self.read_headers()?;
        let res = self
            .client
            .http
            .get(self.message_data_url(message_id, preview, square))
            .headers(headers)
            .timeout(self.client.config.long_timeout)
            .send()
            .await?;
        ensure_ok(res.status(), "Message data download failed")?;
        let mime = res.headers().get("content-type").and_then(|v| v.to_str().ok()).map(str::to_string);
        let data = res.bytes().await?;
        let info = self.message_obs_metadata(message_id, square).await?;
        Ok(ObsFile { data, name: Some(info.name), mime })
    }

    /// `object_info.obs` of a message's media.
    pub async fn message_obs_metadata(&self, message_id: &str, square: bool) -> Result<ObsMetadata> {
        let headers = self.read_headers()?;
        let res = self
            .client
            .http
            .get(self.message_metadata_url(message_id, square))
            .headers(headers)
            .timeout(self.client.config.timeout)
            .send()
            .await?;
        ensure_ok(res.status(), "Message metadata request failed")?;
        Ok(res.json().await?)
    }

    /// Upload media for a talk (or square, for `m`/`t` mids) message. With
    /// `oid` the object is attached to an existing message id; otherwise LINE
    /// sends it to `to` itself. `duration_ms` (audio/video) is shown verbatim
    /// as the clip length (default 1919).
    pub async fn upload_obj_talk(
        &self,
        to: &str,
        o_type: ObjType,
        file: ObsFile,
        oid: Option<&str>,
        duration_ms: Option<u64>,
    ) -> Result<ObsUpload> {
        self.client.require_token()?;
        let ext = file.mime.as_deref().and_then(mime::extension).unwrap_or("undefined");
        let reqseq = self.client.get_reqseq("talk").await?;
        let name = file.name.clone().unwrap_or_else(|| format!("linejs.{ext}"));
        let mut params: Vec<(String, String)> = vec![
            ("ver".into(), "2.0".into()),
            ("name".into(), name.clone()),
            ("type".into(), o_type.as_str().into()),
        ];
        match oid {
            Some(oid) => params.push(("oid".into(), oid.into())),
            None => {
                params.push(("oid".into(), "reqseq".into()));
                params.push(("tomid".into(), to.into()));
                params.push(("reqseq".into(), reqseq.to_string()));
            }
        }
        match o_type {
            ObjType::Image => params.push(("cat".into(), "original".into())),
            ObjType::Gif => {
                params.push(("cat".into(), "original".into()));
                merge_params(&mut params, [("type".to_string(), "image".to_string())]);
            }
            ObjType::Audio | ObjType::Video => {
                params.push(("duration".into(), duration_ms.unwrap_or(1919).to_string()));
            }
            ObjType::File => {}
        }
        let to_type = if to.starts_with('m') || to.starts_with('t') { "g2" } else { "talk" };
        self.upload_object_for_service(ObjectUpload {
            data: file.data,
            o_type,
            obs_path: Some(format!("{to_type}/m/{}", oid.unwrap_or("reqseq"))),
            params,
            filename: Some(name),
            add_headers: Vec::new(),
        })
        .await
    }

    /// Raw upload to `/r/{obs_path}`.
    pub async fn upload_object_for_service(&self, opts: ObjectUpload) -> Result<ObsUpload> {
        let obs_path = opts.obs_path.as_deref().unwrap_or("myhome/h");
        self.client.log(
            "Obs.uploadObjectForService",
            serde_json::json!({"oType": opts.o_type.as_str(), "obsPath": obs_path, "size": opts.data.len()}),
        );
        if opts.data.is_empty() {
            return Err(Error::internal("ObsError", "No data to send."));
        }
        let filename = opts.filename.clone().unwrap_or_else(uuid_v4);
        let mut params =
            vec![("type".to_string(), opts.o_type.as_str().to_string()), ("ver".into(), "2.0".into()), ("name".into(), filename)];
        merge_params(&mut params, opts.params);

        let mut headers = self.client.line_headers("POST");
        merge_params(&mut headers, [
            ("content-type".to_string(), "application/octet-stream".to_string()),
            ("x-obs-params".to_string(), obs_params_header(&params)),
        ]);
        let headers = to_header_map(headers.into_iter().chain(opts.add_headers))?;

        let res = self
            .client
            .http
            .post(format!("{}r/{obs_path}", self.prefix))
            .headers(headers)
            .timeout(self.client.config.long_timeout)
            .body(opts.data)
            .send()
            .await?;
        let headers = res.headers().clone();
        let obj_id = header_str(&headers, "x-obs-oid");
        let obj_hash = header_str(&headers, "x-obs-hash");
        self.client.log(
            "Obs.uploadObjectForServiceResponse",
            serde_json::json!({"objId": obj_id, "objHash": obj_hash, "status": res.status().as_u16()}),
        );
        Ok(ObsUpload { obj_id, obj_hash, headers })
    }

    /// Raw download from `/r/{obs_path}/{oid}` (or `{oid}` substituted in
    /// `obs_path`).
    pub async fn download_object_for_service(
        &self,
        obs_path: &str,
        oid: &str,
        add_headers: Vec<(String, String)>,
    ) -> Result<Bytes> {
        let path = if obs_path.contains("{oid}") { obs_path.replacen("{oid}", oid, 1) } else { format!("{obs_path}/{oid}") };
        let headers = to_header_map(self.client.line_headers("GET").into_iter().chain(add_headers))?;
        let res = self
            .client
            .http
            .get(format!("{}r/{path}", self.prefix))
            .headers(headers)
            .timeout(self.client.config.long_timeout)
            .send()
            .await?;
        ensure_ok(res.status(), "Object download failed")?;
        Ok(res.bytes().await?)
    }
}

/// Random RFC 4122 v4 UUID.
pub(crate) fn uuid_v4() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

#[cfg(test)]
mod tests;
