//! Which upload flow a chat's media uses (`q31/a`, `r31/*`).
//!
//! LINE decides per chat and content type whether media is sent plain
//! ([`MediaFlow::V1`]: upload to `m/reqseq` and the server creates the
//! message) or with Letter Sealing ([`MediaFlow::V2`]: encrypted upload to
//! `em*` and a `sendMessage` carrying the key). The answer comes from
//! `determineMediaMessageFlow(chatMid)` and is cached for `cacheTtlMillis`.
//! A content type missing from the answer uses V1. Chats where E2EE media
//! is not allowed (official accounts, big groups, members without Letter
//! Sealing) get V1.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::ObjType;
use crate::thrift::Value;

/// One upload flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaFlow {
    /// Plain upload; the server creates the message.
    #[default]
    V1,
    /// E2EE upload, then `sendMessage` with the key material.
    V2,
}

impl MediaFlow {
    /// `MediaMessageFlow` value: 2 is V2; 1, unknown and missing are V1.
    fn from_value(v: Option<i64>) -> Self {
        if v == Some(2) {
            MediaFlow::V2
        } else {
            MediaFlow::V1
        }
    }
}

/// The flow of each media kind in one chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MediaFlows {
    pub image: MediaFlow,
    pub video: MediaFlow,
    pub audio: MediaFlow,
    pub file: MediaFlow,
}

impl MediaFlows {
    /// All kinds use the same flow.
    pub fn all(flow: MediaFlow) -> Self {
        Self {
            image: flow,
            video: flow,
            audio: flow,
            file: flow,
        }
    }

    /// The flow for `o_type` (GIF goes as an image).
    pub fn get(&self, o_type: ObjType) -> MediaFlow {
        match o_type {
            ObjType::Image | ObjType::Gif => self.image,
            ObjType::Video => self.video,
            ObjType::Audio => self.audio,
            ObjType::File => self.file,
        }
    }

    /// Read a `DetermineMediaMessageFlowResponse`: the flows and how long
    /// they may be cached. `flowMap` is keyed by `ContentType` number.
    pub fn from_response(res: &Value) -> (Self, Duration) {
        let entries: &[(Value, Value)] = match &res["flowMap"] {
            Value::Map(entries) => entries,
            _ => &[],
        };
        let lookup = |content_type: i64| {
            MediaFlow::from_value(
                entries
                    .iter()
                    .find(|(k, _)| k.as_i64() == Some(content_type))
                    .and_then(|(_, v)| v.as_i64()),
            )
        };
        let ttl = res["cacheTtlMillis"].as_i64().unwrap_or(0).max(0) as u64;
        (
            Self {
                image: lookup(1),
                video: lookup(2),
                audio: lookup(3),
                file: lookup(14),
            },
            Duration::from_millis(ttl),
        )
    }
}

/// Flows per chat until their TTL runs out (in memory, like the app's
/// `ObsFlowCacheDatabase` minus persistence).
#[derive(Debug, Default)]
pub(crate) struct MediaFlowCache {
    entries: Mutex<HashMap<String, (MediaFlows, Instant)>>,
}

impl MediaFlowCache {
    pub fn get(&self, chat: &str, now: Instant) -> Option<MediaFlows> {
        let mut entries = self.entries.lock().unwrap();
        match entries.get(chat) {
            Some((flows, until)) if now < *until => Some(*flows),
            Some(_) => {
                entries.remove(chat);
                None
            }
            None => None,
        }
    }

    pub fn insert(&self, chat: &str, flows: MediaFlows, ttl: Duration, now: Instant) {
        if ttl.is_zero() {
            return;
        }
        self.entries
            .lock()
            .unwrap()
            .insert(chat.to_string(), (flows, now + ttl));
    }

    /// Forget one chat, or every chat with `None`.
    pub fn remove(&self, chat: Option<&str>) {
        let mut entries = self.entries.lock().unwrap();
        match chat {
            Some(chat) => {
                entries.remove(chat);
            }
            None => entries.clear(),
        }
    }
}
