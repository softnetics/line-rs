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
        Self {
            mid: mid.into(),
            reference: None,
            tracking_meta_type: 5,
            tracking_meta_hint: None,
        }
    }
}

fn opt_str(id: i16, v: Option<&str>) -> Option<(i16, TValue)> {
    v.map(|s| (id, TValue::string(s)))
}

/// Raw `addFriendByMid` args (linejs sends a hand-built `NestedArray`).
pub(crate) fn add_friend_by_mid_args(seq: i64, o: &AddFriendByMidOptions) -> Vec<(i16, TValue)> {
    let meta = TValue::Struct(
        opt_str(1, o.tracking_meta_hint.as_deref())
            .into_iter()
            .collect(),
    );
    let tracking = TValue::Struct(
        opt_str(1, o.reference.as_deref())
            .into_iter()
            .chain([(2, TValue::Struct(vec![(o.tracking_meta_type, meta)]))])
            .collect(),
    );
    vec![(
        1,
        TValue::Struct(vec![
            (1, TValue::I32(seq as i32)),
            (2, TValue::string(o.mid.as_str())),
            (3, tracking),
        ]),
    )]
}

pub(crate) fn get_contacts_v3_args(
    mids: &[String],
    check_user_status_strictly: Option<bool>,
) -> Value {
    let targets: Vec<Value> = mids
        .iter()
        .map(|m| crate::tv!({"targetUserMid": m}))
        .collect();
    let mut req = crate::tv!({"syncReason": "AUTO_REPAIR"});
    req.insert("targetUsers", targets);
    req.insert("checkUserStatusStrictly", check_user_status_strictly);
    let mut args = Value::object();
    args.insert("request", req);
    args
}

impl RelationService<'_> {
    /// Contacts by mids (`syncReason = AUTO_REPAIR`).
    pub async fn get_contacts_v3(
        &self,
        mids: &[String],
        check_user_status_strictly: Option<bool>,
    ) -> Result<Value> {
        self.call(
            "getContactsV3",
            get_contacts_v3_args(mids, check_user_status_strictly),
        )
        .await
    }

    /// Add a friend by mid with tracking metadata.
    pub async fn add_friend_by_mid(&self, options: AddFriendByMidOptions) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await?;
        self.request(
            "addFriendByMid",
            add_friend_by_mid_args(seq, &options),
            Parse::Result,
        )
        .await
    }

    /// Look up a user by LINE ID (search id). Returns a
    /// `GetContactV3Response`: `targetUserMid`, `userType`,
    /// `targetProfileDetail{profileName, picturePath, statusMessage}`,
    /// `friendDetail` and `blockDetail`.
    pub async fn find_contact_by_search_id_or_ticket_v3(&self, search_id: &str) -> Result<Value> {
        let args = vec![(
            1,
            TValue::Struct(vec![(
                1,
                TValue::Struct(vec![(1, TValue::string(search_id))]),
            )]),
        )];
        self.request(
            "findContactBySearchIdOrTicketV3",
            args,
            Parse::Struct("GetContactV3Response"),
        )
        .await
    }

    /// Add a friend by LINE ID.
    pub async fn add_friend_by_user_id(&self, user_id: &str) -> Result<Value> {
        let contact = self.find_contact_by_search_id_or_ticket_v3(user_id).await?;
        self.add_friend_by_mid(AddFriendByMidOptions {
            mid: found_mid(&contact)?,
            reference: Some(r#"{"screen":"friendAdd:idSearch","spec":"native"}"#.into()),
            tracking_meta_type: 3,
            tracking_meta_hint: Some(user_id.into()),
        })
        .await
    }

    /// Look up a user by phone number (a `GetContactV3Response`).
    pub async fn find_contact_by_phone_v3(&self, phone: &str) -> Result<Value> {
        let args = vec![(1, TValue::Struct(vec![(1, TValue::string(phone))]))];
        self.request(
            "findContactByPhoneV3",
            args,
            Parse::Struct("GetContactV3Response"),
        )
        .await
    }

    /// Add a friend by phone number.
    pub async fn add_friend_by_phone(&self, phone: &str) -> Result<Value> {
        let contact = self.find_contact_by_phone_v3(phone).await?;
        self.add_friend_by_mid(AddFriendByMidOptions {
            mid: found_mid(&contact)?,
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
            TValue::Struct(vec![
                (1, TValue::I32(seq)),
                (2, TValue::string(target_user_mid)),
            ]),
        )];
        self.request("blockRelation", args, Parse::Result).await
    }

    /// Unblock a user. `reference` is optional JSON provenance.
    pub async fn unblock_relation(
        &self,
        target_user_mid: &str,
        reference: Option<&str>,
    ) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await? as i32;
        let mut req = vec![(1, TValue::I32(seq)), (2, TValue::string(target_user_mid))];
        if let Some(r) = reference {
            req.push((3, TValue::string(r)));
        }
        self.request(
            "unblockRelation",
            vec![(1, TValue::Struct(req))],
            Parse::Result,
        )
        .await
    }

    /// Block a user as a LINE/LYP Premium "premium block". `reqSeq` auto.
    pub async fn premium_block_relation(&self, target_user_mid: &str) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await? as i32;
        let args = vec![(
            1,
            TValue::Struct(vec![
                (1, TValue::I32(seq)),
                (2, TValue::string(target_user_mid)),
            ]),
        )];
        self.request("premiumBlockRelation", args, Parse::Result)
            .await
    }

    /// Remove a user from the block list. `reqSeq` auto.
    pub async fn delete_from_block_list(&self, target_user_mid: &str) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await? as i32;
        let args = vec![(
            1,
            TValue::Struct(vec![
                (1, TValue::I32(seq)),
                (2, TValue::string(target_user_mid)),
            ]),
        )];
        self.request("deleteFromBlockList", args, Parse::Result)
            .await
    }

    /// Block details for the given user mids. Returns the raw success value
    /// (`GetBlockDetailsResponse`; element layout not recovered).
    pub async fn get_block_details(
        &self,
        target_user_mids: &[String],
        sync_reason: Option<i32>,
    ) -> Result<Value> {
        let targets: Vec<TValue> = target_user_mids
            .iter()
            .map(|m| TValue::Struct(vec![(1, TValue::string(m.as_str()))]))
            .collect();
        let mut req = vec![(1, TValue::List(TType::Struct, targets))];
        if let Some(s) = sync_reason {
            req.push((2, TValue::I32(s)));
        }
        self.request(
            "getBlockDetails",
            vec![(1, TValue::Struct(req))],
            Parse::Raw,
        )
        .await
    }

    /// Add several friends by mid in one request. `reqSeq` auto; `tracking` is
    /// an optional pre-built `AddFriendTracking` value. Returns raw success.
    pub async fn add_friends_by_mids(
        &self,
        mids: &[String],
        tracking: Option<Value>,
    ) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await? as i32;
        let set = TValue::Set(
            TType::String,
            mids.iter().map(|m| TValue::string(m.as_str())).collect(),
        );
        let mut req = Value::object();
        req.insert("reqSeq", seq);
        if let Some(t) = tracking {
            req.insert("tracking", t);
        }
        let mut req = encode_struct(Schema::line(), "AddFriendsByMidsRequest", &req)?;
        req.push((2, set));
        self.request(
            "addFriendsByMids",
            vec![(1, TValue::Struct(req))],
            Parse::Raw,
        )
        .await
    }

    /// Users who added the caller as a friend. `page_token` is `None` for the
    /// first page. Returns raw success (set element layout not recovered).
    pub async fn get_users_who_added_you(&self, page_token: Option<&str>) -> Result<Value> {
        let mut inner = Vec::new();
        if let Some(t) = page_token {
            inner.push((1, TValue::string(t)));
        }
        self.request(
            "getUsersWhoAddedYou",
            vec![(1, TValue::Struct(inner))],
            Parse::Raw,
        )
        .await
    }
}

/// The mid of a `GetContactV3Response`, or `NotFound`.
fn found_mid(contact: &Value) -> Result<String> {
    contact["targetUserMid"]
        .as_str()
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .ok_or_else(|| crate::error::Error::internal("NotFound", "no user with that id"))
}

/// Every id of a paged id list: `request(page_field)` until the response's
/// `next_field` is empty. Stops after 100 pages.
async fn all_ids<F, Fut>(
    ids_field: &str,
    next_field: &str,
    page_field: &str,
    mut call: F,
) -> Result<Vec<String>>
where
    F: FnMut(Value) -> Fut,
    Fut: std::future::Future<Output = Result<Value>>,
{
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..100 {
        let res = call(page_request(page_field, token.as_deref())).await?;
        out.extend(
            crate::client::list(&res[ids_field])
                .into_iter()
                .filter_map(|v| v.as_str().map(str::to_string)),
        );
        match res[next_field].as_str().filter(|t| !t.is_empty()) {
            Some(next) if token.as_deref() != Some(next) => token = Some(next.to_string()),
            _ => break,
        }
    }
    Ok(out)
}

/// Wraps a request struct as the single `request` argument.
fn request_arg(req: Value) -> Value {
    let mut args = Value::object();
    args.insert("request", req);
    args
}

/// A paged id-list request: `{page_field: token}` (omitted on the first page).
fn page_request(page_field: &str, page_token: Option<&str>) -> Value {
    let mut req = Value::object();
    req.insert(page_field, page_token);
    request_arg(req)
}

// --- Methods recovered by static RE of LINE Android 26.x (unverified). ---
impl RelationService<'_> {
    async fn friend_request(&self, friend_mid: &str) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await?;
        Ok(crate::tv!({"reqSeq": seq, "friendMid": friend_mid}))
    }

    /// Remove a friend. `reqSeq` is filled automatically.
    pub async fn delete_friend(&self, friend_mid: &str) -> Result<Value> {
        let req = self.friend_request(friend_mid).await?;
        self.call("deleteFriend", request_arg(req)).await
    }

    /// Mark a friend as a favorite (or not). `reqSeq` auto.
    pub async fn update_friend_favorite(&self, friend_mid: &str, favorite: bool) -> Result<Value> {
        let mut req = self.friend_request(friend_mid).await?;
        req.insert("favorite", favorite);
        self.call("updateFriendFavorite", request_arg(req)).await
    }

    /// Hide a friend from the friend list (or unhide). `reqSeq` auto.
    pub async fn update_friend_hidden(&self, friend_mid: &str, hidden: bool) -> Result<Value> {
        let mut req = self.friend_request(friend_mid).await?;
        req.insert("hidden", hidden);
        self.call("updateFriendHidden", request_arg(req)).await
    }

    /// Set the display name the caller sees for a friend; `""` clears it.
    /// `reqSeq` auto. (LINE spells the method `updateUserFriendOverridenName`.)
    pub async fn update_friend_overridden_name(
        &self,
        friend_mid: &str,
        name: &str,
    ) -> Result<Value> {
        let mut req = self.friend_request(friend_mid).await?;
        req.insert("overridenName", name);
        self.call("updateUserFriendOverridenName", request_arg(req))
            .await
    }

    /// Look up a user by invitation ticket (the ticket variant of
    /// [`find_contact_by_search_id_or_ticket_v3`](Self::find_contact_by_search_id_or_ticket_v3)).
    pub async fn find_contact_by_ticket_v3(&self, ticket: &str) -> Result<Value> {
        let args = vec![(
            1,
            TValue::Struct(vec![(1, TValue::Struct(vec![(2, TValue::string(ticket))]))]),
        )];
        self.request(
            "findContactBySearchIdOrTicketV3",
            args,
            Parse::Struct("GetContactV3Response"),
        )
        .await
    }

    /// Upload address-book changes. `local_contacts` is a list of
    /// `ContactModificationV3` (`{type, luid, phones}`). `reqSeq` auto.
    pub async fn sync_contacts_v3(&self, local_contacts: impl Into<Value>) -> Result<Value> {
        let seq = self.client.get_reqseq("talk").await?;
        let mut req = crate::tv!({"reqSeq": seq});
        req.insert("localContacts", local_contacts.into());
        self.call("syncContactsV3", request_arg(req)).await
    }

    /// Mids of blocked users, one page at a time (`None` for the first page).
    pub async fn get_user_block_ids(&self, page_token: Option<&str>) -> Result<Value> {
        self.call("getUserBlockIds", page_request("userPageToken", page_token))
            .await
    }

    /// Mids of blocked bots / official accounts, one page at a time.
    pub async fn get_bot_block_ids(&self, page_token: Option<&str>) -> Result<Value> {
        self.call("getBotBlockIds", page_request("botPageToken", page_token))
            .await
    }

    /// Mids of recommended users ("people you may know"), one page at a time.
    pub async fn get_user_recommend_ids(&self, page_token: Option<&str>) -> Result<Value> {
        self.call(
            "getUserRecommendIds",
            page_request("userPageToken", page_token),
        )
        .await
    }

    /// Every friend's mid (`getUserFriendIds`, all pages), as LINE for
    /// Android reads its friend list (`lf3/c1.java`).
    pub async fn all_user_friend_ids(&self) -> Result<Vec<String>> {
        all_ids(
            "userFriendMids",
            "nextUserPageToken",
            "userPageToken",
            |a| self.call("getUserFriendIds", a),
        )
        .await
    }

    /// Every blocked user's mid (`getUserBlockIds`, all pages).
    pub async fn all_user_block_ids(&self) -> Result<Vec<String>> {
        all_ids("userMids", "nextUserPageToken", "userPageToken", |a| {
            self.call("getUserBlockIds", a)
        })
        .await
    }

    /// Every official account the user added (`getBotFriendIds`, all pages).
    pub async fn all_bot_friend_ids(&self) -> Result<Vec<String>> {
        all_ids("botFriendMids", "nextBotPageToken", "botPageToken", |a| {
            self.call("getBotFriendIds", a)
        })
        .await
    }

    /// Mids of bots / official accounts the caller has added, one page at a time.
    pub async fn get_bot_friend_ids(&self, page_token: Option<&str>) -> Result<Value> {
        self.call("getBotFriendIds", page_request("botPageToken", page_token))
            .await
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
    fn find_contact_v3_reads_the_v3_response() {
        // A reply as the server encodes it: GetContactV3Response, whose
        // field 2 is the userType enum (it clashed with Contact.createdTime).
        let reply = TValue::Struct(vec![
            (1, s("u-found")),
            (2, TValue::I32(1)),
            (3, TValue::Struct(vec![(2, s("Name")), (3, s("/0hpic"))])),
        ]);
        let v = decode_struct(Schema::line(), Some("GetContactV3Response"), &reply);
        assert_eq!(found_mid(&v).unwrap(), "u-found");
        assert_eq!(v["userType"].as_str(), Some("USER"));
        assert_eq!(
            v["targetProfileDetail"]["profileName"].as_str(),
            Some("Name")
        );
        assert!(found_mid(&crate::tv!({})).is_err());
    }

    #[tokio::test]
    async fn all_ids_follows_page_tokens() {
        let pages = std::sync::Mutex::new(vec![
            crate::tv!({"userFriendMids": ["u3"]}),
            crate::tv!({"userFriendMids": ["u1", "u2"], "nextUserPageToken": "t1"}),
        ]);
        let seen = std::sync::Mutex::new(Vec::new());
        let ids = all_ids(
            "userFriendMids",
            "nextUserPageToken",
            "userPageToken",
            |args| {
                seen.lock().unwrap().push(
                    args["request"]["userPageToken"]
                        .as_str()
                        .map(str::to_string),
                );
                let page = pages.lock().unwrap().pop().unwrap();
                async move { Ok(page) }
            },
        )
        .await
        .unwrap();
        assert_eq!(ids, ["u1", "u2", "u3"]);
        assert_eq!(*seen.lock().unwrap(), [None, Some("t1".to_string())]);
    }

    #[test]
    fn friend_update_requests() {
        let args = encode(
            "updateFriendHidden_args",
            &crate::tv!({"request": {"reqSeq": 2, "friendMid": "u1", "hidden": true}}),
        );
        assert_eq!(path(&args, &[1, 2]), Some(&s("u1")));
        assert!(matches!(path(&args, &[1, 3]), Some(TValue::Bool(true))));
        let args = encode(
            "updateUserFriendOverridenName_args",
            &crate::tv!({"request": {"reqSeq": 2, "friendMid": "u1", "overridenName": "Bob"}}),
        );
        assert_eq!(path(&args, &[1, 3]), Some(&s("Bob")));
    }

    #[test]
    fn block_ids_page_request() {
        let args = encode(
            "getBotBlockIds_args",
            &page_request("botPageToken", Some("t")),
        );
        assert_eq!(path(&args, &[1, 1]), Some(&s("t")));
        let first = encode("getUserBlockIds_args", &page_request("userPageToken", None));
        assert_eq!(path(&first, &[1, 1]), None);
    }

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
        assert_eq!(
            path(&f, &[1, 3, 1]),
            Some(&s(r#"{"screen":"groupMemberList","spec":"native"}"#))
        );
        assert_eq!(path(&f, &[1, 3, 2, 5, 1]), Some(&s("c-group")));
        // and it reads back as the IDL struct
        let v = decode_struct(
            Schema::line(),
            Some("addFriendByMid_args"),
            &TValue::Struct(f),
        );
        assert_eq!(v["request"]["userMid"].as_str(), Some("u-target"));
        assert_eq!(
            v["request"]["tracking"]["trackingMeta"]["groupMemberList"]["chatMid"].as_str(),
            Some("c-group")
        );
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
        let f = encode(
            "getContactsV3_args",
            &get_contacts_v3_args(&["u1".into(), "u2".into()], Some(true)),
        );
        let Some(TValue::List(TType::Struct, targets)) = path(&f, &[1, 1]) else {
            panic!("targets")
        };
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[1].field(1), Some(&s("u2")));
        assert_eq!(path(&f, &[1, 3]), Some(&TValue::Bool(true)));
        assert!(path(&f, &[1, 2]).is_some());
    }

    // --- RE-recovered methods: offline encode against the IDL. ---

    #[test]
    fn block_relation_request_layout() {
        let f = encode(
            "blockRelation_args",
            &crate::tv!({"request": {"reqSeq": 3, "targetUserMid": "u1"}}),
        );
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
        let Some(TValue::List(TType::Struct, targets)) = path(&f, &[1, 1]) else {
            panic!("targets")
        };
        assert_eq!(targets[0].field(1), Some(&s("u1")));
        assert_eq!(path(&f, &[1, 2]), Some(&TValue::I32(0)));
    }

    #[test]
    fn add_friends_by_mids_set_and_tracking() {
        // mids is a set<string> at field 2; reqSeq at 1, tracking at 3
        let set = TValue::Set(TType::String, vec![s("u1"), s("u2")]);
        let req = encode("AddFriendsByMidsRequest", &crate::tv!({"reqSeq": 9}))
            .into_iter()
            .chain([(2, set)]);
        let args = vec![(1, TValue::Struct(req.collect()))];
        assert_eq!(path(&args, &[1, 1]), Some(&TValue::I32(9)));
        assert!(matches!(
            path(&args, &[1, 2]),
            Some(TValue::Set(TType::String, _))
        ));
    }
}
