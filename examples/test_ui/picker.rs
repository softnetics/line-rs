//! Group and peer pickers: list the account's joined groups and friends so
//! the page can fill the group/peer mid inputs instead of pasting mids.
//!
//! Every request waits on the shared [`Throttle`], so a full load costs the
//! same request budget as running tests: groups take 2 calls, friends take
//! 1 + one contacts call per [`CONTACTS_CHUNK`] friends. Like linejs'
//! `fetchUsers`, `getContactsV3` falls back to `getContactsV2` when the
//! device answers "API method not capable" (e.g. desktop devices), but
//! without linejs' last-resort one-call-per-mid loop: if both are refused
//! the mids are listed without names.
//!
//! Keep makes no requests: LEGY does not route `/kp` (see
//! [`KEEP_UNAVAILABLE`]). With [`KEEP_ROUTED`] set it would issue a channel
//! token and call `keep/sync.json?revision=0` ([`KEEP_LIMIT`] items).
//! The Keep REST shapes come from static RE and are unverified, so items are
//! found by walking the JSON for objects with a `contentId`.

use std::collections::HashMap;

use line_rs::client::{Chat, Client, User};
use line_rs::{tv, Value};
use serde::Serialize;

use crate::cases::Throttle;

/// Keep items requested from `keep/sync.json`.
pub const KEEP_LIMIT: i32 = 100;

/// Friends per contacts call.
pub const CONTACTS_CHUNK: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Groups,
    Peers,
    Keep,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "groups" => Some(Kind::Groups),
            "peers" => Some(Kind::Peers),
            "keep" => Some(Kind::Keep),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pick {
    pub mid: String,
    pub name: String,
    /// Secondary line: member count for groups, status message for peers.
    pub detail: String,
}

/// One picker's state as shown on the page.
#[derive(Debug, Clone, Serialize)]
pub struct PickList {
    /// idle | loading | ready | error
    pub status: &'static str,
    pub items: Vec<Pick>,
    pub error: Option<String>,
    /// Non-fatal problem (e.g. names unavailable).
    pub note: Option<String>,
    pub fetched_ms: Option<u64>,
}

impl Default for PickList {
    fn default() -> Self {
        Self { status: "idle", items: Vec::new(), error: None, note: None, fetched_ms: None }
    }
}

pub struct Loaded {
    pub items: Vec<Pick>,
    pub note: Option<String>,
}

pub async fn load(kind: Kind, client: &Client, throttle: &Throttle) -> Result<Loaded, String> {
    match kind {
        Kind::Groups => load_groups(client, throttle).await,
        Kind::Peers => load_peers(client, throttle).await,
        Kind::Keep => load_keep(client, throttle).await,
    }
}

/// Why Keep is not called: the live run on 2026-10-21 (DESKTOPWIN, region
/// TH) got the same bodiless `HTTP 404` from `server=legy` for
/// `keep/sync.json`, `keep/init.json` and a path that cannot exist under
/// `/kp`, on legy and ga2, with the Keep and timeline channel tokens, over
/// HTTP/1.1. LEGY does not route `/kp` for this client, so the picker makes
/// no requests until that changes.
pub const KEEP_UNAVAILABLE: &str =
    "Keep ใช้ไม่ได้: LEGY ไม่ route /kp ให้ client นี้ (ทดสอบจริง 2026-10-21: ทุก path ใต้ /kp ได้ 404 จาก server=legy) — ไม่ได้ส่ง request";

/// Flip once LEGY is seen routing `/kp` again.
const KEEP_ROUTED: bool = false;

async fn load_keep(client: &Client, throttle: &Throttle) -> Result<Loaded, String> {
    if !KEEP_ROUTED {
        return Err(KEEP_UNAVAILABLE.into());
    }
    throttle.wait().await;
    let rest = client.base.rest();
    rest.channel_token(line_rs::rest::channels::KEEP).await.map_err(|e| format!("Keep channel token: {e}"))?;
    throttle.wait().await;
    let res = rest.keep().sync(0, Some(KEEP_LIMIT), None).await.map_err(|e| format!("keep/sync.json: {e}"))?;
    Ok(keep_loaded(&res))
}

fn keep_loaded(res: &serde_json::Value) -> Loaded {
    let items = keep_picks(res);
    let mut notes = Vec::new();
    if items.is_empty() {
        let keys = res.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default();
        let body: String = res.to_string().chars().take(300).collect();
        notes.push(format!("no contentId in keep/sync.json response (keys: {keys}; body: {body})"));
    } else if items.len() >= KEEP_LIMIT as usize {
        notes.push(format!("first {KEEP_LIMIT} items only"));
    }
    Loaded { items, note: (!notes.is_empty()).then(|| notes.join(" · ")) }
}

/// Every object with a string `contentId`, de-duplicated, newest first when a
/// timestamp is present.
fn keep_picks(res: &serde_json::Value) -> Vec<Pick> {
    fn walk(v: &serde_json::Value, out: &mut Vec<(i64, Pick)>) {
        match v {
            serde_json::Value::Object(o) => {
                if let Some(id) = o.get("contentId").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) {
                    if !out.iter().any(|(_, p)| p.mid == id) {
                        out.push((keep_time(v), Pick { mid: id.to_string(), name: keep_title(v), detail: keep_detail(v) }));
                    }
                    return;
                }
                o.values().for_each(|x| walk(x, out));
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(res, &mut out);
    out.sort_by_key(|a| std::cmp::Reverse(a.0));
    out.into_iter().map(|(_, p)| p).collect()
}

/// First non-empty text-like field, searched breadth-first so top-level
/// titles win over nested ones.
fn keep_title(item: &serde_json::Value) -> String {
    const KEYS: [&str; 8] = ["title", "text", "name", "fileName", "displayName", "description", "summary", "url"];
    let mut level = vec![item];
    for _ in 0..4 {
        for v in &level {
            for k in KEYS {
                if let Some(s) = v.get(k).and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
                    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
                    return one_line.chars().take(80).collect();
                }
            }
        }
        level = level
            .iter()
            .flat_map(|v| match v {
                serde_json::Value::Object(o) => o.values().collect::<Vec<_>>(),
                serde_json::Value::Array(a) => a.iter().collect(),
                _ => Vec::new(),
            })
            .collect();
    }
    String::new()
}

fn keep_time(item: &serde_json::Value) -> i64 {
    ["updatedTime", "createdTime", "modifiedTime", "createdAt", "updatedAt"]
        .iter()
        .find_map(|k| item.get(*k).and_then(|x| x.as_i64().or_else(|| x.as_str()?.parse().ok())))
        .unwrap_or(0)
}

fn keep_detail(item: &serde_json::Value) -> String {
    ["contentType", "type", "category"]
        .iter()
        .find_map(|k| item.get(*k).and_then(|x| x.as_str().map(str::to_string).or_else(|| x.as_i64().map(|i| i.to_string()))))
        .unwrap_or_default()
}

async fn load_groups(client: &Client, throttle: &Throttle) -> Result<Loaded, String> {
    throttle.wait().await;
    let joined = client
        .base
        .talk()
        .call("getAllChatMids", tv!({"request": {"withMemberChats": true}, "syncReason": "INTERNAL"}))
        .await
        .map_err(|e| format!("getAllChatMids: {e}"))?;
    let mids: Vec<Value> = joined["memberChatMids"].as_list().unwrap_or_default().to_vec();
    if mids.is_empty() {
        return Ok(Loaded { items: Vec::new(), note: None });
    }
    throttle.wait().await;
    // Members are needed for the count; invitees are not.
    let mut request = tv!({"withMembers": true, "withInvitees": false});
    request.insert("chatMids", Value::List(mids));
    let mut args = tv!({"syncReason": "INTERNAL"});
    args.insert("request", request);
    let res = client.base.talk().call("getChats", args).await.map_err(|e| format!("getChats: {e}"))?;
    let chats = res["chats"].as_list().unwrap_or_default();
    Ok(Loaded { items: group_picks(client, chats), note: None })
}

async fn load_peers(client: &Client, throttle: &Throttle) -> Result<Loaded, String> {
    throttle.wait().await;
    let ids = client
        .base
        .relation()
        .call("getUserFriendIds", tv!({"request": {"blockStatus": "ALL"}}))
        .await
        .map_err(|e| format!("getUserFriendIds: {e}"))?;
    let own = client.mid().unwrap_or_default();
    let mids: Vec<String> = ids["userFriendMids"]
        .as_list()
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .filter(|m| *m != own)
        .collect();
    let mut names = HashMap::new();
    let mut api = ContactsApi::V3;
    for chunk in mids.chunks(CONTACTS_CHUNK) {
        loop {
            throttle.wait().await;
            let res = match api {
                ContactsApi::V3 => {
                    let targets: Vec<serde_json::Value> =
                        chunk.iter().map(|m| serde_json::json!({"targetUserMid": m})).collect();
                    client
                        .base
                        .relation()
                        .call("getContactsV3", tv!({"request": {"targetUsers": targets, "syncReason": "INTERNAL"}}))
                        .await
                        .map(|r| v3_names(client, &r))
                }
                ContactsApi::V2 => client.base.talk().get_contacts_v2(chunk).await.map(|r| v2_names(&r)),
            };
            match res {
                Ok(found) => {
                    names.extend(found);
                    break;
                }
                Err(e) if is_not_capable(&e) => match api {
                    ContactsApi::V3 => api = ContactsApi::V2,
                    ContactsApi::V2 => {
                        return Ok(Loaded {
                            items: peer_picks(&mids, &names),
                            note: Some("getContactsV3/V2 not capable on this device: names unavailable".into()),
                        });
                    }
                },
                Err(e) => return Err(format!("{}: {e}", api.method())),
            }
        }
    }
    let note = (api == ContactsApi::V2).then(|| "names via getContactsV2 (V3 not capable on this device)".into());
    Ok(Loaded { items: peer_picks(&mids, &names), note })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ContactsApi {
    V3,
    V2,
}

impl ContactsApi {
    fn method(self) -> &'static str {
        match self {
            ContactsApi::V3 => "getContactsV3",
            ContactsApi::V2 => "getContactsV2",
        }
    }
}

fn is_not_capable(e: &line_rs::Error) -> bool {
    e.to_string().to_ascii_lowercase().contains("api method not capable")
}

/// mid -> (display name, status message)
type Names = HashMap<String, (String, String)>;

/// `getContactsV3` `responses[]`: `targetProfileDetail.{profileName,statusMessage}`.
fn v3_names(client: &Client, res: &Value) -> Names {
    res["responses"]
        .as_list()
        .unwrap_or_default()
        .iter()
        .filter_map(|r| {
            let mid = r["targetUserMid"].as_str()?.to_string();
            let name = User::new(client.clone(), r.clone()).display_name().unwrap_or("").to_string();
            let status = r["targetProfileDetail"]["statusMessage"].as_str().unwrap_or("").to_string();
            Some((mid, (name, status)))
        })
        .collect()
}

/// `getContactsV2` `contacts: map<mid, ContactEntry{contact: Contact}>`. The
/// user's own rename (`displayNameOverridden`) wins, as in the LINE app.
fn v2_names(res: &Value) -> Names {
    res["contacts"]
        .entries()
        .into_iter()
        .map(|(mid, entry)| {
            let c = &entry["contact"];
            let name = c["displayNameOverridden"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| c["displayName"].as_str())
                .unwrap_or("")
                .to_string();
            let status = c["statusMessage"].as_str().unwrap_or("").to_string();
            (mid, (name, status))
        })
        .collect()
}

fn group_picks(client: &Client, chats: &[Value]) -> Vec<Pick> {
    let mut out: Vec<Pick> = chats
        .iter()
        .map(|raw| {
            let chat = Chat::new(client.clone(), raw.clone());
            let members = raw["extra"]["groupExtra"]["memberMids"].entries().len();
            Pick {
                mid: chat.mid().to_string(),
                name: chat.name().to_string(),
                detail: if members > 0 { format!("{members} members") } else { String::new() },
            }
        })
        .filter(|p| p.mid.starts_with('c'))
        .collect();
    sort(&mut out);
    out
}

/// Every friend mid becomes a row; contacts only add names.
fn peer_picks(mids: &[String], names: &Names) -> Vec<Pick> {
    let mut out: Vec<Pick> = mids
        .iter()
        .map(|mid| {
            let (name, detail) = names.get(mid).cloned().unwrap_or_default();
            Pick { mid: mid.clone(), name, detail }
        })
        .collect();
    sort(&mut out);
    out
}

/// Named rows first, by name (case-insensitive), then unnamed by mid.
fn sort(items: &mut [Pick]) {
    items.sort_by(|a, b| {
        (a.name.is_empty(), a.name.to_lowercase(), &a.mid).cmp(&(b.name.is_empty(), b.name.to_lowercase(), &b.mid))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use line_rs::client::Client;
    use line_rs::{BaseClient, Device};

    fn client() -> Client {
        Client::new(BaseClient::new(Device::DesktopWin).unwrap())
    }

    #[test]
    fn groups_sorted_with_member_count() {
        let chats = [
            tv!({"chatMid": "c2", "chatName": "beta", "extra": {"groupExtra": {"memberMids": {"u1": 1, "u2": 2}}}}),
            tv!({"chatMid": "c1", "chatName": "Alpha"}),
            tv!({"chatMid": "r1", "chatName": "room"}),
            tv!({"chatMid": "c3", "chatName": ""}),
        ];
        let p = group_picks(&client(), &chats);
        let mids: Vec<_> = p.iter().map(|x| x.mid.as_str()).collect();
        assert_eq!(mids, ["c1", "c2", "c3"]);
        assert_eq!(p[1].detail, "2 members");
        assert_eq!(p[0].detail, "");
    }

    #[test]
    fn peers_keep_unnamed_mids() {
        let mids = vec!["u2".to_string(), "u1".to_string(), "u3".to_string()];
        let res = tv!({"responses": [
            {"targetUserMid": "u1", "targetProfileDetail": {"profileName": "Zed", "statusMessage": "hi"}},
            {"targetUserMid": "u2", "targetProfileDetail": {"profileName": "amy"}}
        ]});
        let p = peer_picks(&mids, &v3_names(&client(), &res));
        let rows: Vec<_> = p.iter().map(|x| (x.mid.as_str(), x.name.as_str(), x.detail.as_str())).collect();
        assert_eq!(rows, [("u2", "amy", ""), ("u1", "Zed", "hi"), ("u3", "", "")]);
        assert_eq!(peer_picks(&mids, &Names::new()).len(), 3);
    }

    #[test]
    fn v2_names_prefer_override() {
        let res = tv!({"contacts": {
            "u1": {"contact": {"mid": "u1", "displayName": "Real", "displayNameOverridden": "Mom", "statusMessage": "s"}},
            "u2": {"contact": {"mid": "u2", "displayName": "Bob", "displayNameOverridden": ""}},
            "u3": {"userStatus": "DELETED"}
        }});
        let n = v2_names(&res);
        assert_eq!(n["u1"], ("Mom".to_string(), "s".to_string()));
        assert_eq!(n["u2"].0, "Bob");
        assert_eq!(n["u3"].0, "");
    }

    #[test]
    fn keep_items_found_anywhere() {
        let res = serde_json::json!({"code": 0, "result": {"revision": 9, "contents": [
            {"contentId": "K1", "createdTime": 100, "contentData": {"text": "  hello\n world "}, "contentType": "TEXT"},
            {"contentId": "K2", "createdTime": 300, "title": "Trip", "contentData": {"fileName": "a.jpg"}},
            {"contentId": "K1", "createdTime": 100},
            {"contentId": ""},
            {"other": {"contentId": "K3"}}
        ]}});
        let p = keep_picks(&res);
        let rows: Vec<_> = p.iter().map(|x| (x.mid.as_str(), x.name.as_str(), x.detail.as_str())).collect();
        assert_eq!(rows, [("K2", "Trip", ""), ("K1", "hello world", "TEXT"), ("K3", "", "")]);
        assert!(keep_picks(&serde_json::json!({"result": []})).is_empty());
    }

    #[test]
    fn kind_parse() {
        assert_eq!(Kind::parse("groups"), Some(Kind::Groups));
        assert_eq!(Kind::parse("peers"), Some(Kind::Peers));
        assert_eq!(Kind::parse("keep"), Some(Kind::Keep));
        assert_eq!(Kind::parse("x"), None);
    }
}
