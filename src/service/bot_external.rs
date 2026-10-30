//! `BotExternalService`: pass-through methods generated from the linejs service.
//!
//! Path: `/BOTE`.

use super::BotExternalService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl BotExternalService<'_> {
    /// `notifyChatAdEntry(request: NotifyChatAdEntryRequest)`.
    pub async fn notify_chat_ad_entry(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("notifyChatAdEntry", args).await
    }

    /// `notifyOATalkroomEvents(request: NotifyOATalkroomEventsRequest)`.
    pub async fn notify_oa_talkroom_events(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("notifyOATalkroomEvents", args).await
    }
}
// </generated pass-through>
