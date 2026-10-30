//! Content metadata builders and parsers shared by talk and square messages
//! (linejs `features/message/utils.ts` plus the parsing halves of
//! `talk.ts` / `square.ts`).
//!
//! LINE measures mention/emoji offsets (`S`/`E`) in UTF-16 code units, the
//! same as JS string indices; every offset here follows that convention.

use indexmap::IndexMap;
use serde_json::Value as Json;

use super::types::{
    sticon_url, AppLinkInfo, CallInfo, ChatEventInfo, ContactMeta, Decoration, Emoji, FileInfo, FlexMeta, GiftInfo,
    MentionTarget, PostNotificationInfo,
};
use crate::error::{Error, Result};
use crate::thrift::Value;

pub(crate) fn type_error(msg: &str) -> Error {
    Error::internal("TypeError", msg)
}

/// Length in UTF-16 code units.
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// JS `String.prototype.substring` over UTF-16 indices (clamped, swapped
/// when `start > end`).
pub fn utf16_substring(s: &str, start: usize, end: Option<usize>) -> String {
    let units: Vec<u16> = s.encode_utf16().collect();
    let len = units.len();
    let (mut a, mut b) = (start.min(len), end.unwrap_or(len).min(len));
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    String::from_utf16_lossy(&units[a..b])
}

/// Build text plus `contentMetadata` (`REPLACE`, `STICON_OWNERSHIP`,
/// `MENTION`) from decorated runs.
pub fn build_text_decorations(decorations: &[Decoration]) -> (String, IndexMap<String, String>) {
    let mut text = String::new();
    let mut resources: Vec<String> = Vec::new();
    let mut ownership: Vec<String> = Vec::new();
    let mut mentionees: Vec<String> = Vec::new();
    let q = |s: &str| Json::String(s.to_string()).to_string();
    for d in decorations {
        let mut run = d.text.clone();
        let start = utf16_len(&text);
        if let Some(e) = &d.emoji {
            if run.is_empty() {
                run = "(linejs)".into();
            }
            let end = start + utf16_len(&run);
            resources.push(format!(
                r#"{{"S":{start},"E":{end},"productId":{},"sticonId":{},"version":{},"resourceType":{}}}"#,
                q(&e.product_id),
                q(&e.sticon_id),
                e.version.unwrap_or(1),
                q(e.resource_type.as_deref().unwrap_or("STATIC")),
            ));
            if !ownership.contains(&e.product_id) {
                ownership.push(e.product_id.clone());
            }
        } else if let Some(m) = &d.mention {
            if run.is_empty() {
                run = "@unknown".into();
            }
            let end = start + utf16_len(&run);
            mentionees.push(match m {
                MentionTarget::All => format!(r#"{{"S":"{start}","E":"{end}","A":"1"}}"#),
                MentionTarget::User(mid) => format!(r#"{{"S":"{start}","E":"{end}","M":{}}}"#, q(mid)),
            });
        }
        text.push_str(&run);
    }
    let mut meta = IndexMap::new();
    if !resources.is_empty() {
        meta.insert("REPLACE".into(), format!(r#"{{"sticon":{{"resources":[{}]}}}}"#, resources.join(",")));
        let owners: Vec<String> = ownership.iter().map(|p| q(p)).collect();
        meta.insert("STICON_OWNERSHIP".into(), format!("[{}]", owners.join(",")));
    }
    if !mentionees.is_empty() {
        meta.insert("MENTION".into(), format!(r#"{{"MENTIONEES":[{}]}}"#, mentionees.join(",")));
    }
    (text, meta)
}

/// Metadata for a sticker message (`contentType = STICKER`).
pub fn sticker_metadata(package_id: &str, sticker_id: &str, version: &str) -> IndexMap<String, String> {
    IndexMap::from([
        ("STKPKGID".to_string(), package_id.to_string()),
        ("STKID".to_string(), sticker_id.to_string()),
        ("STKVER".to_string(), version.to_string()),
    ])
}

/// Content for sending a talk or square message.
#[derive(Debug, Clone, Default)]
pub struct MessageInput {
    pub text: Option<String>,
    /// `None` = plain first, retrying with E2EE if the server demands it
    /// (talk only; ignored for square).
    pub e2ee: Option<bool>,
    /// `ContentType` variant name (default `NONE`).
    pub content_type: Option<String>,
    pub content_metadata: IndexMap<String, String>,
    pub related_message_id: Option<String>,
    /// A `Location` struct value.
    pub location: Option<Value>,
    /// Pre-encrypted E2EE chunks.
    pub chunks: Option<Vec<Vec<u8>>>,
}

impl MessageInput {
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: Some(text.into()), ..Default::default() }
    }
    /// Text with mentions / emoji (see [`build_text_decorations`]).
    pub fn decorated(decorations: &[Decoration]) -> Self {
        let (text, content_metadata) = build_text_decorations(decorations);
        Self { text: Some(text), content_metadata, ..Default::default() }
    }
    /// A sticker (`STKVER` is usually `"1"` or `"100"`).
    pub fn sticker(package_id: &str, sticker_id: &str, version: &str) -> Self {
        Self {
            content_type: Some("STICKER".into()),
            content_metadata: sticker_metadata(package_id, sticker_id, version),
            ..Default::default()
        }
    }
    /// Share a contact (`contentType = CONTACT`, metadata `mid` /
    /// `displayName`).
    pub fn contact(mid: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            content_type: Some("CONTACT".into()),
            content_metadata: IndexMap::from([
                ("mid".to_string(), mid.into()),
                ("displayName".to_string(), display_name.into()),
            ]),
            ..Default::default()
        }
    }
    /// A `Location` struct value.
    pub fn location(location: Value) -> Self {
        Self { location: Some(location), ..Default::default() }
    }
    pub fn e2ee(mut self, e2ee: bool) -> Self {
        self.e2ee = Some(e2ee);
        self
    }
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.content_metadata.insert(key.into(), value.into());
        self
    }
    pub fn reply_to(mut self, message_id: impl Into<String>) -> Self {
        self.related_message_id = Some(message_id.into());
        self
    }
}

impl From<&str> for MessageInput {
    fn from(s: &str) -> Self {
        MessageInput::text(s)
    }
}
impl From<String> for MessageInput {
    fn from(s: String) -> Self {
        MessageInput::text(s)
    }
}
impl From<&String> for MessageInput {
    fn from(s: &String) -> Self {
        MessageInput::text(s.clone())
    }
}

/// `contentMetadata` map → `Value::Map` for thrift.
pub(crate) fn metadata_value(meta: &IndexMap<String, String>) -> Value {
    Value::Map(meta.iter().map(|(k, v)| (Value::String(k.clone()), Value::String(v.clone()))).collect())
}

// ---------------------------------------------------------------------------
// Parsing (operates on a thrift `Message` value)
// ---------------------------------------------------------------------------

/// A borrowed view of a thrift `Message`.
pub(crate) struct Content<'a> {
    pub content_type: String,
    pub text: &'a str,
    pub metadata: IndexMap<String, String>,
}

impl<'a> Content<'a> {
    pub fn of(message: &'a Value) -> Self {
        Content {
            content_type: super::types::enum_name(&message["contentType"], "ContentType")
                .unwrap_or_else(|| "NONE".into()),
            text: message["text"].as_str().unwrap_or(""),
            metadata: message["contentMetadata"].string_map(),
        }
    }

    fn json(&self, key: &str) -> Option<Json> {
        self.metadata.get(key).and_then(|s| serde_json::from_str(s).ok())
    }

    pub fn sticker_url(&self) -> Result<String> {
        if self.content_type != "STICKER" {
            return Err(type_error("The message is not sticker."));
        }
        let id = self.metadata.get("STKID").map(String::as_str).unwrap_or_default();
        Ok(if self.metadata.get("STKOPT").map(String::as_str) == Some("A") {
            format!("https://stickershop.line-scdn.net/stickershop/v1/sticker/{id}/android/sticker_animation.png")
        } else {
            format!("https://stickershop.line-scdn.net/stickershop/v1/sticker/{id}/android/sticker.png")
        })
    }

    fn emoji_resources(&self) -> Vec<Json> {
        self.json("REPLACE")
            .and_then(|r| r["sticon"]["resources"].as_array().cloned())
            .unwrap_or_default()
    }

    fn mentionees(&self) -> Vec<Json> {
        self.json("MENTION").and_then(|m| m["MENTIONEES"].as_array().cloned()).unwrap_or_default()
    }

    pub fn emoji_urls(&self) -> Result<Vec<String>> {
        if self.content_type != "NONE" {
            return Err(type_error("The message is not text message."));
        }
        Ok(self
            .emoji_resources()
            .iter()
            .map(|e| sticon_url(e["productId"].as_str().unwrap_or(""), e["sticonId"].as_str().unwrap_or("")))
            .collect())
    }

    pub fn mentions(&self) -> Result<Vec<MentionTarget>> {
        if self.content_type != "NONE" {
            return Err(type_error("Message has no text."));
        }
        Ok(self.mentionees().iter().map(mention_target).collect())
    }

    pub fn text_decorations(&self) -> Result<Vec<Decoration>> {
        if self.content_type != "NONE" {
            return Err(type_error("Message has no text."));
        }
        enum Kind {
            Mention(MentionTarget),
            Emoji(Emoji),
        }
        let num = |v: &Json| -> usize {
            v.as_u64().map(|n| n as usize).or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).unwrap_or(0)
        };
        let mut splits: Vec<(usize, usize, Kind)> = Vec::new();
        for m in self.mentionees() {
            splits.push((num(&m["S"]), num(&m["E"]), Kind::Mention(mention_target(&m))));
        }
        for e in self.emoji_resources() {
            let mut emoji = Emoji::new(e["productId"].as_str().unwrap_or(""), e["sticonId"].as_str().unwrap_or(""));
            emoji.version = e["version"].as_i64();
            emoji.resource_type = e["resourceType"].as_str().map(str::to_string);
            emoji.url = Some(emoji.image_url());
            splits.push((num(&e["S"]), num(&e["E"]), Kind::Emoji(emoji)));
        }
        splits.sort_by_key(|s| s.0);
        let mut out = Vec::new();
        let mut last = 0usize;
        for (start, end, kind) in splits {
            if last != start {
                out.push(Decoration::text(utf16_substring(self.text, last, Some(start))));
            }
            let mut d = Decoration::text(utf16_substring(self.text, start, Some(end)));
            match kind {
                Kind::Mention(m) => d.mention = Some(m),
                Kind::Emoji(e) => d.emoji = Some(e),
            }
            out.push(d);
            last = end;
        }
        out.push(Decoration::text(utf16_substring(self.text, last, None)));
        Ok(out)
    }

    pub fn shared_contact(&self) -> Result<ContactMeta> {
        if self.content_type != "CONTACT" {
            return Err(type_error("The message does not share contact infomation."));
        }
        Ok(ContactMeta {
            mid: self.metadata.get("mid").cloned().unwrap_or_default(),
            display_name: self.metadata.get("displayName").cloned().unwrap_or_default(),
        })
    }

    pub fn flex(&self) -> Result<FlexMeta> {
        if self.content_type != "FLEX" {
            return Err(type_error("The message has no flex items."));
        }
        let raw = self.metadata.get("FLEX_JSON").cloned().unwrap_or_default();
        Ok(FlexMeta {
            flex_json: serde_json::from_str(&raw).unwrap_or(Json::String(raw)),
            alt_text: self.metadata.get("ALT_TEXT").cloned().unwrap_or_default(),
            ver: self.metadata.get("FLEX_VER").cloned().unwrap_or_default(),
            tag: self.metadata.get("EFFECT_TAG").cloned(),
        })
    }

    pub fn file_info(&self) -> Result<FileInfo> {
        if self.content_type != "FILE" {
            return Err(type_error("The message does not provide any files."));
        }
        let int = |k: &str| self.metadata.get(k).and_then(|s| s.parse().ok()).unwrap_or(0);
        Ok(FileInfo {
            size: int("FILE_SIZE"),
            expire: int("FILE_EXPIRE_TIMESTAMP"),
            name: self.metadata.get("FILE_NAME").cloned().unwrap_or_default(),
        })
    }

    fn meta(&self, key: &str) -> Option<String> {
        self.metadata.get(key).cloned()
    }

    /// Link card of an `APPLINK`, `LINK` or `MUSIC` message.
    pub fn app_link(&self) -> Result<AppLinkInfo> {
        if !matches!(self.content_type.as_str(), "APPLINK" | "LINK" | "MUSIC") {
            return Err(type_error("The message is not a link."));
        }
        Ok(AppLinkInfo {
            id: self.meta("id"),
            kind: self.meta("type"),
            title: self.meta("text").or_else(|| (!self.text.is_empty()).then(|| self.text.to_string())),
            sub_text: self.meta("subText"),
            preview_url: self.meta("previewUrl"),
            link_uri: self.meta("linkUri"),
            android_link_uri: self.meta("a-linkUri"),
            ios_link_uri: self.meta("i-linkUri"),
            android_install_url: self.meta("a-installUrl"),
            ios_install_url: self.meta("i-installUrl"),
            android_package_name: self.meta("a-packageName"),
            country_code: self.meta("countryCode"),
        })
    }

    pub fn gift(&self) -> Result<GiftInfo> {
        if self.content_type != "GIFT" {
            return Err(type_error("The message is not a gift."));
        }
        Ok(GiftInfo {
            product_id: self.meta("PRDID").unwrap_or_default(),
            product_type: self.meta("PRDTYPE").unwrap_or_default(),
            template: self.meta("MSGTPL"),
        })
    }

    pub fn call(&self) -> Result<CallInfo> {
        if self.content_type != "CALL" {
            return Err(type_error("The message is not a call log."));
        }
        Ok(CallInfo {
            group_event: self.meta("GC_EVT_TYPE"),
            group_media_type: self.meta("GC_MEDIA_TYPE"),
            group_chat_mid: self.meta("GC_CHAT_MID"),
            kind: self.meta("TYPE"),
            result: self.meta("RESULT"),
            cause: self.meta("CAUSE"),
            duration_ms: self.metadata.get("DURATION").and_then(|d| d.trim().parse().ok()),
        })
    }

    pub fn post_notification(&self) -> Result<PostNotificationInfo> {
        if self.content_type != "POSTNOTIFICATION" {
            return Err(type_error("The message is not a post notification."));
        }
        Ok(PostNotificationInfo {
            post_end_url: self.meta("postEndUrl"),
            service_type: self.meta("serviceType"),
            text: self.text.to_string(),
        })
    }

    pub fn chat_event(&self) -> Result<ChatEventInfo> {
        if self.content_type != "CHATEVENT" {
            return Err(type_error("The message is not a chat event."));
        }
        let loc_args = self
            .metadata
            .get("LOC_ARGS")
            .map(|a| a.split('\u{1e}').map(str::to_string).collect())
            .unwrap_or_default();
        Ok(ChatEventInfo { loc_key: self.meta("LOC_KEY"), loc_args })
    }

    pub fn has_media(&self) -> bool {
        matches!(self.content_type.as_str(), "IMAGE" | "VIDEO" | "AUDIO" | "FILE")
    }
}

fn mention_target(m: &Json) -> MentionTarget {
    let all = match &m["A"] {
        Json::Null => false,
        Json::String(s) => !s.is_empty(),
        Json::Bool(b) => *b,
        _ => true,
    };
    if all {
        MentionTarget::All
    } else {
        MentionTarget::User(m["M"].as_str().unwrap_or_default().to_string())
    }
}

/// Reply target id if the message is a reply.
pub(crate) fn reply_target(message: &Value) -> Option<String> {
    let rel = &message["messageRelationType"];
    let is_reply = rel.as_i64() == Some(3) || rel.as_str() == Some("REPLY");
    message["relatedMessageId"].as_str().filter(|_| is_reply).map(str::to_string)
}

/// Fetch a media URL (`DOWNLOAD_URL` / `PREVIEW_URL`) with the client's
/// HTTP stack.
pub(crate) async fn fetch_bytes(http: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let res = http.get(url).send().await?.error_for_status()?;
    Ok(res.bytes().await?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tv;

    #[test]
    fn builds_mentions_and_emoji() {
        let (text, meta) = build_text_decorations(&[
            Decoration::text("hi "),
            Decoration::mention("u1", "@Alice"),
            Decoration::text(" "),
            Decoration::mention_all(""),
            Decoration::emoji(Emoji::new("p1", "s1"), ""),
            Decoration::emoji(Emoji::new("p1", "s2"), "x"),
        ]);
        assert_eq!(text, "hi @Alice @unknown(linejs)x");
        assert_eq!(
            meta["MENTION"],
            r#"{"MENTIONEES":[{"S":"3","E":"9","M":"u1"},{"S":"10","E":"18","A":"1"}]}"#
        );
        assert_eq!(
            meta["REPLACE"],
            concat!(
                r#"{"sticon":{"resources":[{"S":18,"E":26,"productId":"p1","sticonId":"s1","version":1,"#,
                r#""resourceType":"STATIC"},{"S":26,"E":27,"productId":"p1","sticonId":"s2","version":1,"#,
                r#""resourceType":"STATIC"}]}}"#
            )
        );
        assert_eq!(meta["STICON_OWNERSHIP"], r#"["p1"]"#);
    }

    #[test]
    fn plain_text_has_no_metadata() {
        let (text, meta) = build_text_decorations(&[Decoration::text("a"), Decoration::text("b")]);
        assert_eq!(text, "ab");
        assert!(meta.is_empty());
    }

    #[test]
    fn utf16_offsets() {
        let (text, meta) = build_text_decorations(&[Decoration::text("😀"), Decoration::mention("u1", "@x")]);
        assert_eq!(text, "😀@x");
        assert!(meta["MENTION"].contains(r#""S":"2","E":"4""#));
        assert_eq!(utf16_substring(&text, 2, Some(4)), "@x");
        assert_eq!(utf16_substring(&text, 4, Some(2)), "@x");
        assert_eq!(utf16_substring(&text, 9, None), "");
    }

    #[test]
    fn roundtrip_decorations() {
        let input = [
            Decoration::text("hi "),
            Decoration::mention("u1", "@A"),
            Decoration::text(" and "),
            Decoration::emoji(Emoji::new("p", "s"), "(e)"),
        ];
        let (text, meta) = build_text_decorations(&input);
        let msg = tv!({"text": text, "contentType": "NONE"});
        let mut msg = msg;
        msg.insert("contentMetadata", metadata_value(&meta));
        let c = Content::of(&msg);
        assert_eq!(c.mentions().unwrap(), vec![MentionTarget::User("u1".into())]);
        assert_eq!(c.emoji_urls().unwrap(), vec![sticon_url("p", "s")]);
        let decos = c.text_decorations().unwrap();
        assert_eq!(decos.len(), 5);
        assert_eq!(decos[0], Decoration::text("hi "));
        assert_eq!(decos[1].mention, Some(MentionTarget::User("u1".into())));
        assert_eq!(decos[2].text, " and ");
        let e = decos[3].emoji.as_ref().unwrap();
        assert_eq!((e.product_id.as_str(), e.version, e.url.as_deref()), ("p", Some(1), Some(sticon_url("p", "s").as_str())));
        assert_eq!(decos[4].text, "");
    }

    #[test]
    fn sticker_and_type_errors() {
        let mut m = tv!({"contentType": "STICKER", "contentMetadata": {"STKID": "52", "STKOPT": "A"}});
        assert!(Content::of(&m).sticker_url().unwrap().ends_with("/52/android/sticker_animation.png"));
        assert!(Content::of(&m).mentions().is_err());
        m.insert("contentType", 0);
        assert!(Content::of(&m).sticker_url().is_err());
        assert_eq!(Content::of(&m).content_type, "NONE");
    }

    #[test]
    fn contact_input() {
        let input = MessageInput::contact("u1", "Alice");
        assert_eq!(input.content_type.as_deref(), Some("CONTACT"));
        let mut msg = tv!({"contentType": "CONTACT"});
        msg.insert("contentMetadata", metadata_value(&input.content_metadata));
        assert_eq!(Content::of(&msg).shared_contact().unwrap(), ContactMeta { mid: "u1".into(), display_name: "Alice".into() });
    }

    #[test]
    fn parses_app_link_and_music() {
        let m = tv!({"contentType": 19, "contentMetadata": {"text": "Song", "subText": "Artist", "type": "mt",
            "id": "mt1", "previewUrl": "https://p", "linkUri": "https://l", "a-linkUri": "https://a",
            "i-linkUri": "https://i", "a-packageName": "jp.linecorp.linemusic.android", "countryCode": "JP"}});
        let info = Content::of(&m).app_link().unwrap();
        assert_eq!(info.title.as_deref(), Some("Song"));
        assert_eq!(info.sub_text.as_deref(), Some("Artist"));
        assert_eq!(info.kind.as_deref(), Some("mt"));
        assert_eq!(info.android_link_uri.as_deref(), Some("https://a"));
        assert_eq!(info.ios_link_uri.as_deref(), Some("https://i"));
        assert_eq!(info.country_code.as_deref(), Some("JP"));
        assert_eq!(info.ios_install_url, None);
        let link = tv!({"contentType": "LINK", "text": "https://x"});
        assert_eq!(Content::of(&link).app_link().unwrap().title.as_deref(), Some("https://x"));
        assert!(Content::of(&tv!({"contentType": "NONE"})).app_link().is_err());
    }

    #[test]
    fn parses_gift_call_post_and_chat_event() {
        let gift = tv!({"contentType": "GIFT", "contentMetadata": {"PRDID": "123", "PRDTYPE": "STICKER", "MSGTPL": "4"}});
        assert_eq!(
            Content::of(&gift).gift().unwrap(),
            GiftInfo { product_id: "123".into(), product_type: "STICKER".into(), template: Some("4".into()) }
        );

        let call = tv!({"contentType": 6, "contentMetadata": {"GC_EVT_TYPE": "E", "GC_MEDIA_TYPE": "VIDEO",
            "GC_CHAT_MID": "c1", "DURATION": "61000"}});
        let info = Content::of(&call).call().unwrap();
        assert!(info.is_group_call());
        assert_eq!((info.group_media_type.as_deref(), info.duration_ms), (Some("VIDEO"), Some(61000)));
        let one = tv!({"contentType": "CALL", "contentMetadata": {"TYPE": "A", "RESULT": "CANCELED", "DURATION": "x"}});
        let info = Content::of(&one).call().unwrap();
        assert!(!info.is_group_call());
        assert_eq!((info.kind.as_deref(), info.result.as_deref(), info.duration_ms), (Some("A"), Some("CANCELED"), None));
        assert!(Content::of(&gift).call().is_err());

        let post = tv!({"contentType": "POSTNOTIFICATION", "text": "hello",
            "contentMetadata": {"postEndUrl": "line://home/post?x", "serviceType": "MH"}});
        let info = Content::of(&post).post_notification().unwrap();
        assert_eq!((info.post_end_url.as_deref(), info.service_type.as_deref(), info.text.as_str()),
            (Some("line://home/post?x"), Some("MH"), "hello"));

        let ev = tv!({"contentType": 18, "contentMetadata": {"LOC_KEY": "C_MI", "LOC_ARGS": "u1\u{1e}u2"}});
        let info = Content::of(&ev).chat_event().unwrap();
        assert_eq!(info.loc_key.as_deref(), Some("C_MI"));
        assert_eq!(info.loc_args, ["u1", "u2"]);
        assert!(Content::of(&tv!({"contentType": 18})).chat_event().unwrap().loc_args.is_empty());
    }

    #[test]
    fn reply_target_detection() {
        assert_eq!(reply_target(&tv!({"relatedMessageId": "1", "messageRelationType": "REPLY"})), Some("1".into()));
        assert_eq!(reply_target(&tv!({"relatedMessageId": "1", "messageRelationType": 3})), Some("1".into()));
        assert_eq!(reply_target(&tv!({"relatedMessageId": "1", "messageRelationType": 0})), None);
    }
}
