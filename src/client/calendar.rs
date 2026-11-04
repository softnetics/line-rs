//! LINE Calendar (`line.calendar.bff.*` gRPC services).
//!
//! Field numbers and message shapes were reverse-engineered from the LINE
//! Android app (26.11.0). Encoding and decoding are by hand via
//! [`crate::protobuf`]; see that module for why there is no codegen.
//!
//! Covers listing calendars and events, and creating, updating and deleting
//! events (`EventService/CreateEvents`, `UpdateEvents`, `DeleteEvents`).
//! Recurrence, invitees, reminders and RSVP (`InvitationService`) are not
//! done yet.

use crate::base::BaseClient;
use crate::error::{Error, Result};
use crate::grpc::{self, GrpcRequest};
use crate::protobuf::{Reader, Writer};
use crate::tv;

const CALENDAR_SERVICE: &str = "/line.calendar.bff.CalendarService";
const EVENT_SERVICE: &str = "/line.calendar.bff.EventService";

/// The LINE Calendar internal channel id (the production `realId` from the
/// app's channel table). The gRPC services authenticate with a channel access
/// token issued for this channel, sent as `x-line-channeltoken` — not the
/// main `x-line-access` token.
const CALENDAR_CHANNEL_ID: &str = "2007849914";

/// A calendar the user can see (personal or tied to a chat).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Calendar {
    pub id: String,
    pub name: String,
    pub is_visible: bool,
    pub is_favorite: bool,
    /// Raw `type` enum value (0 = personal, chat types are non-zero).
    pub kind: i32,
    pub color_id: i32,
    /// Group/room/user mid for a chat calendar; empty for a personal one.
    pub chatroom_id: String,
    pub event_updated_at: i64,
    pub client_updated_at: i64,
    pub created_at: i64,
    pub is_reminder_enabled: bool,
    pub is_shown_in_calendar_list: bool,
}

impl Calendar {
    /// Decodes a `line.calendar.bff.Calendar` message.
    fn decode(buf: &[u8]) -> Result<Self> {
        let mut c = Calendar::default();
        let mut r = Reader::new(buf);
        while let Some(f) = r.next_field()? {
            match f.number {
                1 => c.name = f.value.as_str()?.to_string(),
                2 => c.id = f.value.as_str()?.to_string(),
                3 => c.is_visible = f.value.as_bool()?,
                4 => c.is_favorite = f.value.as_bool()?,
                5 => c.kind = f.value.as_i32()?,
                6 => c.color_id = f.value.as_i32()?,
                7 => c.chatroom_id = f.value.as_str()?.to_string(),
                10 => c.event_updated_at = f.value.as_i64()?,
                11 => c.client_updated_at = f.value.as_i64()?,
                12 => c.created_at = f.value.as_i64()?,
                13 => c.is_reminder_enabled = f.value.as_bool()?,
                15 => c.is_shown_in_calendar_list = f.value.as_bool()?,
                _ => {}
            }
        }
        Ok(c)
    }
}

/// An event on a calendar.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Event {
    pub id: String,
    pub calendar_id: String,
    /// Start/end as epoch milliseconds.
    pub start_time: i64,
    pub end_time: i64,
    pub timezone: String,
    pub title: String,
    pub is_all_day: bool,
    pub location: String,
    pub url: String,
    pub description: String,
    pub is_deleted: bool,
    /// The chat message this event was created from, if any.
    pub message_id: String,
    pub client_updated_at: i64,
    pub is_editable_in_line_calendar: bool,
    pub creator_id: String,
    pub color_id: i32,
    pub created_at: i64,
}

impl Event {
    /// Decodes a `line.calendar.bff.Event` message.
    fn decode(buf: &[u8]) -> Result<Self> {
        let mut e = Event::default();
        let mut r = Reader::new(buf);
        while let Some(f) = r.next_field()? {
            match f.number {
                1 => e.id = f.value.as_str()?.to_string(),
                2 => e.calendar_id = f.value.as_str()?.to_string(),
                3 => e.start_time = f.value.as_i64()?,
                4 => e.end_time = f.value.as_i64()?,
                5 => e.timezone = f.value.as_str()?.to_string(),
                6 => e.title = f.value.as_str()?.to_string(),
                7 => e.is_all_day = f.value.as_bool()?,
                8 => e.location = f.value.as_str()?.to_string(),
                9 => e.url = f.value.as_str()?.to_string(),
                10 => e.description = f.value.as_str()?.to_string(),
                14 => e.is_deleted = f.value.as_bool()?,
                15 => e.message_id = f.value.as_str()?.to_string(),
                17 => e.client_updated_at = f.value.as_i64()?,
                18 => e.is_editable_in_line_calendar = f.value.as_bool()?,
                19 => e.creator_id = f.value.as_str()?.to_string(),
                20 => e.color_id = f.value.as_i32()?,
                23 => e.created_at = f.value.as_i64()?,
                _ => {}
            }
        }
        Ok(e)
    }
}

impl BaseClient {
    /// A channel access token for the LINE Calendar channel, for the
    /// `x-line-channeltoken` header. Tries `issueChannelToken` and falls back
    /// to `approveChannelAndIssueChannelToken` (first use grants consent, as
    /// the app does automatically for this first-party channel).
    async fn calendar_channel_token(&self) -> Result<String> {
        let args = tv!({ "channelId": CALENDAR_CHANNEL_ID });
        let token = match self.channel().issue_channel_token(args.clone()).await {
            Ok(t) => t,
            Err(_) => {
                self.channel()
                    .approve_channel_and_issue_channel_token(args)
                    .await?
            }
        };
        // The gRPC BFF expects the v2 channel access token (`channelAccessToken`),
        // falling back to the legacy `token` field.
        let value = token["channelAccessToken"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| token["token"].as_str())
            .unwrap_or_default();
        Ok(value.to_string())
    }

    /// gRPC metadata headers: the same identity headers as a Thrift call, but
    /// split out the way the app sends them for gRPC (not the tab-joined
    /// `x-line-application`), plus the LINE Calendar channel token.
    async fn grpc_headers(&self) -> Result<Vec<(String, String)>> {
        let d = &self.device_details;
        let mut h = vec![
            ("user-agent".to_string(), d.user_agent()),
            (
                "x-line-application-type".to_string(),
                d.device.as_str().to_string(),
            ),
            (
                "x-line-application-version".to_string(),
                d.app_version.clone(),
            ),
            // The gRPC path sends the build phase as its own header (the
            // Thrift path folds it into x-line-application). Emulate RELEASE.
            (
                "x-line-application-phase".to_string(),
                "RELEASE".to_string(),
            ),
            ("x-line-system-name".to_string(), d.system_name.clone()),
            (
                "x-line-system-version".to_string(),
                d.system_version.clone(),
            ),
            ("x-lal".to_string(), "ja_JP".to_string()),
        ];
        if let Some(token) = self.auth_token() {
            h.push(("x-line-access".to_string(), token));
        }
        h.push((
            "x-line-channeltoken".to_string(),
            self.calendar_channel_token().await?,
        ));
        Ok(h)
    }

    async fn grpc_unary(&self, path: &str, body: &[u8]) -> Result<bytes::Bytes> {
        let headers = self.grpc_headers().await?;
        let mut msgs = grpc::call(GrpcRequest {
            host: self.current_endpoint(),
            path,
            headers: &headers,
            body,
        })
        .await?;
        Ok(msgs.drain(..).next().unwrap_or_default())
    }

    /// `CalendarService/ListAllCalendars`: every calendar the user can see.
    pub async fn list_all_calendars(&self) -> Result<Vec<Calendar>> {
        let path = format!("{CALENDAR_SERVICE}/ListAllCalendars");
        // Request is empty (a bare ListAllCalendarsRequest).
        let resp = self.grpc_unary(&path, &[]).await?;
        // Response: { meta = 1, calendars = repeated 2 }.
        let mut out = Vec::new();
        let mut r = Reader::new(&resp);
        while let Some(f) = r.next_field()? {
            if f.number == 2 {
                out.push(Calendar::decode(f.value.as_bytes()?)?);
            }
        }
        Ok(out)
    }

    /// `EventService/CreateEvents` with one event. `id` comes from
    /// [`new_event_id`]. `notify_chat` is the app's "post to the chat" flag
    /// (`notify_to_chatroom`); `None` leaves it to the server. Returns the
    /// calendar's new `event_updated_at`.
    pub async fn create_event(
        &self,
        target: EventTarget<'_>,
        id: &str,
        event: &EventInput,
        notify_chat: Option<bool>,
    ) -> Result<i64> {
        let body = event.encode(id, Write::Create, next_client_updated_at(0), notify_chat);
        let path = format!("{EVENT_SERVICE}/CreateEvents");
        let resp = self
            .grpc_unary(&path, &write_request(target, &[body]))
            .await?;
        write_response("CreateEvents", &resp)
    }

    /// `EventService/UpdateEvents`: replaces event `id` with `event`.
    /// `previous_updated_at` is the event's [`Event::client_updated_at`].
    pub async fn update_event(
        &self,
        target: EventTarget<'_>,
        id: &str,
        event: &EventInput,
        previous_updated_at: i64,
        notify_chat: Option<bool>,
    ) -> Result<i64> {
        let body = event.encode(
            id,
            Write::Update,
            next_client_updated_at(previous_updated_at),
            notify_chat,
        );
        let path = format!("{EVENT_SERVICE}/UpdateEvents");
        let resp = self
            .grpc_unary(&path, &write_request(target, &[body]))
            .await?;
        write_response("UpdateEvents", &resp)
    }

    /// `EventService/DeleteEvents` with one event:
    /// `{ id = 1, client_updated_at = 2, notify_to_chatroom = 3 }`.
    pub async fn delete_event(
        &self,
        target: EventTarget<'_>,
        id: &str,
        previous_updated_at: i64,
        notify_chat: Option<bool>,
    ) -> Result<i64> {
        let mut w = Writer::new();
        w.string_always(1, id);
        w.int64(2, next_client_updated_at(previous_updated_at));
        if let Some(n) = notify_chat {
            w.bool_always(3, n);
        }
        let path = format!("{EVENT_SERVICE}/DeleteEvents");
        let resp = self
            .grpc_unary(&path, &write_request(target, &[w.into_bytes()]))
            .await?;
        write_response("DeleteEvents", &resp)
    }

    /// `EventService/ListEventsStream`: events for one calendar (by calendar
    /// id) or one chat (by chatroom id). `client_last_get_time` is 0 for a
    /// full fetch, or the previous `meta.client_last_get_time` for a delta.
    /// Collects every streamed chunk.
    pub async fn list_events_stream(
        &self,
        target: EventTarget<'_>,
        client_last_get_time: i64,
    ) -> Result<Vec<Event>> {
        let mut w = Writer::new();
        match target {
            // oneof request_type: calendar_id = 1, chatroom_id = 2.
            EventTarget::Calendar(id) => w.string_always(1, id),
            EventTarget::Chat(id) => w.string_always(2, id),
        }
        w.int64(3, client_last_get_time);

        let headers = self.grpc_headers().await?;
        let path = format!("{EVENT_SERVICE}/ListEventsStream");
        let chunks = grpc::call(GrpcRequest {
            host: self.current_endpoint(),
            path: &path,
            headers: &headers,
            body: w.as_bytes(),
        })
        .await?;

        // Each streamed message: { meta = 1, chunk = 2 { events = repeated 1 } }.
        let mut out = Vec::new();
        for msg in chunks {
            let mut r = Reader::new(&msg);
            while let Some(f) = r.next_field()? {
                if f.number == 2 {
                    let mut chunk = f.value.as_message()?;
                    while let Some(cf) = chunk.next_field()? {
                        if cf.number == 1 {
                            out.push(Event::decode(cf.value.as_bytes()?)?);
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}

/// The fields of an event to create, or to replace on update. LINE's
/// update takes the whole event, not a diff.
///
/// Times are epoch milliseconds. An all-day event starts at midnight of its
/// first day in `timezone` and ends at midnight after its last day.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventInput {
    pub title: String,
    pub start_time: i64,
    pub end_time: i64,
    /// IANA zone id, e.g. `Asia/Bangkok`.
    pub timezone: String,
    pub is_all_day: bool,
    pub location: String,
    pub url: String,
    pub description: String,
    pub color_id: i32,
}

impl EventInput {
    /// The `line.calendar.bff` create/update event message. Both share
    /// fields 1 to 9; client_updated_at and the notify flag differ (create:
    /// 15 and 16, update: 16 and 17) and colour too (create 17, update 18).
    fn encode(
        &self,
        id: &str,
        write: Write,
        client_updated_at: i64,
        notify: Option<bool>,
    ) -> Vec<u8> {
        let (updated_at, notify_field, color) = match write {
            Write::Create => (15, 16, 17),
            Write::Update => (16, 17, 18),
        };
        let mut w = Writer::new();
        w.string_always(1, id);
        w.int64(2, self.start_time);
        w.int64(3, self.end_time);
        w.string(4, &self.timezone);
        w.string(5, &self.title);
        w.bool(6, self.is_all_day);
        w.string(7, &self.location);
        w.string(8, &self.url);
        w.string(9, &self.description);
        w.int64(updated_at, client_updated_at);
        if let Some(n) = notify {
            w.bool_always(notify_field, n);
        }
        w.uint32(color, self.color_id.max(0) as u32);
        w.into_bytes()
    }
}

#[derive(Debug, Clone, Copy)]
enum Write {
    Create,
    Update,
}

/// A new event id the way the app makes them: `a-` and a random UUID v4 as
/// unpadded base64url.
pub fn new_event_id() -> String {
    use base64::Engine;
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "a-{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
    )
}

/// A `client_updated_at` the server accepts: now, but always later than the
/// event's previous value (older ones fail with `CLIENT_UPDATED_AT_OUTDATED`).
fn next_client_updated_at(previous: i64) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default();
    now.max(previous + 1)
}

/// Name of a `line.calendar.bff.ErrorCode`.
fn error_code_name(code: i32) -> &'static str {
    match code {
        11 => "INVALID_ARGUMENT_VALUE",
        12 => "INVALID_ARGUMENT_FORMAT",
        13 => "INVALID_CLIENT_UPDATED_AT",
        14 => "PERSONAL_CALENDAR_EXCEEDED",
        15 => "OCCURRENCE_CHANGE_LIMIT_EXCEEDED",
        21 => "DATA_NOT_FOUND",
        22 => "USER_NOT_FOUND",
        23 => "CALENDAR_NOT_FOUND",
        31 => "FOLDER_EVENT_CONSISTENCY",
        32 => "USER_CONSISTENCY",
        33 => "CLIENT_UPDATED_AT_OUTDATED",
        34 => "PERSONAL_CALENDAR_ID_DUPLICATE",
        41 => "EDIT_PERMISSION_DENIED",
        42 => "GDPR_REGION_RESTRICTED",
        43 => "PAST_EVENT_INVITATION_RESPONSE_FORBIDDEN",
        44 => "AGREEMENT_REQUIRED",
        45 => "CALENDAR_USER_BLOCKED",
        51 => "INTERNAL_SERVER_ERROR",
        52 => "DB_CONNECTION_ERROR",
        53 => "EXTERNAL_API_ERROR",
        61 => "MAINTENANCE_MODE",
        71 => "AUTHENTICATION_REQUIRED",
        72 => "CLIENT_ID_MISMATCH",
        73 => "AUTHENTICATION_OTHER",
        _ => "UNKNOWN",
    }
}

/// A write request: `{ calendar_id = 1 | chatroom_id = 2, events = repeated 3 }`.
fn write_request(target: EventTarget<'_>, events: &[Vec<u8>]) -> Vec<u8> {
    let mut w = Writer::new();
    match target {
        EventTarget::Calendar(id) => w.string_always(1, id),
        EventTarget::Chat(id) => w.string_always(2, id),
    }
    for e in events {
        w.message(3, e);
    }
    w.into_bytes()
}

/// A write response: `{ errors = repeated 1 { error_code = 1, message = 2,
/// event_id = 3 }, event_updated_at = 2 }`. The first error becomes `Err`;
/// otherwise the calendar's new `event_updated_at`.
fn write_response(method: &str, resp: &[u8]) -> Result<i64> {
    let mut updated_at = 0;
    let mut r = Reader::new(resp);
    while let Some(f) = r.next_field()? {
        match f.number {
            1 => {
                let (mut code, mut message) = (0, String::new());
                let mut e = f.value.as_message()?;
                while let Some(ef) = e.next_field()? {
                    match ef.number {
                        1 => code = ef.value.as_i32()?,
                        2 => message = ef.value.as_str()?.to_string(),
                        _ => {}
                    }
                }
                return Err(Error::Internal {
                    kind: format!("Calendar{method}"),
                    message: format!("{} ({code}) {message}", error_code_name(code))
                        .trim_end()
                        .to_string(),
                });
            }
            2 => updated_at = f.value.as_i64()?,
            _ => {}
        }
    }
    Ok(updated_at)
}

/// What to list events for, or write them to.
#[derive(Debug, Clone, Copy)]
pub enum EventTarget<'a> {
    /// A calendar id from [`Calendar::id`].
    Calendar(&'a str),
    /// A chat mid (group/room/user).
    Chat(&'a str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protobuf::Writer;

    #[test]
    fn decodes_a_calendar() {
        let mut w = Writer::new();
        w.string(1, "My Calendar");
        w.string(2, "cal-123");
        w.bool(3, true);
        w.int32(6, 4);
        w.string(7, "c0123");
        w.int64(10, 1_700_000_000_000);
        let c = Calendar::decode(w.as_bytes()).unwrap();
        assert_eq!(c.name, "My Calendar");
        assert_eq!(c.id, "cal-123");
        assert!(c.is_visible);
        assert_eq!(c.color_id, 4);
        assert_eq!(c.chatroom_id, "c0123");
        assert_eq!(c.event_updated_at, 1_700_000_000_000);
    }

    #[test]
    fn decodes_an_event_and_list_wrapper() {
        // Build Event, wrap as chunk { events = [event] }, wrap as response
        // { chunk = 2 }.
        let mut ev = Writer::new();
        ev.string(1, "evt-1");
        ev.string(2, "cal-123");
        ev.int64(3, 1_700_000_000_000);
        ev.int64(4, 1_700_003_600_000);
        ev.string(6, "Lunch");
        ev.bool(7, false);

        let mut chunk = Writer::new();
        chunk.message(1, ev.as_bytes());
        let mut resp = Writer::new();
        resp.message(2, chunk.as_bytes());

        let mut out = Vec::new();
        let mut r = Reader::new(resp.as_bytes());
        while let Some(f) = r.next_field().unwrap() {
            if f.number == 2 {
                let mut c = f.value.as_message().unwrap();
                while let Some(cf) = c.next_field().unwrap() {
                    if cf.number == 1 {
                        out.push(Event::decode(cf.value.as_bytes().unwrap()).unwrap());
                    }
                }
            }
        }
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "evt-1");
        assert_eq!(out[0].title, "Lunch");
        assert_eq!(out[0].end_time, 1_700_003_600_000);
    }

    #[test]
    fn event_ids_look_like_the_apps() {
        let id = new_event_id();
        assert!(id.starts_with("a-"));
        // 16 bytes as unpadded base64url.
        assert_eq!(id.len(), 2 + 22);
        assert!(
            id[2..]
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
        assert_ne!(id, new_event_id());
    }

    #[test]
    fn create_and_update_put_the_trailing_fields_in_different_places() {
        let e = EventInput {
            title: "Lunch".into(),
            start_time: 1,
            end_time: 2,
            timezone: "Asia/Bangkok".into(),
            color_id: 3,
            ..Default::default()
        };
        let fields = |b: &[u8]| {
            let mut r = Reader::new(b);
            let mut n = Vec::new();
            while let Some(f) = r.next_field().unwrap() {
                n.push(f.number);
            }
            n
        };
        assert_eq!(
            fields(&e.encode("a-x", Write::Create, 9, Some(true))),
            [1, 2, 3, 4, 5, 15, 16, 17]
        );
        assert_eq!(
            fields(&e.encode("a-x", Write::Update, 9, Some(false))),
            [1, 2, 3, 4, 5, 16, 17, 18]
        );
        assert_eq!(
            fields(&e.encode("a-x", Write::Create, 9, None)),
            [1, 2, 3, 4, 5, 15, 17]
        );
    }

    #[test]
    fn write_request_wraps_events_for_a_chat() {
        let body = write_request(EventTarget::Chat("c1"), &[vec![0x0a, 0x01, b'x']]);
        assert_eq!(body, [0x12, 0x02, b'c', b'1', 0x1a, 0x03, 0x0a, 0x01, b'x']);
    }

    #[test]
    fn write_response_reports_the_first_error() {
        let mut err = Writer::new();
        err.enumeration(1, 41);
        err.string(2, "no");
        let mut resp = Writer::new();
        resp.message(1, err.as_bytes());
        let e = write_response("UpdateEvents", resp.as_bytes()).unwrap_err();
        assert!(
            e.to_string().contains("EDIT_PERMISSION_DENIED (41) no"),
            "{e}"
        );

        let mut ok = Writer::new();
        ok.int64(2, 77);
        assert_eq!(write_response("CreateEvents", ok.as_bytes()).unwrap(), 77);
    }

    #[test]
    fn next_client_updated_at_is_always_later() {
        let far = 10_i64.pow(15);
        assert_eq!(next_client_updated_at(far), far + 1);
        assert!(next_client_updated_at(0) > 1_700_000_000_000);
    }

    #[test]
    fn list_events_request_encoding() {
        let mut w = Writer::new();
        w.string_always(2, "c0123");
        w.int64(3, 42);
        // field 2 (LEN): tag 0x12, len 5, "c0123"; field 3 (varint): tag 0x18, 42
        assert_eq!(
            w.as_bytes(),
            &[0x12, 0x05, b'c', b'0', b'1', b'2', b'3', 0x18, 42]
        );
    }
}
