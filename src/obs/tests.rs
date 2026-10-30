//! Ports of linejs `obs/upload_e2ee.test.ts` and `obs/download_status.test.ts`.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use indexmap::IndexMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::e2ee::{E2eeMediaBackend, upload_media_by_e2ee};
use super::*;
use crate::device::Device;
use crate::thrift::Value;

type Sent = (String, i32, Vec<Vec<u8>>, IndexMap<String, String>);

#[derive(Default)]
struct Fake {
    /// `(size, obs_path)` per upload.
    records: Mutex<Vec<(usize, String)>>,
    /// Key material passed to message encryption.
    key_materials: Mutex<Vec<String>>,
    sent: Mutex<Vec<Sent>>,
}

#[async_trait]
impl E2eeMediaBackend for Fake {
    async fn upload(&self, data: Bytes, obs_path: String, _params: Vec<(String, String)>) -> Result<String> {
        self.records.lock().unwrap().push((data.len(), obs_path));
        Ok("OBJ-1".into())
    }
    async fn encrypt_message(&self, _to: &str, key_material: &str, _name: &str, _ct: i32) -> Result<Vec<Vec<u8>>> {
        self.key_materials.lock().unwrap().push(key_material.to_string());
        Ok(vec![Vec::new()])
    }
    async fn send(
        &self,
        to: &str,
        chunks: Vec<Vec<u8>>,
        content_type: i32,
        metadata: IndexMap<String, String>,
    ) -> Result<Value> {
        self.sent.lock().unwrap().push((to.to_string(), content_type, chunks, metadata));
        Ok(crate::tv!({"id": "m-1"}))
    }
}

impl Fake {
    fn sizes(&self) -> Vec<usize> {
        self.records.lock().unwrap().iter().map(|r| r.0).collect()
    }
    fn path(&self, i: usize) -> String {
        self.records.lock().unwrap()[i].1.clone()
    }
    fn metadata(&self) -> IndexMap<String, String> {
        let sent = self.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        sent[0].3.clone()
    }
}

fn upload(data: &[u8], o_type: ObjType) -> E2eeMediaUpload {
    E2eeMediaUpload { file: ObsFile::new(data.to_vec()), o_type, to: "u-recipient".into(), ..Default::default() }
}

async fn run(opts: E2eeMediaUpload) -> Fake {
    let fake = Fake::default();
    upload_media_by_e2ee(&fake, opts).await.unwrap();
    fake
}

#[tokio::test]
async fn without_preview_full_edata_uploaded_twice() {
    let fake = run(upload(&[0; 100_000], ObjType::Image)).await;
    let sizes = fake.sizes();
    assert_eq!(sizes.len(), 2);
    assert_eq!(sizes[0], sizes[1]);
    assert!(fake.path(1).ends_with("__ud-preview"));
}

#[tokio::test]
async fn with_preview_uses_small_blob_and_same_key() {
    let mut opts = upload(&[0; 100_000], ObjType::Image);
    opts.preview = Some(Bytes::from(vec![0u8; 2_000]));
    let fake = run(opts).await;
    assert_eq!(fake.sizes(), [100_032, 2_032]);
    assert!(fake.path(1).ends_with("__ud-preview"));
}

#[tokio::test]
async fn preview_is_encrypted_with_the_message_key_material() {
    // Same key material ⇒ the preview ciphertext opens with the key sent in
    // the message.
    let preview = b"thumbnail".to_vec();
    struct Capture(Fake, Mutex<Vec<Bytes>>);
    #[async_trait]
    impl E2eeMediaBackend for Capture {
        async fn upload(&self, data: Bytes, p: String, q: Vec<(String, String)>) -> Result<String> {
            self.1.lock().unwrap().push(data.clone());
            self.0.upload(data, p, q).await
        }
        async fn encrypt_message(&self, to: &str, k: &str, n: &str, ct: i32) -> Result<Vec<Vec<u8>>> {
            self.0.encrypt_message(to, k, n, ct).await
        }
        async fn send(&self, to: &str, c: Vec<Vec<u8>>, ct: i32, m: IndexMap<String, String>) -> Result<Value> {
            self.0.send(to, c, ct, m).await
        }
    }
    let cap = Capture(Fake::default(), Mutex::default());
    let mut opts = upload(b"full image", ObjType::Image);
    opts.preview = Some(Bytes::from(preview.clone()));
    upload_media_by_e2ee(&cap, opts).await.unwrap();
    let key = cap.0.key_materials.lock().unwrap()[0].clone();
    let blobs = cap.1.lock().unwrap();
    assert_eq!(crypto::decrypt_by_key_material_b64(&blobs[0], &key).unwrap(), b"full image");
    assert_eq!(crypto::decrypt_by_key_material_b64(&blobs[1], &key).unwrap(), preview);
}

#[tokio::test]
async fn file_type_has_no_preview_upload() {
    let mut opts = upload(&[0; 50_000], ObjType::File);
    opts.file = opts.file.name("report.pdf");
    let fake = run(opts).await;
    assert_eq!(fake.sizes().len(), 1);
}

#[tokio::test]
async fn video_carries_duration() {
    let mut opts = upload(&[0; 1_000], ObjType::Video);
    opts.duration_ms = Some(4200.4);
    assert_eq!(run(opts).await.metadata()["DURATION"], "4200");
}

#[tokio::test]
async fn no_duration_when_omitted_or_on_image() {
    assert!(!run(upload(&[0; 1_000], ObjType::Video)).await.metadata().contains_key("DURATION"));
    let mut opts = upload(&[0; 1_000], ObjType::Image);
    opts.duration_ms = Some(4200.0);
    assert!(!run(opts).await.metadata().contains_key("DURATION"));
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
        assert_eq!(e2ee_duration(ObjType::Video, Some(d)), None, "durationMs={d}");
    }
}

#[test]
fn valid_durations_round_like_math_round() {
    for (d, expected) in [(0.5, "1"), (1.0, "1"), (4200.5, "4201"), (9_007_199_254_740_991.0, "9007199254740991")] {
        assert_eq!(e2ee_duration(ObjType::Video, Some(d)).as_deref(), Some(expected));
    }
}

#[tokio::test]
async fn duration_does_not_change_non_video_metadata() {
    for o_type in [ObjType::Audio, ObjType::File, ObjType::Image, ObjType::Gif] {
        let mut with = upload(b"media", o_type);
        with.duration_ms = Some(4200.0);
        assert_eq!(run(with).await.metadata(), run(upload(b"media", o_type)).await.metadata());
    }
}

#[tokio::test]
async fn video_metadata_and_preview() {
    let fake = run(E2eeMediaUpload {
        file: ObsFile::new(vec![0u8; 1000]).mime("video/mp4").name("clip.mp4"),
        o_type: ObjType::Video,
        to: "c-group".into(),
        preview: Some(Bytes::from(vec![0u8; 20])),
        duration_ms: Some(4200.0),
    })
    .await;
    let expected: IndexMap<String, String> = [
        ("SID", "emv"),
        ("OID", "OBJ-1"),
        ("FILE_SIZE", "1032"),
        ("e2eeVersion", "2"),
        ("MEDIA_CONTENT_INFO", r#"{"category":"original","fileSize":1032,"extension":"mp4","animated":false}"#),
        ("DURATION", "4200"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert_eq!(fake.metadata(), expected);
    assert_eq!(fake.sizes(), [1032, 52]);
    assert!(fake.path(0).starts_with("talk/emv/reqid-"));
    assert_eq!(fake.path(1), "talk/emv/OBJ-1__ud-preview");
    let sent = fake.sent.lock().unwrap();
    assert_eq!(sent[0].0, "c-group");
    assert_eq!(sent[0].1, 2);
    assert_eq!(sent[0].2, vec![Vec::<u8>::new()]);
}

#[tokio::test]
async fn invalid_mid_is_rejected() {
    let mut opts = upload(b"x", ObjType::Image);
    opts.to = "s-square".into();
    let err = upload_media_by_e2ee(&Fake::default(), opts).await.unwrap_err();
    assert_eq!(err.to_string(), "ObsError: Invalid mid");
}

#[test]
fn talk_meta_header() {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let json: serde_json::Value = serde_json::from_slice(&b64.decode(super::e2ee::talk_meta("1").unwrap()).unwrap()).unwrap();
    let thrift = b64.decode(json["message"].as_str().unwrap()).unwrap();
    // field 4 (string "1"), field 27 (empty list<struct>), stop.
    assert_eq!(thrift, [11, 0, 4, 0, 0, 0, 1, b'1', 15, 0, 27, 12, 0, 0, 0, 0, 0]);
}

#[test]
fn obs_params_are_base64_json() {
    use base64::Engine;
    let p = vec![("type".to_string(), "image".to_string()), ("name".to_string(), "a\"b".to_string())];
    let raw = base64::engine::general_purpose::STANDARD.decode(obs_params_header(&p)).unwrap();
    assert_eq!(String::from_utf8(raw).unwrap(), r#"{"type":"image","name":"a\"b"}"#);
}

// --- download status -------------------------------------------------------

struct Canned {
    status: u16,
    content_type: Option<&'static str>,
    body: &'static [u8],
}

fn canned(status: u16, content_type: Option<&'static str>, body: &'static [u8]) -> Canned {
    Canned { status, content_type, body }
}

/// Serves `responses` in order over plain HTTP and records request paths.
async fn serve(responses: Vec<Canned>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let urls = Arc::new(Mutex::new(Vec::new()));
    let seen = urls.clone();
    tokio::spawn(async move {
        for r in responses {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let line = String::from_utf8_lossy(&buf).lines().next().unwrap_or_default().to_string();
            seen.lock().unwrap().push(line.split(' ').nth(1).unwrap_or_default().to_string());
            let mut head = format!("HTTP/1.1 {} X\r\nconnection: close\r\ncontent-length: {}\r\n", r.status, r.body.len());
            if let Some(ct) = r.content_type {
                head.push_str(&format!("content-type: {ct}\r\n"));
            }
            head.push_str("\r\n");
            sock.write_all(head.as_bytes()).await.unwrap();
            sock.write_all(r.body).await.unwrap();
            sock.shutdown().await.ok();
        }
    });
    (format!("http://{addr}/"), urls)
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
    let err = c.obs().with_prefix(prefix).download_object_for_service("talk/emi", "OBJ-1", vec![]).await.unwrap_err();
    assert_eq!(err.to_string(), "ObsError: Object download failed: HTTP 404");
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
        .download_media_by_e2ee(&message, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=", "photo.jpg")
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "ObsError: Object download failed: HTTP 404");
}

#[tokio::test]
async fn message_data_410_stops_before_metadata() {
    let (prefix, urls) = serve(vec![canned(410, Some("text/html"), b"<html><body>expired</body></html>")]).await;
    let c = client();
    let err = c.obs().with_prefix(prefix).download_message_data("1", false, false).await.unwrap_err();
    assert_eq!(err.to_string(), "ObsError: Message data download failed: HTTP 410");
    assert_eq!(urls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn metadata_empty_403_is_obs_error() {
    let (prefix, _) = serve(vec![canned(403, None, b"")]).await;
    let c = client();
    let err = c.obs().with_prefix(prefix).message_obs_metadata("1", false).await.unwrap_err();
    assert_eq!(err.to_string(), "ObsError: Message metadata request failed: HTTP 403");
}

#[tokio::test]
async fn message_data_2xx_yields_file() {
    let (prefix, urls) = serve(vec![
        canned(200, Some("image/jpeg"), &[1, 2, 3]),
        canned(200, Some("application/json"), br#"{"name":"photo.jpg"}"#),
    ])
    .await;
    let c = client();
    let file = c.obs().with_prefix(prefix).download_message_data("1", false, false).await.unwrap();
    assert_eq!(file.name.as_deref(), Some("photo.jpg"));
    assert_eq!(file.len(), 3);
    assert_eq!(file.mime.as_deref(), Some("image/jpeg"));
    assert_eq!(*urls.lock().unwrap(), ["/r/talk/m/1", "/r/talk/m/1/object_info.obs"]);
}

#[tokio::test]
async fn empty_2xx_body_is_returned() {
    let (prefix, _) = serve(vec![canned(204, None, b"")]).await;
    let c = client();
    let data = c.obs().with_prefix(prefix).download_object_for_service("talk/emi", "OBJ-1", vec![]).await.unwrap();
    assert!(data.is_empty());
}

#[test]
fn urls() {
    let c = client();
    let obs = c.obs();
    assert_eq!(obs.message_data_url("9", true, false), "https://obs.line-apps.com/r/talk/m/9/preview");
    assert_eq!(obs.message_metadata_url("9", true), "https://obs.line-apps.com/r/g2/m/9/object_info.obs");
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
