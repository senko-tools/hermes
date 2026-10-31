//! Shared client logic used by the GUI and TUI clients (PLAN.md section 14): instances,
//! certificate pinning, tokens, the REST and gateway clients, the store, optimistic sending,
//! notifications, slash commands, view helpers and the client config.
//!
//! Voice and audio come in Phase 4.

pub mod address;
pub mod commands;
pub mod config;
mod core;
pub mod files;
pub mod gateway;
pub mod known_hosts;
pub mod notify;
pub mod rest;
pub mod store;
pub mod time;
pub mod tls;
pub mod tokens;
pub mod view;

pub use crate::core::{
    CertStatus, Core, CoreError, CoreEvent, CoreEvents, CoreOptions, Draft, InstanceId, MAX_QUEUED,
    Probe, Store, content_type, now_ms, uuid_v4,
};
pub use hermes_proto as proto;
