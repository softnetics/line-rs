//! `SquareService` (`/SQ1`): OpenChat squares, square chats, members and events.

use indexmap::IndexMap;

use super::SquareService;
use crate::error::{Error, Result};
use crate::thrift::Value;

/// Cover image used by [`SquareService::create_square`] when none is given.
pub const DEFAULT_SQUARE_COVER_IMAGE_OBS_HASH: &str =
    "0h6tJfahRYaVt3H0eLAsAWDFheczgHd3wTCTx2eApNKSoefHNVGRdwfgxbdgUMLi8MSngnPFMeNmpbLi8MSngnPFMeNmpbLi8MSngnPQ";

/// Options for [`SquareService::get_joined_squares`].
#[derive(Debug, Clone, Default)]
pub struct GetJoinedSquaresOptions {
    /// Default 100.
    pub limit: Option<i32>,
    pub continuation_token: Option<String>,
}

/// Options for [`SquareService::fetch_my_events`].
#[derive(Debug, Clone, Default)]
pub struct FetchMyEventsOptions {
    pub sync_token: Option<String>,
    pub continuation_token: Option<String>,
    /// Default 100.
    pub limit: Option<i32>,
    pub subscription_id: Option<i64>,
}

/// Options for [`SquareService::fetch_square_chat_events`].
#[derive(Debug, Clone, Default)]
pub struct FetchSquareChatEventsOptions {
    pub square_chat_mid: String,
    pub thread_mid: Option<String>,
    pub sync_token: Option<String>,
    /// Default 100.
    pub limit: Option<i32>,
    /// `FetchDirection` variant name or number.
    pub direction: Option<Value>,
}

/// Options for [`SquareService::send_message`].
#[derive(Debug, Clone, Default)]
pub struct SquareSendMessageOptions {
    pub square_chat_mid: String,
    pub text: Option<String>,
    /// `ContentType` variant name or number (default 0 / `NONE`).
    pub content_type: Option<Value>,
    pub content_metadata: IndexMap<String, String>,
    /// Reply target.
    pub related_message_id: Option<String>,
    /// A `Location` struct value.
    pub location: Option<Value>,
}

impl SquareSendMessageOptions {
    /// A plain text message.
    pub fn text(square_chat_mid: impl Into<String>, text: impl Into<String>) -> Self {
        Self { square_chat_mid: square_chat_mid.into(), text: Some(text.into()), ..Default::default() }
    }
}

/// Options for [`SquareService::create_square`].
#[derive(Debug, Clone, Default)]
pub struct CreateSquareOptions {
    pub square_name: String,
    /// The creator's display name in the square.
    pub display_name: String,
    pub profile_image_obs_hash: Option<String>,
    pub description: Option<String>,
    pub searchable: Option<bool>,
    /// `SquareJoinMethodType` variant name or number.
    pub square_join_method_type: Option<Value>,
}

/// Options for [`SquareService::join_square`].
#[derive(Debug, Clone, Default)]
pub struct JoinSquareOptions {
    pub square_mid: String,
    pub display_name: String,
    pub able_to_receive_message: Option<bool>,
    pub pass_code: Option<String>,
    pub join_message: Option<String>,
}

/// A square message reference for [`SquareService::destroy_message`] and
/// [`SquareService::unsend_message`].
#[derive(Debug, Clone, Default)]
pub struct SquareMessageRef {
    pub message_id: String,
    pub square_chat_mid: String,
    pub thread_mid: Option<String>,
}

/// Options for [`SquareService::get_square_chat_members`] and
/// [`SquareService::search_square_chat_members`].
#[derive(Debug, Clone, Default)]
pub struct SquareChatMembersOptions {
    pub square_chat_mid: String,
    pub continuation_token: Option<String>,
    /// Default 100.
    pub limit: Option<i32>,
    /// `SquareChatMemberSearchOption` (search only).
    pub search_option: Option<Value>,
}

/// Options for [`SquareService::create_square_chat_announcement`].
#[derive(Debug, Clone, Default)]
pub struct CreateSquareChatAnnouncementOptions {
    pub square_chat_mid: String,
    pub sender_mid: String,
    pub message_id: String,
    pub text: String,
    pub created_at: i64,
}

fn request(req: Value) -> Value {
    let mut v = Value::object();
    v.insert("request", req);
    v
}

pub(crate) fn send_message_args(req_seq: i64, o: &SquareSendMessageOptions) -> Value {
    let mut message = crate::tv!({"to": o.square_chat_mid});
    message.insert("text", o.text.clone());
    message.insert("contentType", o.content_type.clone().unwrap_or(Value::Int(0)));
    message.insert(
        "contentMetadata",
        Value::Struct(o.content_metadata.iter().map(|(k, v)| (k.clone(), Value::from(v))).collect()),
    );
    message.insert("location", o.location.clone());
    if let Some(id) = &o.related_message_id {
        message.insert("relatedMessageId", id);
        message.insert("relatedMessageServiceCode", "SQUARE");
        message.insert("messageRelationType", "REPLY");
    }
    let mut square_message = crate::tv!({"squareMessageRevision": 4});
    square_message.insert("message", message);
    let mut req = crate::tv!({"reqSeq": req_seq, "squareChatMid": o.square_chat_mid});
    req.insert("squareMessage", square_message);
    request(req)
}

pub(crate) fn create_square_args(req_seq: i64, o: &CreateSquareOptions) -> Value {
    let mut join_method = Value::object();
    join_method.insert("type", o.square_join_method_type.clone());
    let mut square = crate::tv!({
        "name": o.square_name,
        "profileImageObsHash": o.profile_image_obs_hash.as_deref().unwrap_or(DEFAULT_SQUARE_COVER_IMAGE_OBS_HASH),
        "type": "OPEN",
        "categoryId": 1,
        "revision": 0,
        "ableToUseInvitationTicket": true,
        "adultOnly": "NONE",
        "svcTags": [],
    });
    square.insert("desc", o.description.clone());
    square.insert("searchable", o.searchable);
    square.insert("joinMethod", join_method);
    let mut req = crate::tv!({
        "reqSeq": req_seq,
        "creator": {"displayName": o.display_name, "ableToReceiveMessage": true, "revision": 0},
    });
    req.insert("square", square);
    request(req)
}

pub(crate) fn join_square_args(o: &JoinSquareOptions) -> Value {
    let mut approval = Value::object();
    approval.insert("message", o.join_message.clone());
    let mut code = Value::object();
    code.insert("code", o.pass_code.clone());
    let mut join_value = Value::object();
    join_value.insert("approvalValue", approval);
    join_value.insert("codeValue", code);
    let mut member = crate::tv!({"squareMid": o.square_mid, "displayName": o.display_name, "revision": 0});
    member.insert("ableToReceiveMessage", o.able_to_receive_message);
    let mut req = crate::tv!({"squareMid": o.square_mid});
    req.insert("joinValue", join_value);
    req.insert("member", member);
    request(req)
}

fn message_ref_args(o: &SquareMessageRef) -> Value {
    let mut req = crate::tv!({"messageId": o.message_id, "squareChatMid": o.square_chat_mid});
    req.insert("threadMid", o.thread_mid.clone());
    request(req)
}

fn chat_members_args(o: &SquareChatMembersOptions, search: bool) -> Value {
    let mut req = crate::tv!({"squareChatMid": o.square_chat_mid, "limit": o.limit.unwrap_or(100)});
    req.insert("continuationToken", o.continuation_token.clone());
    if search {
        req.insert("searchOption", o.search_option.clone());
    }
    request(req)
}

impl SquareService<'_> {
    /// Squares the user has joined.
    pub async fn get_joined_squares(&self, options: GetJoinedSquaresOptions) -> Result<Value> {
        let mut req = crate::tv!({"limit": options.limit.unwrap_or(100)});
        req.insert("continuationToken", options.continuation_token);
        self.call("getJoinedSquares", request(req)).await
    }

    /// Poll the user's square events.
    pub async fn fetch_my_events(&self, options: FetchMyEventsOptions) -> Result<Value> {
        let mut req = crate::tv!({"limit": options.limit.unwrap_or(100)});
        req.insert("syncToken", options.sync_token);
        req.insert("continuationToken", options.continuation_token);
        req.insert("subscriptionId", options.subscription_id);
        self.call("fetchMyEvents", request(req)).await
    }

    /// Poll events of one square chat.
    pub async fn fetch_square_chat_events(&self, options: FetchSquareChatEventsOptions) -> Result<Value> {
        let mut req = crate::tv!({"squareChatMid": options.square_chat_mid, "limit": options.limit.unwrap_or(100)});
        req.insert("threadMid", options.thread_mid);
        req.insert("syncToken", options.sync_token);
        req.insert("direction", options.direction);
        self.call("fetchSquareChatEvents", request(req)).await
    }

    /// Send a message to a square chat (reqseq bucket `sq`).
    pub async fn send_message(&self, options: SquareSendMessageOptions) -> Result<Value> {
        let seq = self.client.get_reqseq("sq").await?;
        self.call("sendMessage", send_message_args(seq, &options)).await
    }

    /// A square by mid.
    pub async fn get_square(&self, square_mid: &str) -> Result<Value> {
        self.call("getSquare", crate::tv!({"request": {"mid": square_mid}})).await
    }

    /// Create an open square (reqseq bucket `sq`).
    pub async fn create_square(&self, options: CreateSquareOptions) -> Result<Value> {
        let seq = self.client.get_reqseq("sq").await?;
        self.call("createSquare", create_square_args(seq, &options)).await
    }

    /// Join a square.
    pub async fn join_square(&self, options: JoinSquareOptions) -> Result<Value> {
        self.call("joinSquare", join_square_args(&options)).await
    }

    /// Destroy (delete for everyone) a square message.
    pub async fn destroy_message(&self, message: SquareMessageRef) -> Result<Value> {
        self.call("destroyMessage", message_ref_args(&message)).await
    }

    /// Unsend a square message.
    pub async fn unsend_message(&self, message: SquareMessageRef) -> Result<Value> {
        self.call("unsendMessage", message_ref_args(&message)).await
    }

    /// Members of a square chat (default limit 100).
    pub async fn get_square_chat_members(&self, options: SquareChatMembersOptions) -> Result<Value> {
        self.call("getSquareChatMembers", chat_members_args(&options, false)).await
    }

    /// Search members of a square chat (default limit 100).
    pub async fn search_square_chat_members(&self, options: SquareChatMembersOptions) -> Result<Value> {
        self.call("searchSquareChatMembers", chat_members_args(&options, true)).await
    }

    /// Pin a text message as a square chat announcement.
    pub async fn create_square_chat_announcement(&self, o: CreateSquareChatAnnouncementOptions) -> Result<Value> {
        let args = crate::tv!({"createSquareChatAnnouncementRequest": {
            "reqSeq": 0,
            "squareChatMid": o.square_chat_mid,
            "squareChatAnnouncement": {
                "announcementSeq": 0,
                "contents": {"textMessageAnnouncementContents": {
                    "senderSquareMemberMid": o.sender_mid,
                    "messageId": o.message_id,
                    "text": o.text,
                }},
                "createdAt": o.created_at,
                "type": 0,
            },
        }});
        self.call("createSquareChatAnnouncement", args).await
    }

    /// A square chat by mid.
    pub async fn get_square_chat(&self, square_chat_mid: &str) -> Result<Value> {
        self.call("getSquareChat", crate::tv!({"request": {"squareChatMid": square_chat_mid}})).await
    }

    /// A square member by mid.
    pub async fn get_square_member(&self, square_member_mid: &str) -> Result<Value> {
        self.call("getSquareMember", crate::tv!({"request": {"squareMemberMid": square_member_mid}})).await
    }

    /// Kick a member (`p...` mid) out of a square.
    pub async fn delete_other_from_square(&self, square_member_mid: &str) -> Result<Value> {
        if !square_member_mid.starts_with('p') {
            return Err(Error::internal("TypeError", "Invalid value: squareMemberMid"));
        }
        let res = self.get_square_member(square_member_mid).await?;
        let mut member = res["squareMember"].clone();
        member.insert("membershipState", "KICK_OUT");
        let mut req = crate::tv!({"updatedAttrs": [5], "updatedPreferenceAttrs": []});
        req.insert("squareMember", member);
        self.update_square_member(request(req)).await
    }
}

// <generated pass-through>
impl SquareService<'_> {
    /// `approveSquareMembers(request: ApproveSquareMembersRequest)`.
    pub async fn approve_square_members(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("approveSquareMembers", args).await
    }

    /// `checkJoinCode(request: CheckJoinCodeRequest)`.
    pub async fn check_join_code(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("checkJoinCode", args).await
    }

    /// `createSquareChat(request: CreateSquareChatRequest)`.
    pub async fn create_square_chat(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createSquareChat", args).await
    }

    /// `deleteSquare(request: DeleteSquareRequest)`.
    pub async fn delete_square(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("deleteSquare", args).await
    }

    /// `deleteSquareChat(request: DeleteSquareChatRequest)`.
    pub async fn delete_square_chat(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("deleteSquareChat", args).await
    }

    /// `deleteSquareChatAnnouncement(deleteSquareChatAnnouncementRequest: DeleteSquareChatAnnouncementRequest)`.
    pub async fn delete_square_chat_announcement(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("deleteSquareChatAnnouncement", args).await
    }

    /// `destroyMessages(request: DestroyMessagesRequest)`.
    pub async fn destroy_messages(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("destroyMessages", args).await
    }

    /// `findSquareByEmid(findSquareByEmidRequest: FindSquareByEmidRequest)`.
    pub async fn find_square_by_emid(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("findSquareByEmid", args).await
    }

    /// `findSquareByInvitationTicket(request: FindSquareByInvitationTicketRequest)`.
    pub async fn find_square_by_invitation_ticket(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("findSquareByInvitationTicket", args).await
    }

    /// `findSquareByInvitationTicketV2(request: FindSquareByInvitationTicketV2Request)`.
    pub async fn find_square_by_invitation_ticket_v2(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("findSquareByInvitationTicketV2", args).await
    }

    /// `getCategories(request: GetSquareCategoriesRequest)`.
    pub async fn get_categories(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getCategories", args).await
    }

    /// `getGoogleAdOptions(request: GetGoogleAdOptionsRequest)`.
    pub async fn get_google_ad_options(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getGoogleAdOptions", args).await
    }

    /// `getInvitationTicketUrl(request: GetInvitationTicketUrlRequest)`.
    pub async fn get_invitation_ticket_url(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getInvitationTicketUrl", args).await
    }

    /// `getJoinableSquareChats(request: GetJoinableSquareChatsRequest)`.
    pub async fn get_joinable_square_chats(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getJoinableSquareChats", args).await
    }

    /// `getJoinedSquareChats(request: GetJoinedSquareChatsRequest)`.
    pub async fn get_joined_square_chats(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getJoinedSquareChats", args).await
    }

    /// `getNoteStatus(request: GetNoteStatusRequest)`.
    pub async fn get_note_status(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getNoteStatus", args).await
    }

    /// `getPopularKeywords(request: GetPopularKeywordsRequest)`.
    pub async fn get_popular_keywords(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getPopularKeywords", args).await
    }

    /// `getSquareAuthorities(request: GetSquareAuthoritiesRequest)`.
    pub async fn get_square_authorities(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareAuthorities", args).await
    }

    /// `getSquareAuthority(request: GetSquareAuthorityRequest)`.
    pub async fn get_square_authority(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareAuthority", args).await
    }

    /// `getSquareChatAnnouncements(getSquareChatAnnouncementsRequest: GetSquareChatAnnouncementsRequest)`.
    pub async fn get_square_chat_announcements(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareChatAnnouncements", args).await
    }

    /// `getSquareChatEmid(request: GetSquareChatEmidRequest)`.
    pub async fn get_square_chat_emid(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareChatEmid", args).await
    }

    /// `getSquareChatFeatureSet(request: GetSquareChatFeatureSetRequest)`.
    pub async fn get_square_chat_feature_set(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareChatFeatureSet", args).await
    }

    /// `getSquareChatMember(request: GetSquareChatMemberRequest)`.
    pub async fn get_square_chat_member(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareChatMember", args).await
    }

    /// `getSquareChatStatus(request: GetSquareChatStatusRequest)`.
    pub async fn get_square_chat_status(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareChatStatus", args).await
    }

    /// `getSquareEmid(request: GetSquareEmidRequest)`.
    pub async fn get_square_emid(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareEmid", args).await
    }

    /// `getSquareFeatureSet(request: GetSquareFeatureSetRequest)`.
    pub async fn get_square_feature_set(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareFeatureSet", args).await
    }

    /// `getSquareMemberRelation(request: GetSquareMemberRelationRequest)`.
    pub async fn get_square_member_relation(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareMemberRelation", args).await
    }

    /// `getSquareMemberRelations(request: GetSquareMemberRelationsRequest)`.
    pub async fn get_square_member_relations(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareMemberRelations", args).await
    }

    /// `getSquareMembers(request: GetSquareMembersRequest)`.
    pub async fn get_square_members(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareMembers", args).await
    }

    /// `getSquareMembersBySquare(request: GetSquareMembersBySquareRequest)`.
    pub async fn get_square_members_by_square(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareMembersBySquare", args).await
    }

    /// `getSquareStatus(request: GetSquareStatusRequest)`.
    pub async fn get_square_status(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareStatus", args).await
    }

    /// `getSquareThread(request: GetSquareThreadRequest)`.
    pub async fn get_square_thread(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareThread", args).await
    }

    /// `getSquareThreadMid(request: GetSquareThreadMidRequest)`.
    pub async fn get_square_thread_mid(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getSquareThreadMid", args).await
    }

    /// `getUserSettings(request: GetUserSettingsRequest)`.
    pub async fn get_user_settings(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getUserSettings", args).await
    }

    /// `hideSquareMemberContents(request: HideSquareMemberContentsRequest)`.
    pub async fn hide_square_member_contents(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("hideSquareMemberContents", args).await
    }

    /// `inviteIntoSquareChat(request: InviteIntoSquareChatRequest)`.
    pub async fn invite_into_square_chat(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("inviteIntoSquareChat", args).await
    }

    /// `inviteToSquare(request: InviteToSquareRequest)`.
    pub async fn invite_to_square(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("inviteToSquare", args).await
    }

    /// `joinSquareChat(request: JoinSquareChatRequest)`.
    pub async fn join_square_chat(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("joinSquareChat", args).await
    }

    /// `joinSquareThread(request: JoinSquareThreadRequest)`.
    pub async fn join_square_thread(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("joinSquareThread", args).await
    }

    /// `leaveSquare(request: LeaveSquareRequest)`.
    pub async fn leave_square(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("leaveSquare", args).await
    }

    /// `leaveSquareChat(request: LeaveSquareChatRequest)`.
    pub async fn leave_square_chat(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("leaveSquareChat", args).await
    }

    /// `leaveSquareThread(request: LeaveSquareThreadRequest)`.
    pub async fn leave_square_thread(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("leaveSquareThread", args).await
    }

    /// `manualRepair(request: ManualRepairRequest)`.
    pub async fn manual_repair(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("manualRepair", args).await
    }

    /// `markAsRead(request: MarkAsReadRequest)`.
    pub async fn mark_as_read(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("markAsRead", args).await
    }

    /// `markChatsAsRead(request: MarkChatsAsReadRequest)`.
    pub async fn mark_chats_as_read(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("markChatsAsRead", args).await
    }

    /// `markThreadsAsRead(request: MarkThreadsAsReadRequest)`.
    pub async fn mark_threads_as_read(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("markThreadsAsRead", args).await
    }

    /// `getMessageReactions(request: GetMessageReactionsRequest)`. Lists the
    /// reactions on a square message. Verified present in the IDL; confirmed by
    /// static RE of LINE macOS 26.5.0.
    pub async fn get_message_reactions(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getMessageReactions", args).await
    }

    /// `reactToMessage(request: ReactToMessageRequest)`.
    pub async fn react_to_message(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reactToMessage", args).await
    }

    /// `refreshSubscriptions(request: RefreshSubscriptionsRequest)`.
    pub async fn refresh_subscriptions(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("refreshSubscriptions", args).await
    }

    /// `rejectSquareMembers(request: RejectSquareMembersRequest)`.
    pub async fn reject_square_members(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("rejectSquareMembers", args).await
    }

    /// `removeSubscriptions(request: RemoveSubscriptionsRequest)`.
    pub async fn remove_subscriptions(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("removeSubscriptions", args).await
    }

    /// `reportMessageSummary(request: ReportMessageSummaryRequest)`.
    pub async fn report_message_summary(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reportMessageSummary", args).await
    }

    /// `reportSquare(request: ReportSquareRequest)`.
    pub async fn report_square(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reportSquare", args).await
    }

    /// `reportSquareChat(request: ReportSquareChatRequest)`.
    pub async fn report_square_chat(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reportSquareChat", args).await
    }

    /// `reportSquareMember(request: ReportSquareMemberRequest)`.
    pub async fn report_square_member(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reportSquareMember", args).await
    }

    /// `reportSquareMessage(request: ReportSquareMessageRequest)`.
    pub async fn report_square_message(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("reportSquareMessage", args).await
    }

    /// `searchSquareMembers(request: SearchSquareMembersRequest)`.
    pub async fn search_square_members(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("searchSquareMembers", args).await
    }

    /// `searchSquares(request: SearchSquaresRequest)`.
    pub async fn search_squares(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("searchSquares", args).await
    }

    /// `sendSquareThreadMessage(request: SendSquareThreadMessageRequest)`.
    pub async fn send_square_thread_message(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("sendSquareThreadMessage", args).await
    }

    /// `syncSquareMembers(request: SyncSquareMembersRequest)`.
    pub async fn sync_square_members(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("syncSquareMembers", args).await
    }

    /// `unhideSquareMemberContents(request: UnhideSquareMemberContentsRequest)`.
    pub async fn unhide_square_member_contents(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("unhideSquareMemberContents", args).await
    }

    /// `updateSquare(request: UpdateSquareRequest)`.
    pub async fn update_square(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateSquare", args).await
    }

    /// `updateSquareAuthority(request: UpdateSquareAuthorityRequest)`.
    pub async fn update_square_authority(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateSquareAuthority", args).await
    }

    /// `updateSquareChat(request: UpdateSquareChatRequest)`.
    pub async fn update_square_chat(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateSquareChat", args).await
    }

    /// `updateSquareChatMember(request: UpdateSquareChatMemberRequest)`.
    pub async fn update_square_chat_member(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateSquareChatMember", args).await
    }

    /// `updateSquareFeatureSet(request: UpdateSquareFeatureSetRequest)`.
    pub async fn update_square_feature_set(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateSquareFeatureSet", args).await
    }

    /// `updateSquareMember(request: UpdateSquareMemberRequest)`.
    pub async fn update_square_member(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateSquareMember", args).await
    }

    /// `updateSquareMemberRelation(request: UpdateSquareMemberRelationRequest)`.
    pub async fn update_square_member_relation(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateSquareMemberRelation", args).await
    }

    /// `updateSquareMembers(request: UpdateSquareMembersRequest)`.
    pub async fn update_square_members(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateSquareMembers", args).await
    }

    /// `updateUserSettings(request: UpdateUserSettingsRequest)`.
    pub async fn update_user_settings(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("updateUserSettings", args).await
    }
}
// </generated pass-through>

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::test_util::{encode, path, s};
    use crate::thrift::TValue;

    #[test]
    fn send_message_envelope() {
        let mut o = SquareSendMessageOptions::text("m1", "hi");
        o.related_message_id = Some("9".into());
        let f = encode("SquareService_sendMessage_args", &send_message_args(3, &o));
        assert_eq!(path(&f, &[1, 1]), Some(&TValue::I32(3)));
        assert_eq!(path(&f, &[1, 2]), Some(&s("m1")));
        assert_eq!(path(&f, &[1, 3, 4]), Some(&TValue::I64(4)));
        assert_eq!(path(&f, &[1, 3, 1, 2]), Some(&s("m1")));
        assert_eq!(path(&f, &[1, 3, 1, 10]), Some(&s("hi")));
        assert_eq!(path(&f, &[1, 3, 1, 21]), Some(&s("9")));
        assert_eq!(path(&f, &[1, 3, 1, 22]), Some(&TValue::I32(3)));
        assert_eq!(path(&f, &[1, 3, 1, 24]), Some(&TValue::I32(2)));
    }

    #[test]
    fn create_and_join_encode() {
        let o = CreateSquareOptions { square_name: "n".into(), display_name: "me".into(), ..Default::default() };
        let f = encode("SquareService_createSquare_args", &create_square_args(0, &o));
        assert!(path(&f, &[1]).is_some());
        let o = JoinSquareOptions { square_mid: "s1".into(), display_name: "me".into(), ..Default::default() };
        let f = encode("SquareService_joinSquare_args", &join_square_args(&o));
        assert!(path(&f, &[1]).is_some());
    }
}
