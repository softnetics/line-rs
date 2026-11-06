use super::LogoutReason;
use crate::polling::{FullSyncCategory, FullSyncReason, SyncData};
use crate::thrift::Value;

/// Events emitted by [`BaseClient`](super::BaseClient).
#[derive(Debug, Clone)]
pub enum ClientEvent {
    /// Show this PIN to the user (enter on the primary device).
    PinCall(String),
    /// Show this URL as a QR code to the user.
    QrCall(String),
    /// Login finished; carries the user's profile.
    Ready(Value),
    /// The session ended (logged out / device unauthorized).
    End(Option<Value>),
    /// The server revoked this session (another device took over, the
    /// token was logged out, ...). Emitted once, right before `End`; the
    /// client is disabled afterwards and must log in again.
    KickedOut(LogoutReason),
    UpdateAuthToken(String),
    UpdateProfile(Value),
    UpdateCert(String),
    UpdateQrCert(String),
    UpdateSyncData(SyncData),
    /// The server will not replay the operations since the saved revision
    /// (a talk `sync` returned `fullSyncResponse` or
    /// `partialFullSyncResponse`). Live events continue from the new
    /// revision, but anything that changed in between is only visible by
    /// re-pulling the listed categories: for [`FullSyncCategory::All`] or
    /// [`FullSyncCategory::Message`], reload the chat list
    /// (`getMessageBoxes`, `getAllChatMids`/`getChats`) and recent messages;
    /// for `Contact`, the contacts; for `Profile`, the profile. Expect one on
    /// the first start of a fresh storage (revision 0).
    FullSyncRequired {
        /// `FullSyncResponse.reasons`; empty for a partial full sync.
        reasons: Vec<FullSyncReason>,
        /// What to re-pull; `[All]` for a full sync.
        categories: Vec<FullSyncCategory>,
    },
    /// Diagnostics. Most kinds are debug output, but these are stable and
    /// meant to be matched:
    ///
    /// * `LegyPusherError`: the push connection failed or a talk `sync` on
    ///   it failed (`data.error`; for a sync also `data.method = "sync"` and
    ///   `data.code`, the exception code). The listener reconnects by
    ///   itself; apps may check the session (e.g. `getServerTime`).
    /// * `RefreshError`: refreshing the access token failed (`data.error`).
    /// * `Failover`: requests moved to another LEGY host (`data.host`).
    Log {
        kind: String,
        data: serde_json::Value,
    },
}
