//! `ShopAuthService`: pass-through methods generated from the linejs service.
//!
//! Path: `/SHOPA`.

use super::ShopAuthService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl ShopAuthService<'_> {
    /// `establishE2EESession(request: YN0_Ob1_J)`.
    pub async fn establish_e2ee_session(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("establishE2EESession", args).await
    }
}
// </generated pass-through>
