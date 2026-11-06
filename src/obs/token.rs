//! The OBS access token: `acquireEncryptedAccessToken(OBS_GENERAL)`.
//!
//! The Android app (`hk8/c`, `hk8/b`) authenticates OBS uploads with this
//! token rather than the session token. The call returns
//! `"<ttlSec>\x1e<token>"`; the token is kept until
//! `max(30, min(600, ttl / 20))` seconds before it expires. Acquiring is
//! tried up to three times, 200 ms and 400 ms apart; when every try fails
//! the last token is used even if stale. An HTTP 401 from OBS drops it.
//!
//! When there has never been a token (the call keeps failing), uploads fall
//! back to the session token in `x-line-access`, which is what line-rs sent
//! before and which OBS accepts for most accounts.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::base::BaseClient;
use crate::error::{Error, Result};

/// Tries per acquisition (`hk8/c.c`).
pub const OBS_TOKEN_ATTEMPTS: u32 = 3;

/// Where a fresh token comes from; the talk service in the client, a fake
/// in tests.
#[async_trait]
pub(crate) trait ObsTokenSource: Sync {
    /// The raw `"<ttlSec>\x1e<token>"` string.
    async fn acquire(&self) -> Result<String>;
}

#[async_trait]
impl ObsTokenSource for BaseClient {
    async fn acquire(&self) -> Result<String> {
        let res = self
            .talk()
            .acquire_encrypted_access_token(crate::tv!({"featureType": "OBS_GENERAL"}))
            .await?;
        res.as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::internal("ObsError", "no encrypted access token"))
    }
}

/// Parse `"<ttlSec>\x1e<token>"` into the token and how long to keep it.
pub fn parse_obs_token(raw: &str) -> Result<(String, Duration)> {
    let bad = || Error::internal("ObsError", "Unexpected encrypted access token format.");
    let (ttl, token) = raw.split_once('\u{1e}').ok_or_else(bad)?;
    let ttl: u64 = ttl.trim().parse().map_err(|_| bad())?;
    let token = token.split('\u{1e}').next().unwrap_or_default();
    if token.is_empty() {
        return Err(bad());
    }
    let margin = (ttl / 20).clamp(30, 600);
    Ok((
        token.to_string(),
        Duration::from_secs(ttl.saturating_sub(margin)),
    ))
}

#[derive(Debug, Clone)]
struct Cached {
    token: String,
    refresh_at: Instant,
}

/// In-memory OBS token cache, one per client.
#[derive(Debug, Default)]
pub(crate) struct ObsTokenCache {
    inner: Mutex<Option<Cached>>,
}

impl ObsTokenCache {
    /// A token to upload with: the cached one while it is fresh, otherwise
    /// a new one (stale one if acquiring fails). `None` when there has
    /// never been one; the caller then uses the session token.
    pub async fn get(&self, source: &dyn ObsTokenSource) -> Option<String> {
        // Held across the call so concurrent uploads share one acquisition.
        let mut slot = self.inner.lock().await;
        if let Some(c) = slot.as_ref()
            && Instant::now() < c.refresh_at
        {
            return Some(c.token.clone());
        }
        for attempt in 0..OBS_TOKEN_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(200 << (attempt - 1))).await;
            }
            let acquired = source.acquire().await.and_then(|raw| parse_obs_token(&raw));
            match acquired {
                Ok((token, keep)) => {
                    *slot = Some(Cached {
                        token: token.clone(),
                        refresh_at: Instant::now() + keep,
                    });
                    return Some(token);
                }
                Err(e) => {
                    tracing::debug!(attempt, error = %e, "acquireEncryptedAccessToken failed")
                }
            }
        }
        slot.as_ref().map(|c| c.token.clone())
    }

    /// Drop the token (after OBS answered 401).
    pub async fn clear(&self) {
        *self.inner.lock().await = None;
    }

    #[cfg(test)]
    pub async fn set(&self, token: &str, keep: Duration) {
        *self.inner.lock().await = Some(Cached {
            token: token.to_string(),
            refresh_at: Instant::now() + keep,
        });
    }
}
