//! Login flows: QR code (SQR v1 / ForSecure v2), email or phone number +
//! password (v1 / v3), existing auth tokens, and the IDL-backed parts of
//! passwordless (PAAK) login ([`pwless`]).

pub mod pwless;
mod rsa_crypto;

pub use rsa_crypto::rsa_encrypt_credentials;

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

use crate::base::{BaseClient, ClientEvent};
use crate::device::Device;
use crate::e2ee;
use crate::error::{Error, Result};
use crate::request::auth_token::parse_auth_token_input;
use crate::request::{Parse, RequestOptions};
use crate::thrift::{Protocol, TValue, Value};

const SQR_PATH: &str = "/acct/lgn/sq/v1";
const SQR_LP_PATH: &str = "/acct/lp/lgn/sq/v1";

/// How to log in.
#[derive(Debug, Clone)]
pub enum LoginOption {
    /// Show a QR code (`ClientEvent::QrCall`). `v3: None` picks by device.
    Qr { v3: Option<bool> },
    /// Email (or phone number, see [`IdentityProvider`]) + password.
    /// `pincode` must be 6 digits (default `114514`).
    Password { email: String, password: String, pincode: Option<String>, v3: Option<bool>, e2ee: bool },
    /// An existing token (plain, `access\nrefresh`, or JSON; see
    /// [`parse_auth_token_input`]).
    AuthToken(String),
}

impl LoginOption {
    pub fn qr() -> Self {
        LoginOption::Qr { v3: None }
    }
    pub fn password(email: impl Into<String>, password: impl Into<String>) -> Self {
        LoginOption::Password { email: email.into(), password: password.into(), pincode: None, v3: None, e2ee: true }
    }
    /// Phone number + password (`+66812345678`; spaces, dashes, dots and
    /// parentheses are ignored). Same flow as email login with
    /// `identityProvider = LINE_PHONE`.
    pub fn phone_password(phone: impl Into<String>, password: impl Into<String>) -> Self {
        Self::password(phone, password)
    }
    pub fn token(token: impl Into<String>) -> Self {
        LoginOption::AuthToken(token.into())
    }
}

pub fn registration_auth_endpoint(device: Device) -> &'static str {
    match device {
        Device::Android | Device::AndroidSecondary => "/api/v4p/rs",
        _ => "/api/v3p/rs",
    }
}

fn s(v: impl Into<String>) -> TValue {
    TValue::string(v)
}

fn timeout_like(e: &Error) -> bool {
    if e.is_timeout() {
        return true;
    }
    let msg = e.to_string();
    ["Timeout", "timed out", "status=408", "status=410"].iter().any(|p| msg.contains(p))
}

pub fn is_valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else { return false };
    !local.is_empty()
        && local.chars().all(|c| c.is_ascii_alphanumeric() || ".!#$%&'*+/=?^_`{|}~-".contains(c))
        && !domain.is_empty()
        && domain.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// `LoginRequest.identityProvider` (IDL enum `IdentityProvider`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdentityProvider {
    /// Email address (`LINE = 1`).
    Line,
    /// Phone number (`LINE_PHONE = 3`).
    ///
    /// Phone login derived from static RE of LINE macOS 26.5.0; unverified
    /// against live server. Evidence: the binary's `LoginRequest` thrift
    /// writer sits next to the `identityProvider` field name and the
    /// `NAVER_KR`/`LINE_PHONE` enum names, the login panel is
    /// `LoginEmailPhoneNumberPanel` (`loginByEmailPhoneNumber`), and it logs
    /// `[AUTH] requestLogin with id & certificate : cert=%d, phone=%d`. The
    /// exact identifier format the server expects (the app calls
    /// `AuthService.normalizePhoneNumber` first, which is not in the bundled
    /// IDL) is unknown; we send digits with a leading `+` country code
    /// as given.
    LinePhone,
}

impl IdentityProvider {
    /// IDL enum value.
    pub fn as_i32(self) -> i32 {
        match self {
            IdentityProvider::Line => 1,
            IdentityProvider::LinePhone => 3,
        }
    }

    /// `getRSAKeyInfo(provider)`: email login keeps the value linejs uses
    /// (`0`); phone login asks for the `LINE_PHONE` key (unverified).
    fn rsa_key_provider(self) -> i32 {
        match self {
            IdentityProvider::Line => 0,
            IdentityProvider::LinePhone => self.as_i32(),
        }
    }
}

/// Normalize a phone number: drop spaces, `-`, `.`, `(` and `)`; keep an
/// optional leading `+`; require 7 to 15 digits (E.164 length).
pub fn normalize_phone_number(phone: &str) -> Option<String> {
    let cleaned: String = phone.trim().chars().filter(|c| !matches!(c, ' ' | '-' | '.' | '(' | ')')).collect();
    let digits = cleaned.strip_prefix('+').unwrap_or(&cleaned);
    if !(7..=15).contains(&digits.len()) || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(cleaned)
}

/// Classify a login identifier as an email or a phone number and return
/// the provider with the identifier to send.
pub fn classify_identifier(identifier: &str) -> Option<(IdentityProvider, String)> {
    if is_valid_email(identifier) {
        return Some((IdentityProvider::Line, identifier.to_string()));
    }
    normalize_phone_number(identifier).map(|p| (IdentityProvider::LinePhone, p))
}

pub fn is_valid_password(pw: &str) -> bool {
    pw.len() >= 8 && pw.chars().all(|c| c.is_ascii_alphanumeric() || "!@#$%^&*()_+-=[]{};':\"\\|,.<>/?".contains(c))
}

impl BaseClient {
    /// Log in and fetch the profile (emits `Ready`).
    pub async fn login(&self, option: LoginOption) -> Result<()> {
        match option {
            LoginOption::Qr { v3 } => self.login_with_qr(v3).await?,
            LoginOption::AuthToken(token) => {
                let cred = parse_auth_token_input(&token)?;
                self.emit(ClientEvent::UpdateAuthToken(cred.access_token.clone()));
                self.set_auth_token(Some(cred.access_token));
                if let Some(rt) = cred.refresh_token {
                    self.storage.set("refreshToken", rt.into()).await?;
                }
                if let Some(exp) = cred.expire {
                    self.set_token_expire(exp).await?;
                }
            }
            LoginOption::Password { email, password, pincode, v3, e2ee } => {
                self.login_with_password(&email, &password, pincode.as_deref(), v3, e2ee).await?
            }
        }
        self.ready().await
    }

    /// Fetch the profile and emit `Ready`.
    pub async fn ready(&self) -> Result<()> {
        self.require_token()?;
        let profile = self.call("getProfile", crate::tv!({}), "/S4", Protocol::Compact).await?;
        self.set_profile(Some(profile.clone()));
        self.emit(ClientEvent::Ready(profile));
        Ok(())
    }

    pub async fn login_with_qr(&self, v3: Option<bool>) -> Result<()> {
        let token = if v3.unwrap_or_else(|| self.device.is_v3_supported()) {
            self.request_sqr2().await?
        } else {
            self.request_sqr().await?
        };
        self.emit(ClientEvent::UpdateAuthToken(token.clone()));
        self.set_auth_token(Some(token));
        self.verify_login_key().await
    }

    pub async fn login_with_password(
        &self,
        email: &str,
        password: &str,
        pincode: Option<&str>,
        v3: Option<bool>,
        e2ee: bool,
    ) -> Result<()> {
        if classify_identifier(email).is_none() {
            return Err(Error::internal("RegExpUnmatch", "invalid email or phone number"));
        }
        if !is_valid_password(password) {
            return Err(Error::internal("RegExpUnmatch", "invalid password"));
        }
        let pincode = pincode.unwrap_or("114514");
        let token = if v3.unwrap_or_else(|| self.device.is_v3_supported()) {
            self.request_email_login_v2(email, password, pincode).await?
        } else {
            self.request_email_login(email, password, pincode, e2ee).await?
        };
        self.emit(ClientEvent::UpdateAuthToken(token.clone()));
        self.set_auth_token(Some(token));
        self.verify_login_key().await
    }

    pub async fn register_cert(&self, cert: &str, email: &str) -> Result<()> {
        self.storage.set(&format!("cert:{email}"), cert.into()).await
    }
    pub async fn get_cert(&self, email: &str) -> Result<Option<String>> {
        self.storage.get_string(&format!("cert:{email}")).await
    }
    pub async fn register_qr_cert(&self, cert: &str) -> Result<()> {
        self.storage.set("qrCert", cert.into()).await
    }
    pub async fn get_qr_cert(&self) -> Result<Option<String>> {
        self.storage.get_string("qrCert").await
    }

    /// SQR v1 (`createQrCode` + `qrCodeLogin`).
    pub async fn request_sqr(&self) -> Result<String> {
        let sqr = self.create_session().await?;
        let mut url = self.create_qr_code(&sqr).await?["1"].as_str().unwrap_or_default().to_string();
        let (secret, secret_url) = self.create_sqr_secret(false);
        url.push_str(&secret_url);
        self.emit(ClientEvent::QrCall(url));
        if !self.check_qr_code_verified(&sqr, 1, 30).await? {
            return Err(Error::internal("TimeoutError", "checkQrCodeVerified timed out"));
        }
        self.verify_cert_or_pin(&sqr, 1, 30).await?;
        let res = self.qr_code_login(&sqr, true).await?;
        if let Some(pem) = res["1"].as_str() {
            self.emit(ClientEvent::UpdateQrCert(pem.to_string()));
            self.register_qr_cert(pem).await?;
        }
        self.finish_e2ee_login(&res["4"], &secret).await?;
        res["2"].as_str().map(str::to_string).ok_or_else(|| Error::internal("LoginError", "qrCodeLogin returned no token"))
    }

    /// SQR ForSecure (`createQrCodeForSecure` + `qrCodeLoginV2ForSecure`).
    pub async fn request_sqr2(&self) -> Result<String> {
        let sqr = self.create_session().await?;
        let fs = self.create_qr_code_for_secure(&sqr).await?;
        let mut url = fs["1"].as_str().unwrap_or_default().to_string();
        let max_count = fs["2"].as_i64().unwrap_or(12).max(1) as u32;
        let interval = fs["3"].as_i64().unwrap_or(30).max(1) as u64;
        let nonce = fs["4"].as_str().unwrap_or_default().to_string();
        let (secret, secret_url) = self.create_sqr_secret(false);
        url.push_str(&secret_url);
        self.emit(ClientEvent::QrCall(url));
        if !self.check_qr_code_verified(&sqr, max_count, interval).await? {
            return Err(Error::internal("TimeoutError", "checkQrCodeVerified timed out"));
        }
        self.verify_cert_or_pin(&sqr, max_count, interval).await?;
        let res = self.qr_code_login_v2_for_secure(&sqr, &nonce, "evex-device", "linejs-v2", true).await?;
        if let Some(pem) = res["1"].as_str() {
            self.emit(ClientEvent::UpdateQrCert(pem.to_string()));
            self.register_qr_cert(pem).await?;
        }
        let e2ee_info = if res["10"].is_null() { res["6"]["e2eeInfo"].clone() } else { res["10"].clone() };
        self.finish_e2ee_login(&e2ee_info, &secret).await?;
        let token_info = &res["3"];
        self.store_v3_token(token_info).await?;
        token_info["1"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::internal("LoginError", "qrCodeLoginV2ForSecure returned no token"))
    }

    async fn verify_cert_or_pin(&self, sqr: &str, max_count: u32, interval: u64) -> Result<()> {
        // linejs falls through to the PIN flow when the cert cannot be read.
        let cert = self.get_qr_cert().await.ok().flatten();
        if self.verify_certificate(sqr, cert.as_deref()).await.is_err() {
            let pin = self.create_pin_code(sqr).await?["1"].as_str().unwrap_or_default().to_string();
            self.emit(ClientEvent::PinCall(pin));
            self.check_pin_code_verified(sqr, max_count, interval).await?;
        }
        Ok(())
    }

    async fn finish_e2ee_login(&self, e2ee_info: &Value, secret: &[u8]) -> Result<()> {
        let mut decoded = None;
        if !e2ee_info.is_null() {
            decoded = self.decode_e2ee_key_v1(e2ee_info, secret).await?;
        }
        if decoded.is_none() {
            self.register_e2ee_key_pair().await?;
        }
        Ok(())
    }

    /// Persist `refreshToken` and `expire` from a raw `TokenV3IssueResult`.
    async fn store_v3_token(&self, token_info: &Value) -> Result<()> {
        if let Some(rt) = token_info["2"].as_str() {
            self.storage.set("refreshToken", rt.into()).await?;
        }
        if let (Some(a), Some(b)) = (token_info["3"].as_i64(), token_info["6"].as_i64()) {
            self.set_token_expire(a + b).await?;
        }
        Ok(())
    }

    /// Resolve the identifier, fetch the RSA key and encrypt the
    /// credentials. Returns `(provider, keynm, encrypted)`.
    async fn rsa_login_material(&self, identifier: &str, password: &str) -> Result<(IdentityProvider, String, String)> {
        let (provider, email) = classify_identifier(identifier)
            .ok_or_else(|| Error::internal("RegExpUnmatch", "invalid email or phone number"))?;
        let email = email.as_str();
        let key = self.get_rsa_key_info(provider.rsa_key_provider()).await?;
        let keynm = key["keynm"].as_str().unwrap_or_default().to_string();
        let session_key = key["sessionKey"].as_str().unwrap_or_default();
        let n = key["nvalue"].as_str().unwrap_or_default();
        let e = key["evalue"].as_str().unwrap_or_default();
        let mut message = Vec::new();
        for part in [session_key, email, password] {
            message.push(part.len() as u8);
            message.extend_from_slice(part.as_bytes());
        }
        Ok((provider, keynm, rsa_encrypt_credentials(&message, n, e)?))
    }

    /// Email login v1 (`loginZ`).
    pub async fn request_email_login(&self, email: &str, password: &str, pincode: &str, enable_e2ee: bool) -> Result<String> {
        if pincode.len() != 6 {
            return Err(Error::internal("Invalid constant pincode", "The constant pincode should be 6 digits"));
        }
        let (provider, keynm, encrypted) = self.rsa_login_material(email, password).await?;
        let (secret, e2ee_data) = if enable_e2ee {
            let (secret, pk) = self.create_sqr_secret(true);
            let data = e2ee::encrypt_aes_ecb(&e2ee::sha256(pincode.as_bytes()), &B64.decode(pk).unwrap_or_default());
            (Some(secret), Some(data))
        } else {
            (None, None)
        };
        let cert = self.get_cert(email).await?;
        let mut res = self.login_v2(provider, &keynm, &encrypted, None, e2ee_data.as_deref(), cert.as_deref(), "loginZ").await?;
        if res["authToken"].is_null() {
            let pin = res["pinCode"].as_str().unwrap_or(pincode).to_string();
            self.emit(ClientEvent::PinCall(pin));
            let verifier = res["verifier"].as_str().unwrap_or_default().to_string();
            if let Some(secret) = &secret {
                let confirm = self.e2ee_pin_handshake(&verifier, secret).await?;
                res = self
                    .login_v2(provider, &keynm, &encrypted, Some(&confirm), e2ee_data.as_deref(), cert.as_deref(), "loginZ")
                    .await?;
            } else {
                let mut headers = self.verifier_headers(&verifier);
                headers.push(("accept".into(), "application/x-thrift".into()));
                let v = self.get_json("/Q", headers).await?;
                let verifier = v["result"]["verifier"].as_str().unwrap_or_default().to_string();
                res = self
                    .login_v2(provider, &keynm, &encrypted, Some(&verifier), e2ee_data.as_deref(), cert.as_deref(), "loginZ")
                    .await?;
            }
        }
        if let Some(c) = res["certificate"].as_str() {
            self.emit(ClientEvent::UpdateCert(c.to_string()));
            self.register_cert(c, email).await?;
        }
        res["authToken"].as_str().map(str::to_string).ok_or_else(|| Error::internal("LoginError", "loginZ returned no token"))
    }

    /// Email login v3 (`loginV2`, issues a refresh token).
    pub async fn request_email_login_v2(&self, email: &str, password: &str, pincode: &str) -> Result<String> {
        if pincode.len() != 6 {
            return Err(Error::internal("Invalid constant pincode", "The constant pincode should be 6 digits"));
        }
        let (provider, keynm, encrypted) = self.rsa_login_material(email, password).await?;
        let (secret, pk) = self.create_sqr_secret(true);
        let e2ee_data = e2ee::encrypt_aes_ecb(&e2ee::sha256(pincode.as_bytes()), &B64.decode(pk).unwrap_or_default());
        let cert = self.get_cert(email).await?;
        let mut res = self.login_v2(provider, &keynm, &encrypted, None, Some(&e2ee_data), cert.as_deref(), "loginV2").await?;
        if res["9"].is_null() {
            self.emit(ClientEvent::PinCall(pincode.to_string()));
            let verifier = res["3"].as_str().unwrap_or_default().to_string();
            let confirm = self.e2ee_pin_handshake(&verifier, &secret).await?;
            res = self.login_v2(provider, &keynm, &encrypted, Some(&confirm), Some(&e2ee_data), cert.as_deref(), "loginV2").await?;
        }
        if let Some(c) = res["2"].as_str() {
            self.emit(ClientEvent::UpdateCert(c.to_string()));
            self.register_cert(c, email).await?;
        }
        self.store_v3_token(&res["9"]).await?;
        res["9"]["1"].as_str().map(str::to_string).ok_or_else(|| Error::internal("LoginError", "loginV2 returned no token"))
    }

    fn verifier_headers(&self, verifier: &str) -> Vec<(String, String)> {
        vec![
            ("user-agent".into(), self.device_details.user_agent()),
            ("x-line-application".into(), self.device_details.application()),
            ("x-line-access".into(), verifier.to_string()),
            ("x-lal".into(), "ja_JP".into()),
            ("x-lpv".into(), "1".into()),
            ("x-lhm".into(), "GET".into()),
            ("accept-encoding".into(), "gzip".into()),
        ]
    }

    /// Wait for the PIN on `/LF1`, then answer with `confirmE2EELogin`.
    async fn e2ee_pin_handshake(&self, verifier: &str, secret: &[u8]) -> Result<String> {
        let info = self.get_json("/LF1", self.verifier_headers(verifier)).await?;
        let meta = info["result"]["metadata"].clone();
        self.decode_e2ee_key_v1(&Value::from(meta.clone()), secret).await?;
        let pk = B64.decode(meta["publicKey"].as_str().unwrap_or_default()).unwrap_or_default();
        let chain = B64.decode(meta["encryptedKeyChain"].as_str().unwrap_or_default()).unwrap_or_default();
        let device_secret = self.encrypt_device_secret(&pk, secret, &chain)?;
        let res = self.confirm_e2ee_login(verifier, &device_secret).await?;
        Ok(res.as_str().unwrap_or_default().to_string())
    }

    pub async fn get_rsa_key_info(&self, provider: i32) -> Result<Value> {
        self.request(
            vec![(1, TValue::Struct(vec![(2, TValue::I32(provider))]))],
            "getRSAKeyInfo",
            RequestOptions::new("/api/v3/TalkService.do", Protocol::Binary).parse(Parse::Struct("RSAKey")),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn login_v2(
        &self,
        provider: IdentityProvider,
        keynm: &str,
        encrypted: &str,
        verifier: Option<&str>,
        secret: Option<&[u8]>,
        cert: Option<&str>,
        method: &str,
    ) -> Result<Value> {
        let login_type = if verifier.is_some() { 1 } else if secret.is_some() { 2 } else { 0 };
        let mut req = vec![
            (1, TValue::I32(login_type)),
            (2, TValue::I32(provider.as_i32())),
            (3, s(keynm)),
            (4, s(encrypted)),
            (5, TValue::Bool(false)),
            (6, s("")),
            (7, s(self.device.as_str())),
        ];
        if let Some(c) = cert {
            req.push((8, s(c)));
        }
        if let Some(v) = verifier {
            req.push((9, s(v)));
        }
        if let Some(sec) = secret {
            req.push((10, TValue::Binary(sec.to_vec())));
        }
        req.push((11, TValue::I32(1)));
        req.push((12, s("System Product Name")));
        let parse = if method == "loginZ" { Parse::Struct("LoginResult") } else { Parse::Raw };
        self.request(
            vec![(2, TValue::Struct(req))],
            method,
            RequestOptions::new(registration_auth_endpoint(self.device), Protocol::Binary).parse(parse),
        )
        .await
    }

    fn sqr_opts(&self, path: &'static str) -> RequestOptions<'static> {
        RequestOptions::new(path, Protocol::Compact).parse(Parse::Raw)
    }

    pub async fn create_session(&self) -> Result<String> {
        let res = self.request(vec![], "createSession", self.sqr_opts(SQR_PATH)).await?;
        res["1"].as_str().map(str::to_string).ok_or_else(|| Error::internal("LoginError", "createSession returned no id"))
    }

    pub async fn create_qr_code(&self, sqr: &str) -> Result<Value> {
        self.request(vec![(1, TValue::Struct(vec![(1, s(sqr))]))], "createQrCode", self.sqr_opts(SQR_PATH)).await
    }

    pub async fn create_qr_code_for_secure(&self, sqr: &str) -> Result<Value> {
        self.request(vec![(1, TValue::Struct(vec![(1, s(sqr))]))], "createQrCodeForSecure", self.sqr_opts(SQR_PATH))
            .await
    }

    async fn long_poll(&self, method: &str, sqr: &str, max_count: u32, interval_sec: u64) -> Result<bool> {
        let interval_ms = interval_sec * 1000;
        for i in 0..max_count.max(1) {
            let opts = self
                .sqr_opts(SQR_LP_PATH)
                .header("x-lst", interval_ms.to_string())
                .header("x-line-access", sqr)
                .timeout(Duration::from_millis(interval_ms + 5000));
            match self.request(vec![(1, TValue::Struct(vec![(1, s(sqr))]))], method, opts).await {
                Ok(_) => return Ok(true),
                Err(e) if timeout_like(&e) && i + 1 < max_count => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(false)
    }

    pub async fn check_qr_code_verified(&self, sqr: &str, max_count: u32, interval_sec: u64) -> Result<bool> {
        self.long_poll("checkQrCodeVerified", sqr, max_count, interval_sec).await
    }

    pub async fn check_pin_code_verified(&self, sqr: &str, max_count: u32, interval_sec: u64) -> Result<bool> {
        self.long_poll("checkPinCodeVerified", sqr, max_count, interval_sec).await
    }

    pub async fn verify_certificate(&self, sqr: &str, cert: Option<&str>) -> Result<Value> {
        let mut req = vec![(1, s(sqr))];
        if let Some(c) = cert {
            req.push((2, s(c)));
        }
        self.request(vec![(1, TValue::Struct(req))], "verifyCertificate", self.sqr_opts(SQR_PATH)).await
    }

    pub async fn create_pin_code(&self, sqr: &str) -> Result<Value> {
        self.request(vec![(1, TValue::Struct(vec![(1, s(sqr))]))], "createPinCode", self.sqr_opts(SQR_PATH)).await
    }

    pub async fn qr_code_login(&self, sqr: &str, auto_login: bool) -> Result<Value> {
        self.request(
            vec![(1, TValue::Struct(vec![(1, s(sqr)), (2, s(self.device.as_str())), (3, TValue::Bool(auto_login))]))],
            "qrCodeLogin",
            self.sqr_opts(SQR_PATH),
        )
        .await
    }

    pub async fn qr_code_login_v2(&self, sqr: &str, model_name: &str, system_name: &str, auto_login: bool) -> Result<Value> {
        self.request(
            vec![(
                1,
                TValue::Struct(vec![(1, s(sqr)), (2, s(system_name)), (3, s(model_name)), (4, TValue::Bool(auto_login))]),
            )],
            "qrCodeLoginV2",
            self.sqr_opts(SQR_PATH),
        )
        .await
    }

    pub async fn qr_code_login_v2_for_secure(
        &self,
        sqr: &str,
        nonce: &str,
        model_name: &str,
        system_name: &str,
        auto_login: bool,
    ) -> Result<Value> {
        self.request(
            vec![(
                1,
                TValue::Struct(vec![
                    (1, s(sqr)),
                    (2, s(system_name)),
                    (3, s(model_name)),
                    (4, TValue::Bool(auto_login)),
                    (5, s(nonce)),
                ]),
            )],
            "qrCodeLoginV2ForSecure",
            self.sqr_opts(SQR_PATH),
        )
        .await
    }

    pub async fn confirm_e2ee_login(&self, verifier: &str, device_secret: &[u8]) -> Result<Value> {
        self.request(
            vec![(1, s(verifier)), (2, TValue::Binary(device_secret.to_vec()))],
            "confirmE2EELogin",
            RequestOptions::new(registration_auth_endpoint(self.device), Protocol::Binary).parse(Parse::Raw),
        )
        .await
    }

    /// Primary device's answer to another device's PIN login request.
    pub async fn respond_e2ee_login_request(
        &self,
        verifier: &str,
        public_key: (Option<i32>, Option<i32>, Option<Vec<u8>>),
        encrypted_key_chain: &[u8],
        hash_key_chain: &[u8],
        error_code: i32,
    ) -> Result<Value> {
        let (version, key_id, key_data) = public_key;
        let mut pk = Vec::new();
        if let Some(v) = version {
            pk.push((1, TValue::I32(v)));
        }
        if let Some(k) = key_id {
            pk.push((2, TValue::I32(k)));
        }
        if let Some(d) = key_data {
            pk.push((4, TValue::Binary(d)));
        }
        self.request(
            vec![
                (1, s(verifier)),
                (2, TValue::Struct(pk)),
                (3, TValue::Binary(encrypted_key_chain.to_vec())),
                (4, TValue::Binary(hash_key_chain.to_vec())),
                (5, TValue::I32(error_code)),
            ],
            "respondE2EELoginRequest",
            RequestOptions::new("/S4", Protocol::Compact).parse(Parse::Raw),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validators() {
        assert!(is_valid_email("a.b+c@example.co.jp"));
        assert!(!is_valid_email("nope"));
        assert_eq!(normalize_phone_number("+66 (81) 234-5678").as_deref(), Some("+66812345678"));
        assert_eq!(normalize_phone_number("090.1234.5678").as_deref(), Some("09012345678"));
        assert_eq!(normalize_phone_number("+12"), None);
        assert_eq!(normalize_phone_number("+66abc12345"), None);
        assert_eq!(
            classify_identifier("a@example.com"),
            Some((IdentityProvider::Line, "a@example.com".to_string()))
        );
        assert_eq!(classify_identifier("+81 90-1234-5678"), Some((IdentityProvider::LinePhone, "+819012345678".into())));
        assert_eq!(classify_identifier("nope"), None);
        assert_eq!(IdentityProvider::LinePhone.as_i32(), 3);
        assert_eq!(IdentityProvider::Line.rsa_key_provider(), 0);
        assert!(is_valid_password("abcd1234!"));
        assert!(!is_valid_password("short"));
        assert_eq!(registration_auth_endpoint(Device::Android), "/api/v4p/rs");
        assert_eq!(registration_auth_endpoint(Device::DesktopWin), "/api/v3p/rs");
    }
}
