//! Thrift-over-HTTP request pipeline (`RequestClient` in linejs).

pub mod auth_token;
pub mod legy;

use std::sync::atomic::Ordering;
use std::time::Duration;

use futures::future::BoxFuture;

use crate::base::{BaseClient, ClientEvent, LegyMode};
use crate::error::{Error, Result};
use crate::push::backoff::{Backoff, BackoffPolicy};
use crate::thrift::{self, Protocol, Schema, TValue, Value};

const SQUARE_PATHS: [&str; 2] = ["/SQ1", "/SQLV1"];

/// `AccessTokenRefreshService` path (never triggers a proactive refresh).
pub const TOKEN_REFRESH_PATH: &str = "/EXT/auth/tokenrefresh/v1";

/// How the response body is turned into a [`Value`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Parse<'a> {
    /// Use the IDL `<method>_result` struct (`SquareService_` prefixed on
    /// square paths).
    #[default]
    Result,
    /// Decode the success field as this struct.
    Struct(&'a str),
    /// No renaming: struct fields keyed by numeric id.
    Raw,
}

/// Per-request options.
#[derive(Debug, Clone)]
pub struct RequestOptions<'a> {
    pub path: &'a str,
    pub protocol: Protocol,
    pub parse: Parse<'a>,
    /// Extra headers; these override the defaults.
    pub headers: Vec<(String, String)>,
    pub timeout: Option<Duration>,
}

impl<'a> RequestOptions<'a> {
    pub fn new(path: &'a str, protocol: Protocol) -> Self {
        Self {
            path,
            protocol,
            parse: Parse::Result,
            headers: Vec::new(),
            timeout: None,
        }
    }
    pub fn parse(mut self, p: Parse<'a>) -> Self {
        self.parse = p;
        self
    }
    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = Some(t);
        self
    }
}

/// Exception struct names by request path.
pub fn exception_type(path: &str) -> &'static str {
    match path {
        "/CH3" | "/CH4" => "ChannelException",
        "/SQ1" => "SquareException",
        "/LIFF1" => "LiffException",
        _ => "TalkException",
    }
}

fn is_legy_talk_path(path: &str) -> bool {
    matches!(
        path,
        "/S3"
            | "/S4"
            | "/V4"
            | "/SYNC3"
            | "/SYNC4"
            | "/P4"
            | "/P5"
            | "/NP4"
            | "/NP5"
            | "/C5"
            | "/CA5"
            | "/ECA5"
    )
}

/// Raw HTTP response (after LEGY decryption when used).
#[derive(Debug)]
pub struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RawResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

impl BaseClient {
    /// Default LINE headers for a request.
    pub fn line_headers(&self, method: &str) -> Vec<(String, String)> {
        let mut h = vec![
            ("accept".to_string(), "application/x-thrift".to_string()),
            ("user-agent".to_string(), self.device_details.user_agent()),
            (
                "x-line-application".to_string(),
                self.device_details.application(),
            ),
            (
                "content-type".to_string(),
                "application/x-thrift".to_string(),
            ),
            ("x-lal".to_string(), self.config.language.clone()),
            ("x-lpv".to_string(), "1".to_string()),
            ("x-lhm".to_string(), method.to_string()),
            ("accept-encoding".to_string(), "gzip".to_string()),
        ];
        if let Some(token) = self.auth_token() {
            h.push(("x-line-access".to_string(), token));
        }
        h
    }

    /// Call a method with schema-encoded args: `args` is encoded as the IDL
    /// struct `<method>_args` and the reply decoded as `<method>_result`.
    /// Returns the `success` value (or `Null` for void methods).
    pub async fn call(
        &self,
        method: &str,
        args: impl Into<Value>,
        path: &str,
        protocol: Protocol,
    ) -> Result<Value> {
        self.call_with(method, args, RequestOptions::new(path, protocol))
            .await
    }

    pub async fn call_with(
        &self,
        method: &str,
        args: impl Into<Value>,
        opts: RequestOptions<'_>,
    ) -> Result<Value> {
        let schema = Schema::line();
        let square = SQUARE_PATHS.contains(&opts.path);
        let prefixed = format!("SquareService_{method}_args");
        let plain = format!("{method}_args");
        let name = if square && schema.get_struct(&prefixed).is_some() {
            prefixed
        } else {
            plain
        };
        let fields = thrift::encode_struct(schema, &name, &args.into())?;
        self.request(fields, method, opts).await
    }

    /// Send pre-encoded args (the `NestedArray` style API).
    pub async fn request(
        &self,
        args: Vec<(i16, TValue)>,
        method: &str,
        opts: RequestOptions<'_>,
    ) -> Result<Value> {
        self.ensure_enabled()?;
        if opts.path != TOKEN_REFRESH_PATH {
            self.refresh_token_if_due().await;
        }
        // Refresh calls hold `refresh_lock`; never refresh or end the
        // session from inside one (linejs recurses at most once).
        let is_refresh_call = opts.path == TOKEN_REFRESH_PATH;
        let mut refreshed = false;
        loop {
            let sent_token = self.auth_token();
            match self.request_once(&args, method, &opts).await {
                Err(e)
                    if !refreshed
                        && !is_refresh_call
                        && e.code() == Some("MUST_REFRESH_V3_TOKEN") =>
                {
                    if self.storage.get_string("refreshToken").await?.is_none() {
                        return Err(e);
                    }
                    refreshed = true;
                    self.refresh_token_after(sent_token.as_deref()).await?;
                }
                Err(e) => {
                    // Only a request made with our own token can end our
                    // session (login flows send other credentials).
                    if sent_token.is_some()
                        && !is_refresh_call
                        && let Some(reason) = self.logout_reason(&e)
                    {
                        self.kick_out(reason);
                    }
                    return Err(e);
                }
                ok => return ok,
            }
        }
    }

    fn ensure_enabled(&self) -> Result<()> {
        if !self.is_disabled() {
            return Ok(());
        }
        Err(match self.kicked_out() {
            Some(reason) => Error::internal(
                "LoggedOut",
                format!("Request aborted: session ended by server ({reason})"),
            ),
            None => Error::internal(
                "ClientClosed",
                "Request aborted: client has been disabled (logged out)",
            ),
        })
    }

    /// Start a background refresh when the stored `expire` has passed
    /// (`fa0/f0.java` `r()`): this request still goes out with the current
    /// token. Failures are logged as `RefreshError`; the reactive
    /// `MUST_REFRESH_V3_TOKEN` path still applies, and the next proactive
    /// try waits [`PROACTIVE_RETRY_AFTER`].
    async fn refresh_token_if_due(&self) {
        if !self.config.proactive_refresh || self.auth_token().is_none() {
            return;
        }
        if crate::base::session_now() < self.refresh_not_before.load(Ordering::Relaxed)
            || !self.refresh_due().await
            || self.refresh_running.swap(true, Ordering::SeqCst)
        {
            return;
        }
        struct Running(BaseClient);
        impl Drop for Running {
            fn drop(&mut self) {
                self.0.refresh_running.store(false, Ordering::SeqCst);
            }
        }
        let running = Running(self.clone());
        tokio::spawn(async move {
            let c = &running.0;
            let _guard = c.refresh_lock.lock().await;
            // Another refresh may have finished while we waited.
            if !c.refresh_due().await
                || !matches!(c.storage.get_string("refreshToken").await, Ok(Some(_)))
            {
                return;
            }
            if let Err(e) = c.try_refresh_token_unlocked().await {
                c.refresh_not_before.store(
                    crate::base::session_now() + PROACTIVE_RETRY_AFTER.as_secs() as i64,
                    Ordering::Relaxed,
                );
                c.log("RefreshError", serde_json::json!({"error": e.to_string()}));
            }
        });
    }

    /// Whether the stored `expire` (minus `refresh_margin`) has passed.
    async fn refresh_due(&self) -> bool {
        let margin = self.config.refresh_margin.as_secs() as i64;
        self.token_expire()
            .await
            .ok()
            .flatten()
            .is_some_and(|exp| crate::base::session_now() + margin >= exp)
    }

    /// Reactive refresh after `MUST_REFRESH_V3_TOKEN`; skipped when another
    /// task already replaced the token the failed request used.
    pub(crate) async fn refresh_token_after(&self, failed_token: Option<&str>) -> Result<()> {
        let _guard = self.refresh_lock.lock().await;
        if self.auth_token().as_deref() != failed_token {
            return Ok(());
        }
        self.try_refresh_token_unlocked().await
    }

    async fn request_once(
        &self,
        args: &[(i16, TValue)],
        method: &str,
        opts: &RequestOptions<'_>,
    ) -> Result<Value> {
        let body = thrift::write_message(method, args, opts.protocol)?;
        let mut headers = self.line_headers("POST");
        for (k, v) in &opts.headers {
            headers.retain(|(hk, _)| !hk.eq_ignore_ascii_case(k));
            headers.push((k.clone(), v.clone()));
        }
        let timeout = opts.timeout.unwrap_or(self.config.timeout);
        tracing::trace!(method, path = opts.path, "request");
        let sent_access = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("x-line-access"))
            .map(|(_, v)| v.clone());
        let res = self
            .send_http(opts.path, "POST", headers, body, timeout)
            .await?;
        if let Some(next) = res.header("x-line-next-access") {
            self.apply_next_access(sent_access.as_deref(), next);
        }
        let msg = thrift::read_message(&res.body, opts.protocol).map_err(|_| {
            Error::internal(
                "RequestError",
                format!(
                    "Request internal failed: status={} headers={:?} body=<{}>",
                    res.status,
                    res.headers,
                    hex::encode(&res.body)
                ),
            )
        })?;
        self.interpret_result(method, opts, &msg)
    }

    fn interpret_result(
        &self,
        method: &str,
        opts: &RequestOptions<'_>,
        msg: &thrift::TMessage,
    ) -> Result<Value> {
        let schema = Schema::line();
        let TValue::Struct(fields) = &msg.body else {
            unreachable!("read_message returns a struct")
        };
        let success = msg.body.field(0);
        let exception = fields.iter().find(|(id, _)| *id != 0);

        let result_def = {
            let name = if msg.name.is_empty() {
                method
            } else {
                &msg.name
            };
            let square = SQUARE_PATHS.contains(&opts.path);
            let pre = format!("SquareService_{name}_result");
            if square && schema.get_struct(&pre).is_some() {
                pre
            } else {
                format!("{name}_result")
            }
        };

        if let Some((fid, exc)) = exception {
            let data = match opts.parse {
                Parse::Result => match schema
                    .get_struct(&result_def)
                    .and_then(|d| d.field_by_id(*fid))
                {
                    Some(f) => thrift::decode_value(schema, &f.ty, exc),
                    None => thrift::decode_struct(schema, Some(exception_type(opts.path)), exc),
                },
                _ => thrift::decode_struct(schema, Some(exception_type(opts.path)), exc),
            };
            return Err(Error::Request {
                method: method.to_string(),
                path: opts.path.to_string(),
                data,
            });
        }

        let Some(success) = success else {
            return Ok(Value::Null);
        };
        Ok(match opts.parse {
            Parse::Result => match schema
                .get_struct(&result_def)
                .and_then(|d| d.field_by_id(0))
            {
                Some(f) => thrift::decode_value(schema, &f.ty, success),
                None => thrift::decode_raw(success),
            },
            Parse::Struct(name) => thrift::decode_struct(schema, Some(name), success),
            Parse::Raw => thrift::decode_raw(success),
        })
    }

    /// Low-level HTTP to the LINE API host, transparently using the LEGY
    /// encrypted gateway when configured.
    pub async fn send_http(
        &self,
        path: &str,
        method: &str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        timeout: Duration,
    ) -> Result<RawResponse> {
        self.send_http_inner(path, method, headers, body, timeout, true)
            .await
    }

    /// Like [`send_http`](Self::send_http) but never uses the LEGY encrypted
    /// gateway. linejs sends compact messages (`/CA5`, `/ECA5`) with a plain
    /// fetch so their extra headers (`x-lai`) reach the server.
    pub async fn send_http_plain(
        &self,
        path: &str,
        method: &str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        timeout: Duration,
    ) -> Result<RawResponse> {
        self.send_http_inner(path, method, headers, body, timeout, false)
            .await
    }

    async fn send_http_inner(
        &self,
        path: &str,
        method: &str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        timeout: Duration,
        allow_legy: bool,
    ) -> Result<RawResponse> {
        let access = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("x-line-access"))
            .map(|(_, v)| v.clone());
        let use_legy = match self.legy_mode {
            _ if !allow_legy => false,
            LegyMode::Never => false,
            _ if access.is_none() => false,
            LegyMode::Always => true,
            LegyMode::Auto => {
                is_legy_talk_path(path)
                    && auth_token::should_use_legy_encrypted_access(access.as_deref())
            }
        };
        if use_legy {
            let transport = self.legy_transport();
            let enc = transport.encrypt_request(&legy::LegyRequest {
                path,
                access_token: access.as_deref(),
                body: &body,
            });
            let mut req = self
                .http
                .post(&transport.endpoint)
                .timeout(timeout)
                .body(enc);
            for (k, v) in transport.outer_headers(
                &self.device_details.application(),
                &self.device_details.user_agent(),
                &self.config.language,
                method,
            ) {
                req = req.header(k, v);
            }
            let res = req.send().await?;
            let status = res.status().as_u16();
            let mut out_headers = collect_headers(res.headers());
            let bytes = res.bytes().await?;
            let dec = transport.decrypt_response(status, &bytes)?;
            for (k, v) in dec.headers {
                out_headers.retain(|(hk, _)| !hk.eq_ignore_ascii_case(&k));
                out_headers.push((k, v));
            }
            return Ok(RawResponse {
                status: dec.status,
                headers: out_headers,
                body: dec.body,
            });
        }
        let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::POST);
        // Fail over to the next host only when the connection could not be
        // established, so the request certainly never reached a server.
        let mut idx = self.active_endpoint_index();
        let mut attempts = self.endpoint_count();
        let res = loop {
            let url = endpoint_url(self.current_endpoint(), path);
            let mut req = self.http.request(m.clone(), url).timeout(timeout);
            for (k, v) in &headers {
                req = req.header(k, v);
            }
            if !body.is_empty() || method == "POST" {
                req = req.body(body.clone());
            }
            attempts -= 1;
            match req.send().await {
                Err(e) if e.is_connect() && attempts > 0 => idx = self.fail_over_from(idx),
                other => break other?,
            }
        };
        let status = res.status().as_u16();
        let headers = collect_headers(res.headers());
        let body = res.bytes().await?.to_vec();
        Ok(RawResponse {
            status,
            headers,
            body,
        })
    }

    /// `GET https://{endpoint}{path}` with LINE headers, parsed as JSON.
    pub(crate) async fn get_json(
        &self,
        path: &str,
        headers: Vec<(String, String)>,
    ) -> Result<serde_json::Value> {
        let url = endpoint_url(self.current_endpoint(), path);
        let mut req = self.http.get(url).timeout(self.config.long_timeout);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        Ok(req.send().await?.json().await?)
    }

    /// Refresh the v3 access token using the stored refresh token.
    pub fn try_refresh_token(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let _guard = self.refresh_lock.lock().await;
            self.try_refresh_token_unlocked().await
        })
    }

    /// [`try_refresh_token`](Self::try_refresh_token) without taking
    /// `refresh_lock` (callers hold it).
    ///
    /// `refresh` is retried on `RETRY_REQUIRED` and network errors with the
    /// server's `retryPolicy` (saved from the last refresh; defaults 1 s,
    /// ×1.5, at most 60 s, ±10% jitter as in `fa0/o.java`), up to
    /// [`REFRESH_MAX_ATTEMPTS`] tries.
    fn try_refresh_token_unlocked(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let Some(refresh_token) = self.storage.get_string("refreshToken").await? else {
                return Err(Error::internal("RefreshError", "refreshToken not found"));
            };
            let policy = self.refresh_retry_policy().await;
            let res = self
                .call_refresh_api(
                    "refresh",
                    crate::tv!({"request": {"refreshToken": refresh_token}}),
                    policy,
                )
                .await?;
            let token = res["accessToken"]
                .as_str()
                .ok_or_else(|| Error::internal("RefreshError", "refresh returned no accessToken"))?
                .to_string();
            self.set_auth_token(Some(token.clone()));
            self.emit(ClientEvent::UpdateAuthToken(token.clone()));
            if let Some(rt) = res["refreshToken"].as_str() {
                self.storage.set("refreshToken", rt.into()).await?;
            }
            if let (Some(a), Some(b)) = (
                res["tokenIssueTimeEpochSec"].as_i64(),
                res["durationUntilRefreshInSec"].as_i64(),
            ) {
                self.set_token_expire(a + b).await?;
            }
            if !res["retryPolicy"].is_null() {
                self.storage
                    .set(REFRESH_RETRY_POLICY_KEY, res["retryPolicy"].to_json())
                    .await?;
            }
            self.refresh_not_before.store(0, Ordering::Relaxed);
            if self.config.report_refreshed_token {
                // `fa0/o.java` launches the report as its own job; a failure
                // does not undo the refresh.
                let c = self.clone();
                tokio::spawn(async move {
                    let report = c
                        .call_refresh_api(
                            "reportRefreshedAccessToken",
                            crate::tv!({"request": {"accessToken": token}}),
                            policy,
                        )
                        .await;
                    if let Err(e) = report {
                        c.log(
                            "ReportRefreshedAccessTokenError",
                            serde_json::json!({"error": e.to_string()}),
                        );
                    }
                });
            }
            Ok(())
        })
    }

    /// Call an `AccessTokenRefreshService` method, retrying with `policy`.
    async fn call_refresh_api(
        &self,
        method: &str,
        args: Value,
        policy: BackoffPolicy,
    ) -> Result<Value> {
        let mut backoff = Backoff::new(policy);
        let mut attempt = 1;
        loop {
            match self
                .call(method, args.clone(), TOKEN_REFRESH_PATH, Protocol::Compact)
                .await
            {
                Err(e) if attempt < REFRESH_MAX_ATTEMPTS && is_retryable_refresh_error(&e) => {
                    let delay = backoff.next_delay();
                    self.log(
                        "RefreshRetry",
                        serde_json::json!({
                            "method": method,
                            "attempt": attempt,
                            "delayMs": delay.as_millis() as u64,
                            "error": e.to_string(),
                        }),
                    );
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                other => return other,
            }
        }
    }

    /// The saved `retryPolicy` of the last refresh, or the Android
    /// defaults.
    async fn refresh_retry_policy(&self) -> BackoffPolicy {
        let stored = self
            .storage
            .get(REFRESH_RETRY_POLICY_KEY)
            .await
            .ok()
            .flatten();
        stored
            .map(|v| retry_policy_from_json(&v))
            .unwrap_or(DEFAULT_REFRESH_RETRY)
    }

    /// Adopt a rotated token from `X-Line-Next-Access` when the request was
    /// sent with the current token.
    ///
    /// Header name derived from static RE of LINE macOS 26.5.0; unverified
    /// against live server (it is read next to `X-Line-Access` in the
    /// command session). The value is taken to be a drop-in replacement
    /// access token.
    pub(crate) fn apply_next_access(&self, sent: Option<&str>, next: &str) {
        let next = next.trim();
        if next.is_empty() || sent == Some(next) {
            return;
        }
        if self.config.apply_next_access && sent.is_some() && sent == self.auth_token().as_deref() {
            self.set_auth_token(Some(next.to_string()));
            self.emit(ClientEvent::UpdateAuthToken(next.to_string()));
        }
    }
}

/// Storage key of the last `RefreshAccessTokenResponse.retryPolicy`.
pub const REFRESH_RETRY_POLICY_KEY: &str = "refreshRetryPolicy";

/// Tries of one refresh (or report) call before giving up.
pub const REFRESH_MAX_ATTEMPTS: u32 = 5;

/// Wait after a failed background refresh before the next one.
pub const PROACTIVE_RETRY_AFTER: Duration = Duration::from_secs(60);

/// `RetryPolicy` defaults of the Android app (`fa0/o.java`).
const DEFAULT_REFRESH_RETRY: BackoffPolicy = BackoffPolicy {
    initial: Duration::from_millis(1000),
    max: Duration::from_millis(60_000),
    multiplier: 1.5,
    jitter: 0.1,
};

/// A `RetryPolicy` (`initialDelayInMillis`, `maxDelayInMillis`,
/// `multiplier`, `jitterRate`); missing or nonsensical fields keep the
/// defaults.
fn retry_policy_from_json(v: &serde_json::Value) -> BackoffPolicy {
    let d = DEFAULT_REFRESH_RETRY;
    let ms = |k: &str, def: Duration| {
        v[k].as_u64()
            .filter(|&n| n > 0)
            .map_or(def, Duration::from_millis)
    };
    let initial = ms("initialDelayInMillis", d.initial);
    BackoffPolicy {
        initial,
        max: ms("maxDelayInMillis", d.max).max(initial),
        multiplier: v["multiplier"]
            .as_f64()
            .filter(|m| m.is_finite() && *m >= 1.0)
            .unwrap_or(d.multiplier),
        jitter: v["jitterRate"]
            .as_f64()
            .filter(|j| (0.0..=1.0).contains(j))
            .unwrap_or(d.jitter),
    }
}

/// `AccessTokenRefreshException.errorCode = RETRY_REQUIRED`, or the call
/// never got an answer.
fn is_retryable_refresh_error(e: &Error) -> bool {
    match e {
        Error::Request { data, .. } => data["errorCode"].as_str() == Some("RETRY_REQUIRED"),
        Error::Http(_) => true,
        other => other.is_timeout(),
    }
}

fn collect_headers(h: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    h.iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect()
}

/// `https://{host}{path}`; a host given with a scheme
/// (`http://127.0.0.1:8080`) is used as is.
pub(crate) fn endpoint_url(host: &str, path: &str) -> String {
    if host.contains("://") {
        format!("{}{path}", host.trim_end_matches('/'))
    } else {
        format!("https://{host}{path}")
    }
}

#[cfg(test)]
mod tests;
