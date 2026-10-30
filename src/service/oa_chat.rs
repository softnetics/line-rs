//! `OaChatService`: pass-through methods generated from the linejs service.
//!
//! Path: `/ext/oachat/api/`.

use super::OaChatService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl OaChatService<'_> {
    /// `getCallStatus(request: GetCallStatusRequest)`.
    pub async fn get_call_status(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getCallStatus", args).await
    }
}
// </generated pass-through>
