//! `OaMembershipService`: pass-through methods generated from the linejs service.
//!
//! Path: `/EXT/bot/oafan`.

use super::OaMembershipService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl OaMembershipService<'_> {
    /// `activateSubscription(request: ActivateSubscriptionRequest)`.
    pub async fn activate_subscription(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("activateSubscription", args).await
    }

    /// `getJoinedMembership(request: GetJoinedMembershipRequest)`.
    pub async fn get_joined_membership(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getJoinedMembership", args).await
    }
}
// </generated pass-through>
