//! `SecondaryPwlessLoginService`: pass-through methods generated from the linejs service.
//!
//! Path: `/acct/lgn/secpwless/v1`.

use super::SecondaryPwlessLoginService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl SecondaryPwlessLoginService<'_> {
    /// `createSession(request: h80_C25643c)`.
    pub async fn create_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createSession", args).await
    }
}
// </generated pass-through>
