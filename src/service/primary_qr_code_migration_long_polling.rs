//! `PrimaryQrCodeMigrationLongPollingService`: pass-through methods generated from the linejs service.
//!
//! Path: `/EXT/auth/feature-user/lp/api/primary/mig/qr`.

use super::PrimaryQrCodeMigrationLongPollingService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl PrimaryQrCodeMigrationLongPollingService<'_> {
    /// `checkIfEncryptedE2EEKeyReceived(request: CheckIfEncryptedE2EEKeyReceivedRequest)`.
    pub async fn check_if_encrypted_e2ee_key_received(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("checkIfEncryptedE2EEKeyReceived", args).await
    }
}
// </generated pass-through>
