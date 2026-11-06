//! Photo groups, silent sends and `copy.obs` forwards.

use super::*;
use crate::obs::group::send_image_group;

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
