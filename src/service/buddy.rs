//! `BuddyService`: pass-through methods generated from the linejs service.
//!
//! Path: `/BUDDY4`.

use super::BuddyService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl BuddyService<'_> {
    /// `getBuddyDetail(buddyMid: string)`.
    pub async fn get_buddy_detail(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getBuddyDetail", args).await
    }

    /// `getBuddyDetailWithPersonal(buddyMid: string, attributeSet: set<Pb1_D0>)`.
    pub async fn get_buddy_detail_with_personal(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getBuddyDetailWithPersonal", args).await
    }

    /// `getNewlyReleasedBuddyIds(country: string)`.
    pub async fn get_newly_released_buddy_ids(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getNewlyReleasedBuddyIds", args).await
    }

    /// `getPromotedBuddyContacts(language: string, country: string)`.
    pub async fn get_promoted_buddy_contacts(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPromotedBuddyContacts", args).await
    }
}
// </generated pass-through>
