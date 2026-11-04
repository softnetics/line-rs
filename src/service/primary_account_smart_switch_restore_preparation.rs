//! `PrimaryAccountSmartSwitchRestorePreparationService`: pass-through methods generated from the linejs service.
//!
//! Path: `/EXT/auth/feature-user/thrift/account/v1/restore/smart-switch/prepare`.

use super::PrimaryAccountSmartSwitchRestorePreparationService;
use crate::error::Result;
use crate::thrift::Value;

// --- Methods recovered by static RE of LINE Android 26.x (unverified). ---
impl PrimaryAccountSmartSwitchRestorePreparationService<'_> {
    /// `registerSSEncryptionKey(request: RegisterSSEncryptionKeyRequest)`:
    /// registers the key the new device will fetch with `getSSEncryptionKey`.
    pub async fn register_ss_encryption_key(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("registerSSEncryptionKey", args).await
    }
}

// <generated pass-through>
impl PrimaryAccountSmartSwitchRestorePreparationService<'_> {
    /// `createSession(request: h80_C25643c)`.
    pub async fn create_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createSession", args).await
    }
}
// </generated pass-through>
