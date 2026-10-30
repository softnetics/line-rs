//! E2EE ("Letter Sealing") media: encrypt, upload to `talk/em*`, and send
//! the message carrying the key material; and the reverse for downloads.

use async_trait::async_trait;
use bytes::Bytes;
use indexmap::IndexMap;

use super::crypto::{self, encrypt_by_key_material};
use super::{ObjType, Obs, ObjectUpload, ObsFile};
use crate::base::BaseClient;
use crate::e2ee::E2eeContent;
use crate::error::{Error, Result};
use crate::thrift::{self, Protocol, TType, TValue, Value};

/// Options for [`Obs::upload_media_by_e2ee`].
#[derive(Debug, Clone, Default)]
pub struct E2eeMediaUpload {
    /// Plain media; `name`/`mime` decide the extension and file name.
    pub file: ObsFile,
    pub o_type: ObjType,
    /// User (`u`) or group (`c`) mid.
    pub to: String,
    /// Optional thumbnail (image/gif/video), encrypted with the same key
    /// material. Without it the full ciphertext is uploaded as the preview.
    pub preview: Option<Bytes>,
    /// Video clip length in milliseconds, sent as `DURATION`. Rounded like
    /// `Math.round`; dropped unless it is a positive safe integer.
    pub duration_ms: Option<f64>,
}

/// `(obsNamespace, contentType)` for an E2EE media kind.
fn type_set(o_type: ObjType) -> (&'static str, i32) {
    match o_type {
        ObjType::Image | ObjType::Gif => ("emi", 1),
        ObjType::Video => ("emv", 2),
        ObjType::Audio => ("ema", 3),
        ObjType::File => ("emf", 14),
    }
}

fn has_preview(o_type: ObjType) -> bool {
    matches!(o_type, ObjType::Image | ObjType::Gif | ObjType::Video)
}

/// `DURATION` value for a video: JS `Math.round`, then only positive safe
/// integers survive.
pub fn e2ee_duration(o_type: ObjType, duration_ms: Option<f64>) -> Option<String> {
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
    let d = duration_ms?;
    if o_type != ObjType::Video || !d.is_finite() {
        return None;
    }
    let floor = d.floor();
    let rounded = if d - floor >= 0.5 { floor + 1.0 } else { floor };
    (rounded > 0.0 && rounded <= MAX_SAFE).then(|| format!("{}", rounded as u64))
}

/// `contentMetadata` of the message announcing an uploaded E2EE object.
pub fn e2ee_content_metadata(
    o_type: ObjType,
    obj_id: &str,
    encrypted_size: usize,
    extension: Option<&str>,
    duration_ms: Option<f64>,
) -> IndexMap<String, String> {
    let (namespace, _) = type_set(o_type);
    let mut meta = IndexMap::new();
    meta.insert("SID".to_string(), namespace.to_string());
    meta.insert("OID".to_string(), obj_id.to_string());
    meta.insert("FILE_SIZE".to_string(), encrypted_size.to_string());
    meta.insert("e2eeVersion".to_string(), "2".to_string());
    if has_preview(o_type) {
        let ext = extension.map(|e| format!(",\"extension\":{}", serde_json::Value::from(e))).unwrap_or_default();
        meta.insert(
            "MEDIA_CONTENT_INFO".to_string(),
            format!(
                "{{\"category\":\"original\",\"fileSize\":{encrypted_size}{ext},\"animated\":{}}}",
                o_type == ObjType::Gif
            ),
        );
    }
    if let Some(d) = e2ee_duration(o_type, duration_ms) {
        meta.insert("DURATION".to_string(), d);
    }
    meta
}

/// The side effects of an E2EE upload, split out so the flow can be tested
/// without a server.
#[async_trait]
pub(crate) trait E2eeMediaBackend: Sync {
    async fn upload(&self, data: Bytes, obs_path: String, params: Vec<(String, String)>) -> Result<String>;
    async fn encrypt_message(&self, to: &str, key_material: &str, file_name: &str, content_type: i32)
    -> Result<Vec<Vec<u8>>>;
    async fn send(
        &self,
        to: &str,
        chunks: Vec<Vec<u8>>,
        content_type: i32,
        metadata: IndexMap<String, String>,
    ) -> Result<Value>;
}

#[async_trait]
impl E2eeMediaBackend for Obs<'_> {
    async fn upload(&self, data: Bytes, obs_path: String, params: Vec<(String, String)>) -> Result<String> {
        let up = self
            .upload_object_for_service(ObjectUpload {
                data,
                o_type: ObjType::File,
                obs_path: Some(obs_path),
                params,
                ..Default::default()
            })
            .await?;
        Ok(up.obj_id)
    }

    async fn encrypt_message(
        &self,
        to: &str,
        key_material: &str,
        file_name: &str,
        content_type: i32,
    ) -> Result<Vec<Vec<u8>>> {
        let content = E2eeContent::Json(serde_json::json!({"keyMaterial": key_material, "fileName": file_name}));
        self.client.encrypt_e2ee_message(to, content, content_type, 2).await
    }

    async fn send(
        &self,
        to: &str,
        chunks: Vec<Vec<u8>>,
        content_type: i32,
        metadata: IndexMap<String, String>,
    ) -> Result<Value> {
        send_chunked_message(self.client, to, chunks, content_type, metadata).await
    }
}

/// `talk.sendMessage` with pre-encrypted chunks.
async fn send_chunked_message(
    client: &BaseClient,
    to: &str,
    chunks: Vec<Vec<u8>>,
    content_type: i32,
    metadata: IndexMap<String, String>,
) -> Result<Value> {
    let mut message = crate::tv!({
        "to": to,
        "createdTime": 0,
        "deliveredTime": 0,
        "hasContent": false,
        "contentType": content_type,
        "sessionId": 0,
    });
    message.insert("contentMetadata", Value::Struct(metadata.into_iter().map(|(k, v)| (k, Value::String(v))).collect()));
    message.insert("chunks", Value::List(chunks.into_iter().map(Value::Binary).collect()));
    let mut args = crate::tv!({"seq": client.get_reqseq("talk").await?});
    args.insert("message", message);
    client.talk().call("sendMessage", args).await
}

pub(crate) async fn upload_media_by_e2ee(backend: &dyn E2eeMediaBackend, opts: E2eeMediaUpload) -> Result<Value> {
    let E2eeMediaUpload { file, o_type, to, preview, duration_ms } = opts;
    let ext = file.extension();
    let (namespace, content_type) = type_set(o_type);
    let mut params = vec![("type".to_string(), "file".to_string())];
    if o_type == ObjType::Gif {
        params.push(("cat".into(), "original".into()));
    }
    if !(to.starts_with('u') || to.starts_with('c')) {
        return Err(Error::internal("ObsError", "Invalid mid"));
    }
    let enc = encrypt_by_key_material(&file.data, None);
    let edata = Bytes::from(enc.encrypted_data);
    let size = edata.len();
    let temp_id = format!("reqid-{}", super::uuid_v4());
    let obj_id = backend.upload(edata.clone(), format!("talk/{namespace}/{temp_id}"), params.clone()).await?;

    if has_preview(o_type) {
        let preview_data = match preview {
            Some(p) => {
                use base64::Engine;
                let key = base64::engine::general_purpose::STANDARD
                    .decode(&enc.key_material)
                    .map_err(|e| Error::internal("E2EEMediaError", e.to_string()))?;
                Bytes::from(encrypt_by_key_material(&p, Some(&key)).encrypted_data)
            }
            None => edata,
        };
        let obj_id2 = backend.upload(preview_data, format!("talk/{namespace}/{obj_id}__ud-preview"), params).await?;
        if obj_id != obj_id2 {
            return Err(Error::internal("ObsError", format!("objId not match: {obj_id} != {obj_id2}")));
        }
    }

    let file_name = file.name.clone().unwrap_or_else(|| format!("line.{}", ext.as_deref().unwrap_or("undefined")));
    let chunks = backend.encrypt_message(&to, &enc.key_material, &file_name, content_type).await?;
    let metadata = e2ee_content_metadata(o_type, &obj_id, size, ext.as_deref(), duration_ms);
    backend.send(&to, chunks, content_type, metadata).await
}

/// `X-Talk-Meta` header value for downloading the object of message `id`.
pub(crate) fn talk_meta(message_id: &str) -> Result<String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let s = thrift::write_struct(
        &[(4, TValue::Binary(message_id.as_bytes().to_vec())), (27, TValue::List(TType::Struct, Vec::new()))],
        Protocol::Binary,
    )?;
    let json = format!("{{\"message\":{}}}", serde_json::Value::from(b64.encode(s)));
    Ok(b64.encode(json))
}

impl Obs<'_> {
    /// Encrypt and upload media for an E2EE chat, then send the message that
    /// carries the key material. Returns the sent `Message`.
    pub async fn upload_media_by_e2ee(&self, opts: E2eeMediaUpload) -> Result<Value> {
        upload_media_by_e2ee(self, opts).await
    }

    /// Download and decrypt the media of an E2EE message, reading
    /// `keyMaterial` / `fileName` from its decrypted payload.
    pub async fn download_e2ee_message_media(&self, message: &Value) -> Result<Option<ObsFile>> {
        let payload = self.client.decrypt_e2ee_data_message(message).await?;
        let key_material = payload["keyMaterial"]
            .as_str()
            .ok_or_else(|| Error::internal("E2EEMediaError", "decrypted payload has no keyMaterial"))?;
        let file_name = payload["fileName"].as_str().unwrap_or("file");
        self.download_media_by_e2ee(message, key_material, file_name).await
    }

    /// Download and decrypt the media of an E2EE `message`.
    ///
    /// `key_material` (base64) and `file_name` are the fields of the
    /// message's decrypted E2EE payload (linejs `decryptE2EEDataMessage`).
    /// Returns `None` when the message has no chunks.
    pub async fn download_media_by_e2ee(
        &self,
        message: &Value,
        key_material: &str,
        file_name: &str,
    ) -> Result<Option<ObsFile>> {
        let to = message["to"].as_str().unwrap_or_default();
        if !(to.starts_with('u') || to.starts_with('c')) {
            return Err(Error::internal("ObsError", "Invalid mid"));
        }
        if message["chunks"].as_list().is_none_or(|c| c.is_empty()) {
            return Ok(None);
        }
        let id = message["id"].as_str().unwrap_or_default();
        let meta = &message["contentMetadata"];
        let oid = meta["OID"].as_str().unwrap_or_default();
        let sid = meta["SID"].as_str().unwrap_or_default();
        let data =
            self.download_object_for_service(&format!("talk/{sid}"), oid, vec![("X-Talk-Meta".into(), talk_meta(id)?)]).await?;
        let plain = crypto::decrypt_by_key_material_b64(&data, key_material)?;
        Ok(Some(ObsFile::new(plain).name(file_name)))
    }
}
