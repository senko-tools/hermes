//! Buffers: what a pane shows (PLAN.md section 15.1). Mentions, Search, Logs, VoiceRoom and
//! SpaceSettings come with their features.

pub mod channel;
pub mod settings;

use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Buffer {
    Empty,
    /// A text channel, a voice channel's text chat, a DM or a group DM.
    Channel {
        inst: InstanceId,
        channel: Id,
    },
    Settings,
}
