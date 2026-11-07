//! Photo groups, silent sends and `copy.obs` forwards.

use super::*;
use crate::obs::copy::{copy_body, forward_media};
use crate::obs::group::send_image_group;
use crate::obs::meta::message_meta;

/// The `contentMetadata` pairs inside an `X-Talk-Meta` value.
fn meta_pairs(header: &str) -> Vec<(String, String)> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let json: serde_json::Value = serde_json::from_slice(&b64.decode(header).unwrap()).unwrap();
    let bytes = b64.decode(json["message"].as_str().unwrap()).unwrap();
    let Some(at) = bytes.windows(5).position(|w| w == [13, 0, 18, 11, 11]) else {
        return Vec::new();
    };
    let mut pos = at + 5;
    let read_u32 = |pos: &mut usize| {
        let n = u32::from_be_bytes(bytes[*pos..*pos + 4].try_into().unwrap()) as usize;
        *pos += 4;
        n
    };
    let read_str = |pos: &mut usize| {
        let n = read_u32(pos);
        let s = String::from_utf8(bytes[*pos..*pos + n].to_vec()).unwrap();
        *pos += n;
        s
    };
    let count = read_u32(&mut pos);
    (0..count)
        .map(|_| (read_str(&mut pos), read_str(&mut pos)))
        .collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The `X-Talk-Meta` pairs of upload `i` (V2 and V1 counted together).
fn upload_meta(fake: &Fake, i: usize) -> Vec<(String, String)> {
    let headers = fake.headers.lock().unwrap();
    headers[i]
        .iter()
        .find(|(k, _)| k == "X-Talk-Meta")
        .map(|(_, v)| meta_pairs(v))
        .unwrap_or_default()
}

fn images(n: usize) -> Vec<(ObsFile, MediaOptions)> {
    (0..n)
        .map(|i| {
            (
                ObsFile::new(vec![i as u8; 10]).name(format!("{i}.jpg")),
                MediaOptions::new(),
            )
        })
        .collect()
}

// --- photo groups ----------------------------------------------------------

#[tokio::test]
async fn group_goes_plain_with_the_id_from_the_first_upload() {
    let fake = Fake::default();
    fake.plain_answers.lock().unwrap().push_back(Ok("G-9"));
    let results = send_image_group(&fake, "u-recipient", images(3)).await;
    assert_eq!(results.len(), 3);
    assert!(matches!(
        &results[0],
        Ok(SentMedia::Plain { group_id: Some(g), .. }) if g == "G-9"
    ));
    assert!(results.iter().all(|r| r.is_ok()));
    // Plain even though the chat's flow is V2, and LINE is never asked.
    assert_eq!(*fake.flow_calls.lock().unwrap(), 0);
    assert!(fake.uploads.lock().unwrap().is_empty());
    assert_eq!(fake.plain.lock().unwrap().len(), 3);
    assert_eq!(
        upload_meta(&fake, 0),
        pairs(&[("GID", "0"), ("GTOTAL", "3"), ("GSEQ", "1")])
    );
    assert_eq!(
        upload_meta(&fake, 1),
        pairs(&[("GID", "G-9"), ("GTOTAL", "3"), ("GSEQ", "2")])
    );
    assert_eq!(
        upload_meta(&fake, 2),
        pairs(&[("GID", "G-9"), ("GTOTAL", "3"), ("GSEQ", "3")])
    );
}

#[tokio::test]
async fn group_stops_after_a_failed_image() {
    let fake = Fake::default();
    fake.plain_answers
        .lock()
        .unwrap()
        .extend([Ok("G-9"), Err(500)]);
    let results = send_image_group(&fake, "c-group", images(4)).await;
    assert!(results[0].is_ok());
    assert!(
        results[1]
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("HTTP 500")
    );
    for r in &results[2..] {
        assert!(
            r.as_ref()
                .unwrap_err()
                .to_string()
                .contains("earlier image")
        );
    }
    assert_eq!(fake.plain.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn failed_first_image_sends_none_of_the_group() {
    let fake = Fake::default();
    fake.plain_answers.lock().unwrap().push_back(Err(413));
    let results = send_image_group(&fake, "u-recipient", images(2)).await;
    assert!(results.iter().all(|r| r.is_err()));
    assert_eq!(fake.plain.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn without_a_group_id_the_rest_go_one_by_one() {
    let fake = Fake::default();
    let results = send_image_group(&fake, "u-recipient", images(3)).await;
    assert!(results.iter().all(|r| r.is_ok()));
    assert_eq!(fake.plain.lock().unwrap().len(), 1);
    // The other two follow the chat's (V2) flow, with no group meta.
    assert!(matches!(results[1], Ok(SentMedia::E2ee(_))));
    assert_eq!(fake.sent.lock().unwrap().len(), 2);
    assert!(upload_meta(&fake, 1).is_empty());
}

#[tokio::test]
async fn a_single_image_is_not_a_group() {
    let fake = Fake::default();
    let results = send_image_group(&fake, "u-recipient", images(1)).await;
    assert!(matches!(results[..], [Ok(SentMedia::E2ee(_))]));
    assert!(fake.plain.lock().unwrap().is_empty());
    assert!(upload_meta(&fake, 0).is_empty());
}

// --- silent sends ----------------------------------------------------------

#[tokio::test]
async fn silent_send_marks_the_upload_and_the_message() {
    let mut m = media(&[0; 10], ObjType::File);
    m.opts = MediaOptions::new().silent(true);
    let fake = run(m).await;
    assert_eq!(
        upload_meta(&fake, 0),
        pairs(&[("NOTIFICATION_DISABLED", "true")])
    );
    assert_eq!(fake.metadata()["NOTIFICATION_DISABLED"], "true");

    let fake = run(media(&[0; 10], ObjType::File)).await;
    assert!(fake.headers.lock().unwrap()[0].is_empty());
    assert!(!fake.metadata().contains_key("NOTIFICATION_DISABLED"));
}

#[tokio::test]
async fn silent_plain_upload_sends_the_meta() {
    let mut m = media(&[0; 10], ObjType::Image);
    m.opts = MediaOptions::new().silent(true).e2ee(false);
    let fake = run(m).await;
    assert_eq!(
        upload_meta(&fake, 0),
        pairs(&[("NOTIFICATION_DISABLED", "true")])
    );
}

// --- copy.obs forwards -----------------------------------------------------

fn forward(o_type: ObjType, name: &str) -> ForwardMedia {
    ForwardMedia {
        to: "u-self".into(),
        o_type,
        src_oid: "SRC-OID".into(),
        src_message_id: "777".into(),
        key_material: "KEY".into(),
        file_size: 1234,
        file_name: Some(name.into()),
        opts: MediaOptions::new(),
    }
}

#[tokio::test]
async fn forward_copies_and_reuses_the_key() {
    let fake = Fake::default();
    let mut f = forward(ObjType::Image, "cat.PNG");
    f.opts = MediaOptions::new().size(4, 3);
    let res = forward_media(&fake, f).await;
    assert!(matches!(res.unwrap(), SentMedia::E2ee(_)));
    assert_eq!(
        *fake.copies.lock().unwrap(),
        [(ObjType::Image, "SRC-OID".to_string(), "777".to_string())]
    );
    assert!(fake.uploads.lock().unwrap().is_empty());
    assert_eq!(*fake.encrypted.lock().unwrap(), [("KEY".to_string(), None)]);
    let m = fake.metadata();
    assert_eq!(m["SID"], "emi");
    assert_eq!(m["OID"], "COPY-1");
    assert_eq!(m["FILE_SIZE"], "1234");
    assert!(m["MEDIA_CONTENT_INFO"].contains(r#""extension":"png""#));
    assert!(m["MEDIA_CONTENT_INFO"].contains(r#""width":4"#));
    assert_eq!(fake.sent.lock().unwrap()[0].1, 1);
}

#[tokio::test]
async fn forwarded_files_and_audio_keep_name_and_duration() {
    let fake = Fake::default();
    forward_media(&fake, forward(ObjType::File, "report.pdf"))
        .await
        .unwrap();
    assert_eq!(
        fake.encrypted.lock().unwrap()[0].1.as_deref(),
        Some("report.pdf")
    );
    assert_eq!(fake.metadata()["SID"], "emf");

    let fake = Fake::default();
    let mut f = forward(ObjType::Audio, "voice.m4a");
    f.opts = MediaOptions::new().duration_ms(3100);
    forward_media(&fake, f).await.unwrap();
    assert_eq!(fake.encrypted.lock().unwrap()[0].1, None);
    let m = fake.metadata();
    assert_eq!(m["SID"], "ema");
    assert_eq!(m["DURATION"], "3100");
}

#[tokio::test]
async fn forward_refuses_plain_chats_without_copying() {
    let fake = Fake::with_flows(MediaFlows::all(MediaFlow::V1));
    assert!(
        forward_media(&fake, forward(ObjType::Video, "a.mp4"))
            .await
            .is_err()
    );
    assert!(fake.copies.lock().unwrap().is_empty());

    let fake = Fake::default();
    let mut f = forward(ObjType::Video, "a.mp4");
    f.to = "r-room".into();
    assert!(forward_media(&fake, f).await.is_err());
    assert!(fake.copies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_copy_sends_nothing() {
    let fake = Fake {
        copy_fails: true,
        ..Fake::default()
    };
    let err = forward_media(&fake, forward(ObjType::Image, "a.jpg"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("copy failed"));
    assert!(fake.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn forward_reencrypts_but_never_uploads_plain() {
    let fake = Fake::failing_sends(&["E2EE_UPDATE_RECEIVER_KEY"]);
    forward_media(&fake, forward(ObjType::Image, "a.jpg"))
        .await
        .unwrap();
    assert_eq!(fake.sent.lock().unwrap().len(), 2);
    assert_eq!(fake.copies.lock().unwrap().len(), 1);

    let fake = Fake::failing_sends(&["E2EE_RETRY_PLAIN"]);
    let err = forward_media(&fake, forward(ObjType::Image, "a.jpg"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("E2EE_RETRY_PLAIN"));
    assert!(fake.plain.lock().unwrap().is_empty());
    assert_eq!(*fake.dropped.lock().unwrap(), ["u-self"]);
}

#[test]
fn copy_body_matches_the_app() {
    let body = copy_body("emv", "SRC", "reqid-1", "tok", "META");
    assert_eq!(
        body,
        serde_json::json!({
            "src": {"svcCode": "talk", "sid": "emv", "oid": "SRC", "relay": {
                "headers": {"X-Line-Access": "tok", "X-Talk-Meta": "META"},
                "params": {"srcSvcCode": "talk", "srcOid": "SRC", "srcSid": "emv"},
                "commonHeaders": ["x-real-ip"]}},
            "dst": {"relay": {
                "headers": {"X-Line-Access": "tok", "X-Talk-Meta": "META"},
                "params": {"dstSvcCode": "talk", "dstOid": "reqid-1", "dstSid": "emv"},
                "commonHeaders": ["x-real-ip"]}},
            "options": {},
        })
    );
}

#[tokio::test]
async fn copy_posts_to_copy_obs_and_reads_the_new_oid() {
    let (prefix, urls, heads) = serve_heads(vec![uploaded("NEW-OID")]).await;
    let c = client();
    c.obs_state
        .token
        .set("obs-token", Duration::from_secs(60))
        .await;
    let oid = c
        .obs()
        .with_prefix(prefix)
        .copy_e2ee_object_with(ObjType::Video, "SRC", "777", &Tokens::new(vec![]))
        .await
        .unwrap();
    assert_eq!(oid, "NEW-OID");
    let url = urls.lock().unwrap()[0].clone();
    assert!(
        url.starts_with("/r/talk/emv/reqid-") && url.ends_with("/copy.obs"),
        "{url}"
    );
    let head = heads.lock().unwrap()[0].clone();
    assert_eq!(header(&head, "content-type"), Some("application/json"));
    assert_eq!(header(&head, "x-line-access"), Some("obs-token"));
    assert_eq!(header(&head, "x-obs-channeltype"), Some("legy"));
    assert_eq!(header(&head, "x-obs-host"), Some("127.0.0.1"));
    assert_eq!(
        header(&head, "x-talk-meta"),
        Some(message_meta("777").unwrap().as_str())
    );
}

#[tokio::test]
async fn copy_401_gets_a_new_token_and_a_new_request_id() {
    let (prefix, urls, heads) =
        serve_heads(vec![canned(401, None, b""), uploaded("NEW-OID")]).await;
    let c = client();
    c.obs_state.token.set("old", Duration::from_secs(60)).await;
    let tokens = Tokens::new(vec![Ok("3600\u{1e}new".into())]);
    let oid = c
        .obs()
        .with_prefix(prefix)
        .copy_e2ee_object_with(ObjType::Image, "SRC", "777", &tokens)
        .await
        .unwrap();
    assert_eq!(oid, "NEW-OID");
    let urls = urls.lock().unwrap();
    assert_eq!(urls.len(), 2);
    assert_ne!(urls[0], urls[1]);
    let heads = heads.lock().unwrap();
    assert_eq!(header(&heads[1], "x-line-access"), Some("new"));
}

#[tokio::test]
async fn copy_without_oid_or_with_403_is_an_error() {
    let (prefix, _) = serve(vec![canned(200, None, b""), canned(403, None, b"")]).await;
    let c = client();
    c.obs_state.token.set("t", Duration::from_secs(60)).await;
    let obs = c.obs().with_prefix(prefix);
    let tokens = Tokens::new(vec![]);
    let err = obs
        .copy_e2ee_object_with(ObjType::File, "SRC", "1", &tokens)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "ObsError: Object copy failed: no object id"
    );
    let err = obs
        .copy_e2ee_object_with(ObjType::File, "SRC", "1", &tokens)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "ObsError: Object copy failed: HTTP 403");
}
