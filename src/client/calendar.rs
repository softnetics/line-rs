//! LINE Calendar (`line.calendar.bff.*` gRPC services).
//!
//! Field numbers and message shapes were reverse-engineered from the LINE
//! Android app (26.11.0). Encoding and decoding are by hand via
//! [`crate::protobuf`]; see that module for why there is no codegen.
//!
//! Covers listing calendars and events, and creating, updating and deleting
//! events (`EventService/CreateEvents`, `UpdateEvents`, `DeleteEvents`),
//! with their recurrence, changed occurrences, invitees and reminders.

use crate::base::BaseClient;
use crate::error::{Error, Result};
use crate::grpc::{self, GrpcRequest};
use crate::protobuf::{Reader, Value, Writer, repeated_varints};
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
    /// Raw `CalendarType`: [`CALENDAR_PERSONAL`] (1) or
    /// [`CALENDAR_CHATROOM`] (2); 0 is unspecified.
    pub kind: i32,
    pub color_id: i32,
    /// Group/room/user mid for a chat calendar; empty for a personal one.
    pub chatroom_id: String,
    pub event_updated_at: i64,
    pub client_updated_at: i64,
    pub created_at: i64,
    pub is_reminder_enabled: bool,
    /// The calendar the app made for the user on first use.
    pub is_initial: bool,
    /// False for a calendar the user has hidden from the calendar list.
    pub is_shown_in_calendar_list: bool,
    pub to_app_id: String,
}

/// [`Calendar::kind`] of the user's own calendars.
pub const CALENDAR_PERSONAL: i32 = 1;
/// [`Calendar::kind`] of a chat's calendar.
pub const CALENDAR_CHATROOM: i32 = 2;

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
                14 => c.is_initial = f.value.as_bool()?,
                15 => c.is_shown_in_calendar_list = f.value.as_bool()?,
                16 => c.to_app_id = f.value.as_str()?.to_string(),
                _ => {}
            }
        }
        Ok(c)
    }
}

/// How often a recurring event repeats (`Recurrence.frequency`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Frequency {
    #[default]
    Unspecified,
    Daily,
    Weekly,
    Monthly,
    Yearly,
    /// A value this library doesn't know.
    Other(i32),
}

impl Frequency {
    pub fn from_raw(v: i32) -> Self {
        match v {
            0 => Self::Unspecified,
            1 => Self::Daily,
            2 => Self::Weekly,
            3 => Self::Monthly,
            4 => Self::Yearly,
            v => Self::Other(v),
        }
    }

    pub fn raw(self) -> i32 {
        match self {
            Self::Unspecified => 0,
            Self::Daily => 1,
            Self::Weekly => 2,
            Self::Monthly => 3,
            Self::Yearly => 4,
            Self::Other(v) => v,
        }
    }
}

/// How an event repeats (`line.calendar.bff.Recurrence`), an iCalendar
/// RRULE in parts: the app turns it into `FREQ=…;INTERVAL=…;UNTIL=…;BYDAY=…`
/// and expands it with ical4j.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recurrence {
    pub frequency: Frequency,
    /// Every `interval` days/weeks/months/years; 0 or 1 is every one.
    pub interval: u32,
    /// The last moment an occurrence may start, epoch seconds (UTC).
    pub until: Option<i64>,
    /// iCalendar weekdays: `MO`…`SU` for a weekly event, or an ordinal and a
    /// day such as `2TU` or `-1FR` for "the second Tuesday" / "the last
    /// Friday" of a monthly one. Empty means the start's own day.
    pub by_day: Vec<String>,
    /// Occurrences left out of the series, by their original start (epoch
    /// seconds, the same value as [`ChildEvent::recur_id`]).
    pub except_ids: Vec<i64>,
}

impl Recurrence {
    fn decode(buf: &[u8]) -> Result<Self> {
        let mut out = Recurrence::default();
        let mut r = Reader::new(buf);
        while let Some(f) = r.next_field()? {
            match f.number {
                1 => out.frequency = Frequency::from_raw(f.value.as_i32()?),
                2 => out.interval = f.value.as_u64()? as u32,
                3 => out.until = Some(f.value.as_i64()?),
                4 => out.by_day.push(f.value.as_str()?.to_string()),
                5 => out
                    .except_ids
                    .extend(repeated_varints(f.value)?.into_iter().map(|v| v as i64)),
                _ => {}
            }
        }
        Ok(out)
    }
}

/// When LINE reminds the user of an event (`line.calendar.bff.Reminder`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reminder {
    OneHourBefore,
    TwoHoursBefore,
    OneDayBefore,
    TwoDaysBefore,
    /// At 12:00 on the day before.
    NoonTheDayBefore,
    /// At 12:00 two days before.
    NoonTwoDaysBefore,
    /// A value this library doesn't know (kept so an edit sends it back).
    Other(i32),
}

impl Reminder {
    pub fn from_raw(v: i32) -> Self {
        match v {
            1 => Self::OneHourBefore,
            2 => Self::NoonTheDayBefore,
            3 => Self::TwoHoursBefore,
            4 => Self::OneDayBefore,
            5 => Self::TwoDaysBefore,
            6 => Self::NoonTwoDaysBefore,
            v => Self::Other(v),
        }
    }

    pub fn raw(self) -> i32 {
        match self {
            Self::OneHourBefore => 1,
            Self::NoonTheDayBefore => 2,
            Self::TwoHoursBefore => 3,
            Self::OneDayBefore => 4,
            Self::TwoDaysBefore => 5,
            Self::NoonTwoDaysBefore => 6,
            Self::Other(v) => v,
        }
    }
}

/// An invitee's answer (`line.calendar.bff.InvitationStatus`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InvitationStatus {
    #[default]
    Unspecified,
    NeedsAction,
    Accepted,
    Declined,
    Other(i32),
}

impl InvitationStatus {
    pub fn from_raw(v: i32) -> Self {
        match v {
            0 => Self::Unspecified,
            1 => Self::NeedsAction,
            2 => Self::Accepted,
            3 => Self::Declined,
            v => Self::Other(v),
        }
    }

    pub fn raw(self) -> i32 {
        match self {
            Self::Unspecified => 0,
            Self::NeedsAction => 1,
            Self::Accepted => 2,
            Self::Declined => 3,
            Self::Other(v) => v,
        }
    }
}

/// Someone invited to an event, and their answer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Invitee {
    /// The invitee's mid.
    pub invitee_id: String,
    pub status: InvitationStatus,
    pub client_updated_at: i64,
}

impl Invitee {
    fn decode(buf: &[u8]) -> Result<Self> {
        let mut out = Invitee::default();
        let mut r = Reader::new(buf);
        while let Some(f) = r.next_field()? {
            match f.number {
                1 => out.invitee_id = f.value.as_str()?.to_string(),
                3 => out.status = InvitationStatus::from_raw(f.value.as_i32()?),
                4 => out.client_updated_at = f.value.as_i64()?,
                _ => {}
            }
        }
        Ok(out)
    }
}

fn reminders(value: Value<'_>) -> Result<Vec<Reminder>> {
    Ok(repeated_varints(value)?
        .into_iter()
        .map(|v| Reminder::from_raw(v as i32))
        .collect())
}

/// `AppReminder { offset = 1 (packed int32) }`: the app's own reminder
/// offsets. The unit is not known (minutes, probably); they are kept only
/// so that an edit sends them back unchanged.
fn app_reminders(buf: &[u8]) -> Result<Vec<i32>> {
    let mut out = Vec::new();
    let mut r = Reader::new(buf);
    while let Some(f) = r.next_field()? {
        if f.number == 1 {
            out.extend(repeated_varints(f.value)?.into_iter().map(|v| v as i32));
        }
    }
    Ok(out)
}

/// One occurrence of a recurring event that was changed on its own
/// (`line.calendar.bff.RecurrenceChildEvent`). It replaces the occurrence
/// that would have started at [`recur_id`](Self::recur_id).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChildEvent {
    /// The original start of the occurrence it replaces, epoch seconds.
    pub recur_id: i64,
    pub start_time: i64,
    pub end_time: i64,
    pub timezone: String,
    pub title: String,
    pub is_all_day: bool,
    pub location: String,
    pub url: String,
    pub description: String,
    pub invitees: Vec<Invitee>,
    pub reminders: Vec<Reminder>,
    /// The occurrence was deleted.
    pub is_deleted: bool,
    pub message_id: Option<String>,
    pub client_updated_at: i64,
    pub is_editable_in_line_calendar: bool,
    pub creator_id: String,
    pub color_id: i32,
    pub app_reminders: Option<Vec<i32>>,
    pub stamp_id: String,
    pub created_at: i64,
}

impl ChildEvent {
    fn decode(buf: &[u8]) -> Result<Self> {
        let mut c = ChildEvent::default();
        let mut r = Reader::new(buf);
        while let Some(f) = r.next_field()? {
            match f.number {
                1 => c.start_time = f.value.as_i64()?,
                2 => c.end_time = f.value.as_i64()?,
                3 => c.timezone = f.value.as_str()?.to_string(),
                4 => c.title = f.value.as_str()?.to_string(),
                5 => c.is_all_day = f.value.as_bool()?,
                6 => c.location = f.value.as_str()?.to_string(),
                7 => c.url = f.value.as_str()?.to_string(),
                8 => c.description = f.value.as_str()?.to_string(),
                9 => c.invitees.push(Invitee::decode(f.value.as_bytes()?)?),
                10 => c.reminders.extend(reminders(f.value)?),
                11 => c.is_deleted = f.value.as_bool()?,
                12 => c.message_id = Some(f.value.as_str()?.to_string()),
                13 => c.recur_id = f.value.as_i64()?,
                14 => c.client_updated_at = f.value.as_i64()?,
                15 => c.is_editable_in_line_calendar = f.value.as_bool()?,
                16 => c.creator_id = f.value.as_str()?.to_string(),
                17 => c.color_id = f.value.as_i32()?,
                18 => c.app_reminders = Some(app_reminders(f.value.as_bytes()?)?),
                19 => c.stamp_id = f.value.as_str()?.to_string(),
                20 => c.created_at = f.value.as_i64()?,
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
    /// Start/end as epoch seconds. For a recurring event, the first
    /// occurrence.
    pub start_time: i64,
    pub end_time: i64,
    pub timezone: String,
    pub title: String,
    pub is_all_day: bool,
    pub location: String,
    pub url: String,
    pub description: String,
    /// How the event repeats; `None` for a one-off.
    pub recurrence: Option<Recurrence>,
    pub invitees: Vec<Invitee>,
    pub reminders: Vec<Reminder>,
    pub is_deleted: bool,
    /// The chat message this event was created from, if any.
    pub message_id: Option<String>,
    /// Occurrences of a recurring event changed or deleted on their own.
    pub recurrence_child_events: Vec<ChildEvent>,
    pub client_updated_at: i64,
    pub is_editable_in_line_calendar: bool,
    pub creator_id: String,
    pub color_id: i32,
    /// See [`ChildEvent::app_reminders`].
    pub app_reminders: Option<Vec<i32>>,
    pub stamp_id: String,
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
                11 => e.recurrence = Some(Recurrence::decode(f.value.as_bytes()?)?),
                12 => e.invitees.push(Invitee::decode(f.value.as_bytes()?)?),
                13 => e.reminders.extend(reminders(f.value)?),
                14 => e.is_deleted = f.value.as_bool()?,
                15 => e.message_id = Some(f.value.as_str()?.to_string()),
                16 => e
                    .recurrence_child_events
                    .push(ChildEvent::decode(f.value.as_bytes()?)?),
                17 => e.client_updated_at = f.value.as_i64()?,
                18 => e.is_editable_in_line_calendar = f.value.as_bool()?,
                19 => e.creator_id = f.value.as_str()?.to_string(),
                20 => e.color_id = f.value.as_i32()?,
                21 => e.app_reminders = Some(app_reminders(f.value.as_bytes()?)?),
                22 => e.stamp_id = f.value.as_str()?.to_string(),
                23 => e.created_at = f.value.as_i64()?,
                _ => {}
            }
        }
        Ok(e)
    }
}

impl BaseClient {
    /// A channel access token for the LINE Calendar channel, for the
    /// `x-line-channeltoken` header. Kept until its `expiration`, as the app
    /// keeps it (`ak8/m`); `fresh` issues a new one regardless. Issuing
    /// tries `issueChannelToken` and falls back to
    /// `approveChannelAndIssueChannelToken` (first use grants consent, as
    /// the app does automatically for this first-party channel).
    ///
    /// The cache lock is held while issuing, so calls made at once (every
    /// calendar of the all-calendars view) share one token.
    async fn calendar_channel_token(&self, fresh: bool) -> Result<String> {
        let mut cache = self.calendar_token.lock().await;
        let now = now_ms();
        if !fresh
            && let Some((token, expires)) = cache.as_ref()
            && token_usable(*expires, now)
        {
            return Ok(token.clone());
        }
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
            .unwrap_or_default()
            .to_string();
        let expires = token["expiration"].as_i64().unwrap_or_default();
        *cache = (!value.is_empty()).then(|| (value.clone(), cache_until(expires, now)));
        Ok(value)
    }

    /// gRPC metadata headers: the same identity headers as a Thrift call, but
    /// split out the way the app sends them for gRPC (not the tab-joined
    /// `x-line-application`), plus the LINE Calendar channel token.
    async fn grpc_headers(&self, fresh_token: bool) -> Result<Vec<(String, String)>> {
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
            ("x-lal".to_string(), self.config.language.clone()),
        ];
        if let Some(token) = self.auth_token() {
            h.push(("x-line-access".to_string(), token));
        }
        h.push((
            "x-line-channeltoken".to_string(),
            self.calendar_channel_token(fresh_token).await?,
        ));
        Ok(h)
    }

    /// One LINE Calendar call, decoded by `decode`. When the server turns
    /// down the channel token (`AUTHENTICATION_REQUIRED`, or gRPC
    /// `UNAUTHENTICATED`), the token is issued again and the call made once
    /// more.
    async fn calendar_call<T>(
        &self,
        path: &str,
        body: &[u8],
        decode: impl Fn(Vec<bytes::Bytes>) -> Result<T>,
    ) -> Result<T> {
        let mut fresh = false;
        loop {
            let headers = self.grpc_headers(fresh).await?;
            let result = grpc::call(GrpcRequest {
                host: self.current_endpoint(),
                path,
                headers: &headers,
                body,
            })
            .await
            .and_then(&decode);
            match result {
                Err(e) if !fresh && is_token_error(&e) => fresh = true,
                other => return other,
            }
        }
    }

    async fn grpc_unary<T>(
        &self,
        path: &str,
        body: &[u8],
        decode: impl Fn(&[u8]) -> Result<T>,
    ) -> Result<T> {
        self.calendar_call(path, body, |mut msgs| {
            decode(&msgs.drain(..).next().unwrap_or_default())
        })
        .await
    }

    /// `CalendarService/ListAllCalendars`: every calendar the user can see.
    pub async fn list_all_calendars(&self) -> Result<Vec<Calendar>> {
        let path = format!("{CALENDAR_SERVICE}/ListAllCalendars");
        // Request is empty (a bare ListAllCalendarsRequest).
        // Response: { meta = 1, calendars = repeated 2 }.
        self.grpc_unary(&path, &[], |resp| {
            let mut out = Vec::new();
            let mut r = Reader::new(resp);
            while let Some(f) = r.next_field()? {
                if f.number == 2 {
                    out.push(Calendar::decode(f.value.as_bytes()?)?);
                }
            }
            Ok(out)
        })
        .await
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
        self.grpc_unary(&path, &write_request(target, &[body]), |resp| {
            write_response("CreateEvents", resp)
        })
        .await
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
        self.grpc_unary(&path, &write_request(target, &[body]), |resp| {
            write_response("UpdateEvents", resp)
        })
        .await
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
        self.grpc_unary(&path, &write_request(target, &[w.into_bytes()]), |resp| {
            write_response("DeleteEvents", resp)
        })
        .await
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

        let path = format!("{EVENT_SERVICE}/ListEventsStream");
        self.calendar_call(&path, w.as_bytes(), |chunks| {
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
        })
        .await
    }
}

/// The fields of an event to create, or to replace on update. LINE's
/// update takes the whole event, not a diff: build an edit with
/// [`EventInput::from_event`] so the fields you don't touch go back as they
/// were.
///
/// Times are epoch seconds. An all-day event starts at midnight of its
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
    /// How the event repeats. On update, LINE for Android sends the rule
    /// without `except_ids` (the server keeps the series' exceptions), and
    /// so does this.
    pub recurrence: Option<Recurrence>,
    pub reminders: Vec<Reminder>,
    /// Invitee mids the event should have.
    pub invitees: Vec<String>,
    /// The invitees the event had when fetched. An update sends what was
    /// added to and removed from this list (`add_invitees`,
    /// `remove_invitees`); a create ignores it.
    pub original_invitees: Vec<String>,
    /// The chat message the event was made from.
    pub message_id: Option<String>,
    pub stamp_id: String,
    /// See [`Event::app_reminders`].
    pub app_reminders: Option<Vec<i32>>,
}

impl EventInput {
    /// The event as it is, ready to change and send back with
    /// [`BaseClient::update_event`].
    pub fn from_event(e: &Event) -> Self {
        let invitees: Vec<String> = e.invitees.iter().map(|i| i.invitee_id.clone()).collect();
        EventInput {
            title: e.title.clone(),
            start_time: e.start_time,
            end_time: e.end_time,
            timezone: e.timezone.clone(),
            is_all_day: e.is_all_day,
            location: e.location.clone(),
            url: e.url.clone(),
            description: e.description.clone(),
            color_id: e.color_id,
            recurrence: e.recurrence.clone(),
            reminders: e.reminders.clone(),
            original_invitees: invitees.clone(),
            invitees,
            message_id: e.message_id.clone(),
            stamp_id: e.stamp_id.clone(),
            app_reminders: e.app_reminders.clone(),
        }
    }

    /// The `line.calendar.bff` create (`CreateEvent`) or update
    /// (`UpdateEvent`) message. Both share fields 1 to 10; from there the
    /// update has one more invitee list, so the rest sit one number later.
    fn encode(
        &self,
        id: &str,
        write: Write,
        client_updated_at: i64,
        notify: Option<bool>,
    ) -> Vec<u8> {
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
        if let Some(r) = &self.recurrence {
            w.message(10, &r.encode(matches!(write, Write::Create)));
        }
        let reminders = self.reminders.iter().map(|r| r.raw() as i64 as u64);
        let next = match write {
            Write::Create => {
                for id in &self.invitees {
                    // `{ invitee_id = 1, status = 3 }`
                    let mut i = Writer::new();
                    i.string_always(1, id);
                    i.enumeration(3, InvitationStatus::NeedsAction.raw());
                    w.message(11, i.as_bytes());
                }
                w.packed_varints(12, reminders);
                13
            }
            Write::Update => {
                let added = self
                    .invitees
                    .iter()
                    .filter(|i| !self.original_invitees.contains(i));
                let removed = self
                    .original_invitees
                    .iter()
                    .filter(|i| !self.invitees.contains(i));
                for (field, ids) in [(11, added.collect::<Vec<_>>()), (12, removed.collect())] {
                    for id in ids {
                        let mut i = Writer::new();
                        i.string_always(1, id);
                        w.message(field, i.as_bytes());
                    }
                }
                w.packed_varints(13, reminders);
                14
            }
        };
        // Then: message_id, recurrence_child_events (not sent; the app
        // leaves them out of an update too), client_updated_at,
        // notify_to_chatroom, color_id, app_reminders, stamp_id.
        if let Some(m) = &self.message_id {
            w.string_always(next, m);
        }
        w.int64(next + 2, client_updated_at);
        if let Some(n) = notify {
            w.bool_always(next + 3, n);
        }
        w.uint32(next + 4, self.color_id.max(0) as u32);
        if let Some(offsets) = &self.app_reminders {
            let mut a = Writer::new();
            a.packed_varints(1, offsets.iter().map(|o| *o as i64 as u64));
            w.message(next + 5, a.as_bytes());
        }
        if !self.stamp_id.is_empty() {
            w.string_always(next + 6, &self.stamp_id);
        }
        w.into_bytes()
    }
}

impl Recurrence {
    /// `Recurrence`, with `except_ids` only when `with_except_ids`.
    fn encode(&self, with_except_ids: bool) -> Vec<u8> {
        let mut w = Writer::new();
        w.enumeration(1, self.frequency.raw());
        w.uint32(2, self.interval);
        if let Some(until) = self.until.filter(|u| *u > 0) {
            w.int64_always(3, until);
        }
        for d in &self.by_day {
            w.string_always(4, d);
        }
        if with_except_ids {
            w.packed_varints(5, self.except_ids.iter().map(|v| *v as u64));
        }
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

/// A `client_updated_at` the server accepts: now in epoch seconds, but always
/// later than the event's previous value (older ones fail with
/// `CLIENT_UPDATED_AT_OUTDATED`, and over a day ahead with
/// `INVALID_CLIENT_UPDATED_AT`).
fn next_client_updated_at(previous: i64) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    now.max(previous + 1)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// How long to keep a token whose answer had no `expiration`.
const TOKEN_FALLBACK_TTL_MS: i64 = 10 * 60 * 1000;
/// A token this close to its expiry is issued again rather than sent.
const TOKEN_MARGIN_MS: i64 = 60 * 1000;

/// When to stop using a token the server said expires at `expiration`
/// (epoch ms; 0 when it didn't say).
fn cache_until(expiration: i64, now: i64) -> i64 {
    if expiration > 0 {
        expiration
    } else {
        now + TOKEN_FALLBACK_TTL_MS
    }
}

/// The app sends a cached token while `expiration >= now` (`ak8/m` `e()`);
/// this stops a minute early so a call doesn't race the expiry.
fn token_usable(expires: i64, now: i64) -> bool {
    expires - TOKEN_MARGIN_MS >= now
}

/// Whether LINE Calendar turned down the channel token: the BFF's
/// `AUTHENTICATION_REQUIRED` (71) in a write's errors, or gRPC status 16
/// (`UNAUTHENTICATED`).
fn is_token_error(e: &Error) -> bool {
    let text = e.to_string();
    text.contains("AUTHENTICATION_REQUIRED") || text.contains("gRPC status 16")
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
    fn decodes_recurrence_children_invitees_and_reminders() {
        let mut rec = Writer::new();
        rec.enumeration(1, 2);
        rec.uint32(2, 2);
        rec.int64(3, 1_800_000_000);
        rec.string(4, "MO");
        rec.string(4, "WE");
        rec.packed_varints(5, [1_700_604_800, 1_701_209_600]);

        let mut inv = Writer::new();
        inv.string(1, "u1");
        inv.enumeration(3, 2);
        inv.int64(4, 5);

        let mut child = Writer::new();
        child.int64(1, 1_700_608_400);
        child.int64(2, 1_700_612_000);
        child.string(4, "Moved");
        child.bool(11, false);
        child.int64(13, 1_700_604_800);
        child.packed_varints(10, [4]);

        let mut app = Writer::new();
        app.packed_varints(1, [(-30_i64) as u64, 0]);

        let mut ev = Writer::new();
        ev.string(1, "evt-1");
        ev.int64(3, 1_700_000_000);
        ev.message(11, rec.as_bytes());
        ev.message(12, inv.as_bytes());
        ev.packed_varints(13, [1, 2]);
        // A sender may also write a repeated enum unpacked.
        ev.enumeration(13, 6);
        ev.string(15, "msg-9");
        ev.message(16, child.as_bytes());
        ev.int32(20, 3);
        ev.message(21, app.as_bytes());
        ev.string(22, "stamp-1");

        let e = Event::decode(ev.as_bytes()).unwrap();
        let r = e.recurrence.as_ref().unwrap();
        assert_eq!(r.frequency, Frequency::Weekly);
        assert_eq!(r.interval, 2);
        assert_eq!(r.until, Some(1_800_000_000));
        assert_eq!(r.by_day, ["MO", "WE"]);
        assert_eq!(r.except_ids, [1_700_604_800, 1_701_209_600]);
        assert_eq!(e.invitees[0].invitee_id, "u1");
        assert_eq!(e.invitees[0].status, InvitationStatus::Accepted);
        assert_eq!(
            e.reminders,
            [
                Reminder::OneHourBefore,
                Reminder::NoonTheDayBefore,
                Reminder::NoonTwoDaysBefore
            ]
        );
        assert_eq!(e.message_id.as_deref(), Some("msg-9"));
        assert_eq!(e.recurrence_child_events.len(), 1);
        let c = &e.recurrence_child_events[0];
        assert_eq!(
            (c.recur_id, c.title.as_str(), c.reminders.as_slice()),
            (1_700_604_800, "Moved", &[Reminder::OneDayBefore][..])
        );
        assert_eq!(e.color_id, 3);
        assert_eq!(e.app_reminders.as_deref(), Some(&[-30, 0][..]));
        assert_eq!(e.stamp_id, "stamp-1");

        let plain = Event::decode(&[]).unwrap();
        assert!(plain.recurrence.is_none() && plain.message_id.is_none());
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

    fn field_numbers(b: &[u8]) -> Vec<u32> {
        let mut r = Reader::new(b);
        let mut n = Vec::new();
        while let Some(f) = r.next_field().unwrap() {
            n.push(f.number);
        }
        n
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
        assert_eq!(
            field_numbers(&e.encode("a-x", Write::Create, 9, Some(true))),
            [1, 2, 3, 4, 5, 15, 16, 17]
        );
        assert_eq!(
            field_numbers(&e.encode("a-x", Write::Update, 9, Some(false))),
            [1, 2, 3, 4, 5, 16, 17, 18]
        );
        assert_eq!(
            field_numbers(&e.encode("a-x", Write::Create, 9, None)),
            [1, 2, 3, 4, 5, 15, 17]
        );
    }

    /// A fetched event, edited and sent back, keeps every field an update
    /// can carry.
    #[test]
    fn an_update_sends_the_whole_fetched_event_back() {
        let fetched = Event {
            id: "a-1".into(),
            start_time: 100,
            end_time: 200,
            timezone: "Asia/Tokyo".into(),
            title: "Standup".into(),
            recurrence: Some(Recurrence {
                frequency: Frequency::Weekly,
                interval: 1,
                until: Some(9_000),
                by_day: vec!["MO".into(), "TH".into()],
                except_ids: vec![700],
            }),
            invitees: vec![
                Invitee {
                    invitee_id: "u1".into(),
                    ..Default::default()
                },
                Invitee {
                    invitee_id: "u2".into(),
                    ..Default::default()
                },
            ],
            reminders: vec![Reminder::OneHourBefore, Reminder::Other(9)],
            message_id: Some("m1".into()),
            color_id: 5,
            app_reminders: Some(vec![-10]),
            stamp_id: "s1".into(),
            ..Default::default()
        };
        let mut input = EventInput::from_event(&fetched);
        input.title = "Daily standup".into();
        input.invitees.retain(|i| i != "u1");
        input.invitees.push("u3".into());
        let body = input.encode("a-1", Write::Update, 42, None);
        assert_eq!(
            field_numbers(&body),
            [1, 2, 3, 4, 5, 10, 11, 12, 13, 14, 16, 18, 19, 20]
        );

        let mut r = Reader::new(&body);
        while let Some(f) = r.next_field().unwrap() {
            match f.number {
                5 => assert_eq!(f.value.as_str().unwrap(), "Daily standup"),
                10 => {
                    let rec = Recurrence::decode(f.value.as_bytes().unwrap()).unwrap();
                    // Exceptions stay on the server; the rule goes back.
                    assert_eq!(
                        rec,
                        Recurrence {
                            except_ids: vec![],
                            ..fetched.recurrence.clone().unwrap()
                        }
                    );
                }
                11 | 12 => {
                    let mut i = f.value.as_message().unwrap();
                    let id = i.next_field().unwrap().unwrap().value.as_str().unwrap();
                    assert_eq!(id, if f.number == 11 { "u3" } else { "u1" });
                }
                13 => assert_eq!(repeated_varints(f.value).unwrap(), [1, 9]),
                14 => assert_eq!(f.value.as_str().unwrap(), "m1"),
                16 => assert_eq!(f.value.as_i64().unwrap(), 42),
                18 => assert_eq!(f.value.as_i32().unwrap(), 5),
                19 => assert_eq!(app_reminders(f.value.as_bytes().unwrap()).unwrap(), [-10]),
                20 => assert_eq!(f.value.as_str().unwrap(), "s1"),
                _ => {}
            }
        }

        // A create sends the rule with its exceptions, and invitees whole.
        let body = input.encode("a-2", Write::Create, 42, Some(true));
        assert_eq!(
            field_numbers(&body),
            [1, 2, 3, 4, 5, 10, 11, 11, 12, 13, 15, 16, 17, 18, 19]
        );
        let mut r = Reader::new(&body);
        while let Some(f) = r.next_field().unwrap() {
            if f.number == 10 {
                let rec = Recurrence::decode(f.value.as_bytes().unwrap()).unwrap();
                assert_eq!(rec.except_ids, [700]);
            }
        }
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
        let far = 10_i64.pow(12);
        assert_eq!(next_client_updated_at(far), far + 1);
        let now = next_client_updated_at(0);
        // Seconds, not milliseconds: the server rejects a value over a day ahead.
        assert!(now > 1_700_000_000 && now < 10_000_000_000, "{now}");
    }

    #[test]
    fn channel_tokens_are_kept_until_they_expire() {
        let now = 1_800_000_000_000;
        // The server's expiry, in milliseconds.
        let until = cache_until(now + 3_600_000, now);
        assert!(token_usable(until, now));
        assert!(token_usable(until, now + 3_500_000));
        assert!(!token_usable(until, now + 3_590_000));
        assert!(!token_usable(until, now + 3_600_000));
        // No expiry given: kept for a while, not forever.
        let until = cache_until(0, now);
        assert!(token_usable(until, now + 60_000));
        assert!(!token_usable(until, now + 3_600_000));
    }

    #[test]
    fn token_errors_are_told_apart() {
        let mut err = Writer::new();
        err.enumeration(1, 71);
        let mut resp = Writer::new();
        resp.message(1, err.as_bytes());
        let e = write_response("UpdateEvents", resp.as_bytes()).unwrap_err();
        assert!(is_token_error(&e), "{e}");
        assert!(is_token_error(&Error::internal(
            "GrpcError",
            "gRPC status 16: token expired"
        )));
        assert!(!is_token_error(&Error::internal(
            "GrpcError",
            "gRPC status 14"
        )));
        let mut err = Writer::new();
        err.enumeration(1, 33);
        let mut resp = Writer::new();
        resp.message(1, err.as_bytes());
        let e = write_response("UpdateEvents", resp.as_bytes()).unwrap_err();
        assert!(!is_token_error(&e));
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
