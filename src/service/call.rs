//! `CallService`: pass-through methods generated from the linejs service.
//!
//! Path: `/V4`.

use super::CallService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl CallService<'_> {
    /// `acquireCallRoute(to: string, callType: Pb1_D4, fromEnvInfo: map<string, string>)`.
    pub async fn acquire_call_route(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("acquireCallRoute", args).await
    }

    /// `acquireGroupCallRoute` (args struct `acquireGroupCallRoute_args`).
    pub async fn acquire_group_call_route(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("acquireGroupCallRoute", args).await
    }

    /// `acquireOACallRoute(request: AcquireOACallRouteRequest)`.
    pub async fn acquire_oa_call_route(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("acquireOACallRoute", args).await
    }

    /// `acquirePaidCallRoute` (args struct `acquirePaidCallRoute_args`).
    pub async fn acquire_paid_call_route(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("acquirePaidCallRoute", args).await
    }

    /// `createGroupCallUrl(request: CreateGroupCallUrlRequest)`.
    pub async fn create_group_call_url(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createGroupCallUrl", args).await
    }

    /// `deleteGroupCallUrl(request: DeleteGroupCallUrlRequest)`.
    pub async fn delete_group_call_url(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("deleteGroupCallUrl", args).await
    }

    /// `getCallStatus(request: GetCallStatusRequest)`.
    pub async fn get_call_status(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getCallStatus", args).await
    }

    /// `getGroupCall(chatMid: string)`.
    pub async fn get_group_call(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getGroupCall", args).await
    }

    /// `getGroupCallUrlInfo(request: GetGroupCallUrlInfoRequest)`.
    pub async fn get_group_call_url_info(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getGroupCallUrlInfo", args).await
    }

    /// `getGroupCallUrls(request: Pb1_C13042j5)`.
    pub async fn get_group_call_urls(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getGroupCallUrls", args).await
    }

    /// `getPhotoboothBalance(request: Pb1_C13126p5)`.
    pub async fn get_photobooth_balance(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPhotoboothBalance", args).await
    }

    /// `inviteIntoGroupCall(chatMid: string, memberMids: list<string>, mediaType: Pb1_EnumC13237x5)`.
    pub async fn invite_into_group_call(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("inviteIntoGroupCall", args).await
    }

    /// `joinChatByCallUrl(request: JoinChatByCallUrlRequest)`.
    pub async fn join_chat_by_call_url(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("joinChatByCallUrl", args).await
    }

    /// `kickoutFromGroupCall(kickoutFromGroupCallRequest: KickoutFromGroupCallRequest)`.
    pub async fn kickout_from_group_call(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("kickoutFromGroupCall", args).await
    }

    /// `lookupPaidCall(dialedNumber: string, language: string, referer: string)`.
    pub async fn lookup_paid_call(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("lookupPaidCall", args).await
    }

    /// `startPhotobooth(request: StartPhotoboothRequest)`.
    pub async fn start_photobooth(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("startPhotobooth", args).await
    }

    /// `updateGroupCallUrl(request: UpdateGroupCallUrlRequest)`.
    pub async fn update_group_call_url(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateGroupCallUrl", args).await
    }

    /// `usePhotoboothTicket(request: UsePhotoboothTicketRequest)`.
    pub async fn use_photobooth_ticket(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("usePhotoboothTicket", args).await
    }
}
// </generated pass-through>
