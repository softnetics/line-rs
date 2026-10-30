//! Timeline / VOOM / Note posts REST API (linejs `base/timeline`).
//!
//! The channel token from `initTimeline` is cached inside the [`Timeline`]
//! value, so keep one around instead of calling [`BaseClient::timeline`] per
//! request:
//!
//! ```no_run
//! # async fn f(client: line_rs::BaseClient) -> line_rs::Result<()> {
//! use line_rs::timeline::CreatePost;
//! let timeline = client.timeline();
//! let mut post = CreatePost::new("c...");
//! post.contents.text = Some("hello".into());
//! let res = timeline.create_post(post).await?;
//! println!("{}", res.result);
//! # Ok(()) }
//! ```

use reqwest::Method;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use tokio::sync::OnceCell;

use crate::base::BaseClient;
use crate::error::{Error, Result};
use crate::obs::OBS_PREFIX;

/// Timeline channel id used for the channel token.
pub const TIMELINE_CHANNEL_ID: &str = "1341209850";

/// Envelope of every timeline response.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimelineResponse<T = Json> {
    pub code: i64,
    pub message: String,
    pub result: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TextSizeMode {
    Auto,
    #[default]
    Normal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TextAnimation {
    #[default]
    None,
    Slide,
    Zoom,
    Buzz,
    Bounce,
    Blink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ReadPermissionType {
    #[default]
    All,
    Friend,
    Group,
    Event,
    None,
}

/// Reaction of [`Timeline::like_post`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LikeType {
    #[default]
    Good,
    Love,
    Funny,
    Amazing,
    Sad,
    Surprised,
}

impl LikeType {
    pub fn code(self) -> &'static str {
        match self {
            LikeType::Love => "1001",
            LikeType::Funny => "1002",
            LikeType::Good => "1003",
            LikeType::Amazing => "1004",
            LikeType::Surprised => "1005",
            LikeType::Sad => "1006",
        }
    }
}

/// A sticker attached to a post.
#[derive(Debug, Clone, PartialEq)]
pub struct PostSticker {
    pub id: String,
    pub package_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PostLocation {
    pub latitude: f64,
    pub longitude: f64,
    pub name: String,
}

/// Uploaded media (see [`Timeline::upload_note_media`]).
#[derive(Debug, Clone, PartialEq)]
pub struct PostMedia {
    pub object_id: String,
    /// `PHOTO` or `VIDEO`.
    pub kind: String,
}

/// Content shared by [`CreatePost`] and [`UpdatePost`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PostContents {
    pub text: Option<String>,
    pub shared_post_id: Option<String>,
    pub text_size_mode: TextSizeMode,
    /// Default `#FFFFFF`.
    pub background_color: Option<String>,
    pub text_animation: TextAnimation,
    pub holding_time: Option<i64>,
    pub stickers: Vec<PostSticker>,
    pub locations: Vec<PostLocation>,
    pub media: Vec<PostMedia>,
}

impl PostContents {
    fn background(&self) -> &str {
        self.background_color.as_deref().unwrap_or("#FFFFFF")
    }

    fn lists(&self) -> (Json, Json, Json) {
        let stickers = self
            .stickers
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "packageId": s.package_id,
                    "packageVersion": 1,
                    "hasAnimation": true,
                    "hasSound": true,
                    "stickerResourceType": "ANIMATION",
                })
            })
            .collect();
        let locations =
            self.locations.iter().map(|l| json!({"latitude": l.latitude, "longitude": l.longitude, "name": l.name})).collect();
        let media =
            self.media.iter().map(|m| json!({"objectId": m.object_id, "type": m.kind, "obsFace": "[]"})).collect();
        (Json::Array(stickers), Json::Array(locations), Json::Array(media))
    }

    fn finish(&self, post_info: &mut Json, contents: &mut Json) {
        if let Some(t) = self.holding_time {
            post_info["holdingTime"] = t.into();
        }
        if let Some(t) = &self.text {
            contents["text"] = t.as_str().into();
        }
        if let Some(id) = &self.shared_post_id {
            contents["sharedPostId"] = id.as_str().into();
        }
    }
}

/// Options for [`Timeline::create_post`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreatePost {
    /// Group (`c`) or square (`s`) mid. User homes are not supported.
    pub home_id: String,
    pub contents: PostContents,
    pub read_permission_type: ReadPermissionType,
    pub read_permission_gids: Vec<String>,
    /// Default `TIMELINE`.
    pub source_type: Option<String>,
}

impl CreatePost {
    pub fn new(home_id: impl Into<String>) -> Self {
        Self { home_id: home_id.into(), ..Default::default() }
    }

    /// Request body of `post/create.json`.
    pub fn body(&self) -> Json {
        let c = &self.contents;
        let mut post_info = json!({"readPermission": {"type": self.read_permission_type, "gids": self.read_permission_gids}});
        let (stickers, locations, media) = c.lists();
        let mut contents = json!({
            "contentsStyle": {
                "textStyle": {
                    "textSizeMode": c.text_size_mode,
                    "backgroundColor": c.background(),
                    "textAnimation": c.text_animation,
                },
                "mediaStyle": {"displayType": "GRID_1_A"},
            },
            "stickers": stickers,
            "locations": locations,
            "media": media,
        });
        c.finish(&mut post_info, &mut contents);
        json!({"postInfo": post_info, "contents": contents})
    }
}

/// Options for [`Timeline::update_post`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdatePost {
    pub home_id: String,
    pub post_id: String,
    pub contents: PostContents,
}

impl UpdatePost {
    pub fn new(home_id: impl Into<String>, post_id: impl Into<String>) -> Self {
        Self { home_id: home_id.into(), post_id: post_id.into(), ..Default::default() }
    }

    /// Request body of `post/update.json`.
    pub fn body(&self) -> Json {
        let c = &self.contents;
        let mut post_info = json!({
            "postId": self.post_id,
            "editableContents": ["ALL"],
            "readPermission": {"homeID": self.home_id},
        });
        let (stickers, locations, media) = c.lists();
        let mut contents = json!({
            "sticonMetas": [],
            "contentsStyle": {
                "textStyle": {"textSizeMode": c.text_size_mode, "textAnimation": c.text_animation},
                "stickerStyle": {"backgroundColor": c.background()},
                "mediaStyle": {},
            },
            "stickers": stickers,
            "textMeta": [],
            "locations": locations,
            "media": media,
        });
        c.finish(&mut post_info, &mut contents);
        json!({"postInfo": post_info, "contents": contents})
    }
}

/// Options for [`Timeline::list_post`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListPost {
    pub home_id: String,
    pub post_id: Option<String>,
    pub updated_time: Option<i64>,
    /// Default `TALKROOM`.
    pub source_type: Option<String>,
}

/// Note/timeline client; caches the timeline channel token and headers.
pub struct Timeline {
    client: BaseClient,
    headers: OnceCell<HeaderMap>,
}

impl BaseClient {
    /// A new [`Timeline`] (with an empty token cache) for this client.
    pub fn timeline(&self) -> Timeline {
        Timeline::new(self.clone())
    }
}

fn header_map(pairs: impl IntoIterator<Item = (&'static str, String)>) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for (k, v) in pairs {
        let value =
            HeaderValue::from_str(&v).map_err(|e| Error::internal("TimelineError", format!("invalid header `{k}`: {e}")))?;
        map.insert(HeaderName::from_static(k), value);
    }
    Ok(map)
}

fn home_prefix(home_id: &str) -> &'static str {
    if home_id.starts_with('s') { "sn" } else { "mh" }
}

/// Post CRUD prefix. LINE answers every group `/mh/api/...` call with
/// `{"code":500}` now; group notes live under `/ext/note/nt`.
fn post_prefix(home_id: &str) -> &'static str {
    if home_id.starts_with('s') { "sn" } else { "ext/note/nt" }
}

fn note_object_id(mid: &str) -> String {
    let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
    format!("{:x}", md5::compute(format!("{mid}-{millis}")))
}

impl Timeline {
    pub fn new(client: BaseClient) -> Self {
        Self { client, headers: OnceCell::new() }
    }

    pub fn client(&self) -> &BaseClient {
        &self.client
    }

    /// Issue the timeline channel token (once) and build the request headers.
    pub async fn init(&self) -> Result<&HeaderMap> {
        self.headers
            .get_or_try_init(|| async {
                let token = self
                    .client
                    .channel()
                    .call("approveChannelAndIssueChannelToken", crate::tv!({"channelId": TIMELINE_CHANNEL_ID}))
                    .await?;
                let token = token["channelAccessToken"]
                    .as_str()
                    .ok_or_else(|| Error::internal("TimelineError", "no channelAccessToken issued"))?
                    .to_string();
                let mid = self.client.mid().ok_or_else(|| Error::internal("NotAuthorized", "try login first"))?;
                let access = self.client.require_token()?;
                let d = &self.client.device_details;
                header_map([
                    ("x-line-bdbtemplateversion", "v1".to_string()),
                    ("x-lsr", "JP".to_string()),
                    ("user-agent", d.user_agent()),
                    ("x-line-channeltoken", token),
                    ("accept-encoding", "gzip".to_string()),
                    ("x-line-global-config", "discover.enable=true; follow.enable=true; reboot.phase=scenario".to_string()),
                    ("x-line-mid", mid),
                    ("x-line-access", access),
                    ("content-type", "application/json; charset=UTF-8".to_string()),
                    ("x-line-application", d.application()),
                    ("x-lal", "ja_JP".to_string()),
                    ("x-lpv", "1".to_string()),
                ])
            })
            .await
    }

    /// Cached headers, if [`Timeline::init`] has run.
    pub fn headers(&self) -> Option<&HeaderMap> {
        self.headers.get()
    }

    async fn send<T: DeserializeOwned + Default>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        lhm: &'static str,
        body: Option<Json>,
    ) -> Result<TimelineResponse<T>> {
        let mut headers = self.init().await?.clone();
        headers.insert("x-lhm", HeaderValue::from_static(lhm));
        let base = format!("https://{}{path}", self.client.endpoint);
        let url = if query.is_empty() { url::Url::parse(&base) } else { url::Url::parse_with_params(&base, query) }
            .map_err(|e| Error::internal("TimelineError", format!("invalid url: {e}")))?;
        let mut req = self.client.http.request(method, url).headers(headers).timeout(self.client.config.timeout);
        if let Some(b) = body {
            req = req.body(serde_json::to_vec(&b)?);
        }
        Ok(req.send().await?.json().await?)
    }

    pub async fn create_post(&self, opts: CreatePost) -> Result<TimelineResponse> {
        if opts.home_id.starts_with('u') {
            return Err(Error::internal("TimelineError", "Not support oto"));
        }
        let source = opts.source_type.as_deref().unwrap_or("TIMELINE");
        let path = format!("/{}/api/v57/post/create.json", post_prefix(&opts.home_id));
        self.send(Method::POST, &path, &[("homeId", &opts.home_id), ("sourceType", source)], "POST", Some(opts.body())).await
    }

    pub async fn delete_post(&self, home_id: &str, post_id: &str) -> Result<TimelineResponse> {
        let path = format!("/{}/api/v57/post/delete.json", post_prefix(home_id));
        self.send(Method::POST, &path, &[("homeId", home_id), ("postId", post_id)], "GET", None).await
    }

    pub async fn get_post(&self, home_id: &str, post_id: &str) -> Result<TimelineResponse> {
        let path = format!("/{}/api/v57/post/get.json", post_prefix(home_id));
        self.send(Method::GET, &path, &[("homeId", home_id), ("postId", post_id)], "GET", None).await
    }

    pub async fn list_post(&self, opts: ListPost) -> Result<TimelineResponse> {
        let source = opts.source_type.as_deref().unwrap_or("TALKROOM");
        let updated = opts.updated_time.filter(|t| *t != 0).map(|t| t.to_string());
        let mut query =
            vec![("homeId", opts.home_id.as_str()), ("sourceType", source), ("likeLimit", "0"), ("commentLimit", "0")];
        if let Some(id) = opts.post_id.as_deref().filter(|s| !s.is_empty()) {
            query.push(("postId", id));
        }
        if let Some(t) = updated.as_deref() {
            query.push(("updatedTime", t));
        }
        let path = format!("/{}/api/v57/post/list.json", post_prefix(&opts.home_id));
        self.send(Method::GET, &path, &query, "GET", None).await
    }

    pub async fn update_post(&self, opts: UpdatePost) -> Result<TimelineResponse> {
        if opts.home_id.is_empty() {
            return Err(Error::internal("TimelineError", "homeId is required"));
        }
        if opts.post_id.is_empty() {
            return Err(Error::internal("TimelineError", "postId is required"));
        }
        let path = format!("/{}/api/v57/post/update.json", post_prefix(&opts.home_id));
        self.send(Method::POST, &path, &[("homeId", &opts.home_id)], "POST", Some(opts.body())).await
    }

    /// React to a post (`content_id` is the post id).
    pub async fn like_post(
        &self,
        content_id: &str,
        home_id: &str,
        like_type: LikeType,
        source_type: Option<&str>,
    ) -> Result<TimelineResponse> {
        let body = json!({
            "sourceType": source_type.unwrap_or("TIMELINE"),
            "likeType": like_type.code(),
            "contentId": content_id,
        });
        self.send(Method::POST, "/ext/note/nt/api/v57/like/create.json", &[("homeId", home_id)], "POST", Some(body)).await
    }

    /// Comment on a post. `contents_list` holds media/sticker entries (see
    /// [`Timeline::upload_note_comment_image`]).
    pub async fn create_comment(
        &self,
        content_id: &str,
        comment_text: &str,
        home_id: &str,
        source_type: Option<&str>,
        contents_list: Vec<Json>,
    ) -> Result<TimelineResponse> {
        let body = json!({"commentText": comment_text, "contentId": content_id, "contentsList": contents_list});
        let query = [("sourceType", source_type.unwrap_or("TIMELINE")), ("homeId", home_id)];
        self.send(Method::POST, "/ext/note/nt/api/v57/comment/create.json", &query, "POST", Some(body)).await
    }

    /// Share a post into a chat.
    pub async fn share_post(&self, post_id: &str, chat_mid: &str, home_id: &str) -> Result<TimelineResponse> {
        let path = format!("/{}/api/v57/post/sendPostToTalk.json", home_prefix(home_id));
        self.send(Method::POST, &path, &[], "POST", Some(json!({"postId": post_id, "receiveMids": [chat_mid]}))).await
    }

    /// Upload an image or video into `myhome/h` for [`PostContents::media`]
    /// (`PHOTO` / `VIDEO`). Uses the legacy `.nhn` endpoint with a
    /// client-chosen object id; returns `(obj_id, obj_hash)`.
    pub async fn upload_note_media(&self, video: bool, data: impl Into<bytes::Bytes>) -> Result<(String, String)> {
        self.init().await?;
        let obj_id = note_object_id(&self.client.mid().unwrap_or_default());
        let res = self.upload_obj_nhn("myhome/h", &obj_id, video, data.into()).await?;
        Ok((obj_id, res.1))
    }

    /// Upload an image into `myhome/cmt` for a comment's `contentsList`.
    /// The server assigns the object id; returns `(obj_id, obj_hash)`.
    pub async fn upload_note_comment_image(&self, data: impl Into<bytes::Bytes>) -> Result<(String, String)> {
        self.init().await?;
        let seed = note_object_id(&self.client.mid().unwrap_or_default());
        let (oid, hash) = self.upload_obj_nhn("myhome/cmt", &seed, false, data.into()).await?;
        Ok((oid.unwrap_or(seed), hash))
    }

    async fn upload_obj_nhn(
        &self,
        obs_path: &str,
        oid: &str,
        video: bool,
        data: bytes::Bytes,
    ) -> Result<(Option<String>, String)> {
        let (content_type, ext, kind) = if video { ("video/mp4", "mp4", "video") } else { ("image/jpeg", "jpg", "image") };
        let params = [
            ("name".to_string(), format!("{oid}.{ext}")),
            ("oid".to_string(), oid.to_string()),
            ("type".to_string(), kind.to_string()),
            ("ver".to_string(), "2.0".to_string()),
        ];
        let mut headers = self.init().await?.clone();
        headers.extend(header_map([
            ("content-type", content_type.to_string()),
            ("content-length", data.len().to_string()),
            ("x-obs-params", crate::obs::obs_params_header(&params)),
        ])?);
        let res = self
            .client
            .http
            .post(format!("{OBS_PREFIX}{obs_path}/upload.nhn"))
            .headers(headers)
            .timeout(self.client.config.long_timeout)
            .body(data)
            .send()
            .await?;
        if res.status().as_u16() != 201 {
            return Err(Error::internal(
                "TimelineError",
                format!("Note media upload failed ({obs_path}): HTTP {}", res.status().as_u16()),
            ));
        }
        let get = |k: &str| res.headers().get(k).and_then(|v| v.to_str().ok()).map(str::to_string);
        Ok((get("x-obs-oid"), get("x-obs-hash").unwrap_or_default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_post_body() {
        let mut p = CreatePost::new("c1");
        p.contents.text = Some("hi".into());
        p.contents.holding_time = Some(5);
        p.contents.stickers.push(PostSticker { id: "1".into(), package_id: "2".into() });
        p.contents.media.push(PostMedia { object_id: "o".into(), kind: "PHOTO".into() });
        let b = p.body();
        assert_eq!(b["postInfo"]["readPermission"], json!({"type": "ALL", "gids": []}));
        assert_eq!(b["postInfo"]["holdingTime"], 5);
        assert_eq!(
            b["contents"]["contentsStyle"]["textStyle"],
            json!({"textSizeMode": "NORMAL", "backgroundColor": "#FFFFFF", "textAnimation": "NONE"})
        );
        assert_eq!(b["contents"]["text"], "hi");
        assert_eq!(b["contents"]["stickers"][0]["packageId"], "2");
        assert_eq!(b["contents"]["media"][0], json!({"objectId": "o", "type": "PHOTO", "obsFace": "[]"}));
        assert!(b["contents"].get("sharedPostId").is_none());
    }

    #[test]
    fn update_post_body() {
        let mut p = UpdatePost::new("s1", "p1");
        p.contents.text_animation = TextAnimation::Zoom;
        let b = p.body();
        assert_eq!(b["postInfo"]["postId"], "p1");
        assert_eq!(b["postInfo"]["readPermission"]["homeID"], "s1");
        assert_eq!(b["contents"]["contentsStyle"]["textStyle"]["textAnimation"], "ZOOM");
        assert_eq!(b["contents"]["contentsStyle"]["stickerStyle"]["backgroundColor"], "#FFFFFF");
        assert_eq!(home_prefix("s1"), "sn");
        assert_eq!(home_prefix("c1"), "mh");
        assert_eq!(post_prefix("s1"), "sn");
        assert_eq!(post_prefix("c1"), "ext/note/nt");
    }

    #[test]
    fn response_envelope() {
        let r: TimelineResponse = serde_json::from_str(r#"{"code":0,"message":"success","result":{"a":1}}"#).unwrap();
        assert_eq!(r.result["a"], 1);
        assert_eq!(LikeType::Good.code(), "1003");
        assert_eq!(note_object_id("u").len(), 32);
    }
}
