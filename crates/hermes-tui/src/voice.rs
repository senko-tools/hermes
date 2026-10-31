//! Voice in the TUI (PLAN.md section 16): `J` joins, `L` leaves, `m` mutes, `D` deafens, the
//! people in each room in the sidebar, incoming calls and the voice part of the status line.
//! There is no push-to-talk (section 16.3): voice activity and toggle mute only.

use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::store::InstanceStore;
use hermes_client_core::{Core, InstanceId, VoiceStatus, view};

use crate::app::{App, Focus, SidebarItem, TaskMsg};

/// Channels with a voice room.
pub fn has_room(kind: ChannelKind) -> bool {
    matches!(
        kind,
        ChannelKind::Voice | ChannelKind::Dm | ChannelKind::GroupDm
    )
}

/// The people in the room of `channel`, in join order, as sidebar rows.
pub fn voice_users(
    core: &Core,
    s: &InstanceStore,
    inst: &InstanceId,
    channel: Id,
    space: Option<Id>,
) -> Vec<SidebarItem> {
    let mut states: Vec<_> = s
        .voice_states
        .values()
        .filter(|v| v.channel_id == channel)
        .collect();
    states.sort_by_key(|v| (v.joined_at, v.user_id));
    states
        .into_iter()
        .map(|v| SidebarItem::VoiceUser {
            inst: inst.clone(),
            channel,
            user: v.user_id,
            name: s.display_name(space, v.user_id),
            speaking: core.is_speaking(inst, v.user_id),
            muted: v.self_mute || v.server_mute,
            deafened: v.self_deaf || v.server_deaf,
        })
        .collect()
}

/// `voice: Lounge 38ms m d`, `voice: Lounge connecting…`; `None` while not in voice.
pub fn status_text(core: &Core) -> Option<String> {
    let (inst, channel) = core.voice_channel()?;
    let name = {
        let store = core.store();
        store
            .instance(&inst)
            .and_then(|s| Some(view::channel_title(s, s.channel(channel)?)))
            .unwrap_or_else(|| "voice".into())
    };
    let state = match core.voice_status() {
        VoiceStatus::Connected { ping_ms, .. } => format!("{ping_ms}ms"),
        VoiceStatus::Connecting => "connecting…".into(),
        VoiceStatus::Reconnecting { attempt } => format!("reconnecting ({attempt})…"),
        VoiceStatus::Disconnected => "disconnected".into(),
    };
    let mut text = format!("voice: {name} {state}");
    if core.self_deaf() {
        text.push_str(" d");
    } else if core.self_mute() {
        text.push_str(" m");
    }
    Some(text)
}

impl App {
    /// `J`: the room of the selected sidebar entry (a voice channel, a DM, or someone in a
    /// room); from the messages, the open channel's room.
    pub fn join_selected(&mut self) {
        let target = match (self.focus, self.sidebar.get(self.sidebar_sel)) {
            (
                Focus::Sidebar,
                Some(SidebarItem::Channel {
                    inst,
                    channel,
                    kind,
                    ..
                }),
            ) if has_room(*kind) => Some((inst.clone(), *channel)),
            (Focus::Sidebar, Some(SidebarItem::VoiceUser { inst, channel, .. })) => {
                Some((inst.clone(), *channel))
            }
            _ => self.current.clone().filter(|(inst, channel)| {
                self.core
                    .store()
                    .instance(inst)
                    .and_then(|s| s.channel(*channel))
                    .is_some_and(|c| has_room(c.kind))
            }),
        };
        match target {
            Some((inst, channel)) => self.join_room(inst, channel),
            None => self.error("select a voice channel or a DM to join its voice room".into()),
        }
    }

    /// Joins a room (an empty DM room starts a call).
    pub fn join_room(&mut self, inst: InstanceId, channel: Id) {
        if self.core.voice_channel() == Some((inst.clone(), channel)) {
            return;
        }
        if self
            .ringing
            .as_ref()
            .is_some_and(|(i, c, _)| *i == inst && *c == channel)
        {
            self.ringing = None;
            self.core.stop_sounds();
        }
        let name = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| Some(view::channel_title(s, s.channel(channel)?)))
                .unwrap_or_default()
        };
        self.info(format!("joining {name}…"));
        let core = self.core.clone();
        let tasks = self.tasks.clone();
        tokio::spawn(async move {
            let msg = match core.join_or_call(&inst, channel).await {
                Ok(()) => TaskMsg::Info(format!("joined voice: {name}")),
                Err(e) => TaskMsg::Error(format!("could not join voice: {e}")),
            };
            let _ = tasks.send(msg);
        });
    }

    pub fn leave_voice(&mut self) {
        if self.core.voice_channel().is_none() {
            return self.info("not in voice".into());
        }
        let core = self.core.clone();
        let tasks = self.tasks.clone();
        tokio::spawn(async move {
            core.leave_voice().await;
            let _ = tasks.send(TaskMsg::Info("left voice".into()));
        });
    }

    pub fn toggle_mute(&mut self) {
        // Unmuting while deafened undeafens too (deafened implies muted).
        if self.core.self_deaf() {
            self.core.set_self_deaf(false);
            self.core.set_self_mute(false);
        } else {
            self.core.set_self_mute(!self.core.self_mute());
        }
        self.info(
            if self.core.self_mute() {
                "muted"
            } else {
                "unmuted"
            }
            .into(),
        );
        self.rebuild_sidebar();
    }

    pub fn toggle_deafen(&mut self) {
        self.core.set_self_deaf(!self.core.self_deaf());
        self.info(
            if self.core.self_deaf() {
                "deafened"
            } else {
                "undeafened"
            }
            .into(),
        );
        self.rebuild_sidebar();
    }

    /// `:voice [channel]`: a voice channel by name; without a name the ringing call, else the
    /// open channel's room.
    pub fn voice_command(&mut self, name: &str) {
        if name.trim().is_empty() {
            if let Some((inst, channel, _)) = self.ringing.clone() {
                self.open(inst.clone(), channel);
                self.focus = Focus::Messages;
                return self.join_room(inst, channel);
            }
            self.focus = Focus::Messages;
            return self.join_selected();
        }
        let near = self.current.as_ref().map(|(_, c)| *c);
        let mut instances = self.core.instance_ids();
        // The open channel's instance first.
        if let Some((inst, _)) = &self.current {
            instances.retain(|i| i != inst);
            instances.insert(0, inst.clone());
        }
        let found = instances.into_iter().find_map(|inst| {
            let store = self.core.store();
            let s = store.instance(&inst)?;
            view::find_voice_channel(s, near, name).map(|c| (inst.clone(), c))
        });
        match found {
            Some((inst, channel)) => self.join_room(inst, channel),
            None => self.error(format!("no voice channel named {name:?}")),
        }
    }

    pub fn incoming_call(&mut self, inst: InstanceId, channel: Id, caller: Id) {
        if self.core.voice_channel() == Some((inst.clone(), channel)) {
            return;
        }
        let name = self
            .core
            .store()
            .instance(&inst)
            .map_or_else(|| "someone".into(), |s| s.display_name(None, caller));
        // Terminal bell, like notifications.
        print!("\x07");
        self.info(format!(
            "{name} is calling: :voice to answer (or J on the DM)"
        ));
        self.ringing = Some((inst, channel, name));
    }

    pub fn call_ring_stopped(&mut self, inst: &str, channel: Id) {
        if let Some((i, c, name)) = &self.ringing
            && i == inst
            && *c == channel
        {
            let text = format!("{name} stopped calling");
            self.ringing = None;
            self.info(text);
        }
    }
}
