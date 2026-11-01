//! Shared client logic used by the GUI and TUI clients (PLAN.md section 14): instances,
//! certificate pinning, tokens, the REST and gateway clients, the store, optimistic sending,
//! notifications, slash commands, view helpers, the client config, and voice with the audio
//! engine.

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
pub mod voice;
pub mod volumes;

pub use crate::core::{
    CertStatus, Core, CoreError, CoreEvent, CoreEvents, CoreOptions, Draft, InstanceId, MAX_QUEUED,
    Probe, Store, content_type, now_ms, uuid_v4,
};
pub use hermes_audio as audio;
pub use hermes_proto as proto;
pub use voice::VoiceStatus;
