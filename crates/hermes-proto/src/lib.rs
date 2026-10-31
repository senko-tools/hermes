//! Shared types for everything on the Hermes wire. No I/O.
//!
//! - [`id`]: snowflake IDs
//! - [`models`]: wire models
//! - [`error`]: REST error codes and bodies
//! - [`api`]: REST request and response bodies
//! - [`permissions`]: permission bits and `compute`
//! - [`gateway`]: WebSocket envelope, ops, events and close codes
//! - [`markup`]: message markup parser

pub mod api;
pub mod error;
pub mod gateway;
pub mod id;
pub mod markup;
pub mod models;
pub mod permissions;

pub use error::{ApiError, ErrorBody, ErrorCode};
pub use id::Id;
pub use permissions::Permissions;

/// Gateway and REST protocol version.
pub const PROTOCOL_VERSION: u32 = 1;
