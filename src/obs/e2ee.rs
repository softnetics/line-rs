//! Sending media the way the Android app does, and E2EE ("Letter
//! Sealing") media downloads.
//!
//! A send first asks which flow the chat uses (see [`MediaFlows`]):
//!
//! * **V1**: plain upload to `talk/m/reqseq`; LINE creates the message.
//! * **V2**: encrypt, upload to `talk/{emi|emv|ema|emf}/reqid-<uuid>`, then
//!   `sendMessage` with the key material in an E2EE payload. Images and
//!   videos also get `{oid}__ud-preview`; videos are encrypted with the
//!   chunked (v2) HMAC and get `{oid}__ud-hash`.
//!
//! When `sendMessage` fails with an E2EE code, the message is re-encrypted
//! (or the group key recreated) and sent again with the same object and
//! `reqSeq`; when LINE wants it plain (`E2EE_RETRY_PLAIN`,
//! `E2EE_GROUP_TOO_MANY_MEMBERS`, `REFRESH_MEDIA_FLOW`, ...) the chat's
//! flow is forgotten and the file goes through V1 instead.

use async_trait::async_trait;
use bytes::Bytes;
use indexmap::IndexMap;

use super::crypto::{self, encrypt_by_key_material};
use super::flow::{MediaFlow, MediaFlows};
use super::meta::{self, MediaGroup};
use super::{
    DownloadSink, ObjType, ObjectUpload, Obs, ObsDownload, ObsFile, ObsUpload, stream_body,
};
use crate::base::BaseClient;
use crate::e2ee::crypto::decode_base64;
use crate::e2ee::crypto::derive_key_material;
use crate::e2ee::media::{
    ChunkHashes, MEDIA_HMAC_LEN, MediaDecryptor, MediaHmacVersion, VerifiedMediaDecryptor,
    encrypt_media,
};
use crate::e2ee::{E2eeContent, E2eeSendRecovery, send_recovery};
use crate::error::{Error, Result};
use crate::thrift::{self, Protocol, TType, TValue, Value};

/// Largest file LINE accepts (`yq8/d.i`: 1 GiB).
pub const MAX_FILE_SIZE: usize = 1_073_741_824;
/// Largest video LINE accepts (`a41/m0.a`: 200 MiB).
pub const MAX_VIDEO_SIZE: usize = 209_715_200;
/// `sendMessage` attempts for one media message (`xn8/k.c`).
pub const MAX_MEDIA_SEND_ATTEMPTS: u32 = 3;

/// Result of sending media.
#[derive(Debug, Clone)]
pub enum SentMedia {
    /// Plain OBS upload in "reqseq" mode: the server created the message.
    /// `group_id` is the photo group LINE put it in (`x-line-message-gid`).
    Plain {
        obj_id: String,
        obj_hash: String,
        group_id: Option<String>,
    },
    /// Letter Sealing upload: the sent (still encrypted) `Message`.
    E2ee(Value),
}

/// Optional details of a media message.
///
/// ```
/// use line_rs::obs::MediaOptions;
/// let opts = MediaOptions::new()
///     .preview(vec![0xff, 0xd8])
///     .duration_ms(4200)
///     .size(1280, 720);
/// # let _ = opts;
/// ```
#[derive(Debug, Clone, Default)]
pub struct MediaOptions {
    /// JPEG thumbnail of an image or video, uploaded as `__ud-preview`.
    /// Without one an image is its own preview and a video has none.
    pub preview: Option<Bytes>,
    /// Length of a video or audio clip in milliseconds (`DURATION`).
    pub duration_ms: Option<u64>,
    /// Image width in pixels (`MEDIA_CONTENT_INFO`).
    pub width: Option<u32>,
    /// Image height in pixels (`MEDIA_CONTENT_INFO`).
    pub height: Option<u32>,
    /// Whether the image is animated. Defaults to true for
    /// [`ObjType::Gif`].
    pub animated: Option<bool>,
    /// `None`: use the flow LINE picks for the chat. `Some(false)`: always
    /// upload plain (V1). `Some(true)`: always encrypt (V2), and fail rather
    /// than fall back to plain.
    pub e2ee: Option<bool>,
    /// Place of this image in a photo group; such images always go plain
    /// (V1), as in the app. See [`Obs::send_image_group`].
    pub group: Option<MediaGroup>,
    /// Send without a notification on the recipients' devices
    /// (`NOTIFICATION_DISABLED`).
    pub silent: bool,
}

impl MediaOptions {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn preview(mut self, jpeg: impl Into<Bytes>) -> Self {
        self.preview = Some(jpeg.into());
        self
    }
    pub fn duration_ms(mut self, ms: u64) -> Self {
        self.duration_ms = Some(ms);
        self
    }
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }
    pub fn animated(mut self, animated: bool) -> Self {
        self.animated = Some(animated);
        self
    }
    pub fn e2ee(mut self, e2ee: bool) -> Self {
        self.e2ee = Some(e2ee);
        self
    }
    pub fn group(mut self, group: MediaGroup) -> Self {
        self.group = Some(group);
        self
    }
    pub fn silent(mut self, silent: bool) -> Self {
        self.silent = silent;
        self
    }
}

/// Options for [`Obs::upload_media_by_e2ee`].
#[derive(Debug, Clone, Default)]
pub struct E2eeMediaUpload {
    /// Plain media; `name`/`mime` decide the extension and file name.
    pub file: ObsFile,
    pub o_type: ObjType,
    /// User (`u`) or group (`c`) mid.
    pub to: String,
    /// Optional JPEG thumbnail (image/gif/video), encrypted with the same
    /// key material. Without it an image is uploaded as its own preview and
    /// a video has none.
    pub preview: Option<Bytes>,
    /// Video or audio length in milliseconds, sent as `DURATION`. Rounded
    /// like `Math.round`; dropped unless it is a positive safe integer.
    pub duration_ms: Option<f64>,
    /// Image width and height for `MEDIA_CONTENT_INFO`.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Defaults to true for GIFs.
    pub animated: Option<bool>,
}

/// `(obsNamespace, contentType)` for an E2EE media kind.
pub(super) fn type_set(o_type: ObjType) -> (&'static str, i32) {
    match o_type {
        ObjType::Image | ObjType::Gif => ("emi", 1),
        ObjType::Video => ("emv", 2),
        ObjType::Audio => ("ema", 3),
        ObjType::File => ("emf", 14),
    }
}

fn is_image(o_type: ObjType) -> bool {
    matches!(o_type, ObjType::Image | ObjType::Gif)
}

/// Whether `mid` can take V2 media: users and groups. Rooms and squares
/// always go plain.
fn v2_capable(mid: &str) -> bool {
    mid.starts_with('u') || mid.starts_with('c')
}

/// Fail before uploading what LINE would refuse: files over 1 GiB, videos
/// over 200 MiB. The error kind is `MediaTooLarge`.
pub fn check_media_size(o_type: ObjType, len: usize) -> Result<()> {
    let message = match o_type {
        ObjType::Video if len > MAX_VIDEO_SIZE => "The video is larger than 200MB.",
        _ if len > MAX_FILE_SIZE => "The file is larger than 1GB.",
        _ => return Ok(()),
    };
    Err(Error::internal("MediaTooLarge", message))
}

fn round_duration(d: f64) -> Option<u64> {
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
    if !d.is_finite() {
        return None;
    }
    let floor = d.floor();
    let rounded = if d - floor >= 0.5 { floor + 1.0 } else { floor };
    (rounded > 0.0 && rounded <= MAX_SAFE).then_some(rounded as u64)
}

/// `DURATION` value for a video or audio clip: JS `Math.round`, then only
/// positive safe integers survive.
pub fn e2ee_duration(o_type: ObjType, duration_ms: Option<f64>) -> Option<String> {
    if !matches!(o_type, ObjType::Video | ObjType::Audio) {
        return None;
    }
    round_duration(duration_ms?).map(|d| d.to_string())
}

/// `contentMetadata` of the message announcing an uploaded E2EE object,
/// as the Android app writes it (`yq8/*`, `tq8/t1`): `FILE_SIZE` is the
/// plaintext size; images carry `MEDIA_CONTENT_INFO`; videos and audio
/// `DURATION`. The file name travels only in the encrypted payload.
pub fn e2ee_content_metadata(
    o_type: ObjType,
    obj_id: &str,
    file_size: usize,
    extension: Option<&str>,
    opts: &MediaOptions,
) -> IndexMap<String, String> {
    let (namespace, _) = type_set(o_type);
    let mut meta = IndexMap::new();
    meta.insert("SID".to_string(), namespace.to_string());
    meta.insert("OID".to_string(), obj_id.to_string());
    if file_size > 0 {
        meta.insert("FILE_SIZE".to_string(), file_size.to_string());
    }
    meta.insert("e2eeVersion".to_string(), "2".to_string());
    if is_image(o_type) {
        meta.insert(
            "MEDIA_CONTENT_INFO".to_string(),
            media_content_info(o_type, file_size, extension, opts),
        );
    }
    if matches!(o_type, ObjType::Video | ObjType::Audio)
        && let Some(d) = opts.duration_ms.filter(|d| *d > 0)
    {
        meta.insert("DURATION".to_string(), d.to_string());
    }
    if opts.silent {
        meta.insert("NOTIFICATION_DISABLED".to_string(), "true".to_string());
    }
    meta
}

/// `{"category":"original","fileSize":…,"extension":…,"animated":true,
/// "width":…,"height":…}` (`tq8/t1.m`); `animated` only when true, the
/// size only when known.
fn media_content_info(
    o_type: ObjType,
    file_size: usize,
    extension: Option<&str>,
    opts: &MediaOptions,
) -> String {
    let animated = opts.animated.unwrap_or(o_type == ObjType::Gif);
    let extension = if o_type == ObjType::Gif {
        "gif".to_string()
    } else {
        extension
            .map(str::to_ascii_lowercase)
            .unwrap_or_else(|| "jpg".to_string())
    };
    let mut info = format!(
        "{{\"category\":\"original\",\"fileSize\":{file_size},\"extension\":{}",
        serde_json::Value::from(extension)
    );
    if animated {
        info.push_str(",\"animated\":true");
    }
    if let Some(w) = opts.width {
        info.push_str(&format!(",\"width\":{w}"));
    }
    if let Some(h) = opts.height {
        info.push_str(&format!(",\"height\":{h}"));
    }
    info.push('}');
    info
}

/// `x-obs-params` of a V2 upload (`a41/p.a`, `tx3/a.a`): type is the enum
/// name `FILE` for every kind, and `range` covers the whole body.
pub(crate) fn v2_params(name: &str, len: usize) -> Vec<(String, String)> {
    vec![
        ("ver".to_string(), "2.0".to_string()),
        ("name".to_string(), name.to_string()),
        ("type".to_string(), "FILE".to_string()),
        (
            "range".to_string(),
            format!("bytes 0-{}/{len}", len.saturating_sub(1)),
        ),
    ]
}

/// The side effects of a media send, split out so the flow can be tested
/// without a server.
#[async_trait]
pub(crate) trait MediaBackend: Sync {
    async fn reqseq(&self) -> Result<i64>;
    /// The chat's flows (cached until their TTL).
    async fn media_flows(&self, chat: &str) -> Result<MediaFlows>;
    fn drop_media_flow(&self, chat: &str);
    /// V2 upload with extra `headers`; returns the object id.
    async fn upload(
        &self,
        data: Bytes,
        obs_path: String,
        params: Vec<(String, String)>,
        headers: Vec<(String, String)>,
    ) -> Result<String>;
    /// V1 upload in reqseq mode with extra `headers`.
    async fn upload_plain(
        &self,
        to: &str,
        o_type: ObjType,
        file: ObsFile,
        duration_ms: Option<u64>,
        reqseq: i64,
        headers: Vec<(String, String)>,
    ) -> Result<ObsUpload>;
    /// Server-side copy of the V2 object `src_oid` of message
    /// `src_message_id` (`copy.obs`); returns the new object id.
    async fn copy(&self, o_type: ObjType, src_oid: &str, src_message_id: &str) -> Result<String>;
    async fn encrypt_message(
        &self,
        to: &str,
        key_material: &str,
        file_name: Option<&str>,
        content_type: i32,
    ) -> Result<Vec<Vec<u8>>>;
    async fn prepare_resend(&self, to: &str, recovery: E2eeSendRecovery) -> Result<()>;
    async fn send(
        &self,
        to: &str,
        chunks: Vec<Vec<u8>>,
        content_type: i32,
        metadata: IndexMap<String, String>,
        seq: i64,
    ) -> Result<Value>;
}

#[async_trait]
impl MediaBackend for Obs<'_> {
    async fn reqseq(&self) -> Result<i64> {
        self.client.get_reqseq("talk").await
    }

    async fn media_flows(&self, chat: &str) -> Result<MediaFlows> {
        Obs::media_flows(self, chat).await
    }

    fn drop_media_flow(&self, chat: &str) {
        self.client.drop_media_flow(Some(chat));
    }

    async fn upload(
        &self,
        data: Bytes,
        obs_path: String,
        params: Vec<(String, String)>,
        headers: Vec<(String, String)>,
    ) -> Result<String> {
        let content_type = params
            .iter()
            .find(|(k, _)| k == "name")
            .map(|(_, name)| super::mime::content_type_for_name(name))
            .unwrap_or("");
        let mut add_headers = vec![("content-type".to_string(), content_type.to_string())];
        add_headers.extend(headers);
        let up = self
            .upload_with_obs_token(
                ObjectUpload {
                    data,
                    o_type: ObjType::File,
                    obs_path: Some(obs_path),
                    params,
                    add_headers,
                    ..Default::default()
                },
                self.client,
            )
            .await?;
        // The message refers to the object by this id, so it can't be sent
        // without one.
        if up.obj_id.is_empty() {
            return Err(Error::internal(
                "ObsError",
                "Object upload failed: no object id",
            ));
        }
        Ok(up.obj_id)
    }

    async fn upload_plain(
        &self,
        to: &str,
        o_type: ObjType,
        file: ObsFile,
        duration_ms: Option<u64>,
        reqseq: i64,
        headers: Vec<(String, String)>,
    ) -> Result<ObsUpload> {
        self.client.require_token()?;
        self.upload_obj_talk_with_seq(
            to,
            o_type,
            file,
            None,
            duration_ms,
            reqseq,
            headers,
            self.client,
        )
        .await
    }

    async fn copy(&self, o_type: ObjType, src_oid: &str, src_message_id: &str) -> Result<String> {
        self.copy_e2ee_object(o_type, src_oid, src_message_id).await
    }

    async fn encrypt_message(
        &self,
        to: &str,
        key_material: &str,
        file_name: Option<&str>,
        content_type: i32,
    ) -> Result<Vec<Vec<u8>>> {
        let mut payload = serde_json::json!({"keyMaterial": key_material});
        if let Some(name) = file_name {
            payload["fileName"] = name.into();
        }
        self.client
            .encrypt_e2ee_message(to, E2eeContent::Json(payload), content_type, 2)
            .await
    }

    async fn prepare_resend(&self, to: &str, recovery: E2eeSendRecovery) -> Result<()> {
        self.client.prepare_e2ee_resend(to, recovery).await
    }

    async fn send(
        &self,
        to: &str,
        chunks: Vec<Vec<u8>>,
        content_type: i32,
        metadata: IndexMap<String, String>,
        seq: i64,
    ) -> Result<Value> {
        send_chunked_message(self.client, to, chunks, content_type, metadata, seq).await
    }
}

/// `talk.sendMessage` with pre-encrypted chunks.
async fn send_chunked_message(
    client: &BaseClient,
    to: &str,
    chunks: Vec<Vec<u8>>,
    content_type: i32,
    metadata: IndexMap<String, String>,
    seq: i64,
) -> Result<Value> {
    let mut message = crate::tv!({
        "to": to,
        "createdTime": 0,
        "deliveredTime": 0,
        "hasContent": false,
        "contentType": content_type,
        "sessionId": 0,
    });
    message.insert(
        "contentMetadata",
        Value::Struct(
            metadata
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect(),
        ),
    );
    message.insert(
        "chunks",
        Value::List(chunks.into_iter().map(Value::Binary).collect()),
    );
    let mut args = crate::tv!({"seq": seq});
    args.insert("message", message);
    client.talk().call("sendMessage", args).await
}

/// One media message to send.
#[derive(Debug, Clone)]
pub(crate) struct MediaSend {
    pub to: String,
    pub o_type: ObjType,
    pub file: ObsFile,
    pub opts: MediaOptions,
}

/// An uploaded V2 object and the key that opens it.
pub(super) struct V2Object {
    pub obj_id: String,
    pub key_material: String,
}

/// The flow a send to `to` takes: forced by `e2ee`, plain for rooms and
/// squares, otherwise what LINE says for the chat.
pub(super) async fn choose_flow(
    backend: &dyn MediaBackend,
    to: &str,
    o_type: ObjType,
    e2ee: Option<bool>,
) -> Result<MediaFlow> {
    Ok(match e2ee {
        Some(false) => MediaFlow::V1,
        _ if !v2_capable(to) => {
            if e2ee == Some(true) {
                return Err(Error::internal("ObsError", "Invalid mid"));
            }
            MediaFlow::V1
        }
        Some(true) => MediaFlow::V2,
        None => match backend.media_flows(to).await {
            Ok(flows) => flows.get(o_type),
            Err(e) => {
                // Keep sending as before when LINE can't say: encrypted
                // for users and groups.
                tracing::debug!(error = %e, "determineMediaMessageFlow failed");
                MediaFlow::V2
            }
        },
    })
}

/// Send `m` through the flow its chat uses; see the module docs.
pub(crate) async fn send_media(backend: &dyn MediaBackend, m: MediaSend) -> Result<SentMedia> {
    if m.file.is_empty() {
        return Err(Error::internal("ObsError", "No data to send."));
    }
    check_media_size(m.o_type, m.file.len())?;
    // Images of a photo group always go plain (`yq8/n` builds its
    // uploaders with `q31.a.b.V1`).
    let flow = if m.opts.group.is_some() {
        MediaFlow::V1
    } else {
        choose_flow(backend, &m.to, m.o_type, m.opts.e2ee).await?
    };
    // One request number for the upload and every resend (`uq8/y3`).
    let seq = backend.reqseq().await?;
    if flow == MediaFlow::V1 {
        return send_v1(backend, &m, seq).await;
    }

    let object = upload_v2(backend, &m).await?;
    let metadata = e2ee_content_metadata(
        m.o_type,
        &object.obj_id,
        m.file.len(),
        m.file.extension().as_deref(),
        &m.opts,
    );
    let file_name = (m.o_type == ObjType::File).then(|| payload_file_name(&m.file));
    match send_object(backend, &m.to, m.o_type, &object, metadata, file_name, seq).await? {
        Sent::Message(message) => Ok(SentMedia::E2ee(message)),
        Sent::WantsPlain(err) if m.opts.e2ee == Some(true) => Err(err),
        Sent::WantsPlain(_) => send_v1(backend, &m, seq).await,
    }
}

/// How sending the E2EE message of an uploaded object ended.
pub(super) enum Sent {
    Message(Value),
    /// LINE wants this chat's media plain; the error says why.
    WantsPlain(Error),
}

/// `sendMessage` for an uploaded V2 `object`, re-encrypting on E2EE errors
/// (`xn8/k.c`).
pub(super) async fn send_object(
    backend: &dyn MediaBackend,
    to: &str,
    o_type: ObjType,
    object: &V2Object,
    metadata: IndexMap<String, String>,
    file_name: Option<String>,
    seq: i64,
) -> Result<Sent> {
    let (_, content_type) = type_set(o_type);
    let mut attempts = 0;
    loop {
        attempts += 1;
        let chunks = backend
            .encrypt_message(to, &object.key_material, file_name.as_deref(), content_type)
            .await?;
        let err = match backend
            .send(to, chunks, content_type, metadata.clone(), seq)
            .await
        {
            Ok(message) => return Ok(Sent::Message(message)),
            Err(e) => e,
        };
        let code = err.code().unwrap_or_default();
        let refresh_flow = matches!(code, "REFRESH_MEDIA_FLOW" | "122");
        let recovery = send_recovery(code);
        // SENDER_DISABLED / SENDER_NOT_ALLOWED: this account can't send
        // E2EE right now, so this one goes plain; the chat's flow stays.
        let send_plain = recovery == Some(E2eeSendRecovery::SendPlain);
        if refresh_flow || send_plain || recovery == Some(E2eeSendRecovery::RetryPlain) {
            if !send_plain {
                backend.drop_media_flow(to);
            }
            return Ok(Sent::WantsPlain(err));
        }
        match recovery {
            Some(r) if attempts < MAX_MEDIA_SEND_ATTEMPTS => {
                backend.prepare_resend(to, r).await?;
            }
            _ => return Err(err),
        }
    }
}

/// `X-Talk-Meta` (or `X-Square-Meta`) for an upload of `m`, if any.
fn upload_headers(m: &MediaSend) -> Result<Vec<(String, String)>> {
    Ok(
        meta::upload_meta(&m.to, m.opts.group.as_ref(), m.opts.silent)?
            .into_iter()
            .collect(),
    )
}

async fn send_v1(backend: &dyn MediaBackend, m: &MediaSend, seq: i64) -> Result<SentMedia> {
    let up = backend
        .upload_plain(
            &m.to,
            m.o_type,
            m.file.clone(),
            m.opts.duration_ms,
            seq,
            upload_headers(m)?,
        )
        .await?;
    let group_id = super::header_str(&up.headers, "x-line-message-gid");
    Ok(SentMedia::Plain {
        obj_id: up.obj_id,
        obj_hash: up.obj_hash,
        group_id: (!group_id.is_empty()).then_some(group_id),
    })
}

/// `fileName` of a FILE message's E2EE payload.
fn payload_file_name(file: &ObsFile) -> String {
    file.name.clone().unwrap_or_else(|| match file.extension() {
        Some(ext) => format!("file.{ext}"),
        None => "file".to_string(),
    })
}

/// `name` of the main object: the file's own name; for a video (and a
/// nameless file) `<uuid>.<ext>` as the app does (`a41/h0`).
fn upload_name(o_type: ObjType, file: &ObsFile) -> String {
    let ext = file.extension();
    match (&file.name, o_type) {
        (Some(name), t) if t != ObjType::Video => name.clone(),
        (_, ObjType::Video) => format!("{}.{}", super::uuid_v4(), ext.as_deref().unwrap_or("mp4")),
        _ => match ext {
            Some(ext) => format!("{}.{ext}", super::uuid_v4()),
            None => super::uuid_v4(),
        },
    }
}

/// Encrypt and upload the object, its preview and (video) its chunk
/// hashes (`a41/p.b`).
async fn upload_v2(backend: &dyn MediaBackend, m: &MediaSend) -> Result<V2Object> {
    let (namespace, _) = type_set(m.o_type);
    let video = m.o_type == ObjType::Video;
    let enc = encrypt_media(
        &m.file.data,
        None,
        if video {
            MediaHmacVersion::V2
        } else {
            MediaHmacVersion::V1
        },
    );
    let data = Bytes::from(enc.data);
    let name = upload_name(m.o_type, &m.file);
    let temp_id = format!("reqid-{}", super::uuid_v4());
    let params = v2_params(&name, data.len());
    // Only the main object carries the upload metadata (`a41/p.b`).
    let obj_id = backend
        .upload(
            data.clone(),
            format!("talk/{namespace}/{temp_id}"),
            params,
            upload_headers(m)?,
        )
        .await?;

    // The preview is a separate v1 file under the same key. An image is
    // its own preview; a video without a thumbnail gets none.
    let preview = match (&m.opts.preview, m.o_type) {
        (Some(p), ObjType::Image | ObjType::Gif | ObjType::Video) => {
            use base64::Engine;
            let key = base64::engine::general_purpose::STANDARD
                .decode(&enc.key_material)
                .map_err(|e| Error::internal("E2EEMediaError", e.to_string()))?;
            Some(Bytes::from(
                encrypt_by_key_material(p, Some(&key)).encrypted_data,
            ))
        }
        (None, ObjType::Image | ObjType::Gif) => Some(data),
        _ => None,
    };
    if let Some(preview) = preview {
        let params = v2_params("preview.jpg", preview.len());
        backend
            .upload(
                preview,
                format!("talk/{namespace}/{obj_id}__ud-preview"),
                params,
                Vec::new(),
            )
            .await?;
    }

    // The app uploads the chunk digests alone (`xx3/f.a()`); the HMAC that
    // covers them ends the video itself.
    if let Some(object) = enc.hash_object {
        let digests = Bytes::from(object[..object.len() - MEDIA_HMAC_LEN].to_vec());
        let params = v2_params("hash", digests.len());
        backend
            .upload(
                digests,
                format!("talk/{namespace}/{obj_id}__ud-hash"),
                params,
                Vec::new(),
            )
            .await?;
    }

    Ok(V2Object {
        obj_id,
        key_material: enc.key_material,
    })
}

/// `X-Talk-Meta` header value for downloading the object of message `id`.
pub(crate) fn talk_meta(message_id: &str) -> Result<String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let s = thrift::write_struct(
        &[
            (4, TValue::Binary(message_id.as_bytes().to_vec())),
            (27, TValue::List(TType::Struct, Vec::new())),
        ],
        Protocol::Binary,
    )?;
    let json = format!("{{\"message\":{}}}", serde_json::Value::from(b64.encode(s)));
    Ok(b64.encode(json))
}

impl From<E2eeMediaUpload> for MediaSend {
    fn from(u: E2eeMediaUpload) -> Self {
        MediaSend {
            opts: MediaOptions {
                preview: u.preview,
                duration_ms: u
                    .duration_ms
                    .filter(|_| matches!(u.o_type, ObjType::Video | ObjType::Audio))
                    .and_then(round_duration),
                width: u.width,
                height: u.height,
                animated: u.animated,
                e2ee: Some(true),
                ..Default::default()
            },
            to: u.to,
            o_type: u.o_type,
            file: u.file,
        }
    }
}

/// [`Obs::upload_media_by_e2ee`] against any backend.
pub(crate) async fn upload_media_by_e2ee(
    backend: &dyn MediaBackend,
    opts: E2eeMediaUpload,
) -> Result<Value> {
    match send_media(backend, opts.into()).await? {
        SentMedia::E2ee(message) => Ok(message),
        SentMedia::Plain { .. } => Err(Error::internal("ObsError", "media was sent without E2EE")),
    }
}

impl Obs<'_> {
    /// Send media to `to` the way the official app does: plain or with
    /// Letter Sealing as LINE decides for the chat (see [`MediaOptions::e2ee`]
    /// to force either), recovering from E2EE send errors.
    pub async fn send_media(
        &self,
        to: &str,
        o_type: ObjType,
        file: ObsFile,
        opts: MediaOptions,
    ) -> Result<SentMedia> {
        send_media(
            self,
            MediaSend {
                to: to.to_string(),
                o_type,
                file,
                opts,
            },
        )
        .await
    }

    /// Encrypt and upload media for an E2EE chat, then send the message that
    /// carries the key material. Returns the sent `Message`. Never falls
    /// back to a plain upload.
    pub async fn upload_media_by_e2ee(&self, opts: E2eeMediaUpload) -> Result<Value> {
        upload_media_by_e2ee(self, opts).await
    }

    /// Download and decrypt the media of an E2EE message, reading
    /// `keyMaterial` / `fileName` from its decrypted payload.
    pub async fn download_e2ee_message_media(&self, message: &Value) -> Result<Option<ObsFile>> {
        let payload = self.client.decrypt_e2ee_data_message(message).await?;
        let key_material = payload["keyMaterial"].as_str().ok_or_else(|| {
            Error::internal("E2EEMediaError", "decrypted payload has no keyMaterial")
        })?;
        let file_name = payload["fileName"].as_str().unwrap_or("file");
        self.download_media_by_e2ee(message, key_material, file_name)
            .await
    }

    /// Download and decrypt the media of an E2EE `message` into memory.
    ///
    /// `key_material` (base64) and `file_name` are the fields of the
    /// message's decrypted E2EE payload (linejs `decryptE2EEDataMessage`).
    /// Returns `None` when the message has no chunks. Use
    /// [`download_media_by_e2ee_to`](Self::download_media_by_e2ee_to) for
    /// big files.
    pub async fn download_media_by_e2ee(
        &self,
        message: &Value,
        key_material: &str,
        file_name: &str,
    ) -> Result<Option<ObsFile>> {
        let mut data = Vec::new();
        Ok(self
            .download_media_by_e2ee_to(message, key_material, &mut data)
            .await?
            .map(|_| ObsFile::new(data).name(file_name)))
    }

    /// [`download_e2ee_message_media`](Self::download_e2ee_message_media)
    /// streamed into `out`.
    pub async fn download_e2ee_message_media_to(
        &self,
        message: &Value,
        out: DownloadSink<'_>,
    ) -> Result<Option<ObsDownload>> {
        let payload = self.client.decrypt_e2ee_data_message(message).await?;
        let key_material = payload["keyMaterial"].as_str().ok_or_else(|| {
            Error::internal("E2EEMediaError", "decrypted payload has no keyMaterial")
        })?;
        self.download_media_by_e2ee_to(message, key_material, out)
            .await
    }

    /// Stream the media of an E2EE `message` into `out`, decrypting as it
    /// arrives; memory stays at one 128 KiB chunk whatever the file size.
    /// The HMAC (v1 or v2) is checked at the end: on an error, discard what
    /// was written. Returns `None` when the message has no chunks.
    pub async fn download_media_by_e2ee_to(
        &self,
        message: &Value,
        key_material: &str,
        out: DownloadSink<'_>,
    ) -> Result<Option<ObsDownload>> {
        let to = message["to"].as_str().unwrap_or_default();
        if !(to.starts_with('u') || to.starts_with('c')) {
            return Err(Error::internal("ObsError", "Invalid mid"));
        }
        if message["chunks"].as_list().is_none_or(|c| c.is_empty()) {
            return Ok(None);
        }
        let (sid, oid, meta) = e2ee_object(message)?;
        let res = self
            .get_chat_object(&format!("talk/{sid}"), &oid, &meta)
            .await?;
        self.decrypt_body(res, &decode_base64(key_material), out)
            .await
            .map(Some)
    }

    /// [`stream_media_by_e2ee_to`](Self::stream_media_by_e2ee_to), reading
    /// `keyMaterial` from the message's decrypted payload.
    pub async fn stream_e2ee_message_media_to(
        &self,
        message: &Value,
        out: DownloadSink<'_>,
        on_len: &mut (dyn FnMut(u64) + Send),
    ) -> Result<Option<ObsDownload>> {
        let payload = self.client.decrypt_e2ee_data_message(message).await?;
        let key_material = payload["keyMaterial"].as_str().ok_or_else(|| {
            Error::internal("E2EEMediaError", "decrypted payload has no keyMaterial")
        })?;
        self.stream_media_by_e2ee_to(message, key_material, out, on_len)
            .await
    }

    /// Stream the media of an E2EE `message` into `out` so it can be played
    /// while it downloads, as LINE for Android plays a video (`kc8/d`):
    /// beside the object it fetches the chunk digests (`{oid}/ud-hash`, on
    /// HTTP 404 the upload name `{oid}__ud-hash`) and the file's last 32
    /// bytes with a Range request (`kc8/j`), checks the digests against
    /// that HMAC, and then writes each 128 KiB chunk only once its digest
    /// matched. Everything written is checked: on an error what was
    /// written so far is still good, but the file is incomplete.
    ///
    /// `on_len` hears the plaintext length before the first byte, when the
    /// server says how long the object is. A file without chunk digests
    /// (an image, or a video sent with the whole-file HMAC) streams as
    /// [`download_media_by_e2ee_to`](Self::download_media_by_e2ee_to)
    /// does, checked only at the end. Returns `None` when the message has
    /// no chunks.
    pub async fn stream_media_by_e2ee_to(
        &self,
        message: &Value,
        key_material: &str,
        out: DownloadSink<'_>,
        on_len: &mut (dyn FnMut(u64) + Send),
    ) -> Result<Option<ObsDownload>> {
        let to = message["to"].as_str().unwrap_or_default();
        if !(to.starts_with('u') || to.starts_with('c')) {
            return Err(Error::internal("ObsError", "Invalid mid"));
        }
        if message["chunks"].as_list().is_none_or(|c| c.is_empty()) {
            return Ok(None);
        }
        let (sid, oid, meta) = e2ee_object(message)?;
        let path = format!("talk/{sid}");
        let key = decode_base64(key_material);
        let res = self.get_chat_object(&path, &oid, &meta).await?;
        let enc_len = res.content_length();
        let hashes = match self.chunk_hashes(&path, &oid, &meta, &key, enc_len).await {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::debug!(error = %e, "no chunk hashes; checking the whole file");
                None
            }
        };
        if let Some(n) = enc_len.and_then(|n| n.checked_sub(MEDIA_HMAC_LEN as u64)) {
            on_len(n);
        }
        let Some(hashes) = hashes else {
            return self.decrypt_body(res, &key, out).await.map(Some);
        };
        let mime = super::content_type(&res);
        let mut decryptor = VerifiedMediaDecryptor::new(&key, hashes);
        let mut len = 0u64;
        stream_body(res, self.client.config.download_idle_timeout, |c| {
            decryptor.push(c);
            while let Some(plain) = decryptor.next_chunk() {
                let plain = plain?;
                len += plain.len() as u64;
                out.write_all(&plain)?;
            }
            Ok(())
        })
        .await?;
        let last = decryptor.finish()?;
        len += last.len() as u64;
        out.write_all(&last)?;
        out.flush()?;
        Ok(Some(ObsDownload { len, mime }))
    }

    /// The checked chunk digests of an E2EE object whose ciphertext is
    /// `enc_len` bytes, fetched together with its trailing HMAC.
    async fn chunk_hashes(
        &self,
        path: &str,
        oid: &str,
        meta: &[(String, String)],
        key_material: &[u8],
        enc_len: Option<u64>,
    ) -> Result<ChunkHashes> {
        let digests = async {
            match self
                .download_chat_object(path, &format!("{oid}/ud-hash"), meta)
                .await
            {
                Err(e) if e.http_status() == Some(404) => {
                    self.download_chat_object(path, &format!("{oid}__ud-hash"), meta)
                        .await
                }
                other => other,
            }
        };
        let tail = async {
            let start = enc_len?.checked_sub(MEDIA_HMAC_LEN as u64)?;
            let mut headers = meta.to_vec();
            headers.push(("range".into(), format!("bytes={start}-")));
            let res = self
                .get_chat_object(path, oid, &headers)
                .await
                .inspect_err(|e| tracing::debug!(error = %e, "media HMAC range request"))
                .ok()?;
            // A server that ignores Range would send the whole file again.
            if res.status() != reqwest::StatusCode::PARTIAL_CONTENT
                && res.content_length() != Some(MEDIA_HMAC_LEN as u64)
            {
                return None;
            }
            let tail = res.bytes().await.ok()?;
            (tail.len() == MEDIA_HMAC_LEN).then_some(tail)
        };
        let (digests, tail) = tokio::join!(digests, tail);
        let mac_key = derive_key_material(key_material).mac_key;
        ChunkHashes::parse(&digests?, &mac_key, tail.as_deref())
    }

    /// Decrypt `res` into `out` as it arrives, checking the HMAC (v1 or v2)
    /// at the end.
    async fn decrypt_body(
        &self,
        res: reqwest::Response,
        key_material: &[u8],
        out: DownloadSink<'_>,
    ) -> Result<ObsDownload> {
        let mime = super::content_type(&res);
        let mut decryptor = MediaDecryptor::new(key_material);
        let mut len = 0u64;
        stream_body(res, self.client.config.download_idle_timeout, |c| {
            let plain = decryptor.update(c);
            len += plain.len() as u64;
            Ok(out.write_all(&plain)?)
        })
        .await?;
        decryptor.finish()?;
        out.flush()?;
        Ok(ObsDownload { len, mime })
    }

    /// Download and decrypt the thumbnail of an E2EE image or video,
    /// reading `keyMaterial` from its decrypted payload. See
    /// [`download_e2ee_preview`](Self::download_e2ee_preview).
    pub async fn download_e2ee_message_preview(&self, message: &Value) -> Result<Vec<u8>> {
        let payload = self.client.decrypt_e2ee_data_message(message).await?;
        let key_material = payload["keyMaterial"].as_str().ok_or_else(|| {
            Error::internal("E2EEMediaError", "decrypted payload has no keyMaterial")
        })?;
        self.download_e2ee_preview(message, key_material).await
    }

    /// Download and decrypt the thumbnail the sender uploaded beside an
    /// E2EE image or video, encrypted with the same `key_material`.
    ///
    /// Asks for `r/talk/{sid}/{oid}/ud-preview`, as LINE for Android does
    /// (`e80/f0.b`, MESSAGE_IMAGE_THUMB), and on HTTP 404 for the name it
    /// was uploaded under, `{oid}__ud-preview` (unverified that OBS serves
    /// that name). Never downloads the media itself: a video without a
    /// thumbnail fails with the 404.
    pub async fn download_e2ee_preview(
        &self,
        message: &Value,
        key_material: &str,
    ) -> Result<Vec<u8>> {
        let (sid, oid, meta) = e2ee_object(message)?;
        let path = format!("talk/{sid}");
        let data = match self
            .download_chat_object(&path, &format!("{oid}/ud-preview"), &meta)
            .await
        {
            Err(e) if e.http_status() == Some(404) => {
                self.download_chat_object(&path, &format!("{oid}__ud-preview"), &meta)
                    .await?
            }
            other => other?,
        };
        crypto::decrypt_by_key_material_b64(&data, key_material)
    }
}

/// `[X-Talk-Meta]`, the header of an E2EE object download.
type TalkMetaHeader = Vec<(String, String)>;

/// `(sid, oid, [X-Talk-Meta])` of an E2EE media message.
fn e2ee_object(message: &Value) -> Result<(String, String, TalkMetaHeader)> {
    let meta = &message["contentMetadata"];
    let oid = meta["OID"].as_str().unwrap_or_default();
    let sid = meta["SID"].as_str().unwrap_or_default();
    if oid.is_empty() || sid.is_empty() {
        return Err(Error::internal("ObsError", "message has no OID/SID"));
    }
    let id = message["id"].as_str().unwrap_or_default();
    Ok((
        sid.to_string(),
        oid.to_string(),
        vec![("X-Talk-Meta".into(), talk_meta(id)?)],
    ))
}
