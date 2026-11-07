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
    Sticker {
        package_id: String,
        sticker_id: String,
    },
    #[serde(rename = "image", rename_all = "camelCase")]
    Image {
        original_content_url: String,
        preview_image_url: String,
    },
    #[serde(rename = "flex", rename_all = "camelCase")]
    Flex {
        alt_text: String,
        contents: serde_json::Value,
    },
    #[serde(rename = "template", rename_all = "camelCase")]
    Template {
        alt_text: String,
        template: serde_json::Value,
    },
    /// Any other message object, sent verbatim.
    #[serde(untagged)]
    Raw(serde_json::Value),
}

pub fn text(body: impl Into<String>, sent_by: Option<SentBy>) -> LiffMessage {
    LiffMessage::Text {
        text: body.into(),
        sent_by,
    }
}

pub fn sticker(package_id: impl Into<String>, sticker_id: impl Into<String>) -> LiffMessage {
    LiffMessage::Sticker {
        package_id: package_id.into(),
        sticker_id: sticker_id.into(),
    }
}

/// An image; `preview` defaults to the original URL.
pub fn image(
    original_content_url: impl Into<String>,
    preview_image_url: Option<String>,
) -> LiffMessage {
    let original = original_content_url.into();
    LiffMessage::Image {
        preview_image_url: preview_image_url.unwrap_or_else(|| original.clone()),
        original_content_url: original,
    }
}

pub fn flex(alt_text: impl Into<String>, contents: serde_json::Value) -> LiffMessage {
    LiffMessage::Flex {
        alt_text: alt_text.into(),
        contents,
    }
}

/// Options for [`Liff::issue_view`] / [`Liff::get_token`].
#[derive(Debug, Clone, Default)]
pub struct LiffViewOptions {
    pub chat_mid: Option<String>,
    /// Defaults to the client's default LIFF id.
    pub liff_id: Option<String>,
    /// Defaults to the client's [`Config::language`](crate::base::Config::language).
    pub lang: Option<String>,
    /// `deviceSetting`: what the device allows, sent with every view.
    pub device_setting: Option<LiffDeviceSetting>,
    /// `domain`: see [`LiffViewOptions::for_url`].
    pub domain: Option<String>,
    /// `requestedUrl`: see [`LiffViewOptions::for_url`].
    pub requested_url: Option<String>,
    /// `subsequentLiff`: the view replaces another LIFF app in the same
    /// window. The app sets it when one LIFF app opens another in place
    /// and sends `false` otherwise.
    pub subsequent_liff: bool,
    /// `msit` and `interWindowSession`, which the app passes on when it
    /// reissues a view it opened before; left out otherwise.
    pub msit: Option<String>,
    pub inter_window_session: Option<String>,
}

/// `LiffDeviceSetting`. `advertisingId` (field 2) is left
/// out, as the app leaves it out when the device has no advertising id.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LiffDeviceSetting {
    /// Videos may play on their own (the app's autoplay setting and network).
    pub video_auto_play_allowed: bool,
    /// In-app purchases can be made.
    pub in_app_purchase_available: bool,
}

impl LiffViewOptions {
    /// `domain` and `requestedUrl` for opening the LIFF URL `url`, as
    /// LINE expects them: the domain is the `liff.domain` query value of
    /// a `line://` or `lineb://` URL and the host of any other, and the
    /// requested URL is `url` without its `liff.domain` value.
    pub fn for_url(mut self, url: &str) -> Self {
        let Ok(parsed) = url::Url::parse(url) else {
            return self;
        };
        let liff_domain = parsed
            .query_pairs()
            .find(|(k, _)| k == "liff.domain")
            .map(|(_, v)| v.into_owned());
        self.domain = if matches!(parsed.scheme(), "line" | "lineb") {
            liff_domain.clone()
        } else {
            parsed.host_str().map(str::to_string)
        };
        self.requested_url = Some(if liff_domain.is_some() {
            let kept: Vec<(String, String)> = parsed
                .query_pairs()
                .filter(|(k, _)| k != "liff.domain")
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            let mut out = parsed.clone();
            out.set_query(None);
            if !kept.is_empty() {
                out.query_pairs_mut().extend_pairs(kept);
            }
            out.to_string()
        } else {
            url.to_string()
        });
        self
    }

    /// `LiffViewRequest` fields 4 to 9, in the order the app writes them.
    fn extra_fields(&self) -> Vec<(i16, TValue)> {
        let mut out = Vec::new();
        if let Some(d) = &self.device_setting {
            out.push((
                4,
                TValue::Struct(vec![
                    (1, TValue::Bool(d.video_auto_play_allowed)),
                    (3, TValue::Bool(d.in_app_purchase_available)),
                ]),
            ));
        }
        if let Some(m) = &self.msit {
            out.push((5, TValue::string(m)));
        }
        out.push((6, TValue::Bool(self.subsequent_liff)));
        if let Some(d) = &self.domain {
            out.push((7, TValue::string(d)));
        }
        if let Some(s) = &self.inter_window_session {
            out.push((8, TValue::string(s)));
        }
        if let Some(u) = &self.requested_url {
            out.push((9, TValue::string(u)));
        }
        out
    }
}

/// `issueLiffView` args, encoded by hand like linejs: the "no context"
/// variant is an empty struct in field 1, which the schema encoder would drop.
pub(crate) fn issue_liff_view_args(
    liff_id: &str,
    chat_mid: Option<&str>,
    lang: &str,
) -> Vec<(i16, TValue)> {
    let context = match chat_mid {
        Some(mid) => {
            let id = if mid.starts_with(['u', 'c', 'r']) {
                2
            } else {
                3
            };
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

/// [`issue_liff_view_args`] with the request's other fields from `opts`.
fn view_request(liff_id: &str, lang: &str, opts: &LiffViewOptions) -> Vec<(i16, TValue)> {
    let mut args = issue_liff_view_args(liff_id, opts.chat_mid.as_deref(), lang);
    if let TValue::Struct(req) = &mut args[0].1 {
        req.extend(opts.extra_fields());
    }
    args
}

/// LINE's `CONSENT_REQUIRED` answer to `issueLiffView`: the LIFF app needs
/// the user's consent first, on LINE's consent page (`consentRequired`
/// payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiffConsent {
    pub channel_id: String,
    /// LINE's consent page for this app, to open for the user when it is a
    /// valid URL.
    pub consent_url: String,
}

/// The consent `issueLiffView` failed for, if that is why it failed.
pub fn consent_required(e: &Error) -> Option<LiffConsent> {
    let Error::Request { data, .. } = e else {
        return None;
    };
    let exc = if data.get("liffException").is_some() {
        &data["liffException"]
    } else {
        data
    };
    let code = &exc["code"];
    if code.as_str() != Some("CONSENT_REQUIRED") && code.as_i64() != Some(3) {
        return None;
    }
    let consent = &exc["payload"]["consentRequired"];
    Some(LiffConsent {
        channel_id: consent["channelId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        consent_url: consent["consentUrl"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    })
}

/// One permission the consent page asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsentPermission {
    /// LINE's code, e.g. `P` (profile), `OC` (send messages to chats).
    pub code: String,
    /// The page's own wording, in the account's language.
    pub label: String,
}

/// LINE's consent page as fetched for the logged-in user: what it asks for,
/// and the form that its "Allow" button posts.
#[derive(Debug, Clone)]
pub struct ConsentPage {
    pub consent: LiffConsent,
    /// Permissions the user hasn't granted yet.
    pub permissions: Vec<ConsentPermission>,
    html: String,
    cookies: String,
}

/// How a share-target picker ended, for `storeShareTargetPickerResult`
/// (`ShareTargetPickerResultRequest.resultCode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerResult {
    Success,
    Failure,
    Cancel,
}

/// `storeShareTargetPickerResult` args:
/// `ShareTargetPickerResultRequest { ott = 1, liffId = 2, resultCode = 3,
/// resultDescription = 4 }`.
pub(crate) fn picker_result_args(
    ott: &str,
    liff_id: &str,
    result: PickerResult,
    description: Option<&str>,
) -> Vec<(i16, TValue)> {
    let code = match result {
        PickerResult::Success => 0,
        PickerResult::Failure => 1,
        PickerResult::Cancel => 2,
    };
    let mut req = vec![
        (1, TValue::string(ott)),
        (2, TValue::string(liff_id)),
        (3, TValue::I32(code)),
    ];
    if let Some(d) = description {
        req.push((4, TValue::string(d)));
    }
    vec![(1, TValue::Struct(req))]
}

/// The permissions a consent page asks for, from its `app-data` JSON.
fn consent_permissions(html: &str) -> Vec<ConsentPermission> {
    crate::service::liff::consent_app_data(html)
        .map(|data| {
            data["props"]["authz-allow-newly"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    Some(ConsentPermission {
                        code: p["code"].as_str()?.to_string(),
                        label: p["label"].as_str().unwrap_or_default().to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
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
        let liff_id = opts
            .liff_id
            .clone()
            .unwrap_or_else(|| self.default_liff_id());
        let args = view_request(
            &liff_id,
            opts.lang
                .as_deref()
                .unwrap_or(&self.client.base.config.language),
            opts,
        );
        self.client
            .base
            .request(
                args,
                "issueLiffView",
                RequestOptions::new("/LIFF1", Protocol::Compact),
            )
            .await
    }

    /// `issueSubLiffView` with raw `issueSubLiffView_args` fields.
    pub async fn issue_sub_view(&self, args: Value) -> Result<Value> {
        self.client.base.liff().call("issueSubLiffView", args).await
    }

    /// A LIFF access token, granting consent once if the server asks for it.
    pub async fn get_token(&self, opts: &LiffViewOptions) -> Result<String> {
        let liff_id = opts
            .liff_id
            .clone()
            .unwrap_or_else(|| self.default_liff_id());
        let mut try_consent = true;
        loop {
            let view = LiffViewOptions {
                liff_id: Some(liff_id.clone()),
                ..opts.clone()
            };
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
                let exc = if data.get("liffException").is_some() {
                    &data["liffException"]
                } else {
                    data
                };
                let consent = &exc["payload"]["consentRequired"];
                if try_consent
                    && exc["code"].as_str() == Some("CONSENT_REQUIRED")
                    && let (Some(channel_id), Some(consent_url)) = (
                        consent["channelId"].as_str(),
                        consent["consentUrl"].as_str(),
                    )
                {
                    // The consent page works for every device; the mobile
                    // permissions API answers 200 without granting permissions
                    // it does not list (such as `OC`), so it is the fallback.
                    let to_type = opts
                        .chat_mid
                        .as_deref()
                        .and_then(crate::base::BaseClient::get_to_type);
                    let mobile = to_type != Some(4)
                        && !self.client.base.device.as_str().starts_with("DESKTOP");
                    let ok = self.try_consent_authorize(consent_url, channel_id).await?
                        || (mobile && self.try_consent_liff(channel_id).await?);
                    if ok {
                        try_consent = false;
                        continue;
                    }
                }
            }
            let at = opts
                .chat_mid
                .as_deref()
                .map(|m| format!("@{m}"))
                .unwrap_or_default();
            return Err(Error::internal(
                ERROR_NAME,
                format!("Failed to get LiffToken: {liff_id}{at}"),
            ));
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
        let Some((post_url, body)) = crate::service::liff::consent_form(&html, channel_id) else {
            return Ok(false);
        };
        let mut req = base
            .http
            .post(post_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("cookie", cookies.join("; "))
            .header("referer", consent_url);
        for (k, v) in &headers {
            req = req.header(*k, v);
        }
        Ok(req.body(body).send().await?.status().is_success())
    }

    /// Fetch LINE's consent page for `consent` as the logged-in user, to
    /// show the user what the app asks for before [`Liff::approve`].
    pub async fn consent_page(&self, consent: &LiffConsent) -> Result<ConsentPage> {
        let base = &self.client.base;
        if !consent.consent_url.starts_with("https://") {
            return Err(Error::internal(ERROR_NAME, "consent URL is not https"));
        }
        let mut req = base.http.get(&consent.consent_url);
        for (k, v) in self.consent_headers() {
            req = req.header(k, v);
        }
        let res = req.send().await?;
        if !res.status().is_success() {
            return Err(Error::internal(
                ERROR_NAME,
                format!("consent page answered {}", res.status()),
            ));
        }
        let cookies: Vec<String> = res
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or("").to_string())
            .collect();
        let html = res.text().await?;
        Ok(ConsentPage {
            consent: consent.clone(),
            permissions: consent_permissions(&html),
            html,
            cookies: cookies.join("; "),
        })
    }

    /// Post the consent page's "Allow", as the user tapping it there. Only
    /// call this once the user has agreed to `page.permissions`.
    pub async fn approve(&self, page: &ConsentPage) -> Result<()> {
        let Some((post_url, body)) =
            crate::service::liff::consent_form(&page.html, &page.consent.channel_id)
        else {
            return Err(Error::internal(ERROR_NAME, "consent page has no form"));
        };
        let mut req = self
            .client
            .base
            .http
            .post(post_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("cookie", &page.cookies)
            .header("referer", &page.consent.consent_url);
        for (k, v) in self.consent_headers() {
            req = req.header(k, v);
        }
        let status = req.body(body).send().await?.status();
        if !status.is_success() {
            return Err(Error::internal(
                ERROR_NAME,
                format!("consent answered {status}"),
            ));
        }
        Ok(())
    }

    fn consent_headers(&self) -> [(&'static str, String); 3] {
        let base = &self.client.base;
        [
            ("X-Line-Access", base.auth_token().unwrap_or_default()),
            ("User-Agent", base.device_details.user_agent()),
            ("X-Line-Application", base.device_details.application()),
        ]
    }

    /// `storeShareTargetPickerResult`: tell LINE how the share-target picker
    /// opened with `ott` ended, which the LIFF SDK waits for (the app's
    /// `LiffRemoteDataSourceImpl.storeShareTargetPickerResult`).
    pub async fn store_share_target_picker_result(
        &self,
        liff_id: &str,
        ott: &str,
        result: PickerResult,
        description: Option<&str>,
    ) -> Result<()> {
        self.client
            .base
            .request(
                picker_result_args(ott, liff_id, result, description),
                "storeShareTargetPickerResult",
                RequestOptions::new("/LIFF1", Protocol::Compact),
            )
            .await?;
        Ok(())
    }

    /// Share messages into chat `to` via the LIFF share API (tokens are cached
    /// per chat; `force_issue` mints a new one). linejs issues a token for
    /// every share, so a cached token that the API rejects (4xx) is dropped
    /// and the share retried once with a fresh one.
    pub async fn share_messages(
        &self,
        to: &str,
        messages: &[LiffMessage],
        force_issue: bool,
    ) -> Result<serde_json::Value> {
        let mut force_issue = force_issue;
        loop {
            let cached = self
                .client
                .inner
                .liff_tokens
                .lock()
                .unwrap()
                .get(to)
                .cloned();
            let (token, from_cache) = match cached.filter(|_| !force_issue) {
                Some(t) => (t, true),
                None => {
                    // linejs' sendLiff always uses the service default id here.
                    let opts = LiffViewOptions {
                        chat_mid: Some(to.to_string()),
                        ..Default::default()
                    };
                    let t = self.get_token(&opts).await?;
                    self.client
                        .inner
                        .liff_tokens
                        .lock()
                        .unwrap()
                        .insert(to.to_string(), t.clone());
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
                return Err(Error::internal(
                    ERROR_NAME,
                    format!("Failed to send Liff message: {status} {body}"),
                ));
            }
            return Ok(body);
        }
    }

    pub async fn share_message(
        &self,
        to: &str,
        message: LiffMessage,
        force_issue: bool,
    ) -> Result<serde_json::Value> {
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
        let m = image(
            "https://example.test/orig.jpg",
            Some("https://example.test/thumb.jpg".into()),
        );
        assert_eq!(j(&m)["previewImageUrl"], "https://example.test/thumb.jpg");
        let contents = json!({"type": "bubble", "body": {"type": "box", "layout": "vertical"}});
        assert_eq!(
            j(&flex("alt", contents.clone())),
            json!({"type": "flex", "altText": "alt", "contents": contents})
        );
        assert_eq!(
            j(&LiffMessage::Raw(json!({"type": "x"}))),
            json!({"type": "x"})
        );
    }

    #[test]
    fn reads_a_consent_answer_and_page() {
        let e = Error::Request {
            method: "issueLiffView".into(),
            path: "/LIFF1".into(),
            data: crate::tv!({"liffException": {"code": "CONSENT_REQUIRED", "payload":
                {"consentRequired": {"channelId": "123", "consentUrl": "https://access.line.me/x"}}}}),
        };
        assert_eq!(
            consent_required(&e),
            Some(LiffConsent {
                channel_id: "123".into(),
                consent_url: "https://access.line.me/x".into()
            })
        );
        let other = Error::Request {
            method: "issueLiffView".into(),
            path: "/LIFF1".into(),
            data: crate::tv!({"code": "INVALID_REQUEST"}),
        };
        assert_eq!(consent_required(&other), None);

        let html = concat!(
            r#"<div app-data="{&quot;props&quot;:{&quot;authz-allow-newly&quot;:["#,
            r#"{&quot;code&quot;:&quot;P&quot;,&quot;label&quot;:&quot;Profile&quot;},"#,
            r#"{&quot;code&quot;:&quot;OC&quot;,&quot;label&quot;:&quot;Send &amp; share&quot;}]}}"></div>"#,
        );
        let p = consent_permissions(html);
        assert_eq!(p.len(), 2);
        assert_eq!(
            (p[1].code.as_str(), p[1].label.as_str()),
            ("OC", "Send & share")
        );
        assert!(consent_permissions("<html></html>").is_empty());
    }

    #[test]
    fn picker_result_request() {
        let args = picker_result_args("ott-1", "123-abc", PickerResult::Cancel, None);
        assert_eq!(
            args[0].1,
            TValue::Struct(vec![
                (1, TValue::string("ott-1")),
                (2, TValue::string("123-abc")),
                (3, TValue::I32(2)),
            ])
        );
    }

    #[test]
    fn view_args_and_csrf() {
        let args = issue_liff_view_args("id", Some("c123"), "ja_JP");
        let TValue::Struct(req) = &args[0].1 else {
            panic!()
        };
        assert_eq!(
            req[1].1,
            TValue::Struct(vec![(2, TValue::Struct(vec![(1, TValue::string("c123"))]))])
        );
        let args = issue_liff_view_args("id", Some("s123"), "ja_JP");
        let TValue::Struct(req) = &args[0].1 else {
            panic!()
        };
        assert!(matches!(&req[1].1, TValue::Struct(c) if c[0].0 == 3));
        let args = issue_liff_view_args("id", None, "ja_JP");
        let TValue::Struct(req) = &args[0].1 else {
            panic!()
        };
        assert_eq!(req[1].1, TValue::Struct(vec![(1, TValue::Struct(vec![]))]));
    }

    #[test]
    fn view_request_carries_device_setting_domain_and_url() {
        let opts = LiffViewOptions {
            device_setting: Some(LiffDeviceSetting {
                video_auto_play_allowed: true,
                in_app_purchase_available: false,
            }),
            ..Default::default()
        }
        .for_url("https://liff.line.me/123-abc/path?x=1");
        let args = view_request("123-abc", "th_TH", &opts);
        let TValue::Struct(req) = &args[0].1 else {
            panic!()
        };
        assert_eq!(
            &req[3..],
            &[
                (
                    4,
                    TValue::Struct(vec![(1, TValue::Bool(true)), (3, TValue::Bool(false))])
                ),
                (6, TValue::Bool(false)),
                (7, TValue::string("liff.line.me")),
                (9, TValue::string("https://liff.line.me/123-abc/path?x=1")),
            ]
        );
    }

    #[test]
    fn line_scheme_urls_take_the_domain_from_the_query() {
        let opts =
            LiffViewOptions::default().for_url("line://app/123-abc?liff.domain=example.com&a=b");
        assert_eq!(opts.domain.as_deref(), Some("example.com"));
        assert_eq!(
            opts.requested_url.as_deref(),
            Some("line://app/123-abc?a=b")
        );
        let plain = LiffViewOptions::default().for_url("line://app/123-abc");
        assert_eq!(plain.domain, None);
        assert_eq!(plain.requested_url.as_deref(), Some("line://app/123-abc"));
    }
}
