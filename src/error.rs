//! Error type shared by the whole crate.
//!
//! Match on what went wrong with the helpers on [`Error`] instead of on the
//! error text:
//!
//! * [`Error::error_code`]: the server's `TalkException` code as an
//!   [`ErrorCode`].
//! * [`Error::is_auth_dead`]: the session is over; log in again.
//! * [`Error::is_network`], [`Error::is_timeout`]: the request didn't get
//!   through; try again later.
//! * [`Error::is_e2ee`]: Letter Sealing (E2EE) failed.
//! * [`Error::is_rate_limited`], [`Error::is_server`]: LINE refused or
//!   failed on its side.
//! * [`Error::http_status`]: the HTTP status of a failed HTTP call.
//! * [`Error::is_media_expired`]: a media file was deleted or has
//!   expired on LINE's object storage.
//! * [`Error::full_message`]: the message with every source error, for
//!   logs.

use std::error::Error as StdError;
use std::fmt;

use crate::thrift::Value;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Thrift encoding/decoding or schema failure.
    #[error("thrift: {0}")]
    Thrift(String),

    /// The LINE server answered with an exception struct, or with an error
    /// code in a login response (see [`Error::error_code`]).
    #[error("request {method}({path}) failed: {data}")]
    Request {
        method: String,
        path: String,
        /// The decoded exception (e.g. a `TalkException`
        /// `{code, reason, parameterMap}`).
        data: Value,
    },

    /// HTTP transport failure.
    ///
    /// The message is reqwest's, which leaves out the cause: a timeout
    /// reads `error sending request for url (…)` without "operation timed
    /// out". The cause is in [`source`](std::error::Error::source); use
    /// [`Error::full_message`] to print it, or [`Error::is_network`] and
    /// [`Error::is_timeout`] to test for it.
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    /// A cryptographic failure, nearly always Letter Sealing (E2EE).
    #[error("crypto: {0}")]
    Crypto(String),

    /// Library-level failure with a linejs-compatible type name
    /// (`NotAuthorized`, `TimeoutError`, `ClientClosed`, `LoggedOut`,
    /// `ObsError`, ...).
    #[error("{kind}: {message}")]
    Internal { kind: String, message: String },
}

macro_rules! error_codes {
    ($($(#[$doc:meta])* $variant:ident = $num:literal => $name:literal,)*) => {
        /// `TalkException.code`: the `ErrorCode` enum of LINE's IDL
        /// (`resources/line.thrift`), with the newer codes added after it
        /// (126 to 135).
        ///
        /// Get one from an error with [`Error::error_code`], or parse a name
        /// or number with [`ErrorCode::parse`]. A code this crate doesn't
        /// know is kept as [`ErrorCode::Unknown`] with the name or number the
        /// server sent.
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum ErrorCode {
            $($(#[$doc])* $variant,)*
            /// A code missing from the list above, as the server sent it
            /// (name, or number as text).
            Unknown(String),
        }

        impl ErrorCode {
            /// The code for an IDL name such as `"MUST_REFRESH_V3_TOKEN"`.
            pub fn from_name(name: &str) -> Self {
                match name {
                    $($name => ErrorCode::$variant,)*
                    other => ErrorCode::Unknown(other.to_string()),
                }
            }

            /// The code for its IDL number, such as `119`.
            pub fn from_number(number: i64) -> Self {
                match number {
                    $($num => ErrorCode::$variant,)*
                    other => ErrorCode::Unknown(other.to_string()),
                }
            }

            /// The IDL name, e.g. `MUST_REFRESH_V3_TOKEN`. For
            /// [`ErrorCode::Unknown`], what the server sent.
            pub fn name(&self) -> &str {
                match self {
                    $(ErrorCode::$variant => $name,)*
                    ErrorCode::Unknown(s) => s,
                }
            }

            /// The IDL number, e.g. `119`. `None` for an unknown name;
            /// an unknown number is returned as is.
            pub fn number(&self) -> Option<i64> {
                match self {
                    $(ErrorCode::$variant => Some($num),)*
                    ErrorCode::Unknown(s) => s.parse().ok(),
                }
            }
        }
    };
}

error_codes! {
    IllegalArgument = 0 => "ILLEGAL_ARGUMENT",
    /// The access token is no longer valid. A secondary device is logged out on it.
    AuthenticationFailed = 1 => "AUTHENTICATION_FAILED",
    DbFailed = 2 => "DB_FAILED",
    InvalidState = 3 => "INVALID_STATE",
    ExcessiveAccess = 4 => "EXCESSIVE_ACCESS",
    NotFound = 5 => "NOT_FOUND",
    InvalidMid = 9 => "INVALID_MID",
    NotAMember = 10 => "NOT_A_MEMBER",
    InvalidLength = 6 => "INVALID_LENGTH",
    /// The account is gone or suspended. The session is over.
    NotAvailableUser = 7 => "NOT_AVAILABLE_USER",
    /// This device was logged out (from the phone, or by another login).
    NotAuthorizedDevice = 8 => "NOT_AUTHORIZED_DEVICE",
    NotAuthorizedSession = 14 => "NOT_AUTHORIZED_SESSION",
    IncompatibleAppVersion = 11 => "INCOMPATIBLE_APP_VERSION",
    NotReady = 12 => "NOT_READY",
    NotAvailableSession = 13 => "NOT_AVAILABLE_SESSION",
    SystemError = 15 => "SYSTEM_ERROR",
    NoAvailableVerificationMethod = 16 => "NO_AVAILABLE_VERIFICATION_METHOD",
    NotAuthenticated = 17 => "NOT_AUTHENTICATED",
    InvalidIdentityCredential = 18 => "INVALID_IDENTITY_CREDENTIAL",
    NotAvailableIdentityIdentifier = 19 => "NOT_AVAILABLE_IDENTITY_IDENTIFIER",
    InternalError = 20 => "INTERNAL_ERROR",
    NoSuchIdentityIdenfier = 21 => "NO_SUCH_IDENTITY_IDENFIER",
    DeactivatedAccountBoundToThisIdentity = 22 => "DEACTIVATED_ACCOUNT_BOUND_TO_THIS_IDENTITY",
    IllegalIdentityCredential = 23 => "ILLEGAL_IDENTITY_CREDENTIAL",
    UnknownChannel = 24 => "UNKNOWN_CHANNEL",
    NoSuchMessageBox = 25 => "NO_SUCH_MESSAGE_BOX",
    NotAvailableMessageBox = 26 => "NOT_AVAILABLE_MESSAGE_BOX",
    ChannelDoesNotMatch = 27 => "CHANNEL_DOES_NOT_MATCH",
    NotYourMessage = 28 => "NOT_YOUR_MESSAGE",
    MessageDefinedError = 29 => "MESSAGE_DEFINED_ERROR",
    UserCannotAcceptPresents = 30 => "USER_CANNOT_ACCEPT_PRESENTS",
    UserNotStickerOwner = 32 => "USER_NOT_STICKER_OWNER",
    MaintenanceError = 33 => "MAINTENANCE_ERROR",
    AccountNotMatched = 34 => "ACCOUNT_NOT_MATCHED",
    AbuseBlock = 35 => "ABUSE_BLOCK",
    NotFriend = 36 => "NOT_FRIEND",
    NotAllowedCall = 37 => "NOT_ALLOWED_CALL",
    BlockFriend = 38 => "BLOCK_FRIEND",
    IncompatibleVoipVersion = 39 => "INCOMPATIBLE_VOIP_VERSION",
    InvalidSnsAccessToken = 40 => "INVALID_SNS_ACCESS_TOKEN",
    ExternalServiceNotAvailable = 41 => "EXTERNAL_SERVICE_NOT_AVAILABLE",
    NotAllowedAddContact = 42 => "NOT_ALLOWED_ADD_CONTACT",
    NotCertificated = 43 => "NOT_CERTIFICATED",
    NotAllowedSecondaryDevice = 44 => "NOT_ALLOWED_SECONDARY_DEVICE",
    InvalidPinCode = 45 => "INVALID_PIN_CODE",
    ExceedFileMaxSize = 47 => "EXCEED_FILE_MAX_SIZE",
    ExceedDailyQuota = 48 => "EXCEED_DAILY_QUOTA",
    NotSupportSendFile = 49 => "NOT_SUPPORT_SEND_FILE",
    MustUpgrade = 50 => "MUST_UPGRADE",
    NotAvailablePinCodeSession = 51 => "NOT_AVAILABLE_PIN_CODE_SESSION",
    ExpiredRevision = 52 => "EXPIRED_REVISION",
    NotYetPhoneNumber = 54 => "NOT_YET_PHONE_NUMBER",
    BadCallNumber = 55 => "BAD_CALL_NUMBER",
    UnavailableCallNumber = 56 => "UNAVAILABLE_CALL_NUMBER",
    NotSupportCallService = 57 => "NOT_SUPPORT_CALL_SERVICE",
    CongestionControl = 58 => "CONGESTION_CONTROL",
    NoBalance = 59 => "NO_BALANCE",
    NotPermittedCallerId = 60 => "NOT_PERMITTED_CALLER_ID",
    NoCallerIdLimitExceeded = 61 => "NO_CALLER_ID_LIMIT_EXCEEDED",
    CallerIdVerificationRequired = 62 => "CALLER_ID_VERIFICATION_REQUIRED",
    NoCallerIdLimitExceededAndVerificationRequired = 63 => "NO_CALLER_ID_LIMIT_EXCEEDED_AND_VERIFICATION_REQUIRED",
    MessageNotFound = 64 => "MESSAGE_NOT_FOUND",
    InvalidAccountMigrationPincodeFormat = 65 => "INVALID_ACCOUNT_MIGRATION_PINCODE_FORMAT",
    AccountMigrationPincodeNotMatched = 66 => "ACCOUNT_MIGRATION_PINCODE_NOT_MATCHED",
    AccountMigrationPincodeBlocked = 67 => "ACCOUNT_MIGRATION_PINCODE_BLOCKED",
    InvalidPasswordFormat = 69 => "INVALID_PASSWORD_FORMAT",
    FeatureRestricted = 70 => "FEATURE_RESTRICTED",
    MessageNotDestructible = 71 => "MESSAGE_NOT_DESTRUCTIBLE",
    PaidCallRedeemFailed = 72 => "PAID_CALL_REDEEM_FAILED",
    PreventedJoinByTicket = 73 => "PREVENTED_JOIN_BY_TICKET",
    SendMessageNotPermittedFromLineAt = 75 => "SEND_MESSAGE_NOT_PERMITTED_FROM_LINE_AT",
    SendMessageNotPermittedWhileAutoReply = 76 => "SEND_MESSAGE_NOT_PERMITTED_WHILE_AUTO_REPLY",
    SecurityCenterNotVerified = 77 => "SECURITY_CENTER_NOT_VERIFIED",
    SecurityCenterBlockedBySetting = 78 => "SECURITY_CENTER_BLOCKED_BY_SETTING",
    SecurityCenterBlocked = 79 => "SECURITY_CENTER_BLOCKED",
    TalkProxyException = 80 => "TALK_PROXY_EXCEPTION",
    E2eeInvalidProtocol = 81 => "E2EE_INVALID_PROTOCOL",
    E2eeRetryEncrypt = 82 => "E2EE_RETRY_ENCRYPT",
    E2eeUpdateSenderKey = 83 => "E2EE_UPDATE_SENDER_KEY",
    E2eeUpdateReceiverKey = 84 => "E2EE_UPDATE_RECEIVER_KEY",
    E2eeInvalidArgument = 85 => "E2EE_INVALID_ARGUMENT",
    E2eeInvalidVersion = 86 => "E2EE_INVALID_VERSION",
    E2eeSenderDisabled = 87 => "E2EE_SENDER_DISABLED",
    E2eeReceiverDisabled = 88 => "E2EE_RECEIVER_DISABLED",
    E2eeSenderNotAllowed = 89 => "E2EE_SENDER_NOT_ALLOWED",
    E2eeReceiverNotAllowed = 90 => "E2EE_RECEIVER_NOT_ALLOWED",
    E2eeResendFail = 91 => "E2EE_RESEND_FAIL",
    E2eeResendOk = 92 => "E2EE_RESEND_OK",
    HitokotoBackupNoAvailableData = 93 => "HITOKOTO_BACKUP_NO_AVAILABLE_DATA",
    E2eeUpdatePrimaryDevice = 94 => "E2EE_UPDATE_PRIMARY_DEVICE",
    Success = 95 => "SUCCESS",
    Cancel = 96 => "CANCEL",
    E2eePrimaryNotSupport = 97 => "E2EE_PRIMARY_NOT_SUPPORT",
    E2eeRetryPlain = 98 => "E2EE_RETRY_PLAIN",
    E2eeRecreateGroupKey = 99 => "E2EE_RECREATE_GROUP_KEY",
    E2eeGroupTooManyMembers = 100 => "E2EE_GROUP_TOO_MANY_MEMBERS",
    ServerBusy = 101 => "SERVER_BUSY",
    NotAllowedAddFollow = 102 => "NOT_ALLOWED_ADD_FOLLOW",
    IncomingFriendRequestLimit = 103 => "INCOMING_FRIEND_REQUEST_LIMIT",
    OutgoingFriendRequestLimit = 104 => "OUTGOING_FRIEND_REQUEST_LIMIT",
    OutgoingFriendRequestQuota = 105 => "OUTGOING_FRIEND_REQUEST_QUOTA",
    Duplicated = 106 => "DUPLICATED",
    Banned = 107 => "BANNED",
    NotAnInvitee = 108 => "NOT_AN_INVITEE",
    NotAnOutsider = 109 => "NOT_AN_OUTSIDER",
    EmptyGroup = 111 => "EMPTY_GROUP",
    ExceedFollowLimit = 112 => "EXCEED_FOLLOW_LIMIT",
    UnsupportedAccountType = 113 => "UNSUPPORTED_ACCOUNT_TYPE",
    AgreementRequired = 114 => "AGREEMENT_REQUIRED",
    ShouldRetry = 115 => "SHOULD_RETRY",
    OverMaxChatsPerUser = 116 => "OVER_MAX_CHATS_PER_USER",
    NotAvailableApi = 117 => "NOT_AVAILABLE_API",
    InvalidOtp = 118 => "INVALID_OTP",
    /// The v3 access token must be refreshed. line-rs refreshes and retries once on its own; see [`Error::is_auth_dead`].
    MustRefreshV3Token = 119 => "MUST_REFRESH_V3_TOKEN",
    AlreadyExpired = 120 => "ALREADY_EXPIRED",
    UserNotSticonOwner = 121 => "USER_NOT_STICON_OWNER",
    /// Upload the E2EE media again (handled with the E2EE send errors).
    RefreshMediaFlow = 122 => "REFRESH_MEDIA_FLOW",
    ExceedFollowerLimit = 123 => "EXCEED_FOLLOWER_LIMIT",
    IncompatibleAppType = 124 => "INCOMPATIBLE_APP_TYPE",
    NotPremium = 125 => "NOT_PREMIUM",
    /// Not in `resources/line.thrift`; a newer code.
    BitmaxAccountExist = 126 => "BITMAX_ACCOUNT_EXIST",
    /// Not in `resources/line.thrift`; a newer code.
    MessageNotEditable = 127 => "MESSAGE_NOT_EDITABLE",
    /// Not in `resources/line.thrift`; a newer code.
    MessageNotEditableTimelimit = 128 => "MESSAGE_NOT_EDITABLE_TIMELIMIT",
    /// Not in `resources/line.thrift`; a newer code.
    ScheduledMessageTimelimit = 129 => "SCHEDULED_MESSAGE_TIMELIMIT",
    /// Not in `resources/line.thrift`; a newer code.
    ScheduledMessageServerTimeslotLimit = 130 => "SCHEDULED_MESSAGE_SERVER_TIMESLOT_LIMIT",
    /// Not in `resources/line.thrift`; a newer code.
    ScheduledMessageUserTotalLimit = 131 => "SCHEDULED_MESSAGE_USER_TOTAL_LIMIT",
    /// Not in `resources/line.thrift`; a newer code.
    ScheduledMessageUserTimeslotLimit = 132 => "SCHEDULED_MESSAGE_USER_TIMESLOT_LIMIT",
    /// Not in `resources/line.thrift`; a newer code.
    ScheduledMessageUpdateTimelimit = 133 => "SCHEDULED_MESSAGE_UPDATE_TIMELIMIT",
    /// Not in `resources/line.thrift`; a newer code.
    ScheduledMessageRescheduleTimelimit = 134 => "SCHEDULED_MESSAGE_RESCHEDULE_TIMELIMIT",
    /// Not in `resources/line.thrift`; a newer code.
    NotLabEnabled = 135 => "NOT_LAB_ENABLED",
}

impl ErrorCode {
    /// Parse a name (`"NOT_A_MEMBER"`) or a number (`"10"`).
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        match s.parse::<i64>() {
            Ok(n) => Self::from_number(n),
            Err(_) => Self::from_name(s),
        }
    }

    /// Letter Sealing (E2EE) codes: every `E2EE_*` code and
    /// `REFRESH_MEDIA_FLOW`.
    pub fn is_e2ee(&self) -> bool {
        matches!(self, ErrorCode::RefreshMediaFlow)
            || (!matches!(self, ErrorCode::Unknown(_)) && self.name().starts_with("E2EE_"))
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for ErrorCode {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(Self::parse(s))
    }
}

/// Whether an exception from `path` is a `TalkException`, whose `code`
/// numbers are [`ErrorCode`]s. Other services number their own codes.
fn is_talk_exception_path(path: &str) -> bool {
    crate::request::exception_type(path) == "TalkException"
        && path != crate::request::TOKEN_REFRESH_PATH
        && !path.to_ascii_lowercase().starts_with("/acct/")
}

/// `std::io::ErrorKind`s that mean the network, not the local machine,
/// failed.
fn is_network_io(kind: std::io::ErrorKind) -> bool {
    use std::io::ErrorKind::*;
    matches!(
        kind,
        ConnectionRefused
            | ConnectionReset
            | ConnectionAborted
            | NotConnected
            | AddrNotAvailable
            | BrokenPipe
            | TimedOut
            | UnexpectedEof
            | HostUnreachable
            | NetworkUnreachable
            | NetworkDown
    )
}

/// `err` and its sources, outermost first.
fn chain<'a>(
    err: &'a (dyn StdError + 'static),
) -> impl Iterator<Item = &'a (dyn StdError + 'static)> {
    std::iter::successors(Some(err), |e: &&'a (dyn StdError + 'static)| (*e).source())
}

/// The first `HTTP 404`, `status=404`, `status 404` or `HTTP/2 404` style
/// status in `text` (100 to 599).
fn status_in_text(text: &str) -> Option<u16> {
    ["HTTP ", "status=", "status "]
        .iter()
        .flat_map(|marker| {
            text.match_indices(marker)
                .map(move |(i, _)| i + marker.len())
        })
        .find_map(|start| {
            let digits: String = text[start..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            (digits.len() == 3)
                .then(|| digits.parse::<u16>().ok())
                .flatten()
                .filter(|n| (100..=599).contains(n))
        })
}

/// The status of a `gRPC status N` message.
fn grpc_status_in_text(text: &str) -> Option<i32> {
    let start = text.find("gRPC status ")? + "gRPC status ".len();
    let digits: String = text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

impl Error {
    pub fn internal(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Error::Internal {
            kind: kind.into(),
            message: message.into(),
        }
    }

    /// The `code` field of a server exception, if this is one, as the
    /// server sent it (usually the enum name). See [`Error::error_code`]
    /// for a typed code.
    pub fn code(&self) -> Option<&str> {
        match self {
            Error::Request { data, .. } => data["code"].as_str(),
            _ => None,
        }
    }

    /// The server's error code as an [`ErrorCode`].
    ///
    /// Reads the `code` field of an [`Error::Request`], given as a name
    /// (`"NOT_A_MEMBER"`) or a number (`10`). Names are matched for every
    /// service (other services reuse the same names for the same meaning);
    /// numbers only for `TalkException`s, because other services
    /// (`/acct/…` login, Channel, LIFF) number their codes differently, so
    /// a number from those comes back as [`ErrorCode::Unknown`].
    ///
    /// `None` when the error didn't come from the server or has no `code`
    /// (Square and token refresh errors use `errorCode`; read them from
    /// `data`).
    pub fn error_code(&self) -> Option<ErrorCode> {
        let Error::Request { data, path, .. } = self else {
            return None;
        };
        match &data["code"] {
            Value::Null => None,
            Value::Int(n) if is_talk_exception_path(path) => Some(ErrorCode::from_number(*n)),
            Value::Int(n) => Some(ErrorCode::Unknown(n.to_string())),
            v => {
                let s = v.as_str()?.trim();
                if s.parse::<i64>().is_ok() && !is_talk_exception_path(path) {
                    Some(ErrorCode::Unknown(s.to_string()))
                } else {
                    Some(ErrorCode::parse(s))
                }
            }
        }
    }

    /// Whether the session is over and the user has to log in again.
    ///
    /// True for:
    ///
    /// * `AUTHENTICATION_FAILED`, `NOT_AUTHORIZED_DEVICE` and
    ///   `NOT_AVAILABLE_USER`, the codes that end a session;
    /// * the logout reasons of [`LogoutReason::from_error`]
    ///   (`V3_TOKEN_CLIENT_LOGGED_OUT`, `DEVICE_LOSE`, ...), which also
    ///   emit `ClientEvent::KickedOut`;
    /// * a token `refresh` that LINE refused with `INVALID_REQUEST` (the
    ///   refresh token is no longer valid; `RETRY_REQUIRED` is not dead);
    /// * the library's own `LoggedOut` (kicked out), `ClientClosed` (logged
    ///   out or disabled) and `NotAuthorized` (no token: not logged in)
    ///   errors.
    ///
    /// `MUST_REFRESH_V3_TOKEN` on its own is **not** dead: line-rs refreshes
    /// and retries once, and a refresh that fails returns the refresh's
    /// error instead, which this classifies (a network failure there is
    /// [`is_network`](Self::is_network), not dead). It only surfaces when
    /// there is no refresh token stored, or the retried request asked for a
    /// refresh again; check [`error_code`](Self::error_code) for it if your
    /// app has no other way to refresh.
    ///
    /// During login, the same codes mean the login failed, not that a
    /// session ended.
    ///
    /// [`LogoutReason::from_error`]: crate::base::LogoutReason::from_error
    pub fn is_auth_dead(&self) -> bool {
        match self {
            Error::Request { path, data, .. } => {
                if path == crate::request::TOKEN_REFRESH_PATH {
                    let code = &data["errorCode"];
                    return code.as_str() == Some("INVALID_REQUEST") || code.as_i64() == Some(1000);
                }
                crate::base::LogoutReason::from_error(self).is_some()
                    || matches!(
                        self.error_code(),
                        Some(
                            ErrorCode::AuthenticationFailed
                                | ErrorCode::NotAuthorizedDevice
                                | ErrorCode::NotAvailableUser
                        )
                    )
            }
            Error::Internal { kind, .. } => {
                matches!(
                    kind.as_str(),
                    "LoggedOut" | "ClientClosed" | "NotAuthorized"
                )
            }
            _ => false,
        }
    }

    /// Whether the request timed out, including a timeout reqwest reports
    /// only in its source chain.
    pub fn is_timeout(&self) -> bool {
        match self {
            Error::Http(e) => {
                e.is_timeout()
                    || chain(e).any(|s| {
                        s.downcast_ref::<std::io::Error>()
                            .is_some_and(|io| io.kind() == std::io::ErrorKind::TimedOut)
                    })
            }
            Error::Io(e) => e.kind() == std::io::ErrorKind::TimedOut,
            Error::Internal { kind, .. } => kind == "TimeoutError",
            _ => false,
        }
    }

    /// Whether the request failed in the network: no connection, a timeout,
    /// or a connection that broke while sending or reading. Trying again
    /// later may work. An HTTP error status is not a network failure (see
    /// [`http_status`](Self::http_status)).
    ///
    /// Looks through the whole source chain, so a refused connection or a
    /// timeout is found even when the message only says "error sending
    /// request". Also true for the library's `Offline` and `TimeoutError`
    /// errors.
    pub fn is_network(&self) -> bool {
        match self {
            Error::Http(e) => {
                e.is_connect()
                    || e.is_timeout()
                    || e.is_request()
                    || e.is_body()
                    || chain(e).any(|s| {
                        s.downcast_ref::<std::io::Error>()
                            .is_some_and(|io| is_network_io(io.kind()))
                    })
            }
            Error::Io(e) => is_network_io(e.kind()),
            Error::Internal { kind, .. } => matches!(kind.as_str(), "Offline" | "TimeoutError"),
            _ => false,
        }
    }

    /// Whether Letter Sealing (E2EE) failed: an `E2EE_*` or
    /// `REFRESH_MEDIA_FLOW` server code ([`ErrorCode::is_e2ee`]), a
    /// [`Error::Crypto`] failure (decrypting, the self-check before a
    /// send, the login key chain), or a library error about E2EE keys
    /// (`NoE2EEKey`, `E2EEKeyMismatch`, `E2EEMediaError`, ...).
    pub fn is_e2ee(&self) -> bool {
        match self {
            Error::Request { .. } => self.error_code().is_some_and(|c| c.is_e2ee()),
            Error::Crypto(_) => true,
            Error::Internal { kind, .. } => kind.to_ascii_uppercase().contains("E2EE"),
            _ => false,
        }
    }

    /// Whether LINE refused because of too many requests: HTTP 429, gRPC
    /// `RESOURCE_EXHAUSTED`, or `EXCESSIVE_ACCESS`, `CONGESTION_CONTROL`,
    /// `EXCEED_DAILY_QUOTA`, `OUTGOING_FRIEND_REQUEST_QUOTA`,
    /// `OUTGOING_FRIEND_REQUEST_LIMIT` or `ABUSE_BLOCK` (blocked for
    /// sending too much; it may last longer than a plain rate limit).
    pub fn is_rate_limited(&self) -> bool {
        if self.http_status() == Some(429) || self.grpc_status() == Some(8) {
            return true;
        }
        matches!(
            self.error_code(),
            Some(
                ErrorCode::ExcessiveAccess
                    | ErrorCode::CongestionControl
                    | ErrorCode::ExceedDailyQuota
                    | ErrorCode::OutgoingFriendRequestQuota
                    | ErrorCode::OutgoingFriendRequestLimit
                    | ErrorCode::AbuseBlock
            )
        )
    }

    /// Whether LINE's servers failed: HTTP 5xx, gRPC `INTERNAL` or
    /// `UNAVAILABLE`, or `DB_FAILED`, `SYSTEM_ERROR`, `INTERNAL_ERROR`,
    /// `MAINTENANCE_ERROR`, `TALK_PROXY_EXCEPTION`, `SERVER_BUSY` or
    /// `EXTERNAL_SERVICE_NOT_AVAILABLE`. Trying again later may work.
    pub fn is_server(&self) -> bool {
        if self.http_status().is_some_and(|s| s >= 500)
            || matches!(self.grpc_status(), Some(13 | 14))
        {
            return true;
        }
        matches!(
            self.error_code(),
            Some(
                ErrorCode::DbFailed
                    | ErrorCode::SystemError
                    | ErrorCode::InternalError
                    | ErrorCode::MaintenanceError
                    | ErrorCode::TalkProxyException
                    | ErrorCode::ServerBusy
                    | ErrorCode::ExternalServiceNotAvailable
            )
        )
    }

    /// The HTTP status of a failed HTTP call: reqwest's status, or the
    /// `HTTP 404` / `status=404` in a library error such as
    /// `ObsError: Object download failed: HTTP 404`. `None` for errors
    /// without one (a Thrift exception arrives with HTTP 200).
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Error::Http(e) => chain(e).find_map(|s| {
                s.downcast_ref::<reqwest::Error>()
                    .and_then(reqwest::Error::status)
                    .map(|st| st.as_u16())
            }),
            Error::Internal { message, .. } => status_in_text(message),
            _ => None,
        }
    }

    /// Whether an object storage (OBS) download failed because the file is
    /// gone: HTTP 403, 404 or 410. 403 and 404 mean deleted or expired,
    /// any other status a network failure; 410 is HTTP's own "gone".
    pub fn is_media_expired(&self) -> bool {
        matches!(self, Error::Internal { kind, .. } if kind == "ObsError")
            && matches!(self.http_status(), Some(403 | 404 | 410))
    }

    /// The gRPC status of a failed gRPC call (`GrpcError: gRPC status N`).
    fn grpc_status(&self) -> Option<i32> {
        match self {
            Error::Internal { kind, message } if kind == "GrpcError" => {
                grpc_status_in_text(message)
            }
            _ => None,
        }
    }

    /// The message followed by every source error, joined with `: `, e.g.
    /// `http: error sending request for url (…): operation timed out`.
    ///
    /// `Display` shows only this error's own message, as Rust errors do,
    /// so that reporters that walk [`source`](std::error::Error::source)
    /// (anyhow, tracing) don't print the causes twice. reqwest hides the
    /// cause from its message, so log `full_message()` rather than
    /// `to_string()`. A source whose text is already in the message is
    /// left out.
    pub fn full_message(&self) -> String {
        let mut out = self.to_string();
        for s in chain(self).skip(1) {
            let text = s.to_string();
            if !text.is_empty() && !out.contains(&text) {
                out.push_str(": ");
                out.push_str(&text);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn talk(code: impl Into<Value>) -> Error {
        Error::Request {
            method: "sendMessage".into(),
            path: "/S4".into(),
            data: crate::tv!({"code": code.into(), "reason": ""}),
        }
    }

    fn at(path: &str, data: Value) -> Error {
        Error::Request {
            method: "m".into(),
            path: path.into(),
            data,
        }
    }

    #[test]
    fn error_code_names_and_numbers() {
        assert_eq!(
            ErrorCode::parse("MUST_REFRESH_V3_TOKEN"),
            ErrorCode::MustRefreshV3Token
        );
        assert_eq!(ErrorCode::parse("119"), ErrorCode::MustRefreshV3Token);
        assert_eq!(ErrorCode::from_number(122), ErrorCode::RefreshMediaFlow);
        assert_eq!(ErrorCode::from_number(127), ErrorCode::MessageNotEditable);
        assert_eq!(
            ErrorCode::from_name("MESSAGE_NOT_EDITABLE_TIMELIMIT").number(),
            Some(128)
        );
        assert_eq!(ErrorCode::E2eeInvalidVersion.name(), "E2EE_INVALID_VERSION");
        assert_eq!(ErrorCode::NotAMember.to_string(), "NOT_A_MEMBER");
        assert_eq!("10".parse::<ErrorCode>().unwrap(), ErrorCode::NotAMember);
        let unknown = ErrorCode::parse("SOMETHING_NEW");
        assert_eq!(unknown, ErrorCode::Unknown("SOMETHING_NEW".into()));
        assert_eq!(unknown.number(), None);
        assert_eq!(ErrorCode::from_number(999).number(), Some(999));
        // Every E2EE code from 81 to 100 that starts with E2EE_, plus 122.
        for n in 81..=100 {
            let c = ErrorCode::from_number(n);
            assert_eq!(c.is_e2ee(), c.name().starts_with("E2EE_"), "{c}");
        }
        assert!(ErrorCode::RefreshMediaFlow.is_e2ee());
        assert!(!ErrorCode::NotPremium.is_e2ee());
        assert!(!ErrorCode::Unknown("E2EE_LATER".into()).is_e2ee());
    }

    #[test]
    fn error_code_from_request() {
        assert_eq!(
            talk("NOT_A_MEMBER").error_code(),
            Some(ErrorCode::NotAMember)
        );
        assert_eq!(talk(10).error_code(), Some(ErrorCode::NotAMember));
        assert_eq!(talk("10").error_code(), Some(ErrorCode::NotAMember));
        assert_eq!(talk("NOT_A_MEMBER").code(), Some("NOT_A_MEMBER"));
        // Other services number their codes differently.
        assert_eq!(
            at("/acct/lgn/sq/v1", crate::tv!({"code": 2})).error_code(),
            Some(ErrorCode::Unknown("2".into()))
        );
        assert_eq!(
            at("/CH3", crate::tv!({"code": "1"})).error_code(),
            Some(ErrorCode::Unknown("1".into()))
        );
        assert_eq!(
            at(
                "/acct/lgn/sq/v1",
                crate::tv!({"code": "E2EE_INVALID_VERSION"})
            )
            .error_code(),
            Some(ErrorCode::E2eeInvalidVersion)
        );
        assert_eq!(at("/S4", crate::tv!({"reason": "x"})).error_code(), None);
        assert_eq!(Error::internal("x", "y").error_code(), None);
    }

    #[test]
    fn auth_dead() {
        for code in [
            "AUTHENTICATION_FAILED",
            "NOT_AUTHORIZED_DEVICE",
            "NOT_AVAILABLE_USER",
        ] {
            assert!(talk(code).is_auth_dead(), "{code}");
        }
        assert!(talk(8).is_auth_dead());
        assert!(talk("AUTHENTICATION_DIVESTED_BY_OTHER_DEVICE").is_auth_dead());
        assert!(
            at(
                "/S4",
                crate::tv!({"code": "AUTHENTICATION_FAILED", "reason": "V3_TOKEN_CLIENT_LOGGED_OUT"})
            )
            .is_auth_dead()
        );
        assert!(!talk("MUST_REFRESH_V3_TOKEN").is_auth_dead());
        assert!(!talk("NOT_A_MEMBER").is_auth_dead());
        let refresh = crate::request::TOKEN_REFRESH_PATH;
        assert!(at(refresh, crate::tv!({"errorCode": "INVALID_REQUEST"})).is_auth_dead());
        assert!(at(refresh, crate::tv!({"errorCode": 1000})).is_auth_dead());
        assert!(!at(refresh, crate::tv!({"errorCode": "RETRY_REQUIRED"})).is_auth_dead());
        for kind in ["LoggedOut", "ClientClosed", "NotAuthorized"] {
            assert!(Error::internal(kind, "").is_auth_dead(), "{kind}");
        }
        assert!(!Error::internal("TimeoutError", "").is_auth_dead());
    }

    #[test]
    fn e2ee_rate_limit_server() {
        assert!(talk("E2EE_RETRY_ENCRYPT").is_e2ee());
        assert!(talk(122).is_e2ee());
        assert!(Error::Crypto("aes-256-gcm: unable to authenticate data".into()).is_e2ee());
        assert!(Error::internal("NoE2EEKey", "").is_e2ee());
        assert!(Error::internal("E2EEKeyMismatch", "").is_e2ee());
        assert!(!talk("NOT_A_MEMBER").is_e2ee());

        assert!(talk("EXCESSIVE_ACCESS").is_rate_limited());
        assert!(talk("ABUSE_BLOCK").is_rate_limited());
        assert!(Error::internal("ObsError", "upload failed: HTTP 429").is_rate_limited());
        assert!(Error::internal("GrpcError", "gRPC status 8: slow down").is_rate_limited());
        assert!(!talk("NOT_A_MEMBER").is_rate_limited());

        assert!(talk("INTERNAL_ERROR").is_server());
        assert!(talk(101).is_server());
        assert!(Error::internal("ObsError", "Object download failed: HTTP 503").is_server());
        assert!(Error::internal("GrpcError", "gRPC status 14").is_server());
        assert!(!Error::internal("ObsError", "Object download failed: HTTP 404").is_server());
    }

    #[test]
    fn http_status_from_text() {
        let obs = Error::internal("ObsError", "Object download failed: HTTP 404");
        assert_eq!(obs.http_status(), Some(404));
        assert_eq!(
            Error::internal(
                "RequestError",
                "Request internal failed: status=410 headers=[]"
            )
            .http_status(),
            Some(410)
        );
        assert_eq!(
            Error::internal(
                "LegyPusherError",
                "PUSH responded with status 403 Forbidden: 00"
            )
            .http_status(),
            Some(403)
        );
        assert_eq!(
            Error::internal("ObsError", "HTTP 12345").http_status(),
            None
        );
        assert_eq!(talk("NOT_A_MEMBER").http_status(), None);
    }

    #[test]
    fn expired_media() {
        for status in [403, 404, 410] {
            let e = Error::internal("ObsError", format!("Object download failed: HTTP {status}"));
            assert!(e.is_media_expired(), "{status}");
        }
        assert!(
            !Error::internal("ObsError", "Object download failed: HTTP 500").is_media_expired()
        );
        assert!(
            !Error::internal("ObsError", "Object download failed: HTTP 401").is_media_expired()
        );
        assert!(!Error::internal("RequestError", "status=404").is_media_expired());
        assert!(!talk("NOT_FOUND").is_media_expired());
    }

    #[derive(Debug)]
    struct Outer(std::io::Error);
    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("tcp connect error")
        }
    }
    impl StdError for Outer {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn full_message_includes_sources() {
        let inner =
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused");
        let e = Error::Io(std::io::Error::other(Outer(inner)));
        assert_eq!(e.to_string(), "io: tcp connect error");
        assert_eq!(
            e.full_message(),
            "io: tcp connect error: connection refused"
        );
        // Sources whose text is already in the message are not repeated.
        let plain = Error::Io(std::io::Error::other("disk full"));
        assert_eq!(plain.full_message(), "io: disk full");
        assert_eq!(Error::internal("x", "y").full_message(), "x: y");
    }

    #[test]
    fn network_from_io() {
        assert!(Error::Io(std::io::ErrorKind::ConnectionReset.into()).is_network());
        assert!(Error::Io(std::io::ErrorKind::TimedOut.into()).is_timeout());
        assert!(!Error::Io(std::io::ErrorKind::NotFound.into()).is_network());
        assert!(Error::internal("Offline", "offline").is_network());
        assert!(!talk("NOT_A_MEMBER").is_network());
    }

    #[tokio::test]
    async fn http_errors_are_classified_through_the_source_chain() {
        // Nothing listens on the discard port.
        let refused = reqwest::Client::new()
            .get("http://127.0.0.1:9/")
            .send()
            .await
            .unwrap_err();
        let e = Error::from(refused);
        assert!(e.is_network());
        assert!(!e.is_auth_dead() && !e.is_server());
        assert!(e.full_message().len() > e.to_string().len());

        // A server that accepts and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _hold = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            drop(sock);
        });
        let timed_out = reqwest::Client::new()
            .get(format!("http://{addr}/"))
            .timeout(std::time::Duration::from_millis(100))
            .send()
            .await
            .unwrap_err();
        let e = Error::from(timed_out);
        assert!(e.is_timeout() && e.is_network());
        assert!(
            e.full_message().contains("timed out"),
            "{}",
            e.full_message()
        );
    }
}
