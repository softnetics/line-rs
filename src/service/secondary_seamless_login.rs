//! `SecondarySeamlessLoginService`: pass-through methods generated from the linejs service.
//!
//! Path: `/ext/auth/feature-guest/api/secondary/login/seamless`.

use super::SecondarySeamlessLoginService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl SecondarySeamlessLoginService<'_> {
    /// `createSession(request: h80_C25643c)`.
    pub async fn create_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createSession", args).await
    }
}
// </generated pass-through>
