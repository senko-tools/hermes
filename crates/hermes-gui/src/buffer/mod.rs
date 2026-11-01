//! Buffers: what a pane shows (PLAN.md section 15.1).

pub mod channel;
pub mod results;
pub mod settings;
pub mod settings_tabs;
pub mod space_settings;
pub mod voice_room;

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
    /// A space's settings (PLAN.md sections 15.1 and 15.4).
    SpaceSettings {
        inst: InstanceId,
        space: Id,
    },
    /// Search results, mentions or pins.
    Results(results::Key),
    /// The participants of a voice channel's or DM's voice room.
    VoiceRoom {
        inst: InstanceId,
        channel: Id,
    },
}

impl Buffer {
    /// A list of messages to jump from (search, mentions, pins).
    pub fn is_results(&self) -> bool {
        matches!(self, Buffer::Results(_))
    }
}
