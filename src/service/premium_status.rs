//! `PremiumStatusService`: pass-through methods generated from the linejs service.
//!
//! Path: `/EXT/line-premium/common/thrift/status`.

use super::PremiumStatusService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl PremiumStatusService<'_> {
    /// `getDataRetention(req: fN0_C24473e)`.
    pub async fn get_data_retention(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getDataRetention", args).await
    }

    /// `getIncentiveStatus(req: fN0_C24471c)`.
    pub async fn get_incentive_status(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getIncentiveStatus", args).await
    }

    /// `getPremiumStatus(req: fN0_C24476h)`.
    pub async fn get_premium_status(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPremiumStatus", args).await
    }

    /// `getPremiumStatusForUpgrade(req: fN0_C24475g)`.
    pub async fn get_premium_status_for_upgrade(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPremiumStatusForUpgrade", args).await
    }
}
// </generated pass-through>
