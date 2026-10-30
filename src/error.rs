//! Error type shared by the whole crate.

use crate::thrift::Value;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Thrift encoding/decoding or schema failure.
    #[error("thrift: {0}")]
    Thrift(String),

    /// The LINE server answered with an exception struct.
    #[error("request {method}({path}) failed: {data}")]
    Request {
        method: String,
        path: String,
        /// The decoded exception (e.g. a renamed `TalkException`).
        data: Value,
    },

    /// HTTP transport failure.
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("crypto: {0}")]
    Crypto(String),

    /// Library-level failure with a linejs-compatible type name
    /// (`NotAuthorized`, `TimeoutError`, `ClientClosed`, ...).
    #[error("{kind}: {message}")]
    Internal { kind: String, message: String },
}

impl Error {
    pub fn internal(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Error::Internal { kind: kind.into(), message: message.into() }
    }

    /// The `code` field of a server exception, if this is one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Error::Request { data, .. } => data["code"].as_str(),
            _ => None,
        }
    }

    pub fn is_timeout(&self) -> bool {
        match self {
            Error::Http(e) => e.is_timeout(),
            Error::Internal { kind, .. } => kind == "TimeoutError",
            _ => false,
        }
    }
}
