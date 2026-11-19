//! Shared client logic used by the GUI and TUI clients (PLAN.md section 14): instances,
//! certificate pinning, tokens, the REST and gateway clients, the store, optimistic sending,
//! notifications, slash commands, view helpers, the client config, and voice with the audio
//! engine.

pub mod address;
pub mod channel_keys;
pub mod commands;
pub mod compress;
pub mod config;
mod core;
pub mod crash;
pub mod diagnostics;
pub mod e2ee;
pub mod files;
pub mod gateway;
pub mod known_hosts;
pub mod link_preview;
pub mod logs;
pub mod menus;
pub mod net;
pub mod notify;
pub mod picture;
pub mod read_marks;
pub mod remote_file;
pub mod rest;
pub mod search_index;
pub mod secure;
pub mod store;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod time;
pub mod tls;
pub mod tokens;
pub mod update;
pub mod updater;
pub mod view;
pub mod voice;
pub mod volumes;

pub use crate::core::{
    CertStatus, Core, CoreError, CoreEvent, CoreEvents, CoreOptions, DEFAULT_CHANNEL_STATE_MAX_AGE,
    Draft, InstanceId, MAX_QUEUED, Probe, Store, StoreRef, content_type, now_ms, uuid_v4,
};
pub use hermes_audio as audio;
pub use hermes_proto as proto;
pub use voice::VoiceStatus;
