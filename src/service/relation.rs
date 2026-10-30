//! `RelationService` (`/RE4`): contacts and friend requests.

use super::RelationService;
use crate::error::Result;
use crate::request::Parse;
use crate::thrift::{Schema, TType, TValue, Value, encode_struct};

/// Options for [`RelationService::add_friend_by_mid`].
#[derive(Debug, Clone)]
pub struct AddFriendByMidOptions {
    pub mid: String,
    /// JSON provenance, e.g. `{"screen":"groupMemberList","spec":"native"}`.
    pub reference: Option<String>,
    /// Field id of the tracking meta union (2 by phone, 3 by search id,
    /// 5 group member list; default 5).
    pub tracking_meta_type: i16,
    pub tracking_meta_hint: Option<String>,
}

impl AddFriendByMidOptions {
    pub fn new(mid: impl Into<String>) -> Self {
        Self { mid: mid.into(), reference: None, tracking_meta_type: 5, tracking_meta_hint: None }
    }
}

fn opt_str(id: i16, v: Option<&str>) -> Option<(i16, TValue)> {
    v.map(|s| (id, TValue::string(s)))
}

/// Raw `addFriendByMid` args (linejs sends a hand-built `NestedArray`).
pub(crate) fn add_friend_by_mid_args(seq: i64, o: &AddFriendByMidOptions) -> Vec<(i16, TValue)> {
    let meta = TValue::Struct(opt_str(1, o.tracking_meta_hint.as_deref()).into_iter().collect());
    let tracking = TValue::Struct(
        opt_str(1, o.reference.as_deref())
            .into_iter()
            .chain([(2, TValue::Struct(vec![(o.tracking_meta_type, meta)]))])
            .collect(),
    );
    vec![(1, TValue::Struct(vec![(1, TValue::I32(seq as i32)), (2, TValue::string(o.mid.as_str())), (3, tracking)]))]
}

pub(crate) fn get_contacts_v3_args(mids: &[String], check_user_status_strictly: Option<bool>) -> Value {
    let targets: Vec<Value> = mids.iter().map(|m| crate::tv!({"targetUserMid": m})).collect();
    let mut req = crate::tv!({"syncReason": "AUTO_REPAIR"});
    req.insert("targetUsers", targets);
    req.insert("checkUserStatusStrictly", check_user_status_strictly);
    let mut args = Value::object();
    args.insert("request", req);
    args
}

impl RelationService<'_> {
    /// Contacts by mids (`syncReason = AUTO_REPAIR`).
    pub async fn get_contacts_v3(&self, mids: &[String], check_user_status_strictly: Option<bool>) -> Result<Value> {
        self.call("getContactsV3", get_contacts_v3_args(mids, check_user_status_strictly)).await
    }

    /// Add a friend by mid with tracking metadata.
    pub async fn add_friend_by_mid(&self, options: AddFriendByMidOptions) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await?;
        self.request("addFriendByMid", add_friend_by_mid_args(seq, &options), Parse::Result).await
    }

    /// Look up a contact by LINE ID (search id) or ticket.
    pub async fn find_contact_by_search_id_or_ticket_v3(&self, search_id: &str) -> Result<Value> {
        let args = vec![(1, TValue::Struct(vec![(1, TValue::Struct(vec![(1, TValue::string(search_id))]))]))];
        self.request("findContactBySearchIdOrTicketV3", args, Parse::Struct("Contact")).await
    }

    /// Add a friend by LINE ID.
    pub async fn add_friend_by_user_id(&self, user_id: &str) -> Result<Value> {
        let contact = self.find_contact_by_search_id_or_ticket_v3(user_id).await?;
        self.add_friend_by_mid(AddFriendByMidOptions {
            mid: contact["mid"].as_str().unwrap_or_default().to_string(),
            reference: Some(r#"{"screen":"friendAdd:idSearch","spec":"native"}"#.into()),
            tracking_meta_type: 3,
            tracking_meta_hint: Some(user_id.into()),
        })
        .await
    }

    /// Look up a contact by phone number.
    pub async fn find_contact_by_phone_v3(&self, phone: &str) -> Result<Value> {
        let args = vec![(1, TValue::Struct(vec![(1, TValue::string(phone))]))];
        self.request("findContactByPhoneV3", args, Parse::Struct("Contact")).await
    }

    /// Add a friend by phone number.
    pub async fn add_friend_by_phone(&self, phone: &str) -> Result<Value> {
        let contact = self.find_contact_by_phone_v3(phone).await?;
        self.add_friend_by_mid(AddFriendByMidOptions {
            mid: contact["mid"].as_str().unwrap_or_default().to_string(),
            reference: Some(r#"{"screen":"friendAdd:phoneSearch","spec":"native"}"#.into()),
            tracking_meta_type: 2,
            tracking_meta_hint: Some(phone.into()),
        })
        .await
    }

    // --- Methods recovered by static RE of LINE macOS 26.5.0 (unverified). ---

    /// Block a user. `reqSeq` is filled automatically.
    pub async fn block_relation(&self, target_user_mid: &str) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await? as i32;
        let args = vec![(
            1,
            TValue::Struct(vec![(1, TValue::I32(seq)), (2, TValue::string(target_user_mid))]),
        )];
        self.request("blockRelation", args, Parse::Result).await
    }

    /// Unblock a user. `reference` is optional JSON provenance.
    pub async fn unblock_relation(&self, target_user_mid: &str, reference: Option<&str>) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await? as i32;
        let mut req = vec![(1, TValue::I32(seq)), (2, TValue::string(target_user_mid))];
        if let Some(r) = reference {
            req.push((3, TValue::string(r)));
        }
        self.request("unblockRelation", vec![(1, TValue::Struct(req))], Parse::Result).await
    }

    /// Block a user as a LINE/LYP Premium "premium block". `reqSeq` auto.
    pub async fn premium_block_relation(&self, target_user_mid: &str) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await? as i32;
        let args = vec![(
            1,
            TValue::Struct(vec![(1, TValue::I32(seq)), (2, TValue::string(target_user_mid))]),
        )];
        self.request("premiumBlockRelation", args, Parse::Result).await
    }

    /// Remove a user from the block list. `reqSeq` auto.
    pub async fn delete_from_block_list(&self, target_user_mid: &str) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await? as i32;
        let args = vec![(
            1,
            TValue::Struct(vec![(1, TValue::I32(seq)), (2, TValue::string(target_user_mid))]),
        )];
        self.request("deleteFromBlockList", args, Parse::Result).await
    }

    /// Block details for the given user mids. Returns the raw success value
    /// (`GetBlockDetailsResponse`; element layout not recovered).
    pub async fn get_block_details(&self, target_user_mids: &[String], sync_reason: Option<i32>) -> Result<Value> {
        let targets: Vec<TValue> =
            target_user_mids.iter().map(|m| TValue::Struct(vec![(1, TValue::string(m.as_str()))])).collect();
        let mut req = vec![(1, TValue::List(TType::Struct, targets))];
        if let Some(s) = sync_reason {
            req.push((2, TValue::I32(s)));
        }
        self.request("getBlockDetails", vec![(1, TValue::Struct(req))], Parse::Raw).await
    }

    /// Add several friends by mid in one request. `reqSeq` auto; `tracking` is
    /// an optional pre-built `AddFriendTracking` value. Returns raw success.
    pub async fn add_friends_by_mids(&self, mids: &[String], tracking: Option<Value>) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await? as i32;
        let set = TValue::Set(TType::String, mids.iter().map(|m| TValue::string(m.as_str())).collect());
        let mut req = Value::object();
        req.insert("reqSeq", seq);
        if let Some(t) = tracking {
            req.insert("tracking", t);
        }
        let mut req = encode_struct(Schema::line(), "AddFriendsByMidsRequest", &req)?;
        req.push((2, set));
        self.request("addFriendsByMids", vec![(1, TValue::Struct(req))], Parse::Raw).await
    }

    /// Users who added the caller as a friend. `page_token` is `None` for the
    /// first page. Returns raw success (set element layout not recovered).
    pub async fn get_users_who_added_you(&self, page_token: Option<&str>) -> Result<Value> {
        let mut inner = Vec::new();
        if let Some(t) = page_token {
            inner.push((1, TValue::string(t)));
        }
        self.request("getUsersWhoAddedYou", vec![(1, TValue::Struct(inner))], Parse::Raw).await
    }
}

// <generated pass-through>
impl RelationService<'_> {
    /// `getContactCalendarEvents(request: GetContactCalendarEventsRequest)`.
    pub async fn get_contact_calendar_events(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getContactCalendarEvents", args).await
    }

    /// `getFriendDetails(request: GetFriendDetailsRequest)`.
    pub async fn get_friend_details(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getFriendDetails", args).await
    }

    /// `getRecommendationDetails(request: GetRecommendationDetailsRequest)`.
    pub async fn get_recommendation_details(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getRecommendationDetails", args).await
    }

    /// `getTargetProfiles(request: GetTargetProfilesRequest)`.
    pub async fn get_target_profiles(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getTargetProfiles", args).await
    }

    /// `getUserFriendIds(request: GetUserFriendIdsRequest)`.
    pub async fn get_user_friend_ids(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getUserFriendIds", args).await
    }
}
// </generated pass-through>

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::test_util::{encode, path, s};
    use crate::thrift::{Schema, TType, decode_struct};

    #[test]
    fn add_friend_by_mid_envelope() {
        let o = AddFriendByMidOptions {
            mid: "u-target".into(),
            reference: Some(r#"{"screen":"groupMemberList","spec":"native"}"#.into()),
            tracking_meta_type: 5,
            tracking_meta_hint: Some("c-group".into()),
        };
        let f = add_friend_by_mid_args(42, &o);
        assert_eq!(path(&f, &[1, 1]), Some(&TValue::I32(42)));
        assert_eq!(path(&f, &[1, 2]), Some(&s("u-target")));
        assert_eq!(path(&f, &[1, 3, 1]), Some(&s(r#"{"screen":"groupMemberList","spec":"native"}"#)));
        assert_eq!(path(&f, &[1, 3, 2, 5, 1]), Some(&s("c-group")));
        // and it reads back as the IDL struct
        let v = decode_struct(Schema::line(), Some("addFriendByMid_args"), &TValue::Struct(f));
        assert_eq!(v["request"]["userMid"].as_str(), Some("u-target"));
        assert_eq!(v["request"]["tracking"]["trackingMeta"]["groupMemberList"]["chatMid"].as_str(), Some("c-group"));
    }

    #[test]
    fn add_friend_by_search_id_provenance() {
        let o = AddFriendByMidOptions {
            mid: "u-resolved".into(),
            reference: Some(r#"{"screen":"friendAdd:idSearch","spec":"native"}"#.into()),
            tracking_meta_type: 3,
            tracking_meta_hint: Some("alice".into()),
        };
        let f = add_friend_by_mid_args(42, &o);
        assert_eq!(path(&f, &[1, 3, 2, 3, 1]), Some(&s("alice")));
        assert_eq!(path(&f, &[1, 3, 2, 5]), None);
    }

    #[test]
    fn get_contacts_v3_envelope() {
        let f = encode("getContactsV3_args", &get_contacts_v3_args(&["u1".into(), "u2".into()], Some(true)));
        let Some(TValue::List(TType::Struct, targets)) = path(&f, &[1, 1]) else { panic!("targets") };
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[1].field(1), Some(&s("u2")));
        assert_eq!(path(&f, &[1, 3]), Some(&TValue::Bool(true)));
        assert!(path(&f, &[1, 2]).is_some());
    }

    // --- RE-recovered methods: offline encode against the IDL. ---

    #[test]
    fn block_relation_request_layout() {
        let f =
            encode("blockRelation_args", &crate::tv!({"request": {"reqSeq": 3, "targetUserMid": "u1"}}));
        assert_eq!(path(&f, &[1, 1]), Some(&TValue::I32(3)));
        assert_eq!(path(&f, &[1, 2]), Some(&s("u1")));
    }

    #[test]
    fn unblock_relation_request_has_reference() {
        let f = encode(
            "unblockRelation_args",
            &crate::tv!({"request": {"reqSeq": 1, "targetUserMid": "u1", "reference": "{}"}}),
        );
        assert_eq!(path(&f, &[1, 3]), Some(&s("{}")));
    }

    #[test]
    fn get_block_details_request_targets() {
        let f = encode(
            "getBlockDetails_args",
            &crate::tv!({"request": {"targetUsers": [{"targetUserMid": "u1"}], "syncReason": 0}}),
        );
        let Some(TValue::List(TType::Struct, targets)) = path(&f, &[1, 1]) else { panic!("targets") };
        assert_eq!(targets[0].field(1), Some(&s("u1")));
        assert_eq!(path(&f, &[1, 2]), Some(&TValue::I32(0)));
    }

    #[test]
    fn add_friends_by_mids_set_and_tracking() {
        // mids is a set<string> at field 2; reqSeq at 1, tracking at 3
        let set = TValue::Set(TType::String, vec![s("u1"), s("u2")]);
        let req = encode("AddFriendsByMidsRequest", &crate::tv!({"reqSeq": 9})).into_iter().chain([(2, set)]);
        let args = vec![(1, TValue::Struct(req.collect()))];
        assert_eq!(path(&args, &[1, 1]), Some(&TValue::I32(9)));
        assert!(matches!(path(&args, &[1, 2]), Some(TValue::Set(TType::String, _))));
    }
}
