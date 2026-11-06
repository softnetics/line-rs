//! `X-Talk-Meta` the way the Android app writes it on uploads and copies
//! (`hp8/c.a`, `n31/a.a`): base64 of `{"message": base64(Message)}`, the
//! `Message` serialized with TBinaryProtocol.

use crate::error::Result;
use crate::thrift::{self, Protocol, TType, TValue};

/// Where one image sits in a photo group (`hp8/i`): every image of the
/// group carries the same `GID`, the group size `GTOTAL` and its 1-based
/// place `GSEQ`. The first upload sends `GID` `"0"` and LINE answers with
/// the real id in `x-line-message-gid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaGroup {
    pub id: String,
    pub total: u32,
    pub seq: u32,
}

impl MediaGroup {
    /// The group's first image, before LINE has named the group.
    pub fn first(total: u32) -> Self {
        Self {
            id: "0".to_string(),
            total,
            seq: 1,
        }
    }
}

/// The `Message` the app writes for these headers (`ts8/od`'s writer):
/// unset strings and maps are left out, but the primitive fields
/// (`createdTime`, `deliveredTime`, `hasContent`, `sessionId`) are always
/// written, and `reactions` starts as an empty list.
fn message(id: Option<&str>, metadata: &[(&str, String)]) -> Result<Vec<u8>> {
    let mut fields = Vec::new();
    if let Some(id) = id {
        fields.push((4, TValue::Binary(id.as_bytes().to_vec())));
    }
    fields.push((5, TValue::I64(0)));
    fields.push((6, TValue::I64(0)));
    fields.push((14, TValue::Bool(false)));
    if !metadata.is_empty() {
        fields.push((
            18,
            TValue::Map(
                TType::String,
                TType::String,
                metadata
                    .iter()
                    .map(|(k, v)| {
                        (
                            TValue::Binary(k.as_bytes().to_vec()),
                            TValue::Binary(v.as_bytes().to_vec()),
                        )
                    })
                    .collect(),
            ),
        ));
    }
    fields.push((19, TValue::Byte(0)));
    fields.push((27, TValue::List(TType::Struct, Vec::new())));
    thrift::write_struct(&fields, Protocol::Binary)
}

fn encode(message: &[u8]) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let json = format!(
        "{{\"message\":{}}}",
        serde_json::Value::from(b64.encode(message))
    );
    b64.encode(json)
}

/// Header name for chats in `to`: squares use `X-Square-Meta`.
pub(crate) fn header_name(to: &str) -> &'static str {
    if to.starts_with('m') || to.starts_with('t') {
        "X-Square-Meta"
    } else {
        "X-Talk-Meta"
    }
}

/// Upload metadata (`hp8/c.a`): `NOTIFICATION_DISABLED` for a silent send
/// (not in squares), then the photo group. `None` when there is nothing to
/// say, as the app then sends no header.
pub(crate) fn upload_meta(
    to: &str,
    group: Option<&MediaGroup>,
    silent: bool,
) -> Result<Option<(String, String)>> {
    let name = header_name(to);
    let mut metadata = Vec::new();
    if silent && name == "X-Talk-Meta" {
        metadata.push(("NOTIFICATION_DISABLED", "true".to_string()));
    }
    if let Some(g) = group {
        metadata.push(("GID", g.id.clone()));
        metadata.push(("GTOTAL", g.total.to_string()));
        metadata.push(("GSEQ", g.seq.to_string()));
    }
    if metadata.is_empty() {
        return Ok(None);
    }
    Ok(Some((name.to_string(), encode(&message(None, &metadata)?))))
}

/// `X-Talk-Meta` naming the message an object belongs to (`n31/a.a`), so
/// OBS lets a copy read it.
pub(crate) fn message_meta(message_id: &str) -> Result<String> {
    Ok(encode(&message(Some(message_id), &[])?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn decode(header: &str) -> Vec<u8> {
        let b64 = base64::engine::general_purpose::STANDARD;
        let json: serde_json::Value = serde_json::from_slice(&b64.decode(header).unwrap()).unwrap();
        b64.decode(json["message"].as_str().unwrap()).unwrap()
    }

    #[test]
    fn nothing_to_say_sends_no_header() {
        assert_eq!(upload_meta("u1", None, false).unwrap(), None);
        // Squares have no silent flag in their meta.
        assert_eq!(upload_meta("m1", None, true).unwrap(), None);
    }

    #[test]
    fn group_meta_matches_the_app_bytes() {
        let (name, value) = upload_meta("c1", Some(&MediaGroup::first(3)), true)
            .unwrap()
            .unwrap();
        assert_eq!(name, "X-Talk-Meta");
        let mut want = vec![
            10, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0, // createdTime
            10, 0, 6, 0, 0, 0, 0, 0, 0, 0, 0, // deliveredTime
            2, 0, 14, 0, // hasContent
            13, 0, 18, 11, 11, 0, 0, 0, 4, // contentMetadata, 4 entries
        ];
        for (k, v) in [
            ("NOTIFICATION_DISABLED", "true"),
            ("GID", "0"),
            ("GTOTAL", "3"),
            ("GSEQ", "1"),
        ] {
            for s in [k, v] {
                want.extend_from_slice(&(s.len() as u32).to_be_bytes());
                want.extend_from_slice(s.as_bytes());
            }
        }
        want.extend_from_slice(&[3, 0, 19, 0]); // sessionId
        want.extend_from_slice(&[15, 0, 27, 12, 0, 0, 0, 0]); // reactions
        want.push(0);
        assert_eq!(decode(&value), want);
    }

    #[test]
    fn square_groups_use_the_square_header() {
        let group = MediaGroup {
            id: "g".into(),
            total: 2,
            seq: 2,
        };
        let (name, _) = upload_meta("m1", Some(&group), false).unwrap().unwrap();
        assert_eq!(name, "X-Square-Meta");
    }

    #[test]
    fn message_meta_names_the_message() {
        let bytes = decode(&message_meta("123").unwrap());
        assert_eq!(&bytes[..7], &[11, 0, 4, 0, 0, 0, 3]);
        assert_eq!(&bytes[7..10], b"123");
        assert!(!bytes.windows(3).any(|w| w == [13, 0, 18]));
    }
}
