//! Media send flow (ports of linejs `obs/upload_e2ee.test.ts`, extended to
//! the Android flow) and `obs/download_status.test.ts`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use indexmap::IndexMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::e2ee::{MediaBackend, MediaSend, send_media, upload_media_by_e2ee, v2_params};
use super::flow::MediaFlowCache;
use super::token::{ObsTokenCache, ObsTokenSource};
use super::*;
use crate::device::Device;
use crate::e2ee::E2eeSendRecovery;
use crate::e2ee::media::{MEDIA_CHUNK_SIZE, MediaHmacVersion, decrypt_media};
use crate::thrift::Value;

/// One `send` call: `(to, contentType, metadata, seq)`.
type Sent = (String, i32, IndexMap<String, String>, i64);
/// One V2 upload: `(data, obs_path, params)`.
type Upload = (Bytes, String, Vec<(String, String)>);
/// One V1 upload: `(o_type, size, duration, reqseq)`.
type PlainUpload = (ObjType, usize, Option<u64>, i64);

/// Records every side effect; answers with `flows` and fails sends with
/// the codes in `send_errors`, in order.
struct Fake {
    flows: Result<MediaFlows>,
    flow_calls: Mutex<u32>,
    dropped: Mutex<Vec<String>>,
    uploads: Mutex<Vec<Upload>>,
    plain: Mutex<Vec<PlainUpload>>,
    /// `(key material, file name)` per encryption.
    encrypted: Mutex<Vec<(String, Option<String>)>>,
    prepared: Mutex<Vec<E2eeSendRecovery>>,
    send_errors: Mutex<VecDeque<&'static str>>,
    sent: Mutex<Vec<Sent>>,
    /// Extra headers of each V2 and V1 upload, in order.
    headers: Mutex<Vec<Vec<(String, String)>>>,
    /// `x-line-message-gid` (or `Err` for a failure) of each V1 upload,
    /// in order; none left means success without a group id.
    plain_answers: Mutex<VecDeque<std::result::Result<&'static str, u16>>>,
    /// `(o_type, src_oid, src_message_id)` per copy.
    copies: Mutex<Vec<(ObjType, String, String)>>,
    copy_fails: bool,
}

impl Default for Fake {
    fn default() -> Self {
        Self::with_flows(MediaFlows::all(MediaFlow::V2))
    }
}

impl Fake {
    fn with_flows(flows: MediaFlows) -> Self {
        Self {
            flows: Ok(flows),
            flow_calls: Mutex::default(),
            dropped: Mutex::default(),
            uploads: Mutex::default(),
            plain: Mutex::default(),
            encrypted: Mutex::default(),
            prepared: Mutex::default(),
            send_errors: Mutex::default(),
            sent: Mutex::default(),
            headers: Mutex::default(),
            plain_answers: Mutex::default(),
            copies: Mutex::default(),
            copy_fails: false,
        }
    }
    fn failing_sends(codes: &[&'static str]) -> Self {
        let fake = Self::default();
        fake.send_errors.lock().unwrap().extend(codes);
        fake
    }
    fn sizes(&self) -> Vec<usize> {
        self.uploads
            .lock()
            .unwrap()
            .iter()
            .map(|u| u.0.len())
            .collect()
    }
    fn path(&self, i: usize) -> String {
        self.uploads.lock().unwrap()[i].1.clone()
    }
    fn blob(&self, i: usize) -> Bytes {
        self.uploads.lock().unwrap()[i].0.clone()
    }
    fn param(&self, i: usize, key: &str) -> String {
        self.uploads.lock().unwrap()[i]
            .2
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
    fn key_material(&self) -> String {
        self.encrypted.lock().unwrap()[0].0.clone()
    }
    fn metadata(&self) -> IndexMap<String, String> {
        let sent = self.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        sent[0].2.clone()
    }
}

fn talk_error(code: &str) -> Error {
    Error::Request {
        method: "sendMessage".into(),
        path: "/S4".into(),
        data: crate::tv!({"code": code}),
    }
}

#[async_trait]
impl MediaBackend for Fake {
    async fn reqseq(&self) -> Result<i64> {
        Ok(41)
    }
    async fn media_flows(&self, _chat: &str) -> Result<MediaFlows> {
        *self.flow_calls.lock().unwrap() += 1;
        match &self.flows {
            Ok(f) => Ok(*f),
            Err(_) => Err(Error::internal("RequestError", "offline")),
        }
    }
    fn drop_media_flow(&self, chat: &str) {
        self.dropped.lock().unwrap().push(chat.to_string());
    }
    async fn upload(
        &self,
        data: Bytes,
        obs_path: String,
        params: Vec<(String, String)>,
        headers: Vec<(String, String)>,
    ) -> Result<String> {
        self.uploads.lock().unwrap().push((data, obs_path, params));
        self.headers.lock().unwrap().push(headers);
        Ok("OBJ-1".into())
    }
    async fn upload_plain(
        &self,
        _to: &str,
        o_type: ObjType,
        file: ObsFile,
        duration_ms: Option<u64>,
        reqseq: i64,
        headers: Vec<(String, String)>,
    ) -> Result<ObsUpload> {
        self.plain
            .lock()
            .unwrap()
            .push((o_type, file.len(), duration_ms, reqseq));
        self.headers.lock().unwrap().push(headers);
        let mut answer = reqwest::header::HeaderMap::new();
        match self.plain_answers.lock().unwrap().pop_front() {
            Some(Err(status)) => {
                return Err(Error::internal(
                    "ObsError",
                    format!("Object upload failed: HTTP {status}"),
                ));
            }
            Some(Ok(gid)) => {
                answer.insert("x-line-message-gid", gid.parse().unwrap());
            }
            None => {}
        }
        Ok(ObsUpload {
            obj_id: "PLAIN-1".into(),
            obj_hash: "HASH".into(),
            headers: answer,
        })
    }
    async fn copy(&self, o_type: ObjType, src_oid: &str, src_message_id: &str) -> Result<String> {
        self.copies
            .lock()
            .unwrap()
            .push((o_type, src_oid.to_string(), src_message_id.to_string()));
        if self.copy_fails {
            return Err(Error::internal("ObsError", "Object copy failed: HTTP 403"));
        }
        Ok("COPY-1".into())
    }
    async fn encrypt_message(
        &self,
        _to: &str,
        key_material: &str,
        file_name: Option<&str>,
        _ct: i32,
    ) -> Result<Vec<Vec<u8>>> {
        self.encrypted
            .lock()
            .unwrap()
            .push((key_material.to_string(), file_name.map(str::to_string)));
        Ok(vec![Vec::new()])
    }
    async fn prepare_resend(&self, _to: &str, recovery: E2eeSendRecovery) -> Result<()> {
        self.prepared.lock().unwrap().push(recovery);
        Ok(())
    }
    async fn send(
        &self,
        to: &str,
        _chunks: Vec<Vec<u8>>,
        content_type: i32,
        metadata: IndexMap<String, String>,
        seq: i64,
    ) -> Result<Value> {
        self.sent
            .lock()
            .unwrap()
            .push((to.to_string(), content_type, metadata, seq));
        match self.send_errors.lock().unwrap().pop_front() {
            Some(code) => Err(talk_error(code)),
            None => Ok(crate::tv!({"id": "m-1"})),
        }
    }
}

fn media(data: &[u8], o_type: ObjType) -> MediaSend {
    MediaSend {
        to: "u-recipient".into(),
        o_type,
        file: ObsFile::new(data.to_vec()),
        opts: MediaOptions::default(),
    }
}

async fn run_with(fake: Fake, m: MediaSend) -> (Fake, Result<SentMedia>) {
    let res = send_media(&fake, m).await;
    (fake, res)
}

async fn run(m: MediaSend) -> Fake {
    let (fake, res) = run_with(Fake::default(), m).await;
    res.unwrap();
    fake
}

// --- uploads ---------------------------------------------------------------

#[tokio::test]
async fn image_without_preview_is_its_own_preview() {
    let fake = run(media(&[0; 100_000], ObjType::Image)).await;
    let sizes = fake.sizes();
    assert_eq!(sizes, [100_032, 100_032]);
    assert!(fake.path(0).starts_with("talk/emi/reqid-"));
    assert_eq!(fake.path(1), "talk/emi/OBJ-1__ud-preview");
}

#[tokio::test]
async fn image_preview_is_small_and_opens_with_the_message_key() {
    let mut m = media(b"full image", ObjType::Image);
    m.opts = MediaOptions::new().preview(b"thumbnail".to_vec());
    let fake = run(m).await;
    assert_eq!(fake.sizes(), [10 + 32, 9 + 32]);
    let key = fake.key_material();
    assert_eq!(
        crypto::decrypt_by_key_material_b64(&fake.blob(0), &key).unwrap(),
        b"full image"
    );
    assert_eq!(
        crypto::decrypt_by_key_material_b64(&fake.blob(1), &key).unwrap(),
        b"thumbnail"
    );
}

#[tokio::test]
async fn file_and_audio_have_no_preview() {
    for o_type in [ObjType::File, ObjType::Audio] {
        let mut m = media(&[0; 50_000], o_type);
        m.opts = MediaOptions::new().preview(vec![1u8; 10]);
        assert_eq!(run(m).await.sizes(), [50_032], "{o_type}");
    }
}

#[tokio::test]
async fn video_uses_chunk_hashes_and_no_fake_preview() {
    let raw: Vec<u8> = (0..2 * MEDIA_CHUNK_SIZE + 5).map(|i| i as u8).collect();
    let mut m = media(&raw, ObjType::Video);
    m.file = m.file.name("holiday.mov");
    let fake = run(m).await;
    // Main object and hash object; no preview without a thumbnail.
    assert_eq!(fake.sizes(), [raw.len() + 32, 3 * 32]);
    assert!(fake.path(0).starts_with("talk/emv/reqid-"));
    assert_eq!(fake.path(1), "talk/emv/OBJ-1__ud-hash");
    assert_eq!(fake.param(1, "name"), "hash");
    // The local name is not uploaded; the extension is kept.
    let name = fake.param(0, "name");
    assert!(
        name.ends_with(".mov") && !name.contains("holiday"),
        "{name}"
    );

    use base64::Engine;
    let key = base64::engine::general_purpose::STANDARD
        .decode(fake.key_material())
        .unwrap();
    let main = fake.blob(0);
    let (plain, version) = decrypt_media(&main, &key).unwrap();
    assert_eq!(plain, raw);
    assert_eq!(version, MediaHmacVersion::V2);
    // The hash object is the digests of the ciphertext chunks.
    let body = &main[..raw.len()];
    let digests: Vec<u8> = crate::e2ee::media::chunk_hashes(body)
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(fake.blob(1).as_ref(), digests.as_slice());
}

#[tokio::test]
async fn video_with_thumbnail_uploads_preview_then_hash() {
    let mut m = media(&[7; 1000], ObjType::Video);
    m.opts = MediaOptions::new()
        .preview(vec![0xffu8; 20])
        .duration_ms(4200);
    let fake = run(m).await;
    assert_eq!(fake.sizes(), [1032, 52, 32]);
    assert_eq!(fake.path(1), "talk/emv/OBJ-1__ud-preview");
    assert_eq!(fake.param(1, "name"), "preview.jpg");
    assert_eq!(fake.path(2), "talk/emv/OBJ-1__ud-hash");
    let preview = crypto::decrypt_by_key_material_b64(&fake.blob(1), &fake.key_material());
    assert_eq!(preview.unwrap(), vec![0xffu8; 20]);
}

#[tokio::test]
async fn v2_upload_params_match_the_app() {
    let mut m = media(&[0; 10], ObjType::File);
    m.file = m.file.name("report.pdf");
    let fake = run(m).await;
    assert_eq!(fake.param(0, "type"), "FILE");
    assert_eq!(fake.param(0, "ver"), "2.0");
    assert_eq!(fake.param(0, "name"), "report.pdf");
    assert_eq!(fake.param(0, "range"), "bytes 0-41/42");
    assert_eq!(fake.param(0, "cat"), "");
    assert_eq!(
        v2_params("hash", 64),
        [
            ("ver".to_string(), "2.0".to_string()),
            ("name".to_string(), "hash".to_string()),
            ("type".to_string(), "FILE".to_string()),
            ("range".to_string(), "bytes 0-63/64".to_string()),
        ]
    );
}

// --- metadata --------------------------------------------------------------

fn meta(pairs: &[(&str, &str)]) -> IndexMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[tokio::test]
async fn image_metadata() {
    let mut m = media(&[0; 1000], ObjType::Image);
    m.file = m.file.name("Photo.JPEG");
    m.opts = MediaOptions::new().size(640, 480);
    assert_eq!(
        run(m).await.metadata(),
        meta(&[
            ("SID", "emi"),
            ("OID", "OBJ-1"),
            ("FILE_SIZE", "1000"),
            ("e2eeVersion", "2"),
            (
                "MEDIA_CONTENT_INFO",
                r#"{"category":"original","fileSize":1000,"extension":"jpeg","width":640,"height":480}"#
            ),
        ])
    );
}

#[tokio::test]
async fn gif_is_animated() {
    let fake = run(media(&[0; 10], ObjType::Gif)).await;
    assert_eq!(
        fake.metadata()["MEDIA_CONTENT_INFO"],
        r#"{"category":"original","fileSize":10,"extension":"gif","animated":true}"#
    );
}

#[tokio::test]
async fn video_and_audio_metadata() {
    let mut m = media(&[0; 1000], ObjType::Video);
    m.opts = MediaOptions::new().duration_ms(4200).size(1, 1);
    m.to = "c-group".into();
    let fake = run(m).await;
    assert_eq!(
        fake.metadata(),
        meta(&[
            ("SID", "emv"),
            ("OID", "OBJ-1"),
            ("FILE_SIZE", "1000"),
            ("e2eeVersion", "2"),
            ("DURATION", "4200"),
        ])
    );
    {
        let sent = fake.sent.lock().unwrap();
        assert_eq!((sent[0].0.as_str(), sent[0].1), ("c-group", 2));
    }

    let mut m = media(&[0; 10], ObjType::Audio);
    m.opts = MediaOptions::new().duration_ms(900);
    let fake = run(m).await;
    assert_eq!(fake.metadata()["SID"], "ema");
    assert_eq!(fake.metadata()["DURATION"], "900");
}

#[tokio::test]
async fn file_name_only_in_file_payloads() {
    let mut m = media(&[0; 10], ObjType::File);
    m.file = m.file.name("report.pdf");
    let fake = run(m).await;
    assert_eq!(
        fake.encrypted.lock().unwrap()[0].1.as_deref(),
        Some("report.pdf")
    );
    assert!(!fake.metadata().contains_key("FILE_NAME"));
    assert!(!fake.metadata().contains_key("MEDIA_CONTENT_INFO"));

    let mut m = media(&[0; 10], ObjType::Image);
    m.file = m.file.name("cat.png");
    let fake = run(m).await;
    assert_eq!(fake.encrypted.lock().unwrap()[0].1, None);
}

#[test]
fn content_metadata_without_size_or_duration() {
    let m = e2ee_content_metadata(ObjType::Audio, "O", 0, None, &MediaOptions::default());
    assert_eq!(
        m,
        meta(&[("SID", "ema"), ("OID", "O"), ("e2eeVersion", "2")])
    );
    let m = e2ee_content_metadata(ObjType::Image, "O", 5, None, &MediaOptions::default());
    assert_eq!(
        m["MEDIA_CONTENT_INFO"],
        r#"{"category":"original","fileSize":5,"extension":"jpg"}"#
    );
}

#[tokio::test]
async fn upload_media_by_e2ee_rounds_durations() {
    let fake = Fake::default();
    upload_media_by_e2ee(
        &fake,
        E2eeMediaUpload {
            file: ObsFile::new(vec![0u8; 10]),
            o_type: ObjType::Video,
            to: "u-recipient".into(),
            duration_ms: Some(4200.5),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(fake.metadata()["DURATION"], "4201");
    // Forced E2EE does not ask for the chat's flow.
    assert_eq!(*fake.flow_calls.lock().unwrap(), 0);
}

#[test]
fn invalid_durations_are_dropped() {
    for d in [
        0.0,
        -1.0,
        f64::NAN,
        f64::INFINITY,
        0.1,
        0.49,
        f64::MIN_POSITIVE,
        5e-324,
        f64::NEG_INFINITY,
        9_007_199_254_740_992.0,
        1e21,
        f64::MAX,
    ] {
        assert_eq!(
            e2ee_duration(ObjType::Video, Some(d)),
            None,
            "durationMs={d}"
        );
    }
}

#[test]
fn valid_durations_round_like_math_round() {
    for (d, expected) in [
        (0.5, "1"),
        (1.0, "1"),
        (4200.5, "4201"),
        (9_007_199_254_740_991.0, "9007199254740991"),
    ] {
        assert_eq!(
            e2ee_duration(ObjType::Video, Some(d)).as_deref(),
            Some(expected)
        );
    }
    assert_eq!(
        e2ee_duration(ObjType::Audio, Some(10.0)).as_deref(),
        Some("10")
    );
    assert_eq!(e2ee_duration(ObjType::Image, Some(10.0)), None);
}

#[tokio::test]
async fn invalid_mid_is_rejected_when_e2ee_is_forced() {
    let fake = Fake::default();
    let err = upload_media_by_e2ee(
        &fake,
        E2eeMediaUpload {
            file: ObsFile::new(b"x".to_vec()),
            to: "s-square".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.to_string(), "ObsError: Invalid mid");
}

// --- size limits -----------------------------------------------------------

#[test]
fn size_limits() {
    assert!(check_media_size(ObjType::File, MAX_FILE_SIZE).is_ok());
    let err = check_media_size(ObjType::File, MAX_FILE_SIZE + 1).unwrap_err();
    assert_eq!(
        err.to_string(),
        "MediaTooLarge: The file is larger than 1GB."
    );
    assert!(check_media_size(ObjType::Video, MAX_VIDEO_SIZE).is_ok());
    let err = check_media_size(ObjType::Video, MAX_VIDEO_SIZE + 1).unwrap_err();
    assert_eq!(
        err.to_string(),
        "MediaTooLarge: The video is larger than 200MB."
    );
    assert!(check_media_size(ObjType::Image, MAX_VIDEO_SIZE + 1).is_ok());
}

#[tokio::test]
async fn oversized_video_fails_before_upload() {
    let (fake, res) = run_with(
        Fake::default(),
        media(&vec![0u8; MAX_VIDEO_SIZE + 1], ObjType::Video),
    )
    .await;
    assert!(res.unwrap_err().to_string().starts_with("MediaTooLarge"));
    assert!(fake.sizes().is_empty());
    assert!(fake.plain.lock().unwrap().is_empty());
}

// --- flow selection --------------------------------------------------------

#[tokio::test]
async fn v1_flow_uploads_plain_with_one_reqseq() {
    let flows = MediaFlows {
        image: MediaFlow::V2,
        ..MediaFlows::all(MediaFlow::V1)
    };
    let mut m = media(&[0; 10], ObjType::Video);
    m.opts = MediaOptions::new().duration_ms(1500);
    let (fake, res) = run_with(Fake::with_flows(flows), m).await;
    assert!(matches!(res.unwrap(), SentMedia::Plain { obj_id, .. } if obj_id == "PLAIN-1"));
    assert_eq!(
        *fake.plain.lock().unwrap(),
        [(ObjType::Video, 10, Some(1500), 41)]
    );
    assert!(fake.sizes().is_empty());
    assert!(fake.sent.lock().unwrap().is_empty());

    let (fake, res) = run_with(Fake::with_flows(flows), media(&[0; 10], ObjType::Gif)).await;
    assert!(matches!(res.unwrap(), SentMedia::E2ee(_)));
    assert_eq!(fake.sizes().len(), 2);
}

#[tokio::test]
async fn plain_requests_and_rooms_skip_the_flow_call() {
    let mut m = media(&[0; 10], ObjType::Image);
    m.opts = MediaOptions::new().e2ee(false);
    let (fake, res) = run_with(Fake::default(), m).await;
    assert!(matches!(res.unwrap(), SentMedia::Plain { .. }));
    assert_eq!(*fake.flow_calls.lock().unwrap(), 0);

    let mut m = media(&[0; 10], ObjType::Image);
    m.to = "r-room".into();
    let (fake, res) = run_with(Fake::default(), m).await;
    assert!(matches!(res.unwrap(), SentMedia::Plain { .. }));
    assert_eq!(*fake.flow_calls.lock().unwrap(), 0);
}

#[tokio::test]
async fn flow_lookup_failure_keeps_encrypting() {
    let fake = Fake {
        flows: Err(Error::internal("x", "y")),
        ..Fake::default()
    };
    let (fake, res) = run_with(fake, media(&[0; 10], ObjType::File)).await;
    assert!(matches!(res.unwrap(), SentMedia::E2ee(_)));
    assert_eq!(*fake.flow_calls.lock().unwrap(), 1);
}

#[test]
fn flows_from_response_default_to_v1() {
    let res = crate::tv!({"cacheTtlMillis": 60_000});
    let mut res = res;
    res.insert(
        "flowMap",
        Value::Map(vec![
            (Value::Int(1), Value::Int(2)),
            (Value::Int(2), Value::Int(1)),
            (Value::Int(14), Value::Int(2)),
            (Value::Int(3), Value::Int(9)),
        ]),
    );
    let (flows, ttl) = MediaFlows::from_response(&res);
    assert_eq!(
        flows,
        MediaFlows {
            image: MediaFlow::V2,
            video: MediaFlow::V1,
            audio: MediaFlow::V1,
            file: MediaFlow::V2,
        }
    );
    assert_eq!(flows.get(ObjType::Gif), MediaFlow::V2);
    assert_eq!(ttl, Duration::from_secs(60));

    let (flows, ttl) = MediaFlows::from_response(&Value::object());
    assert_eq!(flows, MediaFlows::all(MediaFlow::V1));
    assert_eq!(ttl, Duration::ZERO);
}

#[test]
fn flow_cache_honours_ttl_and_drops() {
    let cache = MediaFlowCache::default();
    let t0 = Instant::now();
    let v2 = MediaFlows::all(MediaFlow::V2);
    cache.insert("c1", v2, Duration::from_secs(10), t0);
    cache.insert("c2", v2, Duration::from_secs(10), t0);
    cache.insert("c3", v2, Duration::ZERO, t0);
    assert_eq!(cache.get("c1", t0 + Duration::from_secs(9)), Some(v2));
    assert_eq!(cache.get("c1", t0 + Duration::from_secs(10)), None);
    assert_eq!(cache.get("c3", t0), None);
    cache.remove(Some("c2"));
    assert_eq!(cache.get("c2", t0), None);
    cache.insert("c1", v2, Duration::from_secs(10), t0);
    cache.remove(None);
    assert_eq!(cache.get("c1", t0), None);
}

#[tokio::test]
async fn media_flows_are_cached_per_chat() {
    let c = client();
    let v1 = MediaFlows::all(MediaFlow::V1);
    c.obs_state
        .flows
        .insert("c-group", v1, Duration::from_secs(60), Instant::now());
    // Served from the cache: no request to LINE.
    assert_eq!(c.obs().media_flows("c-group").await.unwrap(), v1);
    c.drop_media_flow(Some("c-group"));
    assert_eq!(c.obs_state.flows.get("c-group", Instant::now()), None);
}

// --- E2EE send recovery ----------------------------------------------------

#[tokio::test]
async fn reencrypts_and_resends_without_uploading_again() {
    let (fake, res) = run_with(
        Fake::failing_sends(&["E2EE_RETRY_ENCRYPT", "E2EE_UPDATE_RECEIVER_KEY"]),
        media(&[0; 10], ObjType::File),
    )
    .await;
    assert!(matches!(res.unwrap(), SentMedia::E2ee(_)));
    assert_eq!(fake.sizes().len(), 1);
    let encrypted = fake.encrypted.lock().unwrap();
    assert_eq!(encrypted.len(), 3);
    assert!(encrypted.iter().all(|e| e.0 == encrypted[0].0));
    let sent = fake.sent.lock().unwrap();
    assert!(sent.iter().all(|s| s.3 == 41 && s.2["OID"] == "OBJ-1"));
    assert_eq!(
        *fake.prepared.lock().unwrap(),
        [E2eeSendRecovery::Reencrypt, E2eeSendRecovery::Reencrypt]
    );
}

#[tokio::test]
async fn recreates_the_group_key() {
    let mut m = media(&[0; 10], ObjType::Image);
    m.to = "c-group".into();
    let (fake, res) = run_with(Fake::failing_sends(&["E2EE_RECREATE_GROUP_KEY"]), m).await;
    assert!(matches!(res.unwrap(), SentMedia::E2ee(_)));
    assert_eq!(
        *fake.prepared.lock().unwrap(),
        [E2eeSendRecovery::RecreateGroupKey]
    );
    assert_eq!(fake.sent.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn gives_up_after_three_sends() {
    let (fake, res) = run_with(
        Fake::failing_sends(&["E2EE_RETRY_ENCRYPT"; 5]),
        media(&[0; 10], ObjType::File),
    )
    .await;
    assert_eq!(res.unwrap_err().code(), Some("E2EE_RETRY_ENCRYPT"));
    assert_eq!(fake.sent.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn other_errors_are_returned_at_once() {
    let (fake, res) = run_with(
        Fake::failing_sends(&["NOT_A_MEMBER"]),
        media(&[0; 10], ObjType::File),
    )
    .await;
    assert_eq!(res.unwrap_err().code(), Some("NOT_A_MEMBER"));
    assert_eq!(fake.sent.lock().unwrap().len(), 1);
    assert!(fake.plain.lock().unwrap().is_empty());
}

#[tokio::test]
async fn plain_codes_drop_the_flow_and_upload_v1() {
    for code in [
        "E2EE_RETRY_PLAIN",
        "E2EE_GROUP_TOO_MANY_MEMBERS",
        "REFRESH_MEDIA_FLOW",
    ] {
        let mut m = media(&[0; 10], ObjType::Audio);
        m.opts = MediaOptions::new().duration_ms(700);
        let (fake, res) = run_with(Fake::failing_sends(&[code]), m).await;
        assert!(matches!(res.unwrap(), SentMedia::Plain { .. }), "{code}");
        assert_eq!(*fake.dropped.lock().unwrap(), ["u-recipient"]);
        assert_eq!(
            *fake.plain.lock().unwrap(),
            [(ObjType::Audio, 10, Some(700), 41)]
        );
        assert_eq!(fake.sent.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn forced_e2ee_does_not_fall_back_to_plain() {
    let mut m = media(&[0; 10], ObjType::File);
    m.opts = MediaOptions::new().e2ee(true);
    let (fake, res) = run_with(Fake::failing_sends(&["E2EE_RETRY_PLAIN"]), m).await;
    assert_eq!(res.unwrap_err().code(), Some("E2EE_RETRY_PLAIN"));
    assert!(fake.plain.lock().unwrap().is_empty());
}

// --- OBS token -------------------------------------------------------------

#[test]
fn obs_token_parse_and_refresh_margin() {
    let (token, keep) = parse_obs_token("3600\u{1e}tok").unwrap();
    assert_eq!(token, "tok");
    assert_eq!(keep, Duration::from_secs(3600 - 180));
    // Margin at least 30 s, at most 600 s.
    assert_eq!(
        parse_obs_token("100\u{1e}t").unwrap().1,
        Duration::from_secs(70)
    );
    assert_eq!(
        parse_obs_token("86400\u{1e}t").unwrap().1,
        Duration::from_secs(86400 - 600)
    );
    assert_eq!(parse_obs_token("10\u{1e}t").unwrap().1, Duration::ZERO);
    for bad in ["tok", "x\u{1e}tok", "60\u{1e}", ""] {
        assert!(parse_obs_token(bad).is_err(), "{bad:?}");
    }
}

/// Answers with `answers` in order (an error once they run out).
struct Tokens(Mutex<VecDeque<Result<String>>>, Mutex<u32>);

impl Tokens {
    fn new(answers: Vec<Result<String>>) -> Self {
        Self(Mutex::new(answers.into()), Mutex::default())
    }
    fn calls(&self) -> u32 {
        *self.1.lock().unwrap()
    }
}

#[async_trait]
impl ObsTokenSource for Tokens {
    async fn acquire(&self) -> Result<String> {
        *self.1.lock().unwrap() += 1;
        self.0
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(Error::internal("x", "no token")))
    }
}

#[tokio::test]
async fn obs_token_is_cached_until_its_margin() {
    let cache = ObsTokenCache::default();
    let tokens = Tokens::new(vec![
        Ok("3600\u{1e}first".into()),
        Ok("3600\u{1e}second".into()),
    ]);
    assert_eq!(cache.get(&tokens).await.as_deref(), Some("first"));
    assert_eq!(cache.get(&tokens).await.as_deref(), Some("first"));
    assert_eq!(tokens.calls(), 1);
    cache.clear().await;
    assert_eq!(cache.get(&tokens).await.as_deref(), Some("second"));
    assert_eq!(tokens.calls(), 2);
}

#[tokio::test]
async fn obs_token_retries_then_keeps_the_stale_one() {
    let cache = ObsTokenCache::default();
    let tokens = Tokens::new(vec![
        Err(Error::internal("x", "1")),
        Ok("garbage".into()),
        Ok("3600\u{1e}third".into()),
    ]);
    let started = Instant::now();
    assert_eq!(cache.get(&tokens).await.as_deref(), Some("third"));
    assert_eq!(tokens.calls(), 3);
    assert!(started.elapsed() >= Duration::from_millis(600));

    // Expired and every try fails: the stale token is still used.
    cache.set("stale", Duration::ZERO).await;
    let failing = Tokens::new(vec![]);
    assert_eq!(cache.get(&failing).await.as_deref(), Some("stale"));
    assert_eq!(failing.calls(), OBS_TOKEN_ATTEMPTS);

    // Never had one: None, so the session token is used.
    cache.clear().await;
    assert_eq!(cache.get(&Tokens::new(vec![])).await, None);
}

#[test]
fn talk_meta_header() {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let json: serde_json::Value =
        serde_json::from_slice(&b64.decode(super::e2ee::talk_meta("1").unwrap()).unwrap()).unwrap();
    let thrift = b64.decode(json["message"].as_str().unwrap()).unwrap();
    // field 4 (string "1"), field 27 (empty list<struct>), stop.
    assert_eq!(
        thrift,
        [11, 0, 4, 0, 0, 0, 1, b'1', 15, 0, 27, 12, 0, 0, 0, 0, 0]
    );
}

#[test]
fn obs_params_are_base64_json() {
    use base64::Engine;
    let p = vec![
        ("type".to_string(), "image".to_string()),
        ("name".to_string(), "a\"b".to_string()),
    ];
    let raw = base64::engine::general_purpose::STANDARD
        .decode(obs_params_header(&p))
        .unwrap();
    assert_eq!(
        String::from_utf8(raw).unwrap(),
        r#"{"type":"image","name":"a\"b"}"#
    );
}

// --- HTTP ------------------------------------------------------------------

struct Canned {
    status: u16,
    content_type: Option<&'static str>,
    body: &'static [u8],
    headers: Vec<(&'static str, &'static str)>,
}

fn canned(status: u16, content_type: Option<&'static str>, body: &'static [u8]) -> Canned {
    Canned {
        status,
        content_type,
        body,
        headers: Vec::new(),
    }
}

/// A 200 upload answer carrying `x-obs-oid`.
fn uploaded(oid: &'static str) -> Canned {
    let mut c = canned(200, None, b"");
    c.headers.push(("x-obs-oid", oid));
    c
}

/// Serves `responses` in order over plain HTTP and records request paths.
async fn serve(responses: Vec<Canned>) -> (String, Arc<Mutex<Vec<String>>>) {
    let (prefix, urls, _) = serve_heads(responses).await;
    (prefix, urls)
}

/// [`serve`], also recording each request head.
async fn serve_heads(
    responses: Vec<Canned>,
) -> (String, Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let urls = Arc::new(Mutex::new(Vec::new()));
    let heads = Arc::new(Mutex::new(Vec::new()));
    let (seen, seen_heads) = (urls.clone(), heads.clone());
    tokio::spawn(async move {
        for r in responses {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            let head_end = loop {
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
                let n = sock.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break buf.len();
                }
                buf.extend_from_slice(&chunk[..n]);
            };
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let body_len: usize = header(&head, "content-length")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            while buf.len() < head_end + body_len {
                let n = sock.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let line = String::from_utf8_lossy(&buf)
                .lines()
                .next()
                .unwrap_or_default()
                .to_string();
            seen.lock()
                .unwrap()
                .push(line.split(' ').nth(1).unwrap_or_default().to_string());
            seen_heads.lock().unwrap().push(head);
            let mut out = format!(
                "HTTP/1.1 {} X\r\nconnection: close\r\ncontent-length: {}\r\n",
                r.status,
                r.body.len()
            );
            if let Some(ct) = r.content_type {
                out.push_str(&format!("content-type: {ct}\r\n"));
            }
            for (k, v) in &r.headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str("\r\n");
            sock.write_all(out.as_bytes()).await.unwrap();
            sock.write_all(r.body).await.unwrap();
            sock.shutdown().await.ok();
        }
    });
    (format!("http://{addr}/"), urls, heads)
}

fn header<'h>(head: &'h str, name: &str) -> Option<&'h str> {
    head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

fn client() -> BaseClient {
    let c = BaseClient::new(Device::DesktopWin).unwrap();
    c.set_auth_token(Some("token".into()));
    c
}

#[tokio::test]
async fn download_object_404_is_obs_error() {
    let (prefix, urls) = serve(vec![canned(404, None, b"")]).await;
    let c = client();
    let err = c
        .obs()
        .with_prefix(prefix)
        .download_object_for_service("talk/emi", "OBJ-1", vec![])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "ObsError: Object download failed: HTTP 404"
    );
    assert_eq!(*urls.lock().unwrap(), ["/r/talk/emi/OBJ-1"]);
}

#[tokio::test]
async fn expired_e2ee_object_fails_before_decryptor() {
    let (prefix, _) = serve(vec![canned(404, None, b"")]).await;
    let c = client();
    let mut message = crate::tv!({"id": "1", "to": "u-recipient", "contentMetadata": {"OID": "OBJ-1", "SID": "emi"}});
    message.insert("chunks", Value::List(vec![Value::Binary(Vec::new())]));
    let err = c
        .obs()
        .with_prefix(prefix)
        .download_media_by_e2ee(
            &message,
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "photo.jpg",
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "ObsError: Object download failed: HTTP 404"
    );
}

#[tokio::test]
async fn message_data_410_stops_before_metadata() {
    let (prefix, urls) = serve(vec![canned(
        410,
        Some("text/html"),
        b"<html><body>expired</body></html>",
    )])
    .await;
    let c = client();
    let err = c
        .obs()
        .with_prefix(prefix)
        .download_message_data("1", false, false)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "ObsError: Message data download failed: HTTP 410"
    );
    assert_eq!(urls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn metadata_empty_403_is_obs_error() {
    let (prefix, _) = serve(vec![canned(403, None, b"")]).await;
    let c = client();
    let err = c
        .obs()
        .with_prefix(prefix)
        .message_obs_metadata("1", false)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "ObsError: Message metadata request failed: HTTP 403"
    );
}

#[tokio::test]
async fn message_data_2xx_yields_file() {
    let (prefix, urls) = serve(vec![
        canned(200, Some("image/jpeg"), &[1, 2, 3]),
        canned(200, Some("application/json"), br#"{"name":"photo.jpg"}"#),
    ])
    .await;
    let c = client();
    let file = c
        .obs()
        .with_prefix(prefix)
        .download_message_data("1", false, false)
        .await
        .unwrap();
    assert_eq!(file.name.as_deref(), Some("photo.jpg"));
    assert_eq!(file.len(), 3);
    assert_eq!(file.mime.as_deref(), Some("image/jpeg"));
    assert_eq!(
        *urls.lock().unwrap(),
        ["/r/talk/m/1", "/r/talk/m/1/object_info.obs"]
    );
}

#[tokio::test]
async fn rejected_upload_is_obs_error() {
    let (prefix, urls) = serve(vec![canned(413, None, b"")]).await;
    let c = client();
    let err = c
        .obs()
        .with_prefix(prefix)
        .upload_object_for_service(ObjectUpload {
            data: Bytes::from_static(b"data"),
            obs_path: Some("talk/emf/reqid-1".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "ObsError: Object upload failed: HTTP 413");
    assert_eq!(*urls.lock().unwrap(), ["/r/talk/emf/reqid-1"]);
}

#[tokio::test]
async fn upload_without_object_id_is_obs_error() {
    let (prefix, _) = serve(vec![canned(200, None, b"")]).await;
    let c = client();
    c.obs_state.token.set("obs", Duration::from_secs(60)).await;
    let err = c
        .obs()
        .with_prefix(prefix)
        .upload(
            Bytes::from_static(b"data"),
            "talk/emf/reqid-1".into(),
            vec![],
            vec![],
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "ObsError: Object upload failed: no object id"
    );
}

#[tokio::test]
async fn v2_upload_sends_obs_token_params_and_content_type() {
    use base64::Engine;
    let (prefix, urls, heads) = serve_heads(vec![uploaded("OID-9")]).await;
    let c = client();
    c.obs_state
        .token
        .set("obs-token", Duration::from_secs(60))
        .await;
    let oid = c
        .obs()
        .with_prefix(prefix)
        .upload(
            Bytes::from_static(b"data"),
            "talk/emi/reqid-1".into(),
            v2_params("cat.PNG", 4),
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(oid, "OID-9");
    assert_eq!(*urls.lock().unwrap(), ["/r/talk/emi/reqid-1"]);
    let head = heads.lock().unwrap()[0].clone();
    assert_eq!(header(&head, "x-line-access"), Some("obs-token"));
    assert_eq!(header(&head, "content-type"), Some("image/png"));
    let params = base64::engine::general_purpose::STANDARD
        .decode(header(&head, "x-obs-params").unwrap())
        .unwrap();
    assert_eq!(
        String::from_utf8(params).unwrap(),
        r#"{"type":"FILE","ver":"2.0","name":"cat.PNG","range":"bytes 0-3/4"}"#
    );
}

#[tokio::test]
async fn hash_upload_has_empty_content_type() {
    let (prefix, _, heads) = serve_heads(vec![uploaded("OID-9")]).await;
    let c = client();
    c.obs_state
        .token
        .set("obs-token", Duration::from_secs(60))
        .await;
    c.obs()
        .with_prefix(prefix)
        .upload(
            Bytes::from_static(b"data"),
            "talk/emv/OID-9__ud-hash".into(),
            v2_params("hash", 4),
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(header(&heads.lock().unwrap()[0], "content-type"), Some(""));
}

#[tokio::test]
async fn upload_401_gets_a_new_token_and_uploads_once_more() {
    let (prefix, urls, heads) = serve_heads(vec![canned(401, None, b""), uploaded("OID-2")]).await;
    let c = client();
    c.obs_state.token.set("old", Duration::from_secs(60)).await;
    let tokens = Tokens::new(vec![Ok("3600\u{1e}new".into())]);
    let up = c
        .obs()
        .with_prefix(prefix)
        .upload_with_obs_token(
            ObjectUpload {
                data: Bytes::from_static(b"data"),
                obs_path: Some("talk/emf/reqid-1".into()),
                ..Default::default()
            },
            &tokens,
        )
        .await
        .unwrap();
    assert_eq!(up.obj_id, "OID-2");
    assert_eq!(tokens.calls(), 1);
    assert_eq!(urls.lock().unwrap().len(), 2);
    let heads = heads.lock().unwrap();
    assert_eq!(header(&heads[0], "x-line-access"), Some("old"));
    assert_eq!(header(&heads[1], "x-line-access"), Some("new"));
}

#[tokio::test]
async fn second_401_is_an_error() {
    let (prefix, urls) = serve(vec![canned(401, None, b""), canned(401, None, b"")]).await;
    let c = client();
    let tokens = Tokens::new(vec![Ok("3600\u{1e}a".into()), Ok("3600\u{1e}b".into())]);
    let err = c
        .obs()
        .with_prefix(prefix)
        .upload_with_obs_token(
            ObjectUpload {
                data: Bytes::from_static(b"data"),
                obs_path: Some("talk/emf/reqid-1".into()),
                ..Default::default()
            },
            &tokens,
        )
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "ObsError: Object upload failed: HTTP 401");
    assert_eq!(urls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn without_obs_token_the_session_token_is_used() {
    let (prefix, _, heads) = serve_heads(vec![uploaded("OID-3")]).await;
    let c = client();
    let tokens = Tokens::new(vec![]);
    c.obs()
        .with_prefix(prefix)
        .upload_with_obs_token(
            ObjectUpload {
                data: Bytes::from_static(b"data"),
                obs_path: Some("talk/emf/reqid-1".into()),
                ..Default::default()
            },
            &tokens,
        )
        .await
        .unwrap();
    assert_eq!(tokens.calls(), OBS_TOKEN_ATTEMPTS);
    assert_eq!(
        header(&heads.lock().unwrap()[0], "x-line-access"),
        Some("token")
    );
}

#[tokio::test]
async fn v1_talk_upload_uses_the_obs_token_and_given_reqseq() {
    use base64::Engine;
    let (prefix, urls, heads) = serve_heads(vec![uploaded("OID-4")]).await;
    let c = client();
    c.obs_state
        .token
        .set("obs-token", Duration::from_secs(60))
        .await;
    let obs = c.obs().with_prefix(prefix);
    MediaBackend::upload_plain(
        &obs,
        "c-group",
        ObjType::Video,
        ObsFile::new(b"mp4".to_vec()).name("a.mp4"),
        Some(3000),
        77,
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(*urls.lock().unwrap(), ["/r/talk/m/reqseq"]);
    let head = heads.lock().unwrap()[0].clone();
    assert_eq!(header(&head, "x-line-access"), Some("obs-token"));
    let params = base64::engine::general_purpose::STANDARD
        .decode(header(&head, "x-obs-params").unwrap())
        .unwrap();
    let params: serde_json::Value = serde_json::from_slice(&params).unwrap();
    assert_eq!(params["reqseq"], "77");
    assert_eq!(params["tomid"], "c-group");
    assert_eq!(params["duration"], "3000");
}

#[tokio::test]
async fn empty_2xx_body_is_returned() {
    let (prefix, _) = serve(vec![canned(204, None, b"")]).await;
    let c = client();
    let data = c
        .obs()
        .with_prefix(prefix)
        .download_object_for_service("talk/emi", "OBJ-1", vec![])
        .await
        .unwrap();
    assert!(data.is_empty());
}

#[test]
fn urls() {
    let c = client();
    let obs = c.obs();
    assert_eq!(
        obs.message_data_url("9", true, false),
        "https://obs.line-apps.com/r/talk/m/9/preview"
    );
    assert_eq!(
        obs.message_metadata_url("9", true),
        "https://obs.line-apps.com/r/g2/m/9/object_info.obs"
    );
}

#[test]
fn metadata_tolerates_nulls_and_numeric_strings() {
    let m: super::ObsMetadata = serde_json::from_value(serde_json::json!({
        "name": "a.jpg",
        "mime": null,
        "ctimeMillis": "1700000000000",
        "offset": null,
        "imageDetails": {"width": "640", "height": 480, "format": null},
        "videoMp4Details": {"durationMillis": "1500", "width": null}
    }))
    .unwrap();
    assert_eq!(m.name, "a.jpg");
    assert_eq!(m.mime, "");
    assert_eq!(m.ctime_millis, 1_700_000_000_000);
    assert_eq!(m.offset, 0);
    let img = m.image_details.unwrap();
    assert_eq!((img.width, img.height), (640, 480));
    let vid = m.video_mp4_details.unwrap();
    assert_eq!((vid.duration_millis, vid.width), (1500, None));
}

mod send_extras;
