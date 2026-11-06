//! `linepostback://` URLs, which `sendPostback` carries to a bot.
//!
//! LINE for Android builds them from flex, template and rich-menu actions
//! (`md2/b.java`) and sends the whole URL as `SendPostbackRequest.url`
//! (`ot1/p.java`). Query values are encoded as Android's
//! `Uri.Builder.appendQueryParameter` does, and optional ones are left out
//! when empty.
//!
//! ```
//! use line_rs::client::postback::Postback;
//! let url = Postback::new("action=buy&id=1").display_text("ซื้อ").url();
//! assert_eq!(
//!     url,
//!     "linepostback://postback?_data=action%3Dbuy%26id%3D1&_displayText=%E0%B8%8B%E0%B8%B7%E0%B9%89%E0%B8%AD"
//! );
//! ```

/// The scheme of postback URLs.
pub const SCHEME: &str = "linepostback";

/// A postback action: `linepostback://postback?_data=…[&_text=…]
/// [&_displayText=…][&_io=…][&_fi=…]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Postback {
    /// The action's `data`, sent even when empty.
    pub data: String,
    /// The legacy `text` field of template actions.
    pub text: Option<String>,
    /// `displayText`: the bot's server shows it in the chat as the user's
    /// message.
    pub display_text: Option<String>,
    /// `inputOption` (`openKeyboard`, `openVoice`, `openRichMenu`,
    /// `closeRichMenu`).
    pub input_option: Option<String>,
    /// `fillInText`, put in the message box with `openKeyboard`.
    pub fill_in_text: Option<String>,
}

impl Postback {
    pub fn new(data: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            ..Default::default()
        }
    }

    pub fn display_text(mut self, text: impl Into<String>) -> Self {
        self.display_text = Some(text.into());
        self
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn input_option(mut self, option: impl Into<String>) -> Self {
        self.input_option = Some(option.into());
        self
    }

    pub fn fill_in_text(mut self, text: impl Into<String>) -> Self {
        self.fill_in_text = Some(text.into());
        self
    }

    /// The URL for `sendPostback`.
    pub fn url(&self) -> String {
        let mut q = Query::default();
        q.push("_data", &self.data);
        q.push_opt("_text", self.text.as_deref());
        q.push_opt("_displayText", self.display_text.as_deref());
        q.push_opt("_io", self.input_option.as_deref());
        q.push_opt("_fi", self.fill_in_text.as_deref());
        format!("{SCHEME}://postback?{}", q.0)
    }
}

/// A `message` action as LINE for Android sends it from flex and template
/// messages: `linepostback://postback?_text=…` through `sendPostback`.
pub fn message_url(text: &str) -> String {
    let mut q = Query::default();
    q.push("_text", text);
    format!("{SCHEME}://postback?{}", q.0)
}

/// What a datetime picker asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateTimeMode {
    /// `yyyy-MM-dd`.
    Date,
    /// `HH:mm`.
    Time,
    /// `yyyy-MM-dd'T'HH:mm`.
    DateTime,
}

impl DateTimeMode {
    /// The `mode` of a datetime picker action.
    pub fn as_str(self) -> &'static str {
        match self {
            DateTimeMode::Date => "date",
            DateTimeMode::Time => "time",
            DateTimeMode::DateTime => "datetime",
        }
    }

    pub fn parse(mode: &str) -> Option<Self> {
        match mode {
            "date" => Some(DateTimeMode::Date),
            "time" => Some(DateTimeMode::Time),
            "datetime" => Some(DateTimeMode::DateTime),
            _ => None,
        }
    }
}

/// The URL sent when the user picks a value in a datetime picker action:
/// `linepostback://postback/datetimepicker?_data=…&_mode=…&<mode>=<value>`
/// (`ot1/o.java`). `value` is in the mode's format (see [`DateTimeMode`]).
/// The value's key being the mode name is inferred from the picker enum
/// (`pt1/b.java`) and unverified.
pub fn datetime_picker_url(data: &str, mode: DateTimeMode, value: &str) -> String {
    let mut q = Query::default();
    q.push("_data", data);
    q.push("_mode", mode.as_str());
    q.push(mode.as_str(), value);
    format!("{SCHEME}://postback/datetimepicker?{}", q.0)
}

/// Whether `url` is a postback URL that goes to `sendPostback` rather than
/// being opened.
pub fn is_postback_url(url: &str) -> bool {
    url.get(..SCHEME.len() + 3)
        .is_some_and(|p| p.eq_ignore_ascii_case(&format!("{SCHEME}://")))
}

#[derive(Default)]
struct Query(String);

impl Query {
    fn push(&mut self, key: &str, value: &str) {
        if !self.0.is_empty() {
            self.0.push('&');
        }
        self.0.push_str(&encode(key));
        self.0.push('=');
        self.0.push_str(&encode(value));
    }

    fn push_opt(&mut self, key: &str, value: Option<&str>) {
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            self.push(key, v);
        }
    }
}

/// `android.net.Uri.encode`: keeps letters, digits and `_-!.~'()*`.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'_'
            | b'-'
            | b'!'
            | b'.'
            | b'~'
            | b'\''
            | b'('
            | b')'
            | b'*' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postback_keeps_the_apps_order_and_skips_empty_fields() {
        let p = Postback {
            data: "a=1 b".into(),
            text: Some(String::new()),
            display_text: Some("ok".into()),
            input_option: Some("openKeyboard".into()),
            fill_in_text: Some("hi there".into()),
        };
        assert_eq!(
            p.url(),
            "linepostback://postback?_data=a%3D1%20b&_displayText=ok&_io=openKeyboard&_fi=hi%20there"
        );
        assert_eq!(Postback::new("").url(), "linepostback://postback?_data=");
    }

    #[test]
    fn message_and_picker_urls() {
        assert_eq!(
            message_url("สวัสดี!"),
            "linepostback://postback?_text=%E0%B8%AA%E0%B8%A7%E0%B8%B1%E0%B8%AA%E0%B8%94%E0%B8%B5!"
        );
        assert_eq!(
            datetime_picker_url("d", DateTimeMode::DateTime, "2026-11-06T09:30"),
            "linepostback://postback/datetimepicker?_data=d&_mode=datetime&datetime=2026-11-06T09%3A30"
        );
        assert_eq!(DateTimeMode::parse("time"), Some(DateTimeMode::Time));
    }

    #[test]
    fn recognises_postback_urls() {
        assert!(is_postback_url("linepostback://postback?_data=x"));
        assert!(is_postback_url("LinePostback://postback"));
        assert!(!is_postback_url("line://postback?data=x"));
        assert!(!is_postback_url("https://example.com"));
    }
}
