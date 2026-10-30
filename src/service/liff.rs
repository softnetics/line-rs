//! `LiffService` (`/LIFF1`): LIFF views/tokens and sharing messages through
//! the LIFF share API.

use super::LiffService;
use crate::base::BaseClient;
use crate::error::{Error, Result};
use crate::request::Parse;
use crate::thrift::{TValue, Value};

/// LIFF share endpoint used by [`LiffService::send_liff`].
pub const LINE_LIFF_ENDPOINT: &str = "https://api.line.me/message/v3/share";
/// Permission consent endpoint for mobile devices.
pub const CONSENT_API_URL: &str = "https://access.line.me/dialog/api/permissions";
/// OAuth consent endpoint for desktop devices / square chats.
pub const AUTH_CONSENT_URL: &str = "https://access.line.me/oauth2/v2.1/authorize/consent";
/// Default LIFF app used by [`LiffService::send_liff`].
pub const DEFAULT_LIFF_ID: &str = "2006747340-AoraPvdD";

const ERROR_NAME: &str = "LiffServiceError";

/// Options for [`LiffService::issue_liff_view`] and [`LiffService::get_liff_token`].
#[derive(Debug, Clone, Default)]
pub struct LiffViewOptions {
    pub liff_id: String,
    /// Chat (`u`/`c`/`r`) or square chat mid to open the view in.
    pub chat_mid: Option<String>,
    /// Default `ja_JP`.
    pub lang: Option<String>,
    /// Try to grant consent on `CONSENT_REQUIRED` (default `true`; token only).
    pub try_consent: Option<bool>,
}

/// Options for [`LiffService::send_liff`].
#[derive(Debug, Clone, Default)]
pub struct SendLiffOptions {
    pub to: String,
    /// LIFF share messages, e.g. `{"type": "text", "text": "hi"}`.
    pub messages: Vec<serde_json::Value>,
    /// Default `true`.
    pub try_consent: Option<bool>,
}

/// Raw `issueLiffView` args (linejs sends a hand-built `NestedArray`).
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

/// Extract `__csrf` from the consent page.
fn find_csrf(html: &str) -> Option<&str> {
    const PREFIX: &str = r#"<input type="hidden" name="__csrf" id="__csrf" value=""#;
    let start = html.find(PREFIX)? + PREFIX.len();
    let len = html[start..].find("\">")?;
    Some(&html[start..start + len])
}

impl LiffService<'_> {
    /// Issue a LIFF view (`LiffViewResponse`).
    pub async fn issue_liff_view(&self, options: &LiffViewOptions) -> Result<Value> {
        let args = issue_liff_view_args(
            &options.liff_id,
            options.chat_mid.as_deref(),
            options.lang.as_deref().unwrap_or("ja_JP"),
        );
        self.request("issueLiffView", args, Parse::Result).await
    }

    /// The LIFF access token for `liff_id` (in `chat_mid`), granting consent
    /// once if the server answers `CONSENT_REQUIRED`.
    pub async fn get_liff_token(&self, options: &LiffViewOptions) -> Result<String> {
        let mut try_consent = options.try_consent.unwrap_or(true);
        loop {
            let err = match self.issue_liff_view(options).await {
                Ok(view) => return Ok(view["accessToken"].as_str().unwrap_or_default().to_string()),
                Err(e) => e,
            };
            if let Error::Request { data, .. } = &err {
                self.client.log("liff-error", data.to_json());
                let exc = if data["liffException"].is_null() { data } else { &data["liffException"] };
                if try_consent && exc["code"].as_str() == Some("CONSENT_REQUIRED") {
                    let consent = &exc["payload"]["consentRequired"];
                    if let (Some(channel_id), Some(consent_url)) =
                        (consent["channelId"].as_str(), consent["consentUrl"].as_str())
                    {
                        let to_type = options.chat_mid.as_deref().and_then(BaseClient::get_to_type);
                        let granted = if to_type == Some(4) || self.client.device.as_str().starts_with("DESKTOP") {
                            self.try_consent_authorize(consent_url, channel_id).await?
                        } else {
                            self.try_consent_liff(channel_id, None).await?
                        };
                        if granted {
                            try_consent = false;
                            continue;
                        }
                    }
                }
            }
            let at = options.chat_mid.as_deref().map(|m| format!("@{m}")).unwrap_or_default();
            return Err(Error::internal(ERROR_NAME, format!("Failed to get LiffToken: {}{at}", options.liff_id)));
        }
    }

    /// Share `messages` to `to` through the LIFF share API (a fresh token is
    /// issued for every call).
    pub async fn send_liff(&self, options: SendLiffOptions) -> Result<Value> {
        let token = self
            .get_liff_token(&LiffViewOptions {
                liff_id: DEFAULT_LIFF_ID.into(),
                chat_mid: Some(options.to.clone()),
                lang: None,
                try_consent: Some(options.try_consent.unwrap_or(true)),
            })
            .await?;
        let res = self
            .client
            .http
            .post(LINE_LIFF_ENDPOINT)
            .header("Accept", "application/json, text/plain, */*")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Linux; Android 4.4.2; G730-U00 Build/JLS36C) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Version/4.0 Chrome/30.0.0.0 Mobile Safari/537.36 Line/9.8.0",
            )
            .header("Accept-Encoding", "gzip, deflate")
            .header("Accept-Language", "zh-TW,zh;q=0.9")
            .header("Authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&serde_json::json!({"messages": options.messages}))?)
            .send()
            .await?;
        let status = res.status();
        let body: serde_json::Value = res.json().await?;
        if !status.is_success() {
            return Err(Error::internal(
                ERROR_NAME,
                format!("Failed to send Liff message: {} {body}", status.canonical_reason().unwrap_or("")),
            ));
        }
        Ok(body.into())
    }

    async fn try_consent_liff(&self, channel_id: &str, referer: Option<&str>) -> Result<bool> {
        let mut req = self
            .client
            .http
            .post(CONSENT_API_URL)
            .header("X-LINE-ChannelId", channel_id)
            .header("X-LINE-Access", self.client.require_token()?)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Linux; Android 8.0.1; SAMSUNG Realise/DeachSword; wv) AppleWebKit/537.36 (KHTML, like \
                 Gecko) Version/4.0 Chrome/56.0.2924.87 Mobile Safari/537.36",
            )
            .header("Content-Type", "application/json")
            .header("X-Line-Application", self.client.device_details.application())
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Accept-Language", "ja-JP,en-US;q=0.8")
            .body(r#"{"on":["P","CM"],"off":[]}"#);
        if let Some(r) = referer {
            req = req.header("referer", r);
        }
        Ok(req.send().await?.status().is_success())
    }

    async fn try_consent_authorize(&self, consent_url: &str, channel_id: &str) -> Result<bool> {
        let permissions = ["P", "CM", "OC"];
        let token = self.client.require_token()?;
        let user_agent = self.client.device_details.user_agent();
        let application = self.client.device_details.application();
        let res = self
            .client
            .http
            .get(consent_url)
            .header("X-Line-Access", &token)
            .header("User-Agent", &user_agent)
            .header("X-Line-Application", &application)
            .send()
            .await?;
        if !res.status().is_success() {
            return Ok(false);
        }
        let cookies: Vec<String> = res
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or_default().to_string())
            .collect();
        let html = res.text().await?;
        let Some(csrf) = find_csrf(&html) else { return Ok(false) };
        let all = permissions.iter().map(|p| format!("allPermission={p}")).collect::<Vec<_>>().join("&");
        let approved = permissions.iter().map(|p| format!("approvedPermission={p}")).collect::<Vec<_>>().join("&");
        let body = format!("{all}&{approved}&__WLS=&channelId={channel_id}&__csrf={csrf}&allow=true");
        let res = self
            .client
            .http
            .post(AUTH_CONSENT_URL)
            .header("X-Line-Access", &token)
            .header("User-Agent", &user_agent)
            .header("X-Line-Application", &application)
            .header("cookie", cookies.join("; "))
            .header("referer", consent_url)
            .body(body)
            .send()
            .await?;
        Ok(res.status().is_success())
    }
}

// <generated pass-through>
impl LiffService<'_> {
    /// `getLiffViewWithoutUserContext(request: LiffViewWithoutUserContextRequest)`.
    pub async fn get_liff_view_without_user_context(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getLiffViewWithoutUserContext", args).await
    }

    /// `issueSubLiffView(request: LiffViewRequest)`.
    pub async fn issue_sub_liff_view(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("issueSubLiffView", args).await
    }
}
// </generated pass-through>

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::test_util::{path, s};

    #[test]
    fn issue_liff_view_contexts() {
        let f = issue_liff_view_args("liff-1", None, "ja_JP");
        assert_eq!(path(&f, &[1, 1]), Some(&s("liff-1")));
        assert_eq!(path(&f, &[1, 2, 1]), Some(&TValue::Struct(Vec::new())));
        assert_eq!(path(&f, &[1, 3]), Some(&s("ja_JP")));
        let f = issue_liff_view_args("liff-1", Some("c123"), "en");
        assert_eq!(path(&f, &[1, 2, 2, 1]), Some(&s("c123")));
        let f = issue_liff_view_args("liff-1", Some("m123"), "en");
        assert_eq!(path(&f, &[1, 2, 3, 1]), Some(&s("m123")));
    }

    #[test]
    fn csrf_extraction() {
        let html = r#"<form><input type="hidden" name="__csrf" id="__csrf" value="tok-123"></form>"#;
        assert_eq!(find_csrf(html), Some("tok-123"));
        assert_eq!(find_csrf("<form></form>"), None);
    }
}
