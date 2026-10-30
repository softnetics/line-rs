use super::LogoutReason;
use crate::polling::SyncData;
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
    Log { kind: String, data: serde_json::Value },
}
