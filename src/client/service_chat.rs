//! LINE's service chats (`line.servicechat.v1.ServiceChatService` over gRPC):
//! the chat-list entries for LINE AI, AI Friends and Agent i.
//!
//! Field numbers and message shapes were reverse-engineered from the LINE
//! Android app (26.x) and have not been checked against a live server.
//! Encoding and decoding are by hand via [`crate::protobuf`]. The calls use
//! the app's default gRPC metadata (`x-line-access`), with no channel token.

use crate::base::BaseClient;
use crate::error::Result;
use crate::protobuf::{Reader, Writer};

const SERVICE: &str = "/line.servicechat.v1.ServiceChatService";

/// `ServiceChatState`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ServiceChatState {
    #[default]
    Unspecified,
    Alive,
    Deleted,
    Blocked,
    Unknown(i32),
}

impl From<i32> for ServiceChatState {
    fn from(v: i32) -> Self {
        match v {
            0 => Self::Unspecified,
            1 => Self::Alive,
            2 => Self::Deleted,
            3 => Self::Blocked,
            n => Self::Unknown(n),
        }
    }
}

/// `ServiceType`: which LINE service the chat belongs to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ServiceType {
    #[default]
    Unspecified,
    LineAi,
    AiFriends,
    AgentI,
    Unknown(i32),
}

impl From<i32> for ServiceType {
    fn from(v: i32) -> Self {
        match v {
            0 => Self::Unspecified,
            1 => Self::LineAi,
            2 => Self::AiFriends,
            3 => Self::AgentI,
            n => Self::Unknown(n),
        }
    }
}

/// A service chat as it appears in the chat list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServiceChat {
    pub id: String,
    pub chat_image_url: String,
    pub chat_title: String,
    /// The preview line shown under the title.
    pub service_notification_text: String,
    /// When the preview line last changed, epoch milliseconds (0 if unset).
    pub service_notification_text_updated_at: i64,
    pub show_new_badge: bool,
    pub notification_enabled: bool,
    /// Where tapping the chat goes (a `line://` or https URL).
    pub deeplink_url: String,
    pub state: ServiceChatState,
    pub service_type: ServiceType,
}

/// `google.protobuf.Timestamp` as epoch milliseconds.
fn timestamp_ms(buf: &[u8]) -> Result<i64> {
    let (mut seconds, mut nanos) = (0i64, 0i64);
    let mut r = Reader::new(buf);
    while let Some(f) = r.next_field()? {
        match f.number {
            1 => seconds = f.value.as_i64()?,
            2 => nanos = f.value.as_i32()? as i64,
            _ => {}
        }
    }
    Ok(seconds * 1000 + nanos / 1_000_000)
}

impl ServiceChat {
    /// Decodes a `line.servicechat.v1.ServiceChat` message.
    fn decode(buf: &[u8]) -> Result<Self> {
        let mut c = ServiceChat::default();
        let mut r = Reader::new(buf);
        while let Some(f) = r.next_field()? {
            match f.number {
                1 => c.id = f.value.as_str()?.to_string(),
                2 => c.chat_image_url = f.value.as_str()?.to_string(),
                3 => c.chat_title = f.value.as_str()?.to_string(),
                4 => c.service_notification_text = f.value.as_str()?.to_string(),
                5 => c.service_notification_text_updated_at = timestamp_ms(f.value.as_bytes()?)?,
                6 => c.show_new_badge = f.value.as_bool()?,
                7 => c.notification_enabled = f.value.as_bool()?,
                8 => c.deeplink_url = f.value.as_str()?.to_string(),
                9 => c.state = f.value.as_i32()?.into(),
                10 => c.service_type = f.value.as_i32()?.into(),
                _ => {}
            }
        }
        Ok(c)
    }
}

/// One page of [`BaseClient::full_sync_service_chats`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServiceChatPage {
    pub service_chats: Vec<ServiceChat>,
    /// Pass to the next call; empty on the last page.
    pub next_cursor: String,
}

impl ServiceChatPage {
    fn decode(buf: &[u8]) -> Result<Self> {
        let mut p = ServiceChatPage::default();
        let mut r = Reader::new(buf);
        while let Some(f) = r.next_field()? {
            match f.number {
                1 => p
                    .service_chats
                    .push(ServiceChat::decode(f.value.as_bytes()?)?),
                2 => p.next_cursor = f.value.as_str()?.to_string(),
                _ => {}
            }
        }
        Ok(p)
    }
}

/// Decodes a response whose field 1 is the updated `ServiceChat`.
fn single_chat(buf: &[u8]) -> Result<Option<ServiceChat>> {
    let mut r = Reader::new(buf);
    while let Some(f) = r.next_field()? {
        if f.number == 1 {
            return ServiceChat::decode(f.value.as_bytes()?).map(Some);
        }
    }
    Ok(None)
}

fn id_request(service_chat_id: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.string(1, service_chat_id);
    w.into_bytes()
}

impl BaseClient {
    /// `FullSyncServiceChats`: one page of the user's service chats.
    /// `cursor` is `""` for the first page.
    pub async fn full_sync_service_chats(
        &self,
        page_size: i32,
        cursor: &str,
    ) -> Result<ServiceChatPage> {
        let mut w = Writer::new();
        w.int32(1, page_size);
        w.string(2, cursor);
        let res = self
            .grpc_unary(&format!("{SERVICE}/FullSyncServiceChats"), w.as_bytes())
            .await?;
        ServiceChatPage::decode(&res)
    }

    /// `GetServiceChat`: one service chat by id.
    pub async fn get_service_chat(&self, service_chat_id: &str) -> Result<Option<ServiceChat>> {
        let res = self
            .grpc_unary(
                &format!("{SERVICE}/GetServiceChat"),
                &id_request(service_chat_id),
            )
            .await?;
        single_chat(&res)
    }

    /// `MarkAsRead`: clear the chat's new badge. Returns the updated chat.
    pub async fn mark_service_chat_as_read(
        &self,
        service_chat_id: &str,
    ) -> Result<Option<ServiceChat>> {
        let res = self
            .grpc_unary(
                &format!("{SERVICE}/MarkAsRead"),
                &id_request(service_chat_id),
            )
            .await?;
        single_chat(&res)
    }

    /// `SetNotificationEnabled`: turn the chat's notifications on or off.
    /// Returns the updated chat.
    pub async fn set_service_chat_notification_enabled(
        &self,
        service_chat_id: &str,
        enabled: bool,
    ) -> Result<Option<ServiceChat>> {
        let mut w = Writer::new();
        w.string(1, service_chat_id);
        w.bool(2, enabled);
        let res = self
            .grpc_unary(&format!("{SERVICE}/SetNotificationEnabled"), w.as_bytes())
            .await?;
        single_chat(&res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_page_of_service_chats() {
        let mut ts = Writer::new();
        ts.int64(1, 1_700_000_000);
        ts.int32(2, 250_000_000);
        let mut chat = Writer::new();
        chat.string(1, "sc1");
        chat.string(3, "LINE AI");
        chat.message(5, ts.as_bytes());
        chat.bool(6, true);
        chat.bool(7, true);
        chat.enumeration(9, 1);
        chat.enumeration(10, 3);
        let mut page = Writer::new();
        page.message(1, chat.as_bytes());
        page.string(2, "next");

        let p = ServiceChatPage::decode(page.as_bytes()).unwrap();
        assert_eq!(p.next_cursor, "next");
        let c = &p.service_chats[0];
        assert_eq!(c.id, "sc1");
        assert_eq!(c.chat_title, "LINE AI");
        assert_eq!(c.service_notification_text_updated_at, 1_700_000_000_250);
        assert!(c.show_new_badge && c.notification_enabled);
        assert_eq!(c.state, ServiceChatState::Alive);
        assert_eq!(c.service_type, ServiceType::AgentI);
    }

    #[test]
    fn single_chat_response() {
        let mut chat = Writer::new();
        chat.string(1, "sc1");
        let mut res = Writer::new();
        res.message(1, chat.as_bytes());
        assert_eq!(single_chat(res.as_bytes()).unwrap().unwrap().id, "sc1");
        assert_eq!(single_chat(&[]).unwrap(), None);
    }
}
