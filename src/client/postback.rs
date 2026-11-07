//! `linepostback://` URLs, which `sendPostback` carries to a bot.
//!
//! They are built from flex, template and rich-menu actions and sent whole
//! as `SendPostbackRequest.url`. Query values are percent-encoded as a URI
//! query parameter builder does, and optional ones are left out
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

/// A `message` action from flex and template messages: `linepostback://postback?_text=…` through
/// `sendPostback`.
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

    /// The query key the picked value goes under: `_date`, `_time` or
    /// `_datetime` for `DATE`, `TIME` and `DATETIME`.
    pub fn value_key(self) -> &'static str {
        match self {
            DateTimeMode::Date => "_date",
            DateTimeMode::Time => "_time",
            DateTimeMode::DateTime => "_datetime",
        }
    }

    /// `value` in this mode's format, normalised as the app's
    /// `SimpleDateFormat` reads it (upper-cased, so `t` reads as `T`);
    /// `None` when it isn't one.
    pub fn read(self, value: &str) -> Option<String> {
        let v = value.trim().to_ascii_uppercase();
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        let date = |s: &str| {
            let p: Vec<&str> = s.split('-').collect();
            p.len() == 3
                && p[0].len() == 4
                && p[1].len() == 2
                && p[2].len() == 2
                && p.iter().all(|x| digits(x))
                && (1..=12).contains(&p[1].parse::<u32>().unwrap_or(0))
                && (1..=31).contains(&p[2].parse::<u32>().unwrap_or(0))
        };
        let time = |s: &str| {
            let p: Vec<&str> = s.split(':').collect();
            p.len() == 2
                && p.iter().all(|x| x.len() == 2 && digits(x))
                && p[0].parse::<u32>().unwrap_or(99) < 24
                && p[1].parse::<u32>().unwrap_or(99) < 60
        };
        let ok = match self {
            DateTimeMode::Date => date(&v),
            DateTimeMode::Time => time(&v),
            DateTimeMode::DateTime => v.split_once('T').is_some_and(|(d, t)| date(d) && time(t)),
        };
        ok.then_some(v)
    }
}

/// A flex or template `datetimepicker` action. Values are in the mode's format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateTimePicker {
    pub data: String,
    pub mode: DateTimeMode,
    pub initial: Option<String>,
    pub max: Option<String>,
    pub min: Option<String>,
}

impl DateTimePicker {
    /// The action as the app turns it into a URL:
    /// `linepostback://postback/datetimepicker?_data=…&_mode=…[&_initial=…]
    /// [&_max=…][&_min=…]`, empty values left out.
    pub fn action_url(&self) -> String {
        let mut q = Query::default();
        q.push("_data", &self.data);
        q.push("_mode", self.mode.as_str());
        q.push_opt("_initial", self.initial.as_deref());
        q.push_opt("_max", self.max.as_deref());
        q.push_opt("_min", self.min.as_deref());
        format!("{SCHEME}://postback/datetimepicker?{}", q.0)
    }

    /// `initial`, `min` and `max` as the picker reads them: a value that
    /// isn't in the mode's format counts as absent.
    fn bounds(&self) -> [Option<String>; 3] {
        let read = |v: &Option<String>| v.as_deref().and_then(|v| self.mode.read(v));
        [read(&self.initial), read(&self.min), read(&self.max)]
    }

    /// Whether to show a picker for this action:
    /// `data` isn't empty and `min <= initial <= max` for the bounds given.
    /// For any other, it sends [`action_url`](Self::action_url) to
    /// `sendPostback` as it is, with no picker.
    pub fn shows_picker(&self) -> bool {
        let [initial, min, max] = self.bounds();
        let le = |a: &Option<String>, b: &Option<String>| match (a, b) {
            (Some(a), Some(b)) => a <= b,
            _ => true,
        };
        !self.data.is_empty() && le(&min, &initial) && le(&initial, &max) && le(&min, &max)
    }

    /// What the picker starts on: `initial`, or `now` when there is none,
    /// moved into `min`…`max`. `now` is
    /// in the mode's format.
    pub fn starting_value(&self, now: &str) -> String {
        let [initial, min, max] = self.bounds();
        let mut v = initial.unwrap_or_else(|| now.to_string());
        if let Some(min) = min.filter(|m| *m > v) {
            v = min;
        }
        if let Some(max) = max.filter(|m| *m < v) {
            v = max;
        }
        v
    }

    /// The URL sent when the user picks `value`.
    pub fn result_url(&self, value: &str) -> String {
        datetime_picker_url(&self.data, self.mode, value)
    }
}

/// The URL sent when the user picks a value in a datetime picker action:
/// `linepostback://postback/datetimepicker?_data=…&_mode=…&_<mode>=<value>`
/// (for the value's key see [`DateTimeMode::value_key`]). `value` is in the mode's format.
pub fn datetime_picker_url(data: &str, mode: DateTimeMode, value: &str) -> String {
    let mut q = Query::default();
    q.push("_data", data);
    q.push("_mode", mode.as_str());
    q.push(mode.value_key(), value);
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

/// Percent-encoding that keeps letters, digits and `_-!.~'()*`.
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
            "linepostback://postback/datetimepicker?_data=d&_mode=datetime&_datetime=2026-11-06T09%3A30"
        );
        assert_eq!(
            datetime_picker_url("d", DateTimeMode::Date, "2026-11-06"),
            "linepostback://postback/datetimepicker?_data=d&_mode=date&_date=2026-11-06"
        );
        assert_eq!(DateTimeMode::parse("time"), Some(DateTimeMode::Time));
    }

    fn picker(mode: DateTimeMode, initial: &str, min: &str, max: &str) -> DateTimePicker {
        let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
        DateTimePicker {
            data: "d".into(),
            mode,
            initial: opt(initial),
            min: opt(min),
            max: opt(max),
        }
    }

    #[test]
    fn picker_action_url_leaves_out_missing_bounds() {
        assert_eq!(
            picker(DateTimeMode::Time, "10:00", "", "23:00").action_url(),
            "linepostback://postback/datetimepicker?_data=d&_mode=time&_initial=10%3A00&_max=23%3A00"
        );
    }

    #[test]
    fn picker_shows_only_for_ordered_bounds() {
        use DateTimeMode::*;
        assert!(picker(Date, "2026-01-02", "2026-01-01", "2026-01-03").shows_picker());
        assert!(picker(Date, "", "", "").shows_picker());
        assert!(!picker(Date, "2026-01-05", "", "2026-01-03").shows_picker());
        assert!(!picker(Time, "", "12:00", "11:00").shows_picker());
        // A bound in the wrong format counts as absent.
        assert!(picker(Date, "2026-01-05", "", "tomorrow").shows_picker());
        assert!(picker(DateTime, "2026-01-01t10:00", "2026-01-01T09:00", "").shows_picker());
        let mut empty = picker(Date, "", "", "");
        empty.data.clear();
        assert!(!empty.shows_picker());
    }

    #[test]
    fn picker_starts_inside_its_bounds() {
        use DateTimeMode::*;
        assert_eq!(picker(Time, "", "", "").starting_value("09:30"), "09:30");
        assert_eq!(
            picker(Time, "", "10:00", "").starting_value("09:30"),
            "10:00"
        );
        assert_eq!(
            picker(Time, "", "", "08:00").starting_value("09:30"),
            "08:00"
        );
        assert_eq!(
            picker(Date, "2026-03-01", "", "").starting_value("2026-11-06"),
            "2026-03-01"
        );
    }

    #[test]
    fn reads_values_in_the_modes_format() {
        use DateTimeMode::*;
        assert_eq!(
            DateTime.read("2026-11-06t09:30").as_deref(),
            Some("2026-11-06T09:30")
        );
        assert_eq!(Date.read("2026-13-01"), None);
        assert_eq!(Time.read("24:00"), None);
        assert_eq!(Time.read("9:30"), None);
    }

    #[test]
    fn recognises_postback_urls() {
        assert!(is_postback_url("linepostback://postback?_data=x"));
        assert!(is_postback_url("LinePostback://postback"));
        assert!(!is_postback_url("line://postback?data=x"));
        assert!(!is_postback_url("https://example.com"));
    }
}
