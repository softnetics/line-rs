//! Test catalogue and case bodies, mirroring `tests/live_auth.rs`.
//!
//! Each case returns `Ok(Outcome)` or `Err(error text)`. No retries: the
//! first error ends the case. [`Ctx::throttle`] runs before every library
//! call (calls that make several requests internally are not spaced).

use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use line_rs::client::{Chat, Client, ClientEvent, MessageInput, ReactionType, TalkEvent, TalkMessage, User};
use line_rs::rest::home::PageInfoQuery;
use line_rs::tv;
use serde::Serialize;
use serde_json::{json, Value as Json};
use tokio::sync::{broadcast, Mutex};

use crate::report::now_secs;

/// Minimum allowed gap between LINE requests.
pub const MIN_GAP_SECS: u64 = 5;

// ---------------------------------------------------------------------------
// Catalogue
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Need {
    None,
    Group,
    KeepId,
    Peer,
    Kickout,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaseInfo {
    pub id: &'static str,
    pub module: &'static str,
    /// Thai help text.
    pub help: &'static str,
    /// What to check on the phone (write/interactive tests).
    pub phone: &'static str,
    /// Which RE guess from LIVE_TESTING.md the test validates.
    pub validates: &'static str,
    pub need: Need,
    /// Waits for you to act on the phone.
    pub interactive: bool,
    /// Wants a human verdict after it runs.
    pub manual: bool,
}

#[allow(clippy::too_many_arguments)]
const fn c(
    id: &'static str,
    module: &'static str,
    help: &'static str,
    phone: &'static str,
    validates: &'static str,
    need: Need,
    interactive: bool,
    manual: bool,
) -> CaseInfo {
    CaseInfo { id, module, help, phone, validates, need, interactive, manual }
}

pub const MODULES: &[&str] = &["read_only", "write", "events", "e2ee", "session"];

pub const CATALOGUE: &[CaseInfo] = &[
    // read_only
    c("read_only::pinned_chats", "read_only", "อ่านรายการแชทที่ปักหมุด", "", "Thrift arg/field ids of RE'd `getPinnedChats`", Need::None, false, false),
    c("read_only::profiles", "read_only", "อ่านโปรไฟล์ทั้งหมด (multi-profile)", "", "Thrift arg/field ids of RE'd `getProfiles`", Need::None, false, false),
    c("read_only::block_details_for_peer", "read_only", "อ่านสถานะบล็อกของ peer (ต้องเปิด peer)", "", "Thrift arg/field ids of RE'd `getBlockDetails`", Need::Peer, false, false),
    c("read_only::product_tiers_for_client", "read_only", "อ่าน product tiers ของ Premium ตาม region", "", "Thrift arg/field ids of RE'd `getProductTiersForClient`", Need::None, false, false),
    c("read_only::user_ticket", "read_only", "อ่าน user ticket (ลิงก์เพิ่มเพื่อน)", "", "Thrift arg/field ids of RE'd `getUserTicket`", Need::None, false, false),
    c("read_only::messages_by_ids", "read_only", "ดึงข้อความล่าสุดใน Keep Memo แล้วอ่านซ้ำด้วย id", "", "Thrift arg/field ids of RE'd `getMessagesByIds`", Need::None, false, false),
    c("read_only::keep_size", "read_only", "อ่านขนาดพื้นที่ Keep", "", "REST channel ids (Keep) and paths", Need::None, false, false),
    c("read_only::keep_get_content", "read_only", "อ่าน Keep content ตาม id (ต้องใส่ keep content id)", "", "REST channel ids (Keep) and paths", Need::KeepId, false, false),
    c("read_only::page_info", "read_only", "ขอ URL preview ของ https://line.me", "", "REST channel ids (Note/Home) and paths", Need::None, false, false),
    c("read_only::translate_hello_to_thai", "read_only", "แปล \"hello\" เป็นภาษาไทย", "", "Translation endpoint/body, `translatedText` response key", Need::None, false, false),
    c("read_only::group_announcements", "read_only", "อ่านประกาศในกลุ่ม (ต้องใส่ group mid)", "", "baseline (not an RE guess): `getChats`, `getChatRoomAnnouncements`", Need::Group, false, false),
    c("read_only::group_read_range", "read_only", "อ่าน read range ของกลุ่ม (ต้องใส่ group mid)", "", "baseline (not an RE guess): `getMessageReadRange`", Need::Group, false, false),
    // write
    c("write::send_text", "write", "ส่งข้อความเข้า Keep Memo", "ข้อความ `[line-rs live test] send_text <ts>`", "baseline: `sendMessage` (E2EE text)", Need::None, false, true),
    c("write::send_image", "write", "ส่งรูป PNG 64x64 สีเขียวเข้า Keep Memo", "รูปสีเขียว 64x64 เปิดได้ปกติ", "image `contentMetadata` keys; E2EE v2 chunked media layout / HMAC", Need::None, false, true),
    c("write::send_file", "write", "ส่งไฟล์ line-rs-live-test.txt เข้า Keep Memo", "ไฟล์ `line-rs-live-test.txt` ดาวน์โหลดได้ มีข้อความทดสอบ", "file name/size `contentMetadata` keys; E2EE v2 chunked media layout / HMAC", Need::None, false, true),
    c("write::send_contact_of_self", "write", "ส่ง contact card ของตัวเองเข้า Keep Memo", "contact card ของตัวเอง มีชื่อถูกต้อง", "contact card `contentMetadata` keys", Need::None, false, true),
    c("write::edit_message", "write", "ส่งข้อความ (before) แล้วแก้เป็น (after)", "ข้อความ (before) ถูกแทนด้วย (after) และมีป้ายแก้ไข (อาจต้องมี Premium)", "Thrift arg/field ids of RE'd `editMessage`", Need::None, false, true),
    c("write::silently_unsend_message", "write", "ส่งข้อความแล้วยกเลิกแบบเงียบ", "ข้อความหายไปโดยไม่มีแจ้ง unsend (อาจต้องมี Premium)", "Thrift arg/field ids of RE'd `silentlyUnsendMessage`", Need::None, false, true),
    c("write::pin_roundtrip", "write", "ปักหมุด Keep Memo แล้วเลิกปัก", "Keep Memo ถูกปักแล้วเลิกปัก (สุดท้าย: ไม่ปัก)", "Thrift arg/field ids of RE'd `updatePinState`", Need::None, false, true),
    c("write::react_roundtrip", "write", "ส่งข้อความ แล้วกด reaction และยกเลิก", "reaction ขึ้นแล้วหายไป (เซิร์ฟเวอร์อาจไม่ยอมให้ react ข้อความตัวเอง)", "baseline: `react`, `cancelReaction`", Need::None, false, true),
    c("write::group_mute_favorite_roundtrip", "write", "mute/unmute และ favorite/unfavorite กลุ่ม (ตั้งค่าเฉพาะเครื่องเรา)", "การแจ้งเตือนกลุ่มและ favorite กลับเป็นปกติ (unmuted, ไม่ favorite)", "baseline: chat mute/favorite settings", Need::Group, false, true),
    c("write::peer_mute_favorite_roundtrip", "write", "mute/favorite รายชื่อ peer แล้วคืนค่า (opt-in)", "ตั้งค่า contact กลับเป็น unmuted, ไม่ favorite", "baseline: contact settings", Need::Peer, false, true),
    c("write::block_roundtrip", "write", "บล็อก peer แล้วปลดบล็อก (opt-in)", "peer ไม่ถูกบล็อกตอนจบ ถ้าปลดไม่สำเร็จให้ปลดเองในมือถือ", "Thrift arg/field ids of RE'd `blockRelation` / `unblockRelation`", Need::Peer, false, true),
    // events
    c("events::read_receipt", "events", "ส่งข้อความหา peer แล้วรอ peer เปิดอ่าน (opt-in)", "peer เปิดแชทกับบัญชีนี้ภายใน 120 วิ", "Talk event param layouts (`param1..3` of read)", Need::Peer, true, true),
    c("events::reaction", "events", "ส่งข้อความหา peer แล้วรอ peer กด reaction (opt-in)", "peer กด reaction ที่ข้อความทดสอบภายในเวลาที่กำหนด (react ข้อความตัวเองไม่ได้)", "Talk event param layouts (reaction)", Need::Peer, true, true),
    c("events::unsend", "events", "รอคุณส่งข้อความใน Keep Memo แล้ว unsend จากมือถือ", "ส่งข้อความใน Keep Memo แล้ว unsend", "Talk event param layouts (unsend); own-device actions arriving as `NOTIFIED_*` ops", Need::None, true, true),
    // e2ee
    c("e2ee::incoming_image_download", "e2ee", "รอรูปที่คุณส่งเข้า Keep Memo แล้วดาวน์โหลด/ถอดรหัส", "ส่งรูปเข้า Keep Memo จากมือถือ", "E2EE v2 chunked media layout / HMAC", Need::None, true, true),
    // session
    c("session::proactive_refresh", "session", "บังคับให้ token หมดอายุ แล้วตรวจว่า refresh ก่อนเรียก API (V3 token เท่านั้น)", "", "Proactive V3 token refresh", Need::None, false, false),
    c("session::kicked_out", "session", "รอคุณ log out session นี้จากมือถือ (Settings > Account > Devices) — token จะใช้ไม่ได้อีก ต้อง QR login ใหม่", "log out session linejs จากมือถือภายใน 180 วิ", "baseline: kick-out detection (`LogoutReason`)", Need::Kickout, true, true),
];

pub fn find(id: &str) -> Option<&'static CaseInfo> {
    CATALOGUE.iter().find(|c| c.id == id)
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// Optional inputs from the page.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
pub struct Inputs {
    #[serde(default)]
    pub group_mid: String,
    #[serde(default)]
    pub peer_mid: String,
    #[serde(default)]
    pub allow_peer: bool,
    #[serde(default)]
    pub allow_kickout: bool,
    #[serde(default)]
    pub keep_content_id: String,
    #[serde(default)]
    pub product_region: String,
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

impl Inputs {
    pub fn group(&self) -> Option<String> {
        non_empty(&self.group_mid)
    }
    pub fn peer(&self) -> Option<String> {
        non_empty(&self.peer_mid).filter(|_| self.allow_peer)
    }
    pub fn keep_id(&self) -> Option<String> {
        non_empty(&self.keep_content_id)
    }
    pub fn region(&self) -> String {
        non_empty(&self.product_region).unwrap_or_else(|| "TH".into())
    }

    /// Why a case cannot run with these inputs (the harness "skipped").
    pub fn skip_reason(&self, need: Need) -> Option<&'static str> {
        match need {
            Need::None => None,
            Need::Group => self.group().is_none().then_some("ต้องใส่ group mid"),
            Need::KeepId => self.keep_id().is_none().then_some("ต้องใส่ keep content id"),
            Need::Peer => self.peer().is_none().then_some("targets another user: ต้องใส่ peer mid และเปิด allow peer tests"),
            Need::Kickout => (!self.allow_kickout).then_some("opt-in only: เปิด allow kick-out test ก่อน"),
        }
    }
}

/// Global request spacing (one test at a time, so a simple lock is enough).
pub struct Throttle {
    last: Mutex<Option<Instant>>,
    gap_secs: std::sync::atomic::AtomicU64,
}

impl Throttle {
    pub fn new(gap: u64) -> Self {
        Self { last: Mutex::new(None), gap_secs: (gap.max(MIN_GAP_SECS)).into() }
    }
    pub fn gap(&self) -> u64 {
        self.gap_secs.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn set_gap(&self, secs: u64) {
        self.gap_secs.store(secs.max(MIN_GAP_SECS), std::sync::atomic::Ordering::Relaxed);
    }
    pub async fn wait(&self) {
        let mut last = self.last.lock().await;
        if let Some(at) = *last {
            let wait = Duration::from_secs(self.gap()).saturating_sub(at.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        *last = Some(Instant::now());
    }
}

/// Why `read_only::translate_hello_to_thai` is skipped without a request.
pub const TRANSLATE_SKIPPED: &str = "translation is optional and not needed; legyTransAPI.nhn answered \
     {\"code\":401} with line_desktop + access token and + timeline token (live, DESKTOPWIN) — ไม่ได้ส่ง request";

pub type PromptFn = Arc<dyn Fn(Option<(String, u64)>) + Send + Sync>;

pub struct Ctx {
    pub client: Client,
    pub throttle: Arc<Throttle>,
    pub inputs: Inputs,
    /// Show (`Some((text, secs))`) or clear the phone prompt banner.
    pub prompt: PromptFn,
    pub log: Arc<StdMutex<Vec<String>>>,
}

pub enum Outcome {
    Pass(Json),
    Skip(String),
}

type R = Result<Outcome, String>;

fn pass(v: Json) -> R {
    Ok(Outcome::Pass(v))
}

fn skip(reason: &str) -> R {
    Ok(Outcome::Skip(reason.to_string()))
}

macro_rules! ensure {
    ($cond:expr, $($fmt:tt)+) => {
        if !$cond {
            return Err(format!($($fmt)+));
        }
    };
}

impl Ctx {
    async fn throttle(&self) {
        self.throttle.wait().await;
    }

    fn note(&self, line: impl Into<String>) {
        if let Ok(mut l) = self.log.lock() {
            l.push(line.into());
        }
    }

    fn prompt(&self, text: impl Into<String>, secs: u64) {
        (self.prompt)(Some((text.into(), secs)));
    }

    fn own_mid(&self) -> Result<String, String> {
        self.client.mid().ok_or_else(|| "logged-in client has no mid".to_string())
    }

    fn self_chat(&self) -> Result<Chat, String> {
        Ok(Chat::new(self.client.clone(), tv!({"chatMid": self.own_mid()?})))
    }

    async fn post_text(&self, to: &str, label: &str) -> Result<TalkMessage, String> {
        self.throttle().await;
        let msg = self
            .client
            .send_message(to, MessageInput::text(unique_text(label)).e2ee(true))
            .await
            .map_err(|e| format!("send_message to {to} failed: {e}"))?;
        ensure!(!msg.id().is_empty(), "sent message has no id: {:?}", msg.raw);
        self.note(format!("sent message {} to {to}", msg.id()));
        Ok(msg)
    }

    /// Wait for talk events until `pick` returns `Some` or `secs` elapse.
    async fn wait_for<T>(
        &self,
        rx: &mut broadcast::Receiver<ClientEvent>,
        secs: u64,
        what: &str,
        mut pick: impl FnMut(&ClientEvent) -> Option<T>,
    ) -> Result<T, String> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = match tokio::time::timeout(left, rx.recv()).await {
                Err(_) => return Err(format!("timed out after {secs}s waiting for {what}")),
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    self.note(format!("(lagged, {n} events dropped)"));
                    continue;
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    return Err(format!("event channel closed while waiting for {what}"));
                }
                Ok(Ok(ev)) => ev,
            };
            match &ev {
                ClientEvent::Event(op) => self.note(format!("op: {} {}", op["type"], op)),
                ClientEvent::Talk(t) => self.note(format!("talk: {t:?}")),
                ClientEvent::Message(m) => {
                    self.note(format!("message {} from {} ({})", m.id(), m.from().id, m.content_type()))
                }
                _ => {}
            }
            if let Some(found) = pick(&ev) {
                return Ok(found);
            }
        }
    }
}

fn unique_text(label: &str) -> String {
    format!("[line-rs live test] {label} {}", now_secs())
}

fn dbg<T: std::fmt::Debug>(v: &T) -> Json {
    Json::String(format!("{v:?}"))
}

/// A valid `size`x`size` solid-colour RGB PNG (stored deflate, no deps).
pub fn tiny_png(size: u32) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &x in data {
            a = (a + u32::from(x)) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }

    let mut row = vec![0u8];
    row.extend([0x06, 0xc7, 0x55].repeat(size as usize));
    let raw = row.repeat(size as usize);
    assert!(raw.len() <= 0xffff, "tiny_png: too large for one stored block");
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    zlib.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    zlib.extend_from_slice(&raw);
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

fn is_image(data: &[u8]) -> bool {
    data.starts_with(&[0xff, 0xd8, 0xff])
        || data.starts_with(b"\x89PNG\r\n\x1a\n")
        || data.starts_with(b"GIF8")
        || (data.len() > 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP")
        || (data.len() > 12 && &data[4..8] == b"ftyp")
}

const EVENT_WAIT_SECS: u64 = 120;
const KICKOUT_WAIT_SECS: u64 = 180;

// ---------------------------------------------------------------------------
// Case bodies
// ---------------------------------------------------------------------------

pub async fn run_case(id: &str, ctx: &Ctx) -> R {
    let info = find(id).ok_or_else(|| format!("unknown test {id}"))?;
    if let Some(reason) = ctx.inputs.skip_reason(info.need) {
        return skip(reason);
    }
    let c = &ctx.client;
    let e = |what: &'static str| move |err: line_rs::Error| format!("{what}: {err}");
    match id {
        // ---- read_only ----
        "read_only::pinned_chats" => {
            ctx.throttle().await;
            let res = c.base.talk().get_pinned_chats(None).await.map_err(e("getPinnedChats"))?;
            pass(res.to_json())
        }
        "read_only::profiles" => {
            ctx.throttle().await;
            let res = c.base.multi_profile().get_profiles("INITIALIZATION").await.map_err(e("getProfiles"))?;
            ensure!(!res.is_null(), "getProfiles returned null");
            pass(res.to_json())
        }
        "read_only::block_details_for_peer" => {
            let peer = ctx.inputs.peer().unwrap_or_default();
            ctx.throttle().await;
            match c.base.relation().get_block_details(&[peer], None).await {
                Ok(res) => pass(res.to_json()),
                // A device capability, not a client bug: the method exists but
                // this device type may not call it.
                Err(err) if err.to_string().contains("API method not capable") => {
                    skip(&format!("getBlockDetails is not available on this device type: {err}"))
                }
                Err(err) => Err(format!("getBlockDetails: {err}")),
            }
        }
        "read_only::product_tiers_for_client" => {
            let region = ctx.inputs.region();
            ctx.throttle().await;
            let res = c
                .base
                .premium_product_definition()
                .get_product_tiers_for_client(tv!({"req": {"region": region}}))
                .await
                .map_err(e("getProductTiersForClient"))?;
            pass(res.to_json())
        }
        "read_only::user_ticket" => {
            ctx.throttle().await;
            let res = c.base.talk().get_user_ticket().await.map_err(e("getUserTicket"))?;
            ensure!(!res.is_null(), "getUserTicket returned null");
            pass(res.to_json())
        }
        "read_only::messages_by_ids" => {
            let chat = ctx.self_chat()?;
            ctx.throttle().await;
            let mut fetcher = chat.message_fetcher().await.map_err(e("getMessageBoxes"))?;
            ctx.throttle().await;
            let page = fetcher.fetch(1).await.map_err(e("getPreviousMessagesV2WithRequest"))?;
            let Some(msg) = page.first() else {
                return skip("self-chat has no messages; send one to Keep Memo first");
            };
            let id: i64 = msg.id().parse().map_err(|_| format!("message id {:?} is not numeric", msg.id()))?;
            let delivered = msg.raw["deliveredTime"].as_i64().unwrap_or(0);
            ctx.throttle().await;
            let res = c
                .base
                .talk()
                .get_messages_by_ids(tv!({"messagesByIdsRequests": [{
                    "chatMid": chat.mid(),
                    "messageIds": [{"deliveredTime": delivered, "messageId": id}],
                }]}))
                .await
                .map_err(e("getMessagesByIds"))?;
            let found = res.as_list().is_some_and(|l| l.iter().any(|m| m["id"].as_str() == Some(msg.id())));
            ensure!(found, "getMessagesByIds did not return message {}: {res}", msg.id());
            pass(res.to_json())
        }
        "read_only::keep_size" => skip(crate::picker::KEEP_UNAVAILABLE),
        "read_only::keep_get_content" => skip(crate::picker::KEEP_UNAVAILABLE),
        "read_only::page_info" => {
            let rest = c.base.rest();
            ctx.throttle().await;
            let q = PageInfoQuery { url: "https://line.me".into(), ..Default::default() };
            let res = rest.home().page_info(q).await.map_err(e("pageinfo/get.json"))?;
            pass(res)
        }
        // Not needed for the client; the endpoint answers {"code":401} with
        // every header/token combination tried so far, so don't spend requests.
        "read_only::translate_hello_to_thai" => skip(TRANSLATE_SKIPPED),
        "read_only::group_announcements" => {
            let group = ctx.inputs.group().unwrap_or_default();
            ctx.throttle().await;
            let chat = c.get_chat(&group).await.map_err(|err| format!("getChats({group}): {err}"))?;
            ctx.throttle().await;
            let list = chat.announcements().await.map_err(e("getChatRoomAnnouncements"))?;
            pass(Json::Array(list.iter().map(|v| v.to_json()).collect()))
        }
        "read_only::group_read_range" => {
            let group = ctx.inputs.group().unwrap_or_default();
            ctx.throttle().await;
            let chat = c.get_chat(&group).await.map_err(|err| format!("getChats({group}): {err}"))?;
            ctx.throttle().await;
            let range = chat.read_range().await.map_err(e("getMessageReadRange"))?;
            pass(range.map(|v| v.to_json()).unwrap_or(Json::Null))
        }

        // ---- write ----
        "write::send_text" => {
            let me = ctx.own_mid()?;
            let msg = ctx.post_text(&me, "send_text").await?;
            ensure!(msg.is_my_message(), "sent message is not ours: {:?}", msg.raw);
            pass(msg.raw.to_json())
        }
        "write::send_image" => {
            let chat = ctx.self_chat()?;
            ctx.throttle().await;
            let sent = chat.send_image(tiny_png(64), Some("line-rs-live-test.png")).await.map_err(e("send_image"))?;
            pass(dbg(&sent))
        }
        "write::send_file" => {
            let chat = ctx.self_chat()?;
            let data = unique_text("send_file").into_bytes();
            ctx.throttle().await;
            let sent = chat.send_file(data, "line-rs-live-test.txt").await.map_err(e("send_file"))?;
            pass(dbg(&sent))
        }
        "write::send_contact_of_self" => {
            let me = ctx.own_mid()?;
            let name =
                c.base.profile().and_then(|p| p["displayName"].as_str().map(str::to_string)).unwrap_or_default();
            let chat = ctx.self_chat()?;
            ctx.throttle().await;
            let msg = chat.send_contact(&me, &name).await.map_err(e("send_contact"))?;
            pass(json!({"id": msg.id(), "contentMetadata": dbg(&msg.content_metadata())}))
        }
        "write::edit_message" => {
            let me = ctx.own_mid()?;
            let msg = ctx.post_text(&me, "edit_message (before)").await?;
            ctx.throttle().await;
            match msg.edit(&unique_text("edit_message (after)")).await {
                Ok(res) => pass(res.to_json()),
                Err(err) if err.code() == Some("NOT_PREMIUM") => skip(&format!("needs LYP Premium: {err}")),
                Err(err) => Err(format!("editMessage: {err}")),
            }
        }
        "write::silently_unsend_message" => {
            let me = ctx.own_mid()?;
            let msg = ctx.post_text(&me, "silently_unsend").await?;
            ctx.throttle().await;
            match c.base.talk().silently_unsend_message(msg.id()).await {
                Ok(res) => pass(res.to_json()),
                Err(err) if err.code() == Some("NOT_PREMIUM") => skip(&format!("needs LYP Premium: {err}")),
                Err(err) => Err(format!("silentlyUnsendMessage: {err}")),
            }
        }
        "write::pin_roundtrip" => {
            let me = ctx.own_mid()?;
            ctx.throttle().await;
            let on = c.base.talk().update_pin_state(&me, true).await.map_err(e("updatePinState(true)"))?;
            ctx.throttle().await;
            let off = c.base.talk().update_pin_state(&me, false).await.map_err(e("updatePinState(false)"))?;
            pass(json!({"pin": on.to_json(), "unpin": off.to_json()}))
        }
        "write::react_roundtrip" => {
            let me = ctx.own_mid()?;
            let msg = ctx.post_text(&me, "react").await?;
            ctx.throttle().await;
            msg.react(ReactionType::Nice).await.map_err(e("react"))?;
            ctx.throttle().await;
            msg.cancel_reaction().await.map_err(e("cancelReaction"))?;
            pass(json!({"messageId": msg.id()}))
        }
        "write::group_mute_favorite_roundtrip" => {
            let group = ctx.inputs.group().unwrap_or_default();
            ctx.throttle().await;
            let chat = c.get_chat(&group).await.map_err(|err| format!("getChats({group}): {err}"))?;
            ctx.throttle().await;
            chat.mute().await.map_err(e("mute"))?;
            ctx.throttle().await;
            chat.unmute().await.map_err(e("unmute"))?;
            ctx.throttle().await;
            chat.favorite().await.map_err(e("favorite"))?;
            ctx.throttle().await;
            chat.unfavorite().await.map_err(e("unfavorite"))?;
            pass(json!("mute, unmute, favorite, unfavorite ok"))
        }
        "write::peer_mute_favorite_roundtrip" => {
            let peer = ctx.inputs.peer().unwrap_or_default();
            let user = User::new(c.clone(), tv!({"targetUserMid": peer}));
            ctx.throttle().await;
            user.set_notification_disabled(true).await.map_err(e("mute contact"))?;
            ctx.throttle().await;
            user.set_notification_disabled(false).await.map_err(e("unmute contact"))?;
            ctx.throttle().await;
            user.set_favorite(true).await.map_err(e("favorite contact"))?;
            ctx.throttle().await;
            user.set_favorite(false).await.map_err(e("unfavorite contact"))?;
            pass(json!("mute, unmute, favorite, unfavorite ok"))
        }
        "write::block_roundtrip" => {
            let peer = ctx.inputs.peer().unwrap_or_default();
            ctx.throttle().await;
            let blocked = c.base.relation().block_relation(&peer).await.map_err(e("blockRelation"))?;
            ctx.throttle().await;
            let unblocked = c
                .base
                .relation()
                .unblock_relation(&peer, None)
                .await
                .map_err(e("unblockRelation (peer is still BLOCKED, unblock on the phone)"))?;
            pass(json!({"block": blocked.to_json(), "unblock": unblocked.to_json()}))
        }

        // ---- events (global listen is running) ----
        "events::read_receipt" => {
            let peer = ctx.inputs.peer().unwrap_or_default();
            let mut rx = c.subscribe();
            ctx.post_text(&peer, "read_receipt").await?;
            ctx.prompt("ให้ peer เปิดแชทกับบัญชีนี้บนมือถือของเขา", EVENT_WAIT_SECS);
            let (chat, reader) = ctx
                .wait_for(&mut rx, EVENT_WAIT_SECS, "TalkEvent::Read", |ev| match ev {
                    ClientEvent::Talk(TalkEvent::Read { chat_mid, reader_mid, .. }) => {
                        Some((chat_mid.clone(), reader_mid.clone()))
                    }
                    _ => None,
                })
                .await?;
            ensure!(reader == peer, "Read.reader_mid (param2) is not the peer: {reader}; chat_mid={chat}");
            pass(json!({"chat_mid": chat, "reader_mid": reader}))
        }
        "events::reaction" => {
            // LINE does not allow reacting to your own messages, so the peer
            // reacts to one we send them.
            let peer = ctx.inputs.peer().unwrap_or_default();
            let mut rx = c.subscribe();
            let msg = ctx.post_text(&peer, "react to me").await?;
            ctx.prompt(format!("ให้ peer เปิดแชทกับบัญชีนี้แล้วกด reaction ที่ข้อความ \"{}\"", msg.text()), EVENT_WAIT_SECS);
            let r = ctx
                .wait_for(&mut rx, EVENT_WAIT_SECS, "TalkEvent::Reaction", |ev| match ev {
                    ClientEvent::Talk(TalkEvent::Reaction(r)) => Some(r.clone()),
                    _ => None,
                })
                .await?;
            ctx.note(format!("reaction: {r:?}"));
            ensure!(r.message_id == msg.id(), "Reaction.message_id (param1) does not match the sent message: {r:?}");
            ensure!(r.reactor_mid == peer, "Reaction.reactor_mid (param3) is not the peer: {r:?}");
            ensure!(r.reaction.is_some(), "reaction type not decoded from param2: {}", r.detail);
            pass(dbg(&r))
        }
        "events::unsend" => {
            let me = ctx.own_mid()?;
            let mut rx = c.subscribe();
            ctx.prompt("ใน Keep Memo ส่งข้อความอะไรก็ได้ 1 ข้อความ แล้ว unsend (ยกเลิกข้อความ)", EVENT_WAIT_SECS);
            let mut sent_from_phone = None;
            let (chat_mid, message_id) = ctx
                .wait_for(&mut rx, EVENT_WAIT_SECS, "TalkEvent::Unsend", |ev| match ev {
                    ClientEvent::Message(m) if m.to().id == me => {
                        sent_from_phone = Some(m.id().to_string());
                        None
                    }
                    ClientEvent::Talk(TalkEvent::Unsend { chat_mid, message_id }) => {
                        Some((chat_mid.clone(), message_id.clone()))
                    }
                    _ => None,
                })
                .await?;
            ensure!(!message_id.is_empty(), "Unsend.message_id (param2) is empty");
            if let Some(id) = &sent_from_phone {
                ensure!(
                    &message_id == id,
                    "Unsend.message_id (param2) {message_id} does not match the message sent from the phone {id}"
                );
            }
            pass(json!({"chat_mid": chat_mid, "message_id": message_id, "phone_message": sent_from_phone}))
        }

        // ---- e2ee ----
        "e2ee::incoming_image_download" => {
            let me = ctx.own_mid()?;
            let mut rx = c.subscribe();
            ctx.prompt("ส่งรูปภาพ 1 รูปเข้า Keep Memo", EVENT_WAIT_SECS);
            let msg = ctx
                .wait_for(&mut rx, EVENT_WAIT_SECS, "an IMAGE message in the self-chat", |ev| match ev {
                    ClientEvent::Message(m) if m.to().id == me && m.content_type() == "IMAGE" => Some(m.clone()),
                    _ => None,
                })
                .await?;
            (ctx.prompt)(None);
            let e2ee = msg.raw.get("chunks").is_some();
            ctx.note(format!("image message {} metadata {:?} e2ee={e2ee}", msg.id(), msg.content_metadata()));
            ctx.throttle().await;
            let data = msg.data(false).await.map_err(|err| format!("download/decrypt image {}: {err}", msg.id()))?;
            let head = format!("{:02x?}", &data[..data.len().min(16)]);
            ensure!(!data.is_empty(), "downloaded image is empty");
            ensure!(is_image(&data), "downloaded data has no known image magic: {head}");
            pass(json!({"messageId": msg.id(), "e2ee": e2ee, "bytes": data.len(), "head": head}))
        }

        // ---- session ----
        "session::proactive_refresh" => {
            let has_refresh = matches!(c.base.storage.get_string("refreshToken").await, Ok(Some(_)));
            if !has_refresh {
                return skip("no refreshToken stored (not a V3 token); proactive refresh does not apply");
            }
            if !c.base.config.proactive_refresh {
                return skip("Config::proactive_refresh is off");
            }
            let before = c.auth_token();
            c.base.set_token_expire(now_secs() - 10).await.map_err(e("set_token_expire"))?;
            ctx.throttle().await;
            c.base
                .talk()
                .call("getProfile", tv!({"syncReason": "INTERNAL"}))
                .await
                .map_err(e("getProfile after forcing expiry"))?;
            let expire = c.base.token_expire().await.map_err(e("token_expire"))?;
            // Never put the token itself in the result.
            ensure!(c.auth_token() != before, "token was not refreshed");
            ensure!(expire.is_some_and(|x| x > now_secs()), "expire not moved forward: {expire:?}");
            pass(json!({"expire": expire, "token_changed": true}))
        }
        "session::kicked_out" => {
            let mut rx = c.base.subscribe();
            ctx.prompt(
                "Settings > Account > Devices แล้ว log out session ของ line-rs (token จะใช้ไม่ได้อีก)",
                KICKOUT_WAIT_SECS,
            );
            let deadline = Instant::now() + Duration::from_secs(KICKOUT_WAIT_SECS);
            let reason = loop {
                let left = deadline.saturating_duration_since(Instant::now());
                match tokio::time::timeout(left, rx.recv()).await {
                    Err(_) => return Err(format!("timed out after {KICKOUT_WAIT_SECS}s waiting for KickedOut")),
                    Ok(Ok(line_rs::ClientEvent::KickedOut(r))) => break r,
                    Ok(Ok(_)) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
                    Ok(Err(broadcast::error::RecvError::Closed)) => return Err("base event channel closed".into()),
                }
            };
            ensure!(c.base.is_disabled(), "client still enabled after KickedOut ({reason:?})");
            pass(dbg(&reason))
        }
        _ => Err(format!("no implementation for {id}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_consistent() {
        let mut ids: Vec<_> = CATALOGUE.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CATALOGUE.len(), "duplicate ids");
        for c in CATALOGUE {
            assert!(MODULES.contains(&c.module), "{}", c.id);
            assert!(c.id.starts_with(&format!("{}::", c.module)), "{}", c.id);
            assert!(!c.validates.is_empty(), "{}", c.id);
        }
    }

    #[test]
    fn gating_matches_harness() {
        let mut i = Inputs::default();
        assert!(i.skip_reason(Need::Peer).is_some());
        i.peer_mid = "u123".into();
        assert!(i.skip_reason(Need::Peer).is_some(), "peer mid alone must not enable peer tests");
        i.allow_peer = true;
        assert!(i.skip_reason(Need::Peer).is_none());
        assert!(i.skip_reason(Need::Kickout).is_some());
        i.allow_kickout = true;
        assert!(i.skip_reason(Need::Kickout).is_none());
        assert!(i.skip_reason(Need::Group).is_some());
        i.group_mid = "  ".into();
        assert!(i.skip_reason(Need::Group).is_some());
        assert_eq!(i.region(), "TH");
    }

    #[test]
    fn throttle_has_floor() {
        let t = Throttle::new(1);
        assert_eq!(t.gap(), MIN_GAP_SECS);
        t.set_gap(9);
        assert_eq!(t.gap(), 9);
        t.set_gap(0);
        assert_eq!(t.gap(), MIN_GAP_SECS);
    }

    #[test]
    fn tiny_png_is_well_formed() {
        let png = tiny_png(64);
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(is_image(&png));
    }
}
