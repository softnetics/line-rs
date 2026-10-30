//! `E2EEKeyBackupService`: pass-through methods generated from the linejs service.
//!
//! Path: `/EKBS4`.

use super::E2EEKeyBackupService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl E2EEKeyBackupService<'_> {
    /// `getE2EEKeyBackupCertificates(request: Pb1_W4)`.
    pub async fn get_e2ee_key_backup_certificates(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getE2EEKeyBackupCertificates", args).await
    }

    /// `getE2EEKeyBackupInfo(request: Pb1_Y4)`.
    pub async fn get_e2ee_key_backup_info(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getE2EEKeyBackupInfo", args).await
    }
}
// </generated pass-through>
