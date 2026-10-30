//! `PrimarySeamlessLoginService`: pass-through methods generated from the linejs service.
//!
//! Path: `/EXT/auth/feature-user/api/secondary/login/seamless`.

use super::PrimarySeamlessLoginService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl PrimarySeamlessLoginService<'_> {
    /// `permitLogin(request: PermitLoginRequest)`.
    pub async fn permit_login(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("permitLogin", args).await
    }
}
// </generated pass-through>
