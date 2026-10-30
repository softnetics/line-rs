//! `SquareBotService`: pass-through methods generated from the linejs service.
//!
//! Path: `/BP1`.

use super::SquareBotService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl SquareBotService<'_> {
    /// `getSquareBot(req: GetSquareBotRequest)`.
    pub async fn get_square_bot(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareBot", args).await
    }
}
// </generated pass-through>
