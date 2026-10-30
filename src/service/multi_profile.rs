//! `MultiProfileService`: pass-through methods generated from the linejs service.
//!
//! Path: `/PFS4`.

use super::MultiProfileService;
use crate::error::Result;
use crate::thrift::Value;

impl MultiProfileService<'_> {
    /// Multiple profiles (multi-profile) for the account.
    /// `sync_reason` is a `Pb1_V7` enum name (e.g. `"INITIALIZATION"`).
    ///
    /// Recovered by static RE of LINE macOS 26.5.0; unverified against a live
    /// server.
    pub async fn get_profiles(&self, sync_reason: impl Into<Value>) -> Result<Value> {
        let mut req = Value::object();
        req.insert("syncReason", sync_reason.into());
        let mut args = Value::object();
        args.insert("request", req);
        self.call("getProfiles", args).await
    }
}

// <generated pass-through>
impl MultiProfileService<'_> {
    /// `createMultiProfile(request: CreateMultiProfileRequest)`.
    pub async fn create_multi_profile(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createMultiProfile", args).await
    }

    /// `deleteMultiProfile(request: DeleteMultiProfileRequest)`.
    pub async fn delete_multi_profile(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("deleteMultiProfile", args).await
    }

    /// `getMappedProfileIds(request: GetMappedProfileIdsRequest)`.
    pub async fn get_mapped_profile_ids(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getMappedProfileIds", args).await
    }

    /// `getProfile(syncReason: Pb1_V7)`.
    pub async fn get_profile(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getProfile", args).await
    }

    /// `getUsersMappedByProfile(request: GetUsersMappedByProfileRequest)`.
    pub async fn get_users_mapped_by_profile(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getUsersMappedByProfile", args).await
    }

    /// `mapProfileToUsers(request: MapProfileToUsersRequest)`.
    pub async fn map_profile_to_users(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("mapProfileToUsers", args).await
    }

    /// `updateProfileAttributes(reqSeq: i32, request: UpdateProfileAttributesRequest)`.
    pub async fn update_profile_attributes(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateProfileAttributes", args).await
    }
}
// </generated pass-through>
