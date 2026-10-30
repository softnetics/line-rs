//! `PrimaryAccountSmartSwitchRestorePreparationService`: pass-through methods generated from the linejs service.
//!
//! Path: `/EXT/auth/feature-user/thrift/account/v1/restore/smart-switch/prepare`.

use super::PrimaryAccountSmartSwitchRestorePreparationService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl PrimaryAccountSmartSwitchRestorePreparationService<'_> {
    /// `createSession(request: h80_C25643c)`.
    pub async fn create_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createSession", args).await
    }
}
// </generated pass-through>
