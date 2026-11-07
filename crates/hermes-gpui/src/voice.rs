//! Voice (PLAN.md sections 12.6, 15.1 and 15.4): joining and leaving, mute and deafen, the panel
//! over the user panel with its network graph, the room page, the per-user volume menu, and the
//! incoming-call banner with its ring.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, FontWeight, MouseButton, Pixels, Point, SharedString, Size,
    Subscription, Window, anchored, canvas, deferred, div, img, size,
};
use hermes_client_core::audio::{InputMode, Sound};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::voice::VoiceLink;
use hermes_client_core::{InstanceId, VoiceStatus, view};

use crate::assets::{self, icon};
use crate::channel::icon_button;
use crate::menu_entries::Target;
use crate::shell::{Page, Shell};
use crate::theme::{self, px};

/// An incoming call rings for at most this long.
const RING_FOR: Duration = Duration::from_secs(30);
/// One ring sound is about 1.6 s; it is repeated at this interval.
const RING_EVERY: Duration = Duration::from_millis(2000);
/// Tiles keep this aspect ratio (width / height), the shape of most screens.
const ASPECT: f32 = 16.0 / 9.0;
const AVATAR_MAX: f32 = 72.0;
const TILE_GAP: f32 = 12.0;
/// How many seconds the network graph shows.
const HISTORY: usize = 120;

/// `voice.join_on`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinOn {
    DoubleClick,
    SingleClick,
}

impl JoinOn {
    pub fn from_config(value: &str) -> JoinOn {
        if value.eq_ignore_ascii_case("single_click") {
            JoinOn::SingleClick
        } else {
            JoinOn::DoubleClick
        }
    }
}

/// A ringing incoming call.
pub struct Call {
    pub inst: InstanceId,
    pub channel: Id,
    pub caller: Id,
    pub since: Instant,
}

/// The open per-user volume menu.
pub struct VolumeMenu {
    pub inst: InstanceId,
    pub user: Id,
    pub name: String,
    pub at: Point<Pixels>,
    pub volume: f32,
    pub muted: bool,
    /// The volume of their screen share's sound, while I watch it.
    pub share_volume: Option<f32>,
    volume_slider: Entity<SliderState>,
    share_slider: Option<Entity<SliderState>>,
    _sliders: Vec<Subscription>,
}

/// One second of the voice connection.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NetSample {
    pub ping_ms: u32,
    /// Lost among the packets sent during this second.
    pub loss_pct: f32,
    pub down_kbit: f32,
    pub up_kbit: f32,
}

/// The last two minutes of the voice connection, for the graph.
#[derive(Debug, Default)]
pub struct NetHistory {
    pub samples: VecDeque<NetSample>,
    pub link: Option<VoiceLink>,
}

impl NetHistory {
    /// Adds a second, from the connection's running totals.
    pub fn push(&mut self, ping_ms: u32, link: Option<VoiceLink>) {
        let sample = match (self.link, link) {
            (Some(old), Some(new)) => {
                let sent = new.sent_packets.saturating_sub(old.sent_packets);
                let lost = new.lost_packets.saturating_sub(old.lost_packets);
                NetSample {
                    ping_ms,
                    loss_pct: if sent == 0 {
                        0.0
                    } else {
                        lost as f32 * 100.0 / sent as f32
                    },
                    down_kbit: new.received_bytes.saturating_sub(old.received_bytes) as f32 * 8.0
                        / 1000.0,
                    up_kbit: new.sent_bytes.saturating_sub(old.sent_bytes) as f32 * 8.0 / 1000.0,
                }
            }
            _ => NetSample {
                ping_ms,
                ..NetSample::default()
            },
        };
        self.link = link;
        self.samples.push_back(sample);
        while self.samples.len() > HISTORY {
            self.samples.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.samples.clear();
        self.link = None;
    }
}

/// The column count and tile size that make `n` tiles of [`ASPECT`] largest in `area` with
/// `gap` between them; on a tie, more columns (side by side reads better than stacked).
pub fn layout(n: usize, area: (f32, f32), gap: f32) -> (usize, (f32, f32)) {
    let mut best = (1, (0.0, 0.0));
    for cols in 1..=n.max(1) {
        let rows = n.max(1).div_ceil(cols);
        let width = (area.0 - gap * (cols - 1) as f32) / cols as f32;
        let height = (area.1 - gap * (rows - 1) as f32) / rows as f32;
        let tile_width = width.min(height * ASPECT).max(0.0);
        if tile_width >= best.1.0 {
            best = (cols, (tile_width, tile_width / ASPECT));
        }
    }
    best
}

/// One participant card.
struct Participant {
    user: Id,
    name: String,
    speaking: bool,
    muted: bool,
    deafened: bool,
    /// Your volume for them (1.0 = 100 %), or `None` for yourself.
    volume: Option<f32>,
    locally_muted: bool,
}

impl Shell {
    /// Join (or switch to) the voice room of a voice channel or DM. In a DM nobody is in, this
    /// starts a call. While a join runs, further requests do nothing.
    pub fn join_voice(&mut self, inst: InstanceId, channel: Id, cx: &mut Context<Self>) {
        let key = (inst.clone(), channel);
        if self.core.voice_channel().as_ref() == Some(&key) || self.joining.is_some() {
            return;
        }
        self.joining = Some(key.clone());
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                core.join_or_call(&inst, channel)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |this, result, _, cx| {
                this.joining = None;
                match result {
                    Ok(()) => {
                        if this
                            .call
                            .as_ref()
                            .is_some_and(|c| c.inst == key.0 && c.channel == key.1)
                        {
                            this.stop_ringing();
                        }
                        this.net.clear();
                        if !this.config.voice.noise_suppression && !this.headphones_hint_shown {
                            this.headphones_hint_shown = true;
                            this.info(
                                "Noise suppression is off: headphones are recommended (there is \
                                 no echo cancellation).",
                                cx,
                            );
                        }
                    }
                    Err(err) => this.error(format!("Could not join voice: {err}"), cx),
                }
            },
        );
        cx.notify();
    }

    pub fn leave_voice(&mut self, cx: &mut Context<Self>) {
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.leave_voice().await },
            |this, (), _, _| {
                this.net.clear();
                this.network_open = None;
            },
        );
    }

    pub fn toggle_mute(&mut self, _cx: &mut Context<Self>) {
        // Unmuting while deafened undeafens too (deafened implies muted).
        if self.core.self_deaf() {
            self.core.set_self_deaf(false);
            self.core.set_self_mute(false);
        } else {
            self.core.set_self_mute(!self.core.self_mute());
        }
        self.update_tray_voice();
    }

    pub fn toggle_deafen(&mut self, _cx: &mut Context<Self>) {
        self.core.set_self_deaf(!self.core.self_deaf());
        self.update_tray_voice();
    }

    /// An incoming call: show the banner and ring (unless I am in that room already).
    pub fn incoming_call(
        &mut self,
        inst: InstanceId,
        channel: Id,
        caller: Id,
        cx: &mut Context<Self>,
    ) {
        if self.core.voice_channel() == Some((inst.clone(), channel)) {
            return;
        }
        self.call = Some(Call {
            inst,
            channel,
            caller,
            since: Instant::now(),
        });
        self.core.play_sound(Sound::Ring);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RING_EVERY).await;
                let again = this.update(cx, |this, cx| {
                    let Some(call) = &this.call else {
                        return false;
                    };
                    if call.since.elapsed() >= RING_FOR {
                        this.stop_ringing();
                        cx.notify();
                        return false;
                    }
                    this.core.play_sound(Sound::Ring);
                    true
                });
                if !matches!(again, Ok(true)) {
                    return;
                }
            }
        })
        .detach();
    }

    /// The caller left or someone answered: stop the banner if it is for `channel`.
    pub fn call_ring_stopped(&mut self, inst: &str, channel: Id) {
        if self
            .call
            .as_ref()
            .is_some_and(|c| c.inst == inst && c.channel == channel)
        {
            self.stop_ringing();
        }
    }

    fn stop_ringing(&mut self) {
        if self.call.take().is_some() {
            self.core.stop_sounds();
        }
    }

    fn answer_call(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(call) = self.call.take() {
            self.core.stop_sounds();
            self.select(call.inst.clone(), call.channel, window, cx);
            self.join_voice(call.inst, call.channel, cx);
        }
    }

    /// Takes a sample of the connection once a second while in voice.
    pub fn start_net_sampler(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let result = this.update(cx, |this, cx| {
                    if let VoiceStatus::Connected { ping_ms, .. } = this.core.voice_status() {
                        let link = this.core.voice_link();
                        this.net.push(ping_ms, link);
                        if this.network_open.is_some() {
                            cx.notify();
                        }
                    }
                });
                if result.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    // ---- the banner ---------------------------------------------------------------------------

    pub fn render_call_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let call = self.call.as_ref()?;
        let (caller, place) = {
            let store = self.core.store();
            let s = store.instance(&call.inst);
            let caller = s.map_or_else(
                || "Someone".to_owned(),
                |s| s.display_name(None, call.caller),
            );
            let place = s
                .and_then(|s| {
                    let c = s.channel(call.channel)?;
                    (c.kind == ChannelKind::GroupDm).then(|| view::channel_title(s, c))
                })
                .map(|title| format!(" in {title}"))
                .unwrap_or_default();
            (caller, place)
        };
        Some(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(8.))
                .px(px(16.))
                .py(px(8.))
                .bg(theme::success_soft())
                .border_b_1()
                .border_color(theme::speaking())
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("{caller} is calling{place}")),
                )
                .child(
                    Button::new("call-join")
                        .primary()
                        .label("Join")
                        .on_click(cx.listener(|this, _, window, cx| this.answer_call(window, cx))),
                )
                .child(
                    Button::new("call-dismiss")
                        .label("Dismiss")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.stop_ringing();
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    // ---- the sidebar's parts ------------------------------------------------------------------

    /// The panel over the user panel while in voice: where, the connection, and the buttons.
    pub fn render_voice_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (inst, channel) = self.core.voice_channel()?;
        let place = {
            let store = self.core.store();
            let s = store.instance(&inst);
            match s.and_then(|s| Some((s, s.channel(channel)?))) {
                Some((s, c)) => match s.space_of(channel).and_then(|id| s.spaces.get(&id)) {
                    Some(space) => {
                        format!("{} · {}", c.name.as_deref().unwrap_or(""), space.space.name)
                    }
                    None => view::channel_title(s, c),
                },
                None => "Voice".to_owned(),
            }
        };
        let status = self.core.voice_status();
        let connected = matches!(status, VoiceStatus::Connected { .. });
        let quality = match &status {
            VoiceStatus::Connected { ping_ms, loss_pct } => {
                format!("{ping_ms} ms · {loss_pct:.1} % loss")
            }
            VoiceStatus::Connecting => "Connecting…".to_owned(),
            VoiceStatus::Reconnecting { attempt } => format!("Reconnecting (try {attempt})…"),
            VoiceStatus::Disconnected => "Disconnected".to_owned(),
        };
        let ptt = (self.core.audio_settings().mode == InputMode::PushToTalk
            || self.core.voice_requires_ptt())
        .then_some(self.ptt_down);
        let state_color = if connected {
            theme::speaking()
        } else {
            theme::accent()
        };
        let sharing = self.core.sharing_screen();
        let (room_inst, _) = (inst.clone(), ());
        let info = div()
            .id("voice-panel-info")
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(4.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|d| d.bg(theme::hover()))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select(room_inst.clone(), channel, window, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(icon(assets::WAVEFORM, 14., state_color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::tiny_size())
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(state_color)
                            .child(if connected {
                                "Voice connected"
                            } else {
                                "Voice connecting"
                            }),
                    )
                    .children(ptt.map(|held| {
                        div()
                            .px(px(6.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .text_size(theme::tiny_size())
                            .when(held, |d| {
                                d.bg(theme::speaking()).text_color(theme::on_accent())
                            })
                            .when(!held, |d| d.text_color(theme::text_tertiary()))
                            .child("PTT")
                    })),
            )
            .child(
                div()
                    .truncate()
                    .text_size(theme::tiny_size())
                    .text_color(theme::text_secondary())
                    .child(place),
            );
        // The connection line opens the network graph.
        let network = div()
            .id("voice-panel-network")
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(4.))
            .py(px(2.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|d| d.bg(theme::hover()))
            .text_size(theme::tiny_size())
            .text_color(theme::text_tertiary())
            .child(div().flex_1().min_w_0().truncate().child(quality))
            .child(self.render_spark())
            .on_click(cx.listener(|this, event: &gpui_kit::ClickEvent, _, cx| {
                this.network_open = match this.network_open {
                    Some(_) => None,
                    None => Some(event.position()),
                };
                cx.notify();
            }));
        let share_label = if sharing {
            "Stop sharing"
        } else {
            "Share screen"
        };
        let buttons = div()
            .flex()
            .gap(px(4.))
            .child(
                panel_button("panel-share", assets::MONITOR_SHARE, sharing, share_label).on_click(
                    cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                        this.share_button(event.position(), window, cx);
                    }),
                ),
            )
            .child(
                panel_button("panel-leave", assets::HANG_UP, true, "Leave voice")
                    .on_click(cx.listener(|this, _, _, cx| this.leave_voice(cx))),
            );
        Some(
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .gap(px(4.))
                .p(px(6.))
                .bg(theme::footer())
                .border_t_1()
                .border_color(theme::border())
                .child(info)
                .child(network)
                .child(buttons)
                .into_any_element(),
        )
    }

    /// The last half minute of ping, small, on the connection line.
    fn render_spark(&self) -> AnyElement {
        let samples: Vec<u32> = self
            .net
            .samples
            .iter()
            .rev()
            .take(24)
            .rev()
            .map(|s| s.ping_ms)
            .collect();
        let top = samples.iter().copied().max().unwrap_or(1).max(40) as f32;
        div()
            .flex()
            .flex_shrink_0()
            .items_end()
            .gap(px(1.))
            .h(px(12.))
            .children(samples.into_iter().map(|ping| {
                div()
                    .w(px(2.))
                    .h(px((ping as f32 / top * 12.).clamp(1., 12.)))
                    .bg(theme::speaking())
            }))
            .into_any_element()
    }

    /// The network details over the sidebar: live graphs and the numbers.
    pub fn render_network(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let at = self.network_open?;
        let last = self.net.samples.back().copied().unwrap_or_default();
        let server = self
            .net
            .link
            .map_or_else(|| "—".to_owned(), |l| l.server.to_string());
        let rtt = self
            .net
            .link
            .map_or_else(|| "—".to_owned(), |l| format!("{} ms", l.rtt_ms));
        let total = self.net.link.map_or_else(
            || "—".to_owned(),
            |l| {
                let lost = l.lost_packets as f32 * 100.0 / l.sent_packets.max(1) as f32;
                format!(
                    "{} sent, {} lost ({lost:.1} %)",
                    l.sent_packets, l.lost_packets
                )
            },
        );
        let ping_values: Vec<f32> = self.net.samples.iter().map(|s| s.ping_ms as f32).collect();
        let loss_values: Vec<f32> = self.net.samples.iter().map(|s| s.loss_pct).collect();
        let down_values: Vec<f32> = self.net.samples.iter().map(|s| s.down_kbit).collect();
        let up_values: Vec<f32> = self.net.samples.iter().map(|s| s.up_kbit).collect();
        Some(
            deferred(
                anchored()
                    .position(at)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id("network")
                            .occlude()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .w(px(340.))
                            .p(px(12.))
                            .rounded(px(8.))
                            .bg(theme::sidebar())
                            .border_1()
                            .border_color(theme::border())
                            .font_family(theme::font())
                            .text_color(theme::text())
                            .text_size(theme::small_size())
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.network_open = None;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .child(
                                        div()
                                            .flex_1()
                                            .font_weight(FontWeight::MEDIUM)
                                            .child("Voice connection"),
                                    )
                                    .child(icon_button("network-close", assets::CLOSE).on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.network_open = None;
                                            cx.notify();
                                        }),
                                    )),
                            )
                            .child(graph(
                                "Latency",
                                format!("{} ms", last.ping_ms),
                                &ping_values,
                                40.,
                                theme::speaking(),
                            ))
                            .child(graph(
                                "Packet loss",
                                format!("{:.1} %", last.loss_pct),
                                &loss_values,
                                2.,
                                theme::danger(),
                            ))
                            .child(graph(
                                "Download",
                                format!("{:.0} kbit/s", last.down_kbit),
                                &down_values,
                                100.,
                                theme::link(),
                            ))
                            .child(graph(
                                "Upload",
                                format!("{:.0} kbit/s", last.up_kbit),
                                &up_values,
                                100.,
                                theme::accent(),
                            ))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .text_size(theme::tiny_size())
                                    .text_color(theme::text_secondary())
                                    .child(format!("Round trip (QUIC): {rtt}"))
                                    .child(format!("Packets: {total}"))
                                    .child(format!("Server: {server}")),
                            ),
                    ),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }

    /// The buttons next to my name: mute, deafen and settings.
    pub fn render_me_buttons(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let muted = self.core.self_mute() || self.core.self_deaf();
        let deafened = self.core.self_deaf();
        vec![
            me_button(
                "me-mute",
                if muted { assets::MIC_OFF } else { assets::MIC },
                muted,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_mute(cx);
                cx.notify();
            }))
            .into_any_element(),
            me_button(
                "me-deafen",
                if deafened {
                    assets::HEADPHONES_OFF
                } else {
                    assets::HEADPHONES
                },
                deafened,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_deafen(cx);
                cx.notify();
            }))
            .into_any_element(),
            me_button("me-settings", assets::COG, false)
                .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx)))
                .into_any_element(),
        ]
    }

    /// Someone in a voice room, under its channel in the sidebar.
    #[allow(clippy::too_many_arguments)]
    pub fn render_voice_user(
        &self,
        inst: InstanceId,
        _channel: Id,
        user: Id,
        name: String,
        muted: bool,
        deafened: bool,
        speaking: bool,
        me: bool,
        nested: bool,
        _dm: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let avatar = {
            let store = self.core.store();
            let found = store.instance(&inst).and_then(|s| s.user(user));
            self.avatar(&inst, found, &name, 22.)
        };
        let target_inst = inst.clone();
        let element = div()
            .id(SharedString::from(format!("voice-user-{user}")))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(8.))
            .h(px(30.))
            .pl(px(if nested { 46. } else { 36. }))
            .pr(px(8.))
            .rounded(px(4.))
            .hover(|d| d.bg(theme::hover()))
            .text_size(theme::small_size())
            .text_color(if speaking {
                theme::text()
            } else {
                theme::text_secondary()
            })
            .child(
                div()
                    .flex_shrink_0()
                    .rounded(px(12.))
                    .border_2()
                    .border_color(if speaking {
                        theme::speaking()
                    } else {
                        gpui_kit::rgba(0)
                    })
                    .child(avatar),
            )
            .child(div().flex_1().min_w_0().truncate().child(name))
            .when(muted && !deafened, |d| {
                d.child(icon(assets::MIC_OFF, 14., theme::muted_voice()))
            })
            .when(deafened, |d| {
                d.child(icon(assets::HEADPHONES_OFF, 14., theme::muted_voice()))
            })
            .when(!me, |d| {
                d.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, _| {
                        this.right_pressed = Some(Target::Member(target_inst.clone(), None, user));
                    }),
                )
                .on_mouse_up(
                    MouseButton::Right,
                    cx.listener(move |this, event: &gpui_kit::MouseUpEvent, window, cx| {
                        if this.right_pressed.is_some() {
                            this.open_volume_menu(inst.clone(), user, event.position, window, cx);
                        }
                    }),
                )
            });
        element.into_any_element()
    }

    // ---- the room -----------------------------------------------------------------------------

    fn participants(&self, inst: &InstanceId, channel: Id) -> Vec<Participant> {
        let store = self.core.store();
        let Some(s) = store.instance(inst) else {
            return Vec::new();
        };
        let me = s.me_id();
        let space = s.space_of(channel);
        let mut states: Vec<_> = s
            .voice_states
            .values()
            .filter(|v| v.channel_id == channel)
            .copied()
            .collect();
        states.sort_by_key(|v| (v.joined_at, v.user_id));
        states
            .into_iter()
            .map(|v| {
                let own = Some(v.user_id) == me;
                Participant {
                    user: v.user_id,
                    name: s.display_name(space, v.user_id),
                    speaking: self.core.is_speaking(inst, v.user_id),
                    muted: v.self_mute || v.server_mute,
                    deafened: v.self_deaf || v.server_deaf,
                    volume: (!own).then(|| self.core.user_volume(inst, v.user_id)),
                    locally_muted: !own && self.core.user_muted(inst, v.user_id),
                }
            })
            .collect()
    }

    pub fn render_voice(
        &self,
        inst: InstanceId,
        channel: Id,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let people = self.participants(&inst, channel);
        let in_call = self.core.voice_channel() == Some((inst.clone(), channel));
        let sharers: HashSet<Id> = if in_call {
            self.core
                .screen_sharers()
                .into_iter()
                .filter(|(i, _)| *i == inst)
                .map(|(_, user)| user)
                .collect()
        } else {
            HashSet::new()
        };
        let summary = match people.len() {
            0 => "Nobody is here yet".to_owned(),
            1 => "1 person".to_owned(),
            n => format!("{n} people"),
        };
        // An enlarged share is the only tile, as long as it has a picture.
        let enlarged = self
            .enlarged_share
            .as_ref()
            .filter(|(i, user)| *i == inst && self.screens.contains_key(&(i.clone(), *user)))
            .map(|(_, user)| *user);
        let shown: Vec<&Participant> = people
            .iter()
            .filter(|p| enlarged.is_none_or(|user| user == p.user))
            .collect();
        let area = (
            self.room_area.width.as_f32(),
            self.room_area.height.as_f32(),
        );
        let (cols, tile_size) = if enlarged.is_some() {
            (1, area)
        } else {
            layout(shown.len(), area, TILE_GAP)
        };
        if let Some(latest) = &self.screen_latest {
            // Enlarged, or the tile of a share: the decoder needs no more pixels than this.
            latest.set_shown_size(
                (tile_size.0 * self.scale) as u32,
                (tile_size.1 * self.scale) as u32,
            );
        }
        let lines: Vec<AnyElement> = shown
            .chunks(cols.max(1))
            .map(|chunk| {
                div()
                    .flex()
                    .justify_center()
                    .gap(px(TILE_GAP))
                    .children(chunk.iter().map(|p| {
                        let share = sharers.contains(&p.user)
                            || (p.volume.is_none() && in_call && self.core.sharing_screen());
                        self.render_tile(&inst, p, tile_size, share, enlarged.is_some(), cx)
                    }))
                    .into_any_element()
            })
            .collect();
        let shell = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, window, cx| {
                let shell = shell.clone();
                window.defer(cx, move |_, cx| {
                    let _ = shell.update(cx, |this, cx| {
                        if this.room_area != bounds.size {
                            this.room_area = bounds.size;
                            cx.notify();
                        }
                    });
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap(px(12.))
            .p(px(16.))
            .child(
                div()
                    .text_size(theme::tiny_size())
                    .text_color(theme::text_tertiary())
                    .child(summary),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .items_center()
                    .justify_center()
                    .gap(px(TILE_GAP))
                    .child(measure)
                    .children(lines),
            )
    }

    fn render_tile(
        &self,
        inst: &InstanceId,
        p: &Participant,
        tile: (f32, f32),
        sharing: bool,
        enlarged: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let share_key = (inst.clone(), p.user);
        let watched = self.watching_shares.contains(&share_key);
        let picture = self.screens.get(&share_key);
        let state_icon = if p.deafened {
            Some(assets::HEADPHONES_OFF)
        } else if p.muted || p.locally_muted {
            Some(assets::MIC_OFF)
        } else {
            None
        };
        let name_row = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(p.name.clone())
            .children(state_icon.map(|g| icon(g, 14., theme::muted_voice())))
            .when(sharing, |d| {
                d.child(icon(assets::MONITOR_SHARE, 14., theme::accent()))
            });
        let own = p.volume.is_none();
        let content: AnyElement = match picture {
            // Their screen fills the tile; the name stays readable on a dark badge.
            Some(frame) => {
                let stop = share_key.clone();
                div()
                    .relative()
                    .size_full()
                    .bg(gpui_kit::rgb(0))
                    .child(
                        img(frame.image.clone())
                            .size_full()
                            .object_fit(gpui_kit::ObjectFit::Contain),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom(px(8.))
                            .left(px(8.))
                            .px(px(8.))
                            .py(px(4.))
                            .rounded(px(4.))
                            .bg(theme::scrim())
                            .child(name_row),
                    )
                    .child(
                        div().absolute().top(px(8.)).right(px(8.)).child(
                            icon_button(
                                SharedString::from(format!("stop-watching-{}", p.user)),
                                assets::CLOSE,
                            )
                            .bg(theme::scrim())
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.watch_share(stop.clone(), false);
                                    cx.notify();
                                },
                            )),
                        ),
                    )
                    .into_any_element()
            }
            None => {
                let side = (tile.1 * 0.4).clamp(24., AVATAR_MAX);
                let avatar = {
                    let store = self.core.store();
                    let user = store.instance(inst).and_then(|s| s.user(p.user));
                    self.avatar(inst, user, &p.name, side)
                };
                let note = if sharing && own {
                    Some(match self.core.share_watchers() {
                        Some(0) => "You are sharing your screen · nobody is watching".to_owned(),
                        Some(1) => "You are sharing your screen · 1 person is watching".to_owned(),
                        Some(n) => format!("You are sharing your screen · {n} people are watching"),
                        None => "You are sharing your screen".to_owned(),
                    })
                } else if sharing && watched {
                    Some("Loading their screen…".to_owned())
                } else if sharing {
                    Some("Sharing their screen · click to watch".to_owned())
                } else {
                    p.volume
                        .filter(|v| (*v - 1.0).abs() > 0.001)
                        .map(|v| format!("volume {:.0} %", v * 100.0))
                };
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .child(avatar)
                    .child(name_row)
                    .children(note.map(|note| {
                        div()
                            .text_size(theme::tiny_size())
                            .text_color(theme::text_tertiary())
                            .child(note)
                    }))
                    .children((sharing && own).then(|| self.render_preference_picker(cx)))
                    .into_any_element()
            }
        };
        let (menu_inst, user) = (inst.clone(), p.user);
        let can_menu = !own;
        let click_key = share_key.clone();
        let has_picture = picture.is_some();
        div()
            .id(SharedString::from(format!("tile-{}", p.user)))
            .w(px(tile.0))
            .h(px(tile.1))
            .rounded(px(10.))
            .overflow_hidden()
            .bg(theme::sidebar())
            .border_color(if p.speaking {
                theme::speaking()
            } else {
                theme::border()
            })
            .when(p.speaking, |d| d.border_2())
            .when(!p.speaking, |d| d.border_1())
            .when((sharing && !own && !watched) || has_picture, |d| {
                d.cursor_pointer()
            })
            .child(content)
            // A click on a share I do not watch starts watching it; on a share enlarges it, and
            // on the enlarged one goes back to everyone.
            .on_click(cx.listener(move |this, _, _, cx| {
                if sharing && !own && !watched {
                    this.watch_share(click_key.clone(), true);
                } else if has_picture {
                    this.enlarged_share = (!enlarged).then(|| click_key.clone());
                }
                cx.notify();
            }))
            .when(can_menu, |d| {
                let pressed = Target::Member(menu_inst.clone(), None, user);
                d.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, _| {
                        this.right_pressed = Some(pressed.clone());
                    }),
                )
                .on_mouse_up(
                    MouseButton::Right,
                    cx.listener(move |this, event: &gpui_kit::MouseUpEvent, window, cx| {
                        if this.right_pressed.is_some() {
                            this.open_volume_menu(
                                menu_inst.clone(),
                                user,
                                event.position,
                                window,
                                cx,
                            );
                        }
                    }),
                )
            })
            .into_any_element()
    }

    // ---- the volume menu ----------------------------------------------------------------------

    pub fn open_volume_menu(
        &mut self,
        inst: InstanceId,
        user: Id,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = {
            let store = self.core.store();
            store.instance(&inst).map_or_else(String::new, |s| {
                let space = s
                    .voice_states
                    .get(&user)
                    .and_then(|v| s.space_of(v.channel_id));
                s.display_name(space, user)
            })
        };
        let volume = self.core.user_volume(&inst, user);
        let share_volume = self
            .watching_shares
            .contains(&(inst.clone(), user))
            .then(|| self.core.share_volume(&inst, user));
        let slider = |value: f32, cx: &mut Context<Self>| {
            cx.new(|_| {
                SliderState::new()
                    .min(0.)
                    .max(2.)
                    .step(0.05)
                    .default_value(value)
            })
        };
        let volume_slider = slider(volume, cx);
        let mut subscriptions = vec![cx.subscribe_in(
            &volume_slider,
            window,
            |this, _, event: &SliderEvent, _, cx| {
                let (SliderEvent::Change(value) | SliderEvent::Release(value)) = event;
                if let Some(menu) = &mut this.volume_menu {
                    menu.volume = value.end();
                    if let Err(err) = this
                        .core
                        .set_user_volume(&menu.inst, menu.user, value.end())
                    {
                        tracing::warn!("volume not saved: {err}");
                    }
                }
                cx.notify();
            },
        )];
        let share_slider = share_volume.map(|value| {
            let state = slider(value, cx);
            subscriptions.push(cx.subscribe_in(
                &state,
                window,
                |this, _, event: &SliderEvent, _, cx| {
                    let (SliderEvent::Change(value) | SliderEvent::Release(value)) = event;
                    if let Some(menu) = &mut this.volume_menu {
                        menu.share_volume = Some(value.end());
                        if let Err(err) =
                            this.core
                                .set_share_volume(&menu.inst, menu.user, value.end())
                        {
                            tracing::warn!("stream volume not saved: {err}");
                        }
                    }
                    cx.notify();
                },
            ));
            state
        });
        self.volume_menu = Some(VolumeMenu {
            muted: self.core.user_muted(&inst, user),
            inst,
            user,
            name,
            at,
            volume,
            share_volume,
            volume_slider,
            share_slider,
            _sliders: subscriptions,
        });
        cx.notify();
    }

    pub fn render_volume_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.volume_menu.as_ref()?;
        let level = |name: &'static str, value: f32| {
            div()
                .flex()
                .text_size(theme::tiny_size())
                .child(div().flex_1().child(name))
                .child(
                    div()
                        .text_color(theme::text_secondary())
                        .child(format!("{:.0} %", value * 100.)),
                )
        };
        Some(
            deferred(
                anchored()
                    .position(menu.at)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id("volume-menu")
                            .occlude()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .w(px(240.))
                            .p(px(10.))
                            .rounded(px(6.))
                            .bg(theme::sidebar())
                            .border_1()
                            .border_color(theme::border())
                            .font_family(theme::font())
                            .text_color(theme::text())
                            .text_size(theme::small_size())
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.volume_menu = None;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(menu.name.clone()),
                            )
                            .child(level("Volume", menu.volume))
                            .child(Slider::new(&menu.volume_slider))
                            .children(menu.share_volume.map(|v| level("Stream volume", v)))
                            .children(menu.share_slider.as_ref().map(Slider::new))
                            .child(
                                Checkbox::new("volume-mute")
                                    .label("Mute for me")
                                    .checked(menu.muted)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        if let Some(menu) = &mut this.volume_menu {
                                            menu.muted = *checked;
                                            this.core
                                                .set_user_muted(&menu.inst, menu.user, *checked);
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }

    // ---- sharing --------------------------------------------------------------------------------

    /// The panel's share button: asks what to share first; a running share stops right away.
    fn share_button(&mut self, at: Point<Pixels>, _window: &mut Window, cx: &mut Context<Self>) {
        if self.core.sharing_screen() {
            self.stop_share(cx);
            return;
        }
        self.share_menu = Some(at);
        self.load_screens(cx);
        cx.notify();
    }

    /// The screens that can be shared, listed in the background.
    pub fn load_screens(&mut self, cx: &mut Context<Self>) {
        self.run(
            cx,
            async {
                tokio::task::spawn_blocking(crate::screen::capture::screens)
                    .await
                    .unwrap_or_default()
            },
            |this, screens, _, _| this.screen_names = screens,
        );
    }

    /// Saves a choice of the share menu, which also stands as the setting.
    fn set_share_setting(&mut self, key: &str, value: &str, cx: &mut Context<Self>) {
        self.persist(key, value, cx);
    }

    /// What my share keeps when it cannot keep up; a running share follows at once.
    pub fn render_preference_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::screen::capture::{PREFERENCES, Preference};
        let current = Preference::from_config(&self.config.voice.share_preference);
        div()
            .flex()
            .gap(px(4.))
            .children(PREFERENCES.into_iter().enumerate().map(|(n, preference)| {
                chip(("preference", n), preference.label(), preference == current).on_click(
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let value = format!("\"{}\"", preference.config_value());
                        this.set_share_setting("voice.share_preference", &value, cx);
                        if let Some(capture) = &this.capture {
                            capture.set_preference(preference);
                        }
                        cx.notify();
                    }),
                )
            }))
            .into_any_element()
    }

    pub fn render_share_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::screen::capture::{FRAME_RATES, RESOLUTIONS};
        let at = self.share_menu?;
        let voice = &self.config.voice;
        let field = |name: &'static str, control: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(theme::tiny_size())
                        .text_color(theme::text_secondary())
                        .child(name),
                )
                .child(control)
        };
        let mut screens = vec![String::new()];
        screens.extend(self.screen_names.iter().skip(1).cloned());
        let screen_chips = screens.into_iter().enumerate().map(|(n, screen)| {
            let label = if screen.is_empty() {
                "Primary screen".to_owned()
            } else {
                screen.clone()
            };
            chip(("share-screen", n), label, screen == voice.share_screen).on_click(cx.listener(
                move |this, _, _, cx| {
                    let value = format!("{screen:?}");
                    this.set_share_setting("voice.share_screen", &value, cx);
                },
            ))
        });
        let resolutions = RESOLUTIONS.into_iter().enumerate().map(|(n, name)| {
            chip(("share-res", n), name, voice.share_resolution == name).on_click(cx.listener(
                move |this, _, _, cx| {
                    let value = format!("{name:?}");
                    this.set_share_setting("voice.share_resolution", &value, cx);
                },
            ))
        });
        let rates = FRAME_RATES.into_iter().enumerate().map(|(n, fps)| {
            chip(
                ("share-fps", n),
                format!("{fps} fps"),
                voice.share_fps == fps,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_share_setting("voice.share_fps", &fps.to_string(), cx);
            }))
        });
        let wrap = || div().flex().flex_wrap().gap(px(4.));
        Some(
            deferred(
                anchored()
                    .position(at)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id("share-menu")
                            .occlude()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .w(px(320.))
                            .p(px(12.))
                            .rounded(px(8.))
                            .bg(theme::sidebar())
                            .border_1()
                            .border_color(theme::border())
                            .font_family(theme::font())
                            .text_color(theme::text())
                            .text_size(theme::small_size())
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.share_menu = None;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Share your screen"),
                            )
                            .child(field(
                                "Screen",
                                wrap().children(screen_chips).into_any_element(),
                            ))
                            .child(field(
                                "Resolution",
                                wrap().children(resolutions).into_any_element(),
                            ))
                            .child(field(
                                "Frame rate",
                                wrap().children(rates).into_any_element(),
                            ))
                            .child(field(
                                "When it cannot keep up, keep",
                                self.render_preference_picker(cx),
                            ))
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(px(8.))
                                    .child(Button::new("share-cancel").label("Cancel").on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.share_menu = None;
                                            cx.notify();
                                        }),
                                    ))
                                    .child(
                                        Button::new("share-start")
                                            .primary()
                                            .label("Start sharing")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.share_menu = None;
                                                this.start_share(cx);
                                            })),
                                    ),
                            ),
                    ),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }

    /// The header's voice button of the open room.
    pub fn voice_header_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (inst, channel) = match &self.page {
            Page::Voice(inst, channel) => (inst.clone(), *channel),
            Page::Chat(inst, channel) => {
                // A DM's call button.
                let kind = self
                    .core
                    .store()
                    .instance(inst)
                    .and_then(|s| s.channel(*channel))
                    .map(|c| c.kind);
                if !matches!(kind, Some(ChannelKind::Dm | ChannelKind::GroupDm)) {
                    return None;
                }
                (inst.clone(), *channel)
            }
            _ => return None,
        };
        let here = self.core.voice_channel() == Some((inst.clone(), channel));
        let joining = self.joining.is_some();
        let label = if here {
            "Leave"
        } else if joining {
            "Joining…"
        } else if matches!(self.page, Page::Chat(..)) {
            "Call"
        } else {
            "Join voice"
        };
        Some(
            Button::new("header-voice")
                .label(label)
                .when(!here, |b| b.primary())
                .when(here, |b| b.danger())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if here {
                        this.leave_voice(cx);
                    } else {
                        this.join_voice(inst.clone(), channel, cx);
                    }
                }))
                .into_any_element(),
        )
    }
}

/// A choice among a few, highlighted when chosen. The caller adds what a click does.
pub fn chip(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    chosen: bool,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id.into())
        .px(px(10.))
        .py(px(5.))
        .rounded(px(4.))
        .cursor_pointer()
        .text_size(theme::small_size())
        .when(chosen, |d| {
            d.bg(theme::accent()).text_color(theme::on_accent())
        })
        .when(!chosen, |d| {
            d.bg(theme::selected()).hover(|d| d.bg(theme::hover()))
        })
        .child(label.into())
}

fn panel_button(
    id: &'static str,
    name: &'static str,
    danger: bool,
    _hint: &'static str,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .h(px(30.))
        .rounded(px(4.))
        .cursor_pointer()
        .when(danger, |d| d.bg(theme::danger()))
        .when(!danger, |d| {
            d.bg(theme::selected()).hover(|d| d.bg(theme::hover()))
        })
        .child(icon(
            name,
            18.,
            if danger {
                theme::on_accent()
            } else {
                theme::text()
            },
        ))
}

fn me_button(id: &'static str, name: &'static str, off: bool) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(30.))
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|d| d.bg(theme::hover()))
        .child(icon(
            name,
            18.,
            if off {
                theme::danger()
            } else {
                theme::text_secondary()
            },
        ))
}

/// A named live graph, as bars: the newest second on the right, scaled to the highest value shown
/// but at least `floor`, so a quiet connection does not look busy.
fn graph(
    name: &'static str,
    now: String,
    values: &[f32],
    floor: f32,
    color: gpui_kit::Rgba,
) -> AnyElement {
    const HEIGHT: f32 = 40.;
    let top = values.iter().copied().fold(floor, f32::max);
    let bars = values.iter().map(|v| {
        div()
            .flex_1()
            .min_w(px(1.))
            .h(px(
                (v / top * HEIGHT).clamp(if *v > 0. { 1. } else { 0. }, HEIGHT)
            ))
            .bg(color)
    });
    let empty = HISTORY.saturating_sub(values.len());
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(
            div()
                .flex()
                .text_size(theme::tiny_size())
                .child(
                    div()
                        .flex_1()
                        .text_color(theme::text_secondary())
                        .child(name),
                )
                .child(now),
        )
        .child(
            div()
                .flex()
                .items_end()
                .h(px(HEIGHT))
                .rounded(px(3.))
                .overflow_hidden()
                .bg(theme::code_background())
                .children((0..empty).map(|_| div().flex_1().min_w(px(1.))))
                .children(bars),
        )
        .into_any_element()
}

/// The size of nothing, before the room was first measured.
pub fn no_area() -> Size<Pixels> {
    size(px(0.), px(0.))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_makes_tiles_as_large_as_it_can() {
        let wide = (1600.0, 900.0);
        // One tile fills the room.
        let (cols, tile) = layout(1, wide, 0.0);
        assert_eq!(cols, 1);
        assert_eq!(tile, (1600.0, 900.0));
        // Two side by side beat two stacked in a wide room...
        assert_eq!(layout(2, wide, 0.0).0, 2);
        // ...and stack in a tall one.
        assert_eq!(layout(2, (600.0, 900.0), 0.0).0, 1);
        // Four make a 2×2 grid; five a 3-column one.
        assert_eq!(layout(4, wide, 12.0).0, 2);
        assert_eq!(layout(5, wide, 12.0).0, 3);
        // Tiles never overflow the room, gaps included.
        for n in 1..=25 {
            let (cols, tile) = layout(n, wide, 12.0);
            let rows = n.div_ceil(cols) as f32;
            assert!(tile.0 * cols as f32 + 12.0 * (cols as f32 - 1.0) <= wide.0 + 0.01);
            assert!(tile.1 * rows + 12.0 * (rows - 1.0) <= wide.1 + 0.01);
            assert!((tile.0 / tile.1 - ASPECT).abs() < 0.01);
        }
        // A room too small for anything gives empty tiles instead of panicking.
        assert_eq!(layout(3, (0.0, 0.0), 12.0).1, (0.0, 0.0));
    }

    fn link(sent: u64, lost: u64, down: u64, up: u64) -> VoiceLink {
        VoiceLink {
            rtt_ms: 30,
            sent_bytes: up,
            received_bytes: down,
            sent_packets: sent,
            lost_packets: lost,
            server: "127.0.0.1:7799".parse().unwrap(),
        }
    }

    #[test]
    fn network_samples_are_per_second_differences() {
        let mut net = NetHistory::default();
        net.push(30, Some(link(100, 0, 1000, 500)));
        assert_eq!(net.samples[0].down_kbit, 0.0, "nothing to compare with yet");
        net.push(31, Some(link(200, 10, 2000, 1000)));
        let sample = net.samples[1];
        assert_eq!(sample.ping_ms, 31);
        assert_eq!(sample.loss_pct, 10.0);
        assert_eq!(sample.down_kbit, 8.0);
        assert_eq!(sample.up_kbit, 4.0);
        for _ in 0..HISTORY * 2 {
            net.push(1, None);
        }
        assert_eq!(net.samples.len(), HISTORY);
    }
}
