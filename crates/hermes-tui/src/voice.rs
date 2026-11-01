//! Voice in the TUI (PLAN.md section 16): `J` joins, `L` leaves, `m` mutes, `D` deafens, the
//! people in each room in the sidebar, incoming calls and the voice part of the status line.
//! There is no push-to-talk (section 16.3): voice activity and toggle mute only.

use std::time::{Duration, Instant};

use hermes_client_core::audio::Sound;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::store::InstanceStore;
use hermes_client_core::{Core, InstanceId, VoiceStatus, view};

use crate::app::{App, Focus, Popup, SidebarItem, TaskMsg};

/// An incoming call rings for at most this long, the sound every `RING_EVERY`.
const RING_FOR: Duration = Duration::from_secs(30);
const RING_EVERY: Duration = Duration::from_secs(2);

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

/// The voice part of the status line: `voice: Lounge 38ms m`, `voice: joining Lounge…`; `None`
/// while not in voice.
pub fn status_line(app: &App) -> Option<String> {
    if let Some(name) = &app.joining {
        return Some(format!("voice: joining {name}…"));
    }
    status_text(&app.core)
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

/// Whether `J` would join something now: the selected sidebar entry (focus there) or the open
/// channel has a voice room.
pub fn room_here(app: &App) -> bool {
    if app.focus == Focus::Sidebar {
        return match app.sidebar.get(app.sidebar_sel) {
            Some(SidebarItem::Channel { kind, .. }) => has_room(*kind),
            Some(SidebarItem::VoiceUser { .. }) => true,
            _ => false,
        };
    }
    app.current.as_ref().is_some_and(|(inst, channel)| {
        app.core
            .store()
            .instance(inst)
            .and_then(|s| s.channel(*channel))
            .is_some_and(|c| has_room(c.kind))
    })
}

/// The bottom line of a voice channel's or DM's message pane: who is in the room and the keys.
pub fn room_footer(app: &App, inst: &str, channel: Id) -> Option<String> {
    let store = app.core.store();
    let s = store.instance(inst)?;
    if !has_room(s.channel(channel)?.kind) {
        return None;
    }
    let mut people: Vec<_> = s
        .voice_states
        .values()
        .filter(|v| v.channel_id == channel)
        .collect();
    people.sort_by_key(|v| (v.joined_at, v.user_id));
    let space = s.space_of(channel);
    let names: Vec<String> = people
        .iter()
        .map(|v| s.display_name(space, v.user_id))
        .collect();
    let k = |a| app.keymap.describe(a);
    use crate::keymap::Action;
    let here = app
        .core
        .voice_channel()
        .as_ref()
        .map(|(i, c)| (i.as_str(), *c))
        == Some((inst, channel));
    let keys = if here {
        format!(
            "{}: mute  {}: deafen  {}: leave",
            k(Action::ToggleMute),
            k(Action::ToggleDeafen),
            k(Action::LeaveVoice)
        )
    } else {
        format!("{}: join voice", k(Action::JoinVoice))
    };
    let who = if names.is_empty() {
        "nobody in voice".to_owned()
    } else {
        format!("in voice: {}", names.join(", "))
    };
    Some(format!(" {who} · {keys} "))
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
            self.dismiss_call();
            if matches!(self.popup, Some(Popup::Call { .. })) {
                self.popup = None;
            }
        }
        let name = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| Some(view::channel_title(s, s.channel(channel)?)))
                .unwrap_or_default()
        };
        self.info(format!("joining {name}…"));
        self.joining = Some(name.clone());
        let core = self.core.clone();
        let tasks = self.tasks.clone();
        tokio::spawn(async move {
            let result = core
                .join_or_call(&inst, channel)
                .await
                .map(|()| name)
                .map_err(|e| e.to_string());
            let _ = tasks.send(TaskMsg::VoiceJoined(result));
        });
    }

    /// A join finished. A failure is shown in a popup so it cannot be missed.
    pub fn voice_joined(&mut self, result: Result<String, String>) {
        let name = self.joining.take().unwrap_or_default();
        match result {
            Ok(room) => {
                self.info(format!(
                    "in voice: {room} ({}: mute, {}: deafen, {}: leave)",
                    self.keymap.describe(crate::keymap::Action::ToggleMute),
                    self.keymap.describe(crate::keymap::Action::ToggleDeafen),
                    self.keymap.describe(crate::keymap::Action::LeaveVoice)
                ));
                self.rebuild_sidebar();
            }
            Err(err) => {
                self.error(format!("could not join voice {name}: {err}"));
                self.popup = Some(Popup::Notice {
                    title: "voice".into(),
                    text: format!("Could not join {name}:\n{err}"),
                });
            }
        }
    }

    pub fn answer_call(&mut self, inst: InstanceId, channel: Id) {
        self.dismiss_call();
        self.open(inst.clone(), channel);
        self.focus = Focus::Messages;
        self.join_room(inst, channel);
    }

    /// Stops the ringing (the popup is closed by the caller).
    pub fn dismiss_call(&mut self) {
        if self.ringing.take().is_some() {
            self.core.stop_sounds();
        }
        self.ring_times = None;
    }

    /// Every 250 ms: repeats the ring sound while a call rings, for at most 30 s.
    pub fn on_tick(&mut self) {
        let Some((since, last)) = self.ring_times else {
            return;
        };
        if since.elapsed() >= RING_FOR {
            if matches!(self.popup, Some(Popup::Call { .. })) {
                self.popup = None;
            }
            if let Some((_, _, name)) = self.ringing.clone() {
                self.info(format!("missed call from {name}"));
            }
            self.dismiss_call();
        } else if last.elapsed() >= RING_EVERY {
            self.core.play_sound(Sound::Ring);
            self.ring_times = Some((since, Instant::now()));
        }
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
                if matches!(self.popup, Some(Popup::Call { .. })) {
                    self.popup = None;
                }
                return self.answer_call(inst, channel);
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
        self.info(format!("{name} is calling"));
        self.core.play_sound(Sound::Ring);
        self.ring_times = Some((Instant::now(), Instant::now()));
        // Do not cover a form or a question; the status line and `:voice` still work then.
        if self.popup.is_none() || matches!(self.popup, Some(Popup::Call { .. })) {
            self.popup = Some(Popup::Call {
                inst: inst.clone(),
                channel,
                name: name.clone(),
            });
        }
        self.ringing = Some((inst, channel, name));
    }

    pub fn call_ring_stopped(&mut self, inst: &str, channel: Id) {
        if let Some((i, c, name)) = &self.ringing
            && i == inst
            && *c == channel
        {
            let text = format!("{name} stopped calling");
            self.dismiss_call();
            if matches!(self.popup, Some(Popup::Call { channel: c, .. }) if c == channel) {
                self.popup = None;
            }
            self.info(text);
        }
    }
}
