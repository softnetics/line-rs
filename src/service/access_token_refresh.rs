//! `AccessTokenRefreshService`: pass-through methods generated from the linejs service.
//!
//! Path: `/EXT/auth/tokenrefresh/v1`.

use super::AccessTokenRefreshService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl AccessTokenRefreshService<'_> {
    /// `refresh(request: RefreshAccessTokenRequest)`.
    pub async fn refresh(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("refresh", args).await
    }

    /// `reportRefreshedAccessToken(request: ReportRefreshedAccessTokenRequest)`.
    pub async fn report_refreshed_access_token(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reportRefreshedAccessToken", args).await
    }
}
// </generated pass-through>
