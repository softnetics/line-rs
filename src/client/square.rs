//! [`Square`] and [`SquareChat`]: OpenChat objects (linejs
//! `features/square`).

use crate::obs::{ObjType, ObsFile};
use std::collections::VecDeque;
use std::time::Duration;

use futures::Stream;

use super::message::{MessageInput, SquareMessage};
use super::{continue_request, list, Client};
use crate::error::Result;
use crate::thrift::Value;
use crate::tv;

/// An OpenChat community (not a chat room). `raw` is the thrift `Square`.
#[derive(Clone)]
pub struct Square {
    client: Client,
    pub raw: Value,
}

impl std::fmt::Debug for Square {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Square").field("raw", &self.raw).finish()
    }
}

/// Shallow `{...base, ...patch}`.
fn merged(base: &Value, patch: Value) -> Value {
    let mut out = if base.as_struct().is_some() { base.clone() } else { Value::object() };
    for (k, v) in patch.entries() {
        out.insert(k, v.clone());
    }
    out
}

impl Square {
    pub fn new(client: Client, raw: Value) -> Self {
        Self { client, raw }
    }

    pub fn mid(&self) -> &str {
        self.raw["mid"].as_str().unwrap_or("")
    }

    pub fn name(&self) -> &str {
        self.raw["name"].as_str().unwrap_or("")
    }

    /// Re-fetch `raw`.
    pub async fn update(&mut self) -> Result<()> {
        let res = self.client.base.square().call("getSquare", tv!({"request": {"mid": self.mid()}})).await?;
        self.raw = res["square"].clone();
        Ok(())
    }

    /// `updateSquare` with `SquareAttribute` names and a partial `Square`
    /// merged over `raw`.
    pub async fn update_square(&self, updated_attrs: &[&str], square: Value) -> Result<Value> {
        let mut request = tv!({"updatedAttrs": updated_attrs});
        request.insert("square", merged(&self.raw, square));
        let mut args = Value::object();
        args.insert("request", request);
        self.client.base.square().call("updateSquare", args).await
    }

    pub async fn update_name(&self, name: &str) -> Result<Value> {
        self.update_square(&["NAME"], tv!({"name": name})).await
    }
}

/// Events yielded by [`SquareChat::listen`].
#[derive(Debug, Clone)]
pub enum SquareChatEvent {
    /// Every raw `SquareEvent`.
    Event(Value),
    Message(SquareMessage),
    /// A received message that mentions the logged-in user.
    Mention(SquareMessage),
    Kick(Value),
    Leave(Value),
    Join(Value),
    Destroy(Value),
    /// The sync token moved; persist it to resume later.
    SyncToken(String),
}

/// An OpenChat room. `raw` is the thrift `SquareChat`.
#[derive(Clone)]
pub struct SquareChat {
    client: Client,
    pub raw: Value,
}

impl std::fmt::Debug for SquareChat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SquareChat").field("raw", &self.raw).finish()
    }
}

impl SquareChat {
    pub fn new(client: Client, raw: Value) -> Self {
        Self { client, raw }
    }

    pub fn mid(&self) -> &str {
        self.raw["squareChatMid"].as_str().unwrap_or("")
    }

    pub fn name(&self) -> &str {
        self.raw["name"].as_str().unwrap_or("")
    }

    /// Re-fetch `raw`.
    pub async fn update(&mut self) -> Result<()> {
        let res =
            self.client.base.square().call("getSquareChat", tv!({"request": {"squareChatMid": self.mid()}})).await?;
        self.raw = res["squareChat"].clone();
        Ok(())
    }

    /// Send a message; returns the `SendMessageResponse`.
    pub async fn send_message(&self, input: impl Into<MessageInput>) -> Result<Value> {
        self.client.send_square_message(self.mid(), input).await
    }

    /// Send an image (OBS upload in "reqseq" mode, so the server creates the
    /// message; it cannot be a reply). Returns `(obj_id, obj_hash)`.
    pub async fn send_image(&self, data: Vec<u8>, filename: Option<&str>) -> Result<(String, String)> {
        self.send_media("image", data, filename.unwrap_or("image"), None).await
    }

    pub async fn send_video(&self, data: Vec<u8>, filename: Option<&str>, duration_ms: Option<u64>) -> Result<(String, String)> {
        self.send_media("video", data, filename.unwrap_or("video"), duration_ms).await
    }

    pub async fn send_audio(&self, data: Vec<u8>, filename: Option<&str>, duration_ms: Option<u64>) -> Result<(String, String)> {
        self.send_media("audio", data, filename.unwrap_or("voice.m4a"), duration_ms).await
    }

    async fn send_media(&self, kind: &str, data: Vec<u8>, filename: &str, duration_ms: Option<u64>) -> Result<(String, String)> {
        let o_type = match kind {
            "video" => ObjType::Video,
            "audio" => ObjType::Audio,
            _ => ObjType::Image,
        };
        let file = ObsFile::new(data).name(filename);
        let up = self.client.base.obs().upload_obj_talk(self.mid(), o_type, file, None, duration_ms).await?;
        Ok((up.obj_id, up.obj_hash))
    }

    /// `updateSquareChat` with `SquareChatAttribute` names and a partial
    /// `SquareChat` merged over `raw`.
    pub async fn update_square_chat(&self, updated_attrs: &[&str], square_chat: Value) -> Result<Value> {
        let mut request = tv!({"updatedAttrs": updated_attrs});
        request.insert("squareChat", merged(&self.raw, square_chat));
        let mut args = Value::object();
        args.insert("request", request);
        self.client.base.square().call("updateSquareChat", args).await
    }

    pub async fn update_name(&self, name: &str) -> Result<Value> {
        self.update_square_chat(&["NAME"], tv!({"name": name})).await
    }

    /// All members (paginated `getSquareChatMembers`).
    pub async fn members(&self) -> Result<Vec<Value>> {
        let mid = self.mid().to_string();
        let res = continue_request(|token| {
            let args = tv!({"request": {"continuationToken": token, "squareChatMid": mid, "limit": 100}});
            async move { self.client.base.square().call("getSquareChatMembers", args).await }
        })
        .await?;
        Ok(list(&res["squareChatMembers"]))
    }

    async fn fetch_events(&self, sync_token: Option<&str>) -> Result<Value> {
        self.client
            .base
            .square()
            .call(
                "fetchSquareChatEvents",
                tv!({"request": {"limit": 100, "squareChatMid": self.mid(), "syncToken": sync_token}}),
            )
            .await
    }

    /// Poll `fetchSquareChatEvents` (every second; 2s after an error). Without
    /// `sync_token` the backlog is skipped first. Request errors are yielded
    /// and polling continues; drop the stream to stop.
    pub fn listen(&self, sync_token: Option<String>) -> impl Stream<Item = Result<SquareChatEvent>> + Send + 'static {
        struct State {
            chat: SquareChat,
            token: Option<String>,
            started: bool,
            queue: VecDeque<Result<SquareChatEvent>>,
            delay: Option<Duration>,
        }
        let state = State { chat: self.clone(), token: sync_token, started: false, queue: VecDeque::new(), delay: None };
        futures::stream::unfold(state, |mut st| async move {
            loop {
                if let Some(item) = st.queue.pop_front() {
                    return Some((item, st));
                }
                if st.chat.client.base.auth_token().is_none() || st.chat.client.base.is_disabled() {
                    return None;
                }
                if let Some(d) = st.delay.take() {
                    tokio::time::sleep(d).await;
                }
                if !st.started {
                    if st.token.is_none() {
                        loop {
                            match st.chat.fetch_events(st.token.as_deref()).await {
                                Ok(res) => {
                                    st.token = res["syncToken"].as_str().map(str::to_string);
                                    if list(&res["events"]).is_empty() {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    st.delay = Some(Duration::from_secs(2));
                                    return Some((Err(e), st));
                                }
                            }
                        }
                    }
                    st.started = true;
                    if let Some(t) = &st.token {
                        st.queue.push_back(Ok(SquareChatEvent::SyncToken(t.clone())));
                    }
                    continue;
                }
                match st.chat.fetch_events(st.token.as_deref()).await {
                    Ok(res) => {
                        let next = res["syncToken"].as_str().map(str::to_string);
                        if next.is_some() && next != st.token {
                            st.token = next.clone();
                            st.queue.push_back(Ok(SquareChatEvent::SyncToken(next.unwrap_or_default())));
                        }
                        let me = st.chat.client.base.mid();
                        for event in list(&res["events"]) {
                            st.queue.extend(classify_event(&st.chat.client, event, me.as_deref()).into_iter().map(Ok));
                        }
                        st.delay = Some(Duration::from_secs(1));
                    }
                    Err(e) => {
                        st.delay = Some(Duration::from_secs(2));
                        return Some((Err(e), st));
                    }
                }
            }
        })
    }
}

/// Map one `SquareEvent` to the events [`SquareChat::listen`] yields.
pub(crate) fn classify_event(client: &Client, event: Value, my_mid: Option<&str>) -> Vec<SquareChatEvent> {
    let kind = super::message::types::enum_name(&event["type"], "SquareEventType").unwrap_or_default();
    let payload = event["payload"].clone();
    let mut out = vec![SquareChatEvent::Event(event)];
    let field = |name: &str| payload.get(name).cloned();
    match kind.as_str() {
        "SEND_MESSAGE" => {
            if let Some(p) = field("sendMessage") {
                out.push(SquareChatEvent::Message(SquareMessage::new(client.clone(), p["squareMessage"].clone())));
            }
        }
        "RECEIVE_MESSAGE" => {
            if let Some(p) = field("receiveMessage") {
                let m = SquareMessage::new(client.clone(), p["squareMessage"].clone());
                let mention = my_mid.is_some_and(|me| m.mentions_mid(me));
                out.push(SquareChatEvent::Message(m.clone()));
                if mention {
                    out.push(SquareChatEvent::Mention(m));
                }
            }
        }
        "NOTIFIED_KICKOUT_FROM_SQUARE" => out.extend(field("notifiedKickoutFromSquare").map(SquareChatEvent::Kick)),
        "NOTIFIED_LEAVE_SQUARE_CHAT" => out.extend(field("notifiedLeaveSquareChat").map(SquareChatEvent::Leave)),
        "NOTIFIED_JOIN_SQUARE_CHAT" => out.extend(field("notifiedJoinSquareChat").map(SquareChatEvent::Join)),
        "NOTIFIED_DESTROY_MESSAGE" => out.extend(field("notifiedDestroyMessage").map(SquareChatEvent::Destroy)),
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::BaseClient;
    use crate::client::message::{build_text_decorations, Decoration};
    use crate::client::message::utils::metadata_value;
    use crate::device::Device;

    #[test]
    fn classifies_receive_message_with_mention() {
        let client = Client::new(BaseClient::new(Device::Android).unwrap());
        let (text, meta) = build_text_decorations(&[Decoration::mention("u-self", "@me"), Decoration::text(" hi")]);
        let mut message = tv!({"to": "sc", "from": "m1", "contentType": "NONE", "text": text});
        message.insert("contentMetadata", metadata_value(&meta));
        let mut sq = Value::object();
        sq.insert("message", message);
        let mut recv = Value::object();
        recv.insert("squareMessage", sq);
        let mut payload = Value::object();
        payload.insert("receiveMessage", recv);
        let mut event = tv!({"type": "RECEIVE_MESSAGE"});
        event.insert("payload", payload);

        let out = classify_event(&client, event.clone(), Some("u-self"));
        assert!(matches!(out[0], SquareChatEvent::Event(_)));
        assert!(matches!(&out[1], SquareChatEvent::Message(m) if m.text() == "@me hi"));
        assert!(matches!(out[2], SquareChatEvent::Mention(_)));
        assert_eq!(classify_event(&client, event, Some("u-other")).len(), 2);

        let kick = tv!({"type": "NOTIFIED_KICKOUT_FROM_SQUARE", "payload": {"notifiedKickoutFromSquare": {"x": 1}}});
        assert!(matches!(classify_event(&client, kick, None)[1], SquareChatEvent::Kick(_)));
    }

    #[test]
    fn merged_overrides_fields() {
        let m = merged(&tv!({"mid": "s", "name": "old"}), tv!({"name": "new"}));
        assert_eq!(m["mid"].as_str(), Some("s"));
        assert_eq!(m["name"].as_str(), Some("new"));
    }
}
