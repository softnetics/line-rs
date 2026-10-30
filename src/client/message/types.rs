//! Shared message data types (linejs `features/message/types.ts` and
//! `internal-types.ts`).

use crate::thrift::{Schema, Value};

/// A mid together with its `MIDType` name (`USER`, `GROUP`, `ROOM`,
/// `SQUARE_CHAT`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mid {
    pub id: String,
    pub kind: String,
}

/// Who a mention points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MentionTarget {
    /// `@All`.
    All,
    /// A single user (or square member) mid.
    User(String),
}

/// An emoji (sticon) decoration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emoji {
    pub product_id: String,
    pub sticon_id: String,
    /// Defaults to `1` when building.
    pub version: Option<i64>,
    /// Defaults to `STATIC` when building.
    pub resource_type: Option<String>,
    /// Image URL (filled when parsing a message).
    pub url: Option<String>,
}

impl Emoji {
    pub fn new(product_id: impl Into<String>, sticon_id: impl Into<String>) -> Self {
        Self { product_id: product_id.into(), sticon_id: sticon_id.into(), version: None, resource_type: None, url: None }
    }

    /// `https://stickershop.line-scdn.net/sticonshop/v1/sticon/<product>/android/<sticon>.png`
    pub fn image_url(&self) -> String {
        sticon_url(&self.product_id, &self.sticon_id)
    }
}

pub(crate) fn sticon_url(product_id: &str, sticon_id: &str) -> String {
    format!("https://stickershop.line-scdn.net/sticonshop/v1/sticon/{product_id}/android/{sticon_id}.png")
}

/// One run of text, optionally decorated with an emoji or a mention.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Decoration {
    pub text: String,
    pub emoji: Option<Emoji>,
    pub mention: Option<MentionTarget>,
}

impl Decoration {
    /// Plain text.
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Default::default() }
    }
    /// A mention of `mid`, displayed as `text` (e.g. `@Alice`).
    pub fn mention(mid: impl Into<String>, text: impl Into<String>) -> Self {
        Self { text: text.into(), mention: Some(MentionTarget::User(mid.into())), ..Default::default() }
    }
    /// An `@All` mention displayed as `text`.
    pub fn mention_all(text: impl Into<String>) -> Self {
        Self { text: text.into(), mention: Some(MentionTarget::All), ..Default::default() }
    }
    /// An emoji; `text` is the fallback text (default `(linejs)` when empty).
    pub fn emoji(emoji: Emoji, text: impl Into<String>) -> Self {
        Self { text: text.into(), emoji: Some(emoji), ..Default::default() }
    }
}

/// Contact shared in a `CONTACT` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactMeta {
    pub mid: String,
    pub display_name: String,
}

/// Flex payload of a `FLEX` message.
#[derive(Debug, Clone, PartialEq)]
pub struct FlexMeta {
    /// Parsed `FLEX_JSON` (the raw string when it is not valid JSON).
    pub flex_json: serde_json::Value,
    pub alt_text: String,
    pub ver: String,
    pub tag: Option<String>,
}

/// File info of a `FILE` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    pub size: i64,
    /// Expiry as unix seconds.
    pub expire: i64,
    pub name: String,
}

/// Link card of an `APPLINK`, `LINK` or `MUSIC` message. Every field is
/// the raw `contentMetadata` value of the key named in its doc; absent keys
/// stay `None`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppLinkInfo {
    /// `id`.
    pub id: Option<String>,
    /// `type` (e.g. `mt` for music).
    pub kind: Option<String>,
    /// `text` (title).
    pub title: Option<String>,
    /// `subText`.
    pub sub_text: Option<String>,
    /// `previewUrl` (thumbnail).
    pub preview_url: Option<String>,
    /// `linkUri` (generic target).
    pub link_uri: Option<String>,
    /// `a-linkUri` (Android target).
    pub android_link_uri: Option<String>,
    /// `i-linkUri` (iOS target).
    pub ios_link_uri: Option<String>,
    /// `a-installUrl`.
    pub android_install_url: Option<String>,
    /// `i-installUrl`.
    pub ios_install_url: Option<String>,
    /// `a-packageName`.
    pub android_package_name: Option<String>,
    /// `countryCode`.
    pub country_code: Option<String>,
}

/// A `GIFT` message (`PRDID`, `PRDTYPE`, `MSGTPL`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GiftInfo {
    /// `PRDID`: product (sticker package, theme, ...) id.
    pub product_id: String,
    /// `PRDTYPE`: `STICKER`, `THEME`, `STICON`, ...
    pub product_type: String,
    /// `MSGTPL`: card template number.
    pub template: Option<String>,
}

/// A `CALL` message (a 1:1 or group call log line).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CallInfo {
    /// `GC_EVT_TYPE`: group call event, `S` (started) or `E` (ended).
    pub group_event: Option<String>,
    /// `GC_MEDIA_TYPE`: `AUDIO`, `VIDEO` or `LIVE` (group calls).
    pub group_media_type: Option<String>,
    /// `GC_CHAT_MID`: chat of a group call.
    pub group_chat_mid: Option<String>,
    /// `TYPE`: 1:1 call kind (`A` audio, `V` video).
    pub kind: Option<String>,
    /// `RESULT` (e.g. `NORMAL`, `CANCELED`, `NO_ANSWER`).
    pub result: Option<String>,
    /// `CAUSE`: end reason code.
    pub cause: Option<String>,
    /// `DURATION` in milliseconds.
    pub duration_ms: Option<i64>,
}

impl CallInfo {
    /// Whether the message logs a group call.
    pub fn is_group_call(&self) -> bool {
        self.group_event.is_some() || self.group_chat_mid.is_some()
    }
}

/// A `POSTNOTIFICATION` message (timeline / note post shared to a chat).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PostNotificationInfo {
    /// `postEndUrl`: link to the post.
    pub post_end_url: Option<String>,
    /// `serviceType` (`MH` timeline, `GB` group note, ...).
    pub service_type: Option<String>,
    /// The message text (post excerpt).
    pub text: String,
}

/// A `CHATEVENT` system message: a localization key plus its arguments
/// (`LOC_KEY` / `LOC_ARGS`, arguments separated by `U+001E`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChatEventInfo {
    pub loc_key: Option<String>,
    pub loc_args: Vec<String>,
}

/// `MessageReactionType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactionType {
    All = 0,
    Undo = 1,
    Nice = 2,
    Love = 3,
    Fun = 4,
    Amazing = 5,
    Sad = 6,
    Omg = 7,
}

impl ReactionType {
    /// IDL variant name.
    pub fn as_str(self) -> &'static str {
        match self {
            ReactionType::All => "ALL",
            ReactionType::Undo => "UNDO",
            ReactionType::Nice => "NICE",
            ReactionType::Love => "LOVE",
            ReactionType::Fun => "FUN",
            ReactionType::Amazing => "AMAZING",
            ReactionType::Sad => "SAD",
            ReactionType::Omg => "OMG",
        }
    }

    /// From the IDL number.
    pub fn from_i64(n: i64) -> Option<Self> {
        Some(match n {
            0 => ReactionType::All,
            1 => ReactionType::Undo,
            2 => ReactionType::Nice,
            3 => ReactionType::Love,
            4 => ReactionType::Fun,
            5 => ReactionType::Amazing,
            6 => ReactionType::Sad,
            7 => ReactionType::Omg,
            _ => return None,
        })
    }

    /// From the IDL variant name (`NICE`, ...).
    pub fn from_name(name: &str) -> Option<Self> {
        (0..=7).filter_map(Self::from_i64).find(|r| r.as_str() == name)
    }
}

/// Render an enum value (name or number) as its IDL variant name.
pub(crate) fn enum_name(v: &Value, enum_name: &str) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Int(i) => Schema::line()
            .get_enum(enum_name)
            .and_then(|e| e.name_of(*i as i32))
            .map(str::to_string)
            .or_else(|| Some(i.to_string())),
        _ => None,
    }
}

/// Numeric value of an enum given by name or number.
pub(crate) fn enum_value(v: &Value, enum_name: &str) -> Option<i32> {
    match v {
        Value::String(s) => Schema::line().get_enum(enum_name).and_then(|e| e.value_of(s)).or_else(|| s.parse().ok()),
        other => other.as_i64().map(|i| i as i32),
    }
}
