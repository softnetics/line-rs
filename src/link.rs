//! LINE deep links: `line://…`, `line://nv/…`, `lineb://…`,
//! `https://line.me/R/…` and the short `https://line.me/ti/…` form, parsed
//! into [`LineLink`], plus helpers that resolve or join them through the
//! matching API call.
//!
//! The scheme list is derived from static RE of LINE macOS 26.5.0;
//! unverified against live server. The app registers the `line` and
//! `lineb` URL schemes, and most routes exist as `line://X`,
//! `line://nv/X` and `https://line.me/R/X` (e.g. `line://ti/g/`,
//! `line://ti/g2/`, `line://ti/p/`, `line://meeting/`,
//! `line://square/livetalk/`, `line://nv/chatMsg?chatId=%s&messageId=%s`).

use std::fmt;

use crate::base::BaseClient;
use crate::error::{Error, Result};
use crate::thrift::Value;
use crate::tv;

/// Target of a `ti/p/` (profile) link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileRef {
    /// `ti/p/~{userid}`: a LINE ID.
    UserId(String),
    /// `ti/p/@{id}`: an official account.
    OfficialAccount(String),
    /// `ti/p/{ticket}`: a user's invitation ticket.
    Ticket(String),
}

/// A parsed LINE deep link.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LineLink {
    /// `ti/g/{ticket}`: group invitation.
    GroupTicket(String),
    /// `ti/g2/{ticket}`: OpenChat (square) invitation.
    SquareTicket(String),
    /// `ti/p/…`: add friend / profile.
    Profile(ProfileRef),
    /// `meeting/{id}`: LINE Meeting.
    Meeting(String),
    /// `meeting/list`.
    MeetingList,
    /// `call`.
    Call,
    /// `square/livetalk/{ticket}`: OpenChat live talk invitation.
    LiveTalk(String),
    /// `nv/chatMsg?chatId=…&messageId=…`.
    ChatMessage { chat_id: String, message_id: Option<String> },
    /// `nv/profilePopup/mid={mid}`.
    ProfilePopup(String),
    /// Any other route (`chat`, `settings/…`, `shop/…`, …): the path after
    /// the scheme (and after `nv/`), without a leading slash.
    Other { path: String, query: Option<String> },
}

impl LineLink {
    /// Parse a deep link. Returns `None` for URLs that are not LINE links.
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        let lower = input.to_ascii_lowercase();
        let rest = if let Some(i) = ["line://", "lineb://"].iter().find_map(|p| lower.starts_with(p).then_some(p.len())) {
            &input[i..]
        } else {
            let i = ["https://", "http://"].iter().find_map(|p| lower.starts_with(p).then_some(p.len()))?;
            let after = &input[i..];
            let (host, path) = after.split_once('/').unwrap_or((after, ""));
            if !host.eq_ignore_ascii_case("line.me") {
                return None;
            }
            match path.split_once('/') {
                Some((r, p)) if r == "R" || r == "r" => p,
                // `https://line.me/ti/...` (no `/R/`).
                _ if path.starts_with("ti/") => path,
                _ => return None,
            }
        };
        let rest = rest.split('#').next().unwrap_or_default();
        let (path, query) = match rest.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (rest, None),
        };
        let path = path.trim_start_matches('/');
        let path = path.strip_prefix("nv/").unwrap_or(path);
        let segs: Vec<String> = path.split('/').filter(|s| !s.is_empty()).map(percent_decode).collect();
        let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
        let query_param = |name: &str| {
            query.and_then(|q| url::form_urlencoded::parse(q.as_bytes()).find(|(k, _)| k == name).map(|(_, v)| v.into_owned()))
        };
        Some(match segs.as_slice() {
            ["ti", "g", t, ..] => LineLink::GroupTicket(t.to_string()),
            ["ti", "g2", t, ..] => LineLink::SquareTicket(t.to_string()),
            ["ti", "p", t, ..] => LineLink::Profile(if let Some(id) = t.strip_prefix('~') {
                ProfileRef::UserId(id.to_string())
            } else if let Some(id) = t.strip_prefix('@') {
                ProfileRef::OfficialAccount(id.to_string())
            } else {
                ProfileRef::Ticket(t.to_string())
            }),
            ["meeting", "list"] => LineLink::MeetingList,
            ["meeting", id, ..] => LineLink::Meeting(id.to_string()),
            ["call"] => LineLink::Call,
            ["square", "livetalk", t, ..] => LineLink::LiveTalk(t.to_string()),
            ["chatMsg"] if query_param("chatId").is_some() => LineLink::ChatMessage {
                chat_id: query_param("chatId").unwrap_or_default(),
                message_id: query_param("messageId"),
            },
            ["profilePopup", m, ..] if m.starts_with("mid=") => LineLink::ProfilePopup(m["mid=".len()..].to_string()),
            _ => LineLink::Other { path: path.to_string(), query: query.map(str::to_string) },
        })
    }

    /// The canonical `https://line.me/R/…` form.
    pub fn to_url(&self) -> String {
        let path = match self {
            LineLink::GroupTicket(t) => format!("ti/g/{t}"),
            LineLink::SquareTicket(t) => format!("ti/g2/{t}"),
            LineLink::Profile(ProfileRef::UserId(id)) => format!("ti/p/~{id}"),
            LineLink::Profile(ProfileRef::OfficialAccount(id)) => format!("ti/p/@{id}"),
            LineLink::Profile(ProfileRef::Ticket(t)) => format!("ti/p/{t}"),
            LineLink::Meeting(id) => format!("meeting/{id}"),
            LineLink::MeetingList => "meeting/list".into(),
            LineLink::Call => "call".into(),
            LineLink::LiveTalk(t) => format!("square/livetalk/{t}"),
            LineLink::ChatMessage { chat_id, message_id } => {
                let mut q = url::form_urlencoded::Serializer::new(String::new());
                q.append_pair("chatId", chat_id);
                if let Some(m) = message_id {
                    q.append_pair("messageId", m);
                }
                format!("nv/chatMsg?{}", q.finish())
            }
            LineLink::ProfilePopup(mid) => format!("nv/profilePopup/mid={mid}"),
            LineLink::Other { path, query: Some(q) } => format!("{path}?{q}"),
            LineLink::Other { path, query: None } => path.clone(),
        };
        format!("https://line.me/R/{path}")
    }
}

impl fmt::Display for LineLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_url())
    }
}

impl std::str::FromStr for LineLink {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        LineLink::parse(s).ok_or_else(|| Error::internal("InvalidLink", format!("not a LINE link: {s}")))
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
            let hex = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
            out.push(hex(b[i + 1]) << 4 | hex(b[i + 2]));
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn unsupported(link: &LineLink, what: &str) -> Error {
    Error::internal("UnsupportedLink", format!("cannot {what} {link}"))
}

impl BaseClient {
    /// Look up what a link points to without joining:
    ///
    /// * `ti/g/` → `findChatByTicket` (`{chat}`)
    /// * `ti/g2/` → `findSquareByInvitationTicket`
    /// * `square/livetalk/` → `findLiveTalkByInvitationTicket`
    /// * `ti/p/~id` → `findContactByUserid`; `ti/p/{ticket}` →
    ///   `findContactByUserTicket`
    ///
    /// Other links give an `UnsupportedLink` error.
    pub async fn resolve_link(&self, link: &LineLink) -> Result<Value> {
        match link {
            LineLink::GroupTicket(t) => self.talk().find_chat_by_ticket(tv!({"request": {"ticketId": t}})).await,
            LineLink::SquareTicket(t) => {
                self.square().find_square_by_invitation_ticket(tv!({"request": {"invitationTicket": t}})).await
            }
            LineLink::LiveTalk(t) => {
                self.livetalk().find_live_talk_by_invitation_ticket(tv!({"request": {"invitationTicket": t}})).await
            }
            LineLink::Profile(ProfileRef::UserId(id)) => self.talk().find_contact_by_userid(tv!({"searchId": id})).await,
            LineLink::Profile(ProfileRef::Ticket(t)) => {
                self.talk().find_contact_by_user_ticket(tv!({"ticketIdWithTag": t})).await
            }
            _ => Err(unsupported(link, "resolve")),
        }
    }

    /// Join what a link points to:
    ///
    /// * `ti/g/` → `findChatByTicket` then `acceptChatInvitationByTicket`
    ///   (returns the chat)
    /// * `meeting/{id}` → `joinChatByCallUrl` with `urlId = id` (that the
    ///   meeting path segment is the call `urlId` is an assumption; unverified
    ///   against live server)
    ///
    /// OpenChat links need a member profile; use
    /// [`resolve_link`](Self::resolve_link) and then `joinSquare` /
    /// `joinSquareChat`.
    pub async fn join_link(&self, link: &LineLink) -> Result<Value> {
        match link {
            LineLink::GroupTicket(t) => {
                let found = self.talk().find_chat_by_ticket(tv!({"request": {"ticketId": t}})).await?;
                let chat = found["chat"].clone();
                let chat_mid = chat["chatMid"]
                    .as_str()
                    .ok_or_else(|| Error::internal("InvalidLink", "findChatByTicket returned no chat"))?
                    .to_string();
                let seq = self.get_reqseq("talk").await? as i32;
                self.talk()
                    .accept_chat_invitation_by_ticket(tv!({"request": {"reqSeq": seq, "chatMid": chat_mid, "ticketId": t}}))
                    .await?;
                Ok(chat)
            }
            LineLink::Meeting(id) => {
                let seq = self.get_reqseq("talk").await? as i32;
                self.call_service().join_chat_by_call_url(tv!({"request": {"urlId": id, "reqSeq": seq}})).await
            }
            _ => Err(unsupported(link, "join")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thrift::{self, Schema, TValue};

    fn p(s: &str) -> LineLink {
        LineLink::parse(s).unwrap_or_else(|| panic!("{s}"))
    }

    #[test]
    fn parses_equivalent_forms() {
        for s in [
            "line://ti/g/AbC123",
            "line://nv/ti/g/AbC123",
            "lineb://ti/g/AbC123",
            "https://line.me/R/ti/g/AbC123",
            "http://line.me/R/ti/g/AbC123?utm=x#frag",
            "https://line.me/ti/g/AbC123",
            "LINE://ti/g/AbC123/",
        ] {
            assert_eq!(p(s), LineLink::GroupTicket("AbC123".into()), "{s}");
        }
    }

    #[test]
    fn parses_routes() {
        assert_eq!(p("https://line.me/ti/g2/sQtKt"), LineLink::SquareTicket("sQtKt".into()));
        assert_eq!(p("line://ti/p/~my.id"), LineLink::Profile(ProfileRef::UserId("my.id".into())));
        assert_eq!(p("https://line.me/R/ti/p/%40oa_id"), LineLink::Profile(ProfileRef::OfficialAccount("oa_id".into())));
        assert_eq!(p("line://ti/p/TiCkEt"), LineLink::Profile(ProfileRef::Ticket("TiCkEt".into())));
        assert_eq!(p("line://meeting/list"), LineLink::MeetingList);
        assert_eq!(p("https://line.me/R/meeting/m1x"), LineLink::Meeting("m1x".into()));
        assert_eq!(p("line://call"), LineLink::Call);
        assert_eq!(p("line://nv/square/livetalk/LT1"), LineLink::LiveTalk("LT1".into()));
        assert_eq!(
            p("line://nv/chatMsg?chatId=c123&messageId=456"),
            LineLink::ChatMessage { chat_id: "c123".into(), message_id: Some("456".into()) }
        );
        assert_eq!(p("line://nv/profilePopup/mid=u00ff"), LineLink::ProfilePopup("u00ff".into()));
        assert_eq!(
            p("line://settings/privacy"),
            LineLink::Other { path: "settings/privacy".into(), query: None }
        );
        assert_eq!(p("line://nv/chat"), LineLink::Other { path: "chat".into(), query: None });
    }

    #[test]
    fn rejects_non_line_urls() {
        assert_eq!(LineLink::parse("https://example.com/R/ti/g/x"), None);
        assert_eq!(LineLink::parse("https://line.me/en/"), None);
        assert_eq!(LineLink::parse("mailto:a@b"), None);
        assert!("nope".parse::<LineLink>().is_err());
    }

    #[test]
    fn round_trips_through_canonical_url() {
        for s in [
            "line://ti/g/A",
            "line://ti/g2/B",
            "line://ti/p/~c",
            "line://ti/p/@d",
            "line://ti/p/e",
            "line://meeting/f",
            "line://meeting/list",
            "line://call",
            "line://square/livetalk/g",
            "line://nv/chatMsg?chatId=h&messageId=i",
            "line://nv/profilePopup/mid=j",
            "line://shop/sticker/detail/1?x=y",
        ] {
            let link = p(s);
            assert_eq!(p(&link.to_url()), link, "{s}");
        }
    }

    #[test]
    fn helper_args_encode_against_idl() {
        let schema = Schema::line();
        let enc = |name: &str, v: Value| thrift::encode_struct(schema, name, &v).unwrap();
        assert_eq!(
            enc("findChatByTicket_args", tv!({"request": {"ticketId": "T"}})),
            vec![(1, TValue::Struct(vec![(1, TValue::string("T"))]))]
        );
        assert_eq!(
            enc("acceptChatInvitationByTicket_args", tv!({"request": {"reqSeq": 1, "chatMid": "c", "ticketId": "T"}})),
            vec![(1, TValue::Struct(vec![(1, TValue::I32(1)), (2, TValue::string("c")), (3, TValue::string("T"))]))]
        );
        assert_eq!(
            enc("joinChatByCallUrl_args", tv!({"request": {"urlId": "u", "reqSeq": 2}})),
            vec![(2, TValue::Struct(vec![(1, TValue::string("u")), (2, TValue::I32(2))]))]
        );
        assert_eq!(
            enc("SquareService_findSquareByInvitationTicket_args", tv!({"request": {"invitationTicket": "S"}})),
            vec![(1, TValue::Struct(vec![(2, TValue::string("S"))]))]
        );
        assert_eq!(
            enc("SquareService_findLiveTalkByInvitationTicket_args", tv!({"request": {"invitationTicket": "L"}})),
            vec![(1, TValue::Struct(vec![(1, TValue::string("L"))]))]
        );
    }

    #[tokio::test]
    async fn unsupported_links_fail_without_network() {
        let base = BaseClient::new(crate::Device::DesktopMac).unwrap();
        assert!(base.resolve_link(&LineLink::Call).await.is_err());
        assert!(base.join_link(&LineLink::SquareTicket("x".into())).await.is_err());
    }
}
