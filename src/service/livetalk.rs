//! `SquareLiveTalkService`: pass-through methods generated from the linejs service.
//!
//! Path: `/SQLV1`.

use super::SquareLiveTalkService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl SquareLiveTalkService<'_> {
    /// `acceptSpeakers(request: AcceptSpeakersRequest)`.
    pub async fn accept_speakers(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("acceptSpeakers", args).await
    }

    /// `acceptToChangeRole(request: AcceptToChangeRoleRequest)`.
    pub async fn accept_to_change_role(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("acceptToChangeRole", args).await
    }

    /// `acceptToListen(request: AcceptToListenRequest)`.
    pub async fn accept_to_listen(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("acceptToListen", args).await
    }

    /// `acceptToSpeak(request: AcceptToSpeakRequest)`.
    pub async fn accept_to_speak(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("acceptToSpeak", args).await
    }

    /// `acquireLiveTalk(request: AcquireLiveTalkRequest)`.
    pub async fn acquire_live_talk(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("acquireLiveTalk", args).await
    }

    /// `cancelToSpeak(request: CancelToSpeakRequest)`.
    pub async fn cancel_to_speak(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("cancelToSpeak", args).await
    }

    /// `fetchLiveTalkEvents(request: FetchLiveTalkEventsRequest)`.
    pub async fn fetch_live_talk_events(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("fetchLiveTalkEvents", args).await
    }

    /// `findLiveTalkByInvitationTicket(request: FindLiveTalkByInvitationTicketRequest)`.
    pub async fn find_live_talk_by_invitation_ticket(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("findLiveTalkByInvitationTicket", args).await
    }

    /// `forceEndLiveTalk(request: ForceEndLiveTalkRequest)`.
    pub async fn force_end_live_talk(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("forceEndLiveTalk", args).await
    }

    /// `getLiveTalkInfoForNonMember(request: GetLiveTalkInfoForNonMemberRequest)`.
    pub async fn get_live_talk_info_for_non_member(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getLiveTalkInfoForNonMember", args).await
    }

    /// `getLiveTalkInvitationUrl(request: GetLiveTalkInvitationUrlRequest)`.
    pub async fn get_live_talk_invitation_url(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getLiveTalkInvitationUrl", args).await
    }

    /// `getLiveTalkSpeakersForNonMember(request: GetLiveTalkSpeakersForNonMemberRequest)`.
    pub async fn get_live_talk_speakers_for_non_member(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getLiveTalkSpeakersForNonMember", args).await
    }

    /// `getSquareInfoByChatMid(request: GetSquareInfoByChatMidRequest)`.
    pub async fn get_square_info_by_chat_mid(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareInfoByChatMid", args).await
    }

    /// `inviteToChangeRole(request: InviteToChangeRoleRequest)`.
    pub async fn invite_to_change_role(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("inviteToChangeRole", args).await
    }

    /// `inviteToListen(request: InviteToListenRequest)`.
    pub async fn invite_to_listen(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("inviteToListen", args).await
    }

    /// `inviteToLiveTalk(request: InviteToLiveTalkRequest)`.
    pub async fn invite_to_live_talk(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("inviteToLiveTalk", args).await
    }

    /// `inviteToSpeak(request: InviteToSpeakRequest)`.
    pub async fn invite_to_speak(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("inviteToSpeak", args).await
    }

    /// `joinLiveTalk(request: JoinLiveTalkRequest)`.
    pub async fn join_live_talk(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("joinLiveTalk", args).await
    }

    /// `kickOutLiveTalkParticipants(request: KickOutLiveTalkParticipantsRequest)`.
    pub async fn kick_out_live_talk_participants(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("kickOutLiveTalkParticipants", args).await
    }

    /// `rejectSpeakers(request: RejectSpeakersRequest)`.
    pub async fn reject_speakers(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("rejectSpeakers", args).await
    }

    /// `rejectToSpeak(request: RejectToSpeakRequest)`.
    pub async fn reject_to_speak(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("rejectToSpeak", args).await
    }

    /// `reportLiveTalk(request: ReportLiveTalkRequest)`.
    pub async fn report_live_talk(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reportLiveTalk", args).await
    }

    /// `reportLiveTalkSpeaker(request: ReportLiveTalkSpeakerRequest)`.
    pub async fn report_live_talk_speaker(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reportLiveTalkSpeaker", args).await
    }

    /// `requestToListen(request: RequestToListenRequest)`.
    pub async fn request_to_listen(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("requestToListen", args).await
    }

    /// `requestToSpeak(request: RequestToSpeakRequest)`.
    pub async fn request_to_speak(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("requestToSpeak", args).await
    }

    /// `updateLiveTalkAttrs(request: UpdateLiveTalkAttrsRequest)`.
    pub async fn update_live_talk_attrs(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateLiveTalkAttrs", args).await
    }
}
// </generated pass-through>
