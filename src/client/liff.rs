//! LIFF helpers (linejs `features/liff.ts` on top of
//! `base/service/liff`): issue LIFF views/tokens and share messages through
//! `api.line.me/message/v3/share`.

use serde::Serialize;

use super::Client;
use crate::error::{Error, Result};
use crate::request::RequestOptions;
use crate::thrift::{Protocol, TValue, Value};

/// linejs' default LIFF app id.
pub const DEFAULT_LIFF_ID: &str = "2006747340-AoraPvdD";
const SHARE_URL: &str = "https://api.line.me/message/v3/share";
const CONSENT_API_URL: &str = "https://access.line.me/dialog/api/permissions";
const AUTH_CONSENT_URL: &str = "https://access.line.me/oauth2/v2.1/authorize/consent";
const ERROR_NAME: &str = "LiffServiceError";

/// "Sent by" override of a text message.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SentBy {
    pub label: String,
    pub icon_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_url: Option<String>,
}

/// A message for [`Liff::share_messages`] (Messaging API shape).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LiffMessage {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(rename = "sentBy", skip_serializing_if = "Option::is_none")]
        sent_by: Option<SentBy>,
    },
    #[serde(rename = "sticker", rename_all = "camelCase")]
    Sticker { package_id: String, sticker_id: String },
    #[serde(rename = "image", rename_all = "camelCase")]
    Image { original_content_url: String, preview_image_url: String },
    #[serde(rename = "flex", rename_all = "camelCase")]
    Flex { alt_text: String, contents: serde_json::Value },
    #[serde(rename = "template", rename_all = "camelCase")]
    Template { alt_text: String, template: serde_json::Value },
    /// Any other message object, sent verbatim.
    #[serde(untagged)]
    Raw(serde_json::Value),
}

pub fn text(body: impl Into<String>, sent_by: Option<SentBy>) -> LiffMessage {
    LiffMessage::Text { text: body.into(), sent_by }
}

pub fn sticker(package_id: impl Into<String>, sticker_id: impl Into<String>) -> LiffMessage {
    LiffMessage::Sticker { package_id: package_id.into(), sticker_id: sticker_id.into() }
}

/// An image; `preview` defaults to the original URL.
pub fn image(original_content_url: impl Into<String>, preview_image_url: Option<String>) -> LiffMessage {
    let original = original_content_url.into();
    LiffMessage::Image { preview_image_url: preview_image_url.unwrap_or_else(|| original.clone()), original_content_url: original }
}

pub fn flex(alt_text: impl Into<String>, contents: serde_json::Value) -> LiffMessage {
    LiffMessage::Flex { alt_text: alt_text.into(), contents }
}

/// Options for [`Liff::issue_view`] / [`Liff::get_token`].
#[derive(Debug, Clone, Default)]
pub struct LiffViewOptions {
    pub chat_mid: Option<String>,
    /// Defaults to the client's default LIFF id.
    pub liff_id: Option<String>,
    /// Defaults to `ja_JP`.
    pub lang: Option<String>,
}

/// `issueLiffView` args, encoded by hand like linejs: the "no context"
/// variant is an empty struct in field 1, which the schema encoder would drop.
pub(crate) fn issue_liff_view_args(liff_id: &str, chat_mid: Option<&str>, lang: &str) -> Vec<(i16, TValue)> {
    let context = match chat_mid {
        Some(mid) => {
            let id = if mid.starts_with(['u', 'c', 'r']) { 2 } else { 3 };
            (id, TValue::Struct(vec![(1, TValue::string(mid))]))
        }
        None => (1, TValue::Struct(Vec::new())),
    };
    vec![(
        1,
        TValue::Struct(vec![
            (1, TValue::string(liff_id)),
            (2, TValue::Struct(vec![context])),
            (3, TValue::string(lang)),
        ]),
    )]
}

/// Extract the CSRF token from the consent page.
pub(crate) fn find_csrf(html: &str) -> Option<&str> {
    const MARK: &str = r#"<input type="hidden" name="__csrf" id="__csrf" value=""#;
    let start = html.find(MARK)? + MARK.len();
    let len = html[start..].find('"')?;
    Some(&html[start..start + len])
}

/// LIFF operations bound to a [`Client`] (see [`Client::liff`]).
#[derive(Clone, Copy)]
pub struct Liff<'a> {
    client: &'a Client,
}

impl Client {
    /// LIFF helpers.
    pub fn liff(&self) -> Liff<'_> {
        Liff { client: self }
    }
}

impl Liff<'_> {
    pub fn default_liff_id(&self) -> String {
        self.client.inner.liff_id.lock().unwrap().clone()
    }

    pub fn set_default_liff_id(&self, liff_id: impl Into<String>) {
        *self.client.inner.liff_id.lock().unwrap() = liff_id.into();
    }

    /// `issueLiffView`; returns the `LiffViewResponse`.
    pub async fn issue_view(&self, opts: &LiffViewOptions) -> Result<Value> {
        let liff_id = opts.liff_id.clone().unwrap_or_else(|| self.default_liff_id());
        let args = issue_liff_view_args(&liff_id, opts.chat_mid.as_deref(), opts.lang.as_deref().unwrap_or("ja_JP"));
        self.client.base.request(args, "issueLiffView", RequestOptions::new("/LIFF1", Protocol::Compact)).await
    }

    /// `issueSubLiffView` with raw `issueSubLiffView_args` fields.
    pub async fn issue_sub_view(&self, args: Value) -> Result<Value> {
        self.client.base.liff().call("issueSubLiffView", args).await
    }

    /// A LIFF access token, granting consent once if the server asks for it.
    pub async fn get_token(&self, opts: &LiffViewOptions) -> Result<String> {
        let liff_id = opts.liff_id.clone().unwrap_or_else(|| self.default_liff_id());
        let mut try_consent = true;
        loop {
            let view = LiffViewOptions { liff_id: Some(liff_id.clone()), ..opts.clone() };
            let err = match self.issue_view(&view).await {
                Ok(v) => {
                    if let Some(t) = v["accessToken"].as_str() {
                        return Ok(t.to_string());
                    }
                    None
                }
                Err(e) => Some(e),
            };
            if let Some(Error::Request { data, .. }) = &err {
                self.client.base.log("liff-error", data.to_json());
                let exc = if data.get("liffException").is_some() { &data["liffException"] } else { data };
                let consent = &exc["payload"]["consentRequired"];
                if try_consent
                    && exc["code"].as_str() == Some("CONSENT_REQUIRED")
                    && let (Some(channel_id), Some(consent_url)) =
                        (consent["channelId"].as_str(), consent["consentUrl"].as_str())
                {
                    let to_type = opts.chat_mid.as_deref().and_then(crate::base::BaseClient::get_to_type);
                    let desktop = self.client.base.device.as_str().starts_with("DESKTOP");
                    let ok = if to_type == Some(4) || desktop {
                        self.try_consent_authorize(consent_url, channel_id).await?
                    } else {
                        self.try_consent_liff(channel_id).await?
                    };
                    if ok {
                        try_consent = false;
                        continue;
                    }
                }
            }
            let at = opts.chat_mid.as_deref().map(|m| format!("@{m}")).unwrap_or_default();
            return Err(Error::internal(ERROR_NAME, format!("Failed to get LiffToken: {liff_id}{at}")));
        }
    }

    async fn try_consent_liff(&self, channel_id: &str) -> Result<bool> {
        let base = &self.client.base;
        let res = base
            .http
            .post(CONSENT_API_URL)
            .header("X-LINE-ChannelId", channel_id)
            .header("X-LINE-Access", base.auth_token().unwrap_or_default())
            .header(
                "User-Agent",
                "Mozilla/5.0 (Linux; Android 8.0.1; SAMSUNG Realise/DeachSword; wv) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Version/4.0 Chrome/56.0.2924.87 Mobile Safari/537.36",
            )
            .header("Content-Type", "application/json")
            .header("X-Line-Application", base.device_details.application())
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Accept-Language", "ja-JP,en-US;q=0.8")
            .body(r#"{"on":["P","CM"],"off":[]}"#)
            .send()
            .await?;
        Ok(res.status().is_success())
    }

    async fn try_consent_authorize(&self, consent_url: &str, channel_id: &str) -> Result<bool> {
        let base = &self.client.base;
        let headers = [
            ("X-Line-Access", base.auth_token().unwrap_or_default()),
            ("User-Agent", base.device_details.user_agent()),
            ("X-Line-Application", base.device_details.application()),
        ];
        let mut req = base.http.get(consent_url);
        for (k, v) in &headers {
            req = req.header(*k, v);
        }
        let res = req.send().await?;
        if !res.status().is_success() {
            return Ok(false);
        }
        let cookies: Vec<String> = res
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or("").to_string())
            .collect();
        let html = res.text().await?;
        let Some(csrf) = find_csrf(&html) else { return Ok(false) };
        let perms = ["P", "CM", "OC"];
        let all: Vec<String> = perms.iter().map(|p| format!("allPermission={p}")).collect();
        let approved: Vec<String> = perms.iter().map(|p| format!("approvedPermission={p}")).collect();
        let body = format!(
            "{}&{}&__WLS=&channelId={channel_id}&__csrf={csrf}&allow=true",
            all.join("&"),
            approved.join("&")
        );
        let mut req = base.http.post(AUTH_CONSENT_URL).header("cookie", cookies.join("; ")).header("referer", consent_url);
        for (k, v) in &headers {
            req = req.header(*k, v);
        }
        Ok(req.body(body).send().await?.status().is_success())
    }

    /// Share messages into chat `to` via the LIFF share API (tokens are cached
    /// per chat; `force_issue` mints a new one). linejs issues a token for
    /// every share, so a cached token that the API rejects (4xx) is dropped
    /// and the share retried once with a fresh one.
    pub async fn share_messages(&self, to: &str, messages: &[LiffMessage], force_issue: bool) -> Result<serde_json::Value> {
        let mut force_issue = force_issue;
        loop {
            let cached = self.client.inner.liff_tokens.lock().unwrap().get(to).cloned();
            let (token, from_cache) = match cached.filter(|_| !force_issue) {
                Some(t) => (t, true),
                None => {
                    // linejs' sendLiff always uses the service default id here.
                    let opts = LiffViewOptions { chat_mid: Some(to.to_string()), ..Default::default() };
                    let t = self.get_token(&opts).await?;
                    self.client.inner.liff_tokens.lock().unwrap().insert(to.to_string(), t.clone());
                    (t, false)
                }
            };
            let res = self
                .client
                .base
                .http
                .post(SHARE_URL)
                .header("Accept", "application/json, text/plain, */*")
                .header(
                    "User-Agent",
                    "Mozilla/5.0 (Linux; Android 4.4.2; G730-U00 Build/JLS36C) AppleWebKit/537.36 (KHTML, like Gecko) \
                     Version/4.0 Chrome/30.0.0.0 Mobile Safari/537.36 Line/9.8.0",
                )
                .header("Accept-Language", "zh-TW,zh;q=0.9")
                .header("Authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(serde_json::to_string(&serde_json::json!({ "messages": messages }))?)
                .send()
                .await?;
            let status = res.status();
            let body: serde_json::Value = res.json().await.unwrap_or(serde_json::Value::Null);
            if from_cache && status.is_client_error() {
                self.client.inner.liff_tokens.lock().unwrap().remove(to);
                force_issue = true;
                continue;
            }
            if !status.is_success() {
                return Err(Error::internal(ERROR_NAME, format!("Failed to send Liff message: {status} {body}")));
            }
            return Ok(body);
        }
    }

    pub async fn share_message(&self, to: &str, message: LiffMessage, force_issue: bool) -> Result<serde_json::Value> {
        self.share_messages(to, &[message], force_issue).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn j(m: &LiffMessage) -> serde_json::Value {
        serde_json::to_value(m).unwrap()
    }

    #[test]
    fn text_plain_and_sent_by() {
        assert_eq!(j(&text("hi", None)), json!({"type": "text", "text": "hi"}));
        let sent_by = SentBy {
            label: "Bot".into(),
            icon_url: "https://example.test/icon.png".into(),
            link_url: Some("https://example.test".into()),
        };
        assert_eq!(
            j(&text("hi", Some(sent_by))),
            json!({"type": "text", "text": "hi", "sentBy": {
                "label": "Bot", "iconUrl": "https://example.test/icon.png", "linkUrl": "https://example.test"}})
        );
    }

    #[test]
    fn sticker_image_flex() {
        assert_eq!(
            j(&sticker("11537", "52002734")),
            json!({"type": "sticker", "packageId": "11537", "stickerId": "52002734"})
        );
        assert_eq!(
            j(&image("https://example.test/a.jpg", None)),
            json!({"type": "image", "originalContentUrl": "https://example.test/a.jpg",
                   "previewImageUrl": "https://example.test/a.jpg"})
        );
        let m = image("https://example.test/orig.jpg", Some("https://example.test/thumb.jpg".into()));
        assert_eq!(j(&m)["previewImageUrl"], "https://example.test/thumb.jpg");
        let contents = json!({"type": "bubble", "body": {"type": "box", "layout": "vertical"}});
        assert_eq!(j(&flex("alt", contents.clone())), json!({"type": "flex", "altText": "alt", "contents": contents}));
        assert_eq!(j(&LiffMessage::Raw(json!({"type": "x"}))), json!({"type": "x"}));
    }

    #[test]
    fn view_args_and_csrf() {
        let args = issue_liff_view_args("id", Some("c123"), "ja_JP");
        let TValue::Struct(req) = &args[0].1 else { panic!() };
        assert_eq!(req[1].1, TValue::Struct(vec![(2, TValue::Struct(vec![(1, TValue::string("c123"))]))]));
        let args = issue_liff_view_args("id", Some("s123"), "ja_JP");
        let TValue::Struct(req) = &args[0].1 else { panic!() };
        assert!(matches!(&req[1].1, TValue::Struct(c) if c[0].0 == 3));
        let args = issue_liff_view_args("id", None, "ja_JP");
        let TValue::Struct(req) = &args[0].1 else { panic!() };
        assert_eq!(req[1].1, TValue::Struct(vec![(1, TValue::Struct(vec![]))]));
        assert_eq!(find_csrf(r#"x<input type="hidden" name="__csrf" id="__csrf" value="tok">y"#), Some("tok"));
        assert_eq!(find_csrf("nothing"), None);
    }
}
