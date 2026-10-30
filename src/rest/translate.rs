//! Message translation (`/ds/translate/legyTrans*.nhn`).
//!
//! Derived from static RE of LINE macOS 26.5.0; unverified against live
//! server. Evidence: `CTranslateCommand` / `CTranslateListCommand` vtables
//! at `0x105e287e8` / `0x105e288e8`: POST, server type 24 (`/ds`), body
//! built in the constructor at `0x101fcdcd0` from the key table at
//! `0x105ee90e8` (`originalText`, `sLang`, `tLang`, `xMode`, `service`,
//! `id`), `id` is a `CFUUIDCreateString` UUID, `xMode` an integer 0 and
//! `service` `"talk"`. Headers: `X-Line-Translate-From: line_desktop`,
//! optional `x-client-channel`. The binary suggested `X-Line-Access` only,
//! but the server wants the timeline channel token too (see `AUTH`). The
//! single-text response carries `translatedText`.

use serde_json::{Map, Value as Json};

use super::{RestAuth, RestClient, RestPrefix, RestRequest, channels};
use crate::error::Result;

/// The access token alone is answered with `{"code": 401, "message":
/// "UNAUTHORIZED"}` (live, DESKTOPWIN); CHRLINE sends its timeline headers,
/// which carry the timeline channel token.
const AUTH: RestAuth = RestAuth::Channel(channels::TIMELINE);

pub const TRANSLATE: &str = "/translate/legyTransAPI.nhn";
pub const TRANSLATE_LIST: &str = "/translate/legyTransListAPI.nhn";
/// `X-Line-Translate-From` value sent by the desktop client.
pub const TRANSLATE_FROM: &str = "line_desktop";

/// One text to translate.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranslateText {
    pub text: String,
    /// Source language; omitted when `None`/empty (server detects it).
    pub source_lang: Option<String>,
    pub target_lang: String,
}

impl TranslateText {
    pub fn new(text: impl Into<String>, target_lang: impl Into<String>) -> Self {
        Self { text: text.into(), source_lang: None, target_lang: target_lang.into() }
    }

    fn fields(&self) -> Map<String, Json> {
        let mut m = Map::new();
        m.insert("originalText".into(), self.text.clone().into());
        if let Some(s) = self.source_lang.as_deref().filter(|s| !s.is_empty()) {
            m.insert("sLang".into(), s.into());
        }
        m.insert("tLang".into(), self.target_lang.clone().into());
        m
    }
}

fn request_id() -> String {
    let b: [u8; 16] = rand::random();
    let h = hex::encode_upper(b);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn finish(mut m: Map<String, Json>, id: String) -> Json {
    m.insert("xMode".into(), 0.into());
    m.insert("id".into(), id.into());
    m.insert("service".into(), "talk".into());
    Json::Object(m)
}

/// Body of `legyTransAPI.nhn`.
pub fn translate_body(t: &TranslateText, id: String) -> Json {
    finish(t.fields(), id)
}

/// Body of `legyTransListAPI.nhn`: `{input: [...], xMode, id, service}`.
pub fn translate_list_body(items: &[TranslateText], id: String) -> Json {
    let mut m = Map::new();
    m.insert("input".into(), Json::Array(items.iter().map(|t| Json::Object(t.fields())).collect()));
    finish(m, id)
}

fn request(path: &str, body: Json, client_channel: Option<&str>) -> RestRequest {
    let mut req = RestRequest::post(RestPrefix::Translation, path, AUTH)
        .header("X-Line-Translate-From", TRANSLATE_FROM)
        .body(body);
    if let Some(c) = client_channel.filter(|c| !c.is_empty()) {
        req = req.header("x-client-channel", c);
    }
    req
}

impl RestClient {
    /// Translate one text; the result's `translatedText` holds the output.
    pub async fn translate(&self, text: &TranslateText, client_channel: Option<&str>) -> Result<Json> {
        self.send(request(TRANSLATE, translate_body(text, request_id()), client_channel)).await
    }

    /// Translate several texts in one request.
    pub async fn translate_list(&self, items: &[TranslateText], client_channel: Option<&str>) -> Result<Json> {
        self.send(request(TRANSLATE_LIST, translate_list_body(items, request_id()), client_channel)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies() {
        let mut t = TranslateText::new("hello", "ja");
        assert_eq!(
            translate_body(&t, "ID".into()),
            serde_json::json!({"originalText": "hello", "tLang": "ja", "xMode": 0, "id": "ID", "service": "talk"})
        );
        t.source_lang = Some("en".into());
        let b = translate_list_body(&[t.clone(), TranslateText::new("x", "th")], "ID".into());
        assert_eq!(b["input"][0], serde_json::json!({"originalText": "hello", "sLang": "en", "tLang": "ja"}));
        assert_eq!(b["input"][1]["tLang"], "th");
        assert_eq!(b["service"], "talk");
        let id = request_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.matches('-').count(), 4);
    }
}
