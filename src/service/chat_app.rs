//! `ChatAppService`: pass-through methods generated from the linejs service.
//!
//! Path: `/CAPP1`.

use super::ChatAppService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl ChatAppService<'_> {
    /// `getChatapp(request: GetChatappRequest)`.
    pub async fn get_chatapp(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getChatapp", args).await
    }

    /// `getMyChatapps(request: GetMyChatappsRequest)`.
    pub async fn get_my_chatapps(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getMyChatapps", args).await
    }
}
// </generated pass-through>
