//! `SettingsService`: pass-through methods generated from the linejs service.
//!
//! Path: `/US4`.

use super::SettingsService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl SettingsService<'_> {
    /// `bulkGetSetting(request: BulkGetRequest)`.
    pub async fn bulk_get_setting(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("bulkGetSetting", args).await
    }

    /// `bulkSetSetting(request: s80_t80_c)`.
    pub async fn bulk_set_setting(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("bulkSetSetting", args).await
    }
}
// </generated pass-through>
