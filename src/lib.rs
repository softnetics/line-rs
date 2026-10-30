//! # line-rs
//!
//! A Rust port of [LINEJS](https://github.com/evex-dev/linejs), an unofficial
//! LINE client library.
//!
//! * [`base::BaseClient`] is the low-level client: login, raw Thrift calls,
//!   services, E2EE and polling.
//! * [`client::Client`] wraps it with message, chat and user objects.
//! * [`thrift`] has the Thrift protocols and the bundled LINE IDL schema.

pub mod base;
pub mod client;
pub mod device;
pub mod e2ee;
pub mod error;
pub mod link;
pub mod login;
pub mod obs;
pub mod polling;
pub mod push;
pub mod request;
pub mod rest;
pub mod service;
pub mod storage;
pub mod thrift;
pub mod timeline;

pub use base::{BaseClient, ClientEvent, LegyMode};
pub use client::Client;
pub use device::Device;
pub use link::LineLink;
pub use error::{Error, Result};
pub use thrift::Value;

// Lets `tv!` work in crates that do not depend on serde_json themselves.
#[doc(hidden)]
pub use serde_json as __serde_json;
