//! Forwarding E2EE media without uploading it again: OBS copies the
//! encrypted object on the server (`copy.obs`, `a41/k`
//! `ObsEncryptionContentCopier`) and the new message reuses the original
//! key material.

use super::e2ee::{
    MediaBackend, Sent, V2Object, choose_flow, e2ee_content_metadata, send_object, type_set,
};
use super::flow::MediaFlow;
use super::token::ObsTokenSource;
use super::{MediaOptions, ObjType, Obs, SentMedia, ensure_ok, header_str, meta, to_header_map};
use crate::error::{Error, Result};

/// An already-sent E2EE media message to send again to `to`.
#[derive(Debug, Clone, Default)]
pub struct ForwardMedia {
    pub to: String,
    pub o_type: ObjType,
    /// `OID` of the source message.
    pub src_oid: String,
    /// Server id of the source message; OBS checks the copy against it.
    pub src_message_id: String,
    /// `keyMaterial` from the source message's decrypted payload.
    pub key_material: String,
    /// Plaintext size (`FILE_SIZE`).
    pub file_size: usize,
    /// File name: the `fileName` of a FILE, and the extension of an image.
    pub file_name: Option<String>,
    /// Duration, size and silence of the new message. `e2ee` and `group`
    /// are ignored: a copy is always E2EE.
    pub opts: MediaOptions,
}

/// Copy the source object, then send the E2EE message for the copy with
/// the same key. Fails, rather than uploading anything, when the chat
/// takes this media plain or LINE asks for it plain: the caller still has
/// the file and can send it the usual way.
pub(crate) async fn forward_media(
    backend: &dyn MediaBackend,
    f: ForwardMedia,
) -> Result<SentMedia> {
    if f.src_oid.is_empty() || f.key_material.is_empty() {
        return Err(Error::internal("ObsError", "No object to copy"));
    }
    // The app copies only on the V2 flow (`a41/g.c`).
    if choose_flow(backend, &f.to, f.o_type, None).await? != MediaFlow::V2 {
        return Err(Error::internal(
            "ObsError",
            "This chat takes this media plain; copy.obs needs E2EE",
        ));
    }
    let seq = backend.reqseq().await?;
    let obj_id = backend
        .copy(f.o_type, &f.src_oid, &f.src_message_id)
        .await?;
    let extension = f
        .file_name
        .as_deref()
        .and_then(|n| n.rsplit_once('.'))
        .map(|(_, ext)| ext);
    let metadata = e2ee_content_metadata(f.o_type, &obj_id, f.file_size, extension, &f.opts);
    let file_name = (f.o_type == ObjType::File)
        .then(|| f.file_name.clone().unwrap_or_else(|| "file".to_string()));
    let object = V2Object {
        obj_id,
        key_material: f.key_material,
    };
    match send_object(backend, &f.to, f.o_type, &object, metadata, file_name, seq).await? {
        Sent::Message(message) => Ok(SentMedia::E2ee(message)),
        Sent::WantsPlain(err) => Err(err),
    }
}

/// Body of a `copy.obs` request (`a41/k.b` → `jx3/a.a`). Both relays carry
/// the OBS token and the source message's `X-Talk-Meta`.
pub(crate) fn copy_body(
    namespace: &str,
    src_oid: &str,
    dst_oid: &str,
    token: &str,
    talk_meta: &str,
) -> serde_json::Value {
    let headers = serde_json::json!({"X-Line-Access": token, "X-Talk-Meta": talk_meta});
    serde_json::json!({
        "src": {
            "svcCode": "talk",
            "sid": namespace,
            "oid": src_oid,
            "relay": {
                "headers": headers,
                "params": {"srcSvcCode": "talk", "srcOid": src_oid, "srcSid": namespace},
                "commonHeaders": ["x-real-ip"],
            },
        },
        "dst": {
            "relay": {
                "headers": headers,
                "params": {"dstSvcCode": "talk", "dstOid": dst_oid, "dstSid": namespace},
                "commonHeaders": ["x-real-ip"],
            },
        },
        "options": {},
    })
}

impl Obs<'_> {
    /// Copy the E2EE object `src_oid` of message `src_message_id` on the
    /// server (`POST r/talk/{ns}/reqid-<uuid>/copy.obs`); returns the new
    /// object id. On HTTP 401 it gets a new OBS token and tries once more
    /// with a new request id, as the app does.
    pub async fn copy_e2ee_object(
        &self,
        o_type: ObjType,
        src_oid: &str,
        src_message_id: &str,
    ) -> Result<String> {
        self.copy_e2ee_object_with(o_type, src_oid, src_message_id, self.client)
            .await
    }

    pub(crate) async fn copy_e2ee_object_with(
        &self,
        o_type: ObjType,
        src_oid: &str,
        src_message_id: &str,
        tokens: &dyn ObsTokenSource,
    ) -> Result<String> {
        let cache = &self.client.obs_state.token;
        let talk_meta = meta::message_meta(src_message_id)?;
        for attempt in 0..2 {
            let token = cache
                .get(tokens)
                .await
                .or_else(|| self.client.auth_token())
                .ok_or_else(|| Error::internal("Not setup yet", "Please call 'login()' first"))?;
            let (status, oid) = self.post_copy(o_type, src_oid, &token, &talk_meta).await?;
            if status == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                cache.clear().await;
                continue;
            }
            ensure_ok(status, "Object copy failed")?;
            if oid.is_empty() {
                return Err(Error::internal(
                    "ObsError",
                    "Object copy failed: no object id",
                ));
            }
            return Ok(oid);
        }
        unreachable!("the second attempt always returns")
    }

    async fn post_copy(
        &self,
        o_type: ObjType,
        src_oid: &str,
        token: &str,
        talk_meta: &str,
    ) -> Result<(reqwest::StatusCode, String)> {
        let (namespace, _) = type_set(o_type);
        let dst_oid = format!("reqid-{}", super::uuid_v4());
        let body = copy_body(namespace, src_oid, &dst_oid, token, talk_meta);
        let obs_host = reqwest::Url::parse(self.prefix())
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default();
        // `qx3/d.f()`: the copy goes out as the LEGY channel type.
        let mut headers = self.client.line_headers("POST");
        headers.extend([
            ("content-type".to_string(), "application/json".to_string()),
            ("x-obs-channeltype".to_string(), "legy".to_string()),
            ("x-obs-host".to_string(), obs_host),
            ("x-line-access".to_string(), token.to_string()),
            ("X-Talk-Meta".to_string(), talk_meta.to_string()),
        ]);
        let res = self
            .client
            .http
            .post(format!(
                "{}r/talk/{namespace}/{dst_oid}/copy.obs",
                self.prefix()
            ))
            .headers(to_header_map(headers)?)
            .timeout(self.client.config.long_timeout)
            .body(body.to_string())
            .send()
            .await?;
        let status = res.status();
        let oid = header_str(res.headers(), "x-obs-oid");
        self.client.log(
            "Obs.copyObject",
            serde_json::json!({"status": status.as_u16(), "oid": oid}),
        );
        Ok((status, oid))
    }

    /// Send an already-sent E2EE image, video, audio clip or file to
    /// `f.to` by copying its object on the server, with the original key
    /// material. Fails when the chat takes that media plain, or the copy
    /// or the send fails; then send the file itself instead.
    pub async fn forward_media(&self, f: ForwardMedia) -> Result<SentMedia> {
        forward_media(self, f).await
    }
}
