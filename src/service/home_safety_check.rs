//! `HomeSafetyCheckService`: pass-through methods generated from the linejs service.
//!
//! Path: `/EXT/home/safety-check/safety-check`.

use super::HomeSafetyCheckService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl HomeSafetyCheckService<'_> {
    /// `deleteSafetyStatus(req: DeleteSafetyStatusRequest)`.
    pub async fn delete_safety_status(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("deleteSafetyStatus", args).await
    }

    /// `getDisasterCases(req: vh_C37633d)`.
    pub async fn get_disaster_cases(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getDisasterCases", args).await
    }

    /// `updateSafetyStatus(req: UpdateSafetyStatusRequest)`.
    pub async fn update_safety_status(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateSafetyStatus", args).await
    }
}
// </generated pass-through>
