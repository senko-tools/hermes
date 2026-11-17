//! Shared types for everything on the Hermes wire. No I/O.
//!
//! - [`id`]: snowflake IDs
//! - [`models`]: wire models
//! - [`error`]: REST error codes and bodies
//! - [`api`]: REST request and response bodies
//! - [`permissions`]: permission bits and `compute`
//! - [`gateway`]: WebSocket envelope, ops, events and close codes
//! - [`markup`]: message markup parser
//! - [`voice`]: voice control stream messages and audio datagrams
//! - [`e2ee`]: end-to-end encrypted payloads, opaque to the server
//! - [`compat`]: what keeps older and newer clients and servers talking (docs/protocol.md)

pub mod api;
pub mod compat;
pub mod e2ee;
pub mod error;
pub mod gateway;
pub mod id;
pub mod markup;
pub mod models;
pub mod permissions;
pub mod voice;

pub use error::{ApiError, ErrorBody, ErrorCode};
pub use id::Id;
pub use permissions::Permissions;

/// Gateway and REST protocol version. 2: end-to-end encryption, which clients and servers from
/// before it cannot take part in (docs/e2ee.md).
///
/// It changes only when a change breaks older peers. Additive changes (new fields, ops, enum
/// values) keep it and announce themselves in [`compat::features`] instead.
pub const PROTOCOL_VERSION: u32 = 2;

/// The oldest client protocol a server of this build still serves.
pub const MIN_CLIENT_PROTOCOL: u32 = 2;

/// The oldest server protocol a client of this build still talks to.
pub const MIN_SERVER_PROTOCOL: u32 = 2;
