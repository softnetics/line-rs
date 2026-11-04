//! `AccountAuthFactorQueryService`: read the account's auth factors.
//!
//! Path: `/EXT/auth/feature-user/thrift/account/auth-factor/v1/query`.
//! Recovered by static RE of LINE Android 26.x; unverified against a live
//! server.

use super::AccountAuthFactorQueryService;
use crate::error::Result;
use crate::thrift::Value;

impl AccountAuthFactorQueryService<'_> {
    /// Passkeys registered on the account:
    /// `{passkeys: [{credentialId, passkeyName, registeredAt, authenticatedAt}]}`.
    pub async fn get_passkeys(&self) -> Result<Value> {
        self.call("getPasskeys", crate::tv!({"request": {}})).await
    }
}
