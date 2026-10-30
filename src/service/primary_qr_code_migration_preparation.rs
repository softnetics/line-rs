//! `PrimaryQrCodeMigrationPreparationService`: pass-through methods generated from the linejs service.
//!
//! Path: `/EXT/auth/feature-user/api/primary/mig/qr/prepare`.

use super::PrimaryQrCodeMigrationPreparationService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl PrimaryQrCodeMigrationPreparationService<'_> {
    /// `createSession(request: h80_C25643c)`.
    pub async fn create_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createSession", args).await
    }

    /// `sendEncryptedE2EEKey(request: SendEncryptedE2EEKeyRequest)`.
    pub async fn send_encrypted_e2ee_key(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("sendEncryptedE2EEKey", args).await
    }
}
// </generated pass-through>
