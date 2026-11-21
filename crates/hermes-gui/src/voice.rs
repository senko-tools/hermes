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
    AnimationExt as _, AnyElement, Context, Entity, FontWeight, Pixels, SharedString, Size,
    Subscription, Window, canvas, div, img, size,
};
use hermes_client_core::audio::{InputMode, Sound};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::voice::VoiceLink;
use hermes_client_core::{InstanceId, VoiceStatus, view};

use crate::assets::{self, icon};
use crate::menu_entries::Target;
use crate::shell::{Page, Shell};
use crate::theme::{self, px};

/// An incoming call rings for at most this long.
const RING_FOR: Duration = Duration::from_secs(30);
/// One ring sound is about 1.6 s; it is repeated at this interval.
const RING_EVERY: Duration = Duration::from_millis(2000);
/// Tiles keep this aspect ratio (width / height), the shape of most screens.
const ASPECT: f32 = 16.0 / 9.0;
const AVATAR_MAX: f32 = 128.0;
/// The smallest a tile gets: past that the tiles scroll instead of shrinking (#12).
const TILE_MIN: (f32, f32) = (176.0, 99.0);
const TILE_GAP: f32 = 12.0;
/// How many seconds of the connection are kept for the network graphs.
const HISTORY: usize = 900;
/// The time ranges the network graphs offer, in seconds.
pub const NET_RANGES: [usize; 3] = [60, 300, 900];

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
    pub at: crate::modal::PopupFrom,
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
    /// When it was taken, Unix milliseconds.
    pub at_ms: i64,
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
                    at_ms: 0,
                }
            }
            _ => NetSample {
                ping_ms,
                ..NetSample::default()
            },
        };
        self.link = link;
        self.samples.push_back(NetSample {
            at_ms: hermes_client_core::now_ms(),
            ..sample
        });
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
/// [`layout`]'s answer, kept to at least [`TILE_MIN`]: as many columns of that size as fit
/// across `width`, and the rows scroll.
fn min_layout(fit: (usize, (f32, f32)), width: f32, gap: f32) -> (usize, (f32, f32)) {
    let (cols, tile) = fit;
    if tile.0 >= TILE_MIN.0 {
        return (cols, tile);
    }
    let across = ((width + gap) / (TILE_MIN.0 + gap)).floor().max(1.) as usize;
    (across.min(cols.max(1)).max(1), TILE_MIN)
}

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
    /// starts a call. While a join runs, further requests do nothing; while leaving runs, the
    /// join waits for it (it used to be dropped: "joining back did nothing").
    pub fn join_voice(&mut self, inst: InstanceId, channel: Id, cx: &mut Context<Self>) {
        let key = (inst.clone(), channel);
        if self.leaving_voice {
            self.join_after_leave = Some(key);
            return;
        }
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
                        if !this.config.voice.noise_suppression
                            && !this.config.voice.echo_cancellation
                            && !this.headphones_hint_shown
                        {
                            this.headphones_hint_shown = true;
                            this.info(
                                "Noise suppression and echo cancellation are off: headphones are \
                                 recommended.",
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
        // The panel closes down: an empty one of its height shrinks away.
        self.close_later("voice-panel", |_| {}, cx);
        self.leaving_voice = true;
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.leave_voice().await },
            |this, (), _, cx| {
                this.leaving_voice = false;
                this.net.clear();
                this.close_later("network-in", |s| s.network_open = None, cx);
                if let Some((inst, channel)) = this.join_after_leave.take() {
                    this.join_voice(inst, channel, cx);
                }
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
        // Do not disturb means no ringing; the call still shows in the channel.
        if self.core.presence() == hermes_client_core::proto::models::PresenceStatus::Dnd {
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
                // The phone rings: a little shake, then a rest, over and over.
                .child(icon(assets::PHONE, 18., theme::speaking()).with_animation(
                    "call-ring",
                    gpui_kit::Animation::new(Duration::from_millis(1600)).repeat(),
                    |d, t| {
                        let shake = if t < 0.4 {
                            (t / 0.4 * std::f32::consts::TAU * 3.).sin() * (1. - t / 0.4)
                        } else {
                            0.
                        };
                        d.with_transformation(gpui_kit::Transformation::rotate(gpui_kit::radians(
                            0.35 * shake,
                        )))
                    },
                ))
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("{caller} is calling{place}")),
                )
                .child(
                    Button::new("call-join")
                        .cursor_pointer()
                        .primary()
                        .label("Join")
                        .on_click(cx.listener(|this, _, window, cx| this.answer_call(window, cx))),
                )
                .child(
                    Button::new("call-dismiss")
                        .cursor_pointer()
                        .label("Dismiss")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.stop_ringing();
                            cx.notify();
                        })),
                )
                .with_animation(
                    "call-banner-in",
                    gpui_kit::Animation::new(theme::fade() * 2)
                        .with_easing(gpui_kit::ease_out_quint()),
                    |d, t| d.relative().top(px(-8. * (1. - t))).opacity(t),
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
        let state_color = match status {
            VoiceStatus::Connected { .. } => theme::success(),
            VoiceStatus::Disconnected => theme::danger(),
            _ => theme::accent(),
        };
        let (room_inst, _) = (inst.clone(), ());
        let info = div()
            .id("voice-panel-info")
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(4.))
            .rounded(theme::radius_small())
            .cursor_pointer()
            // Text in plain divs ignores gpui's group hover, so the place takes the hover colour
            // from here (the status line has its own colour).
            .text_color(theme::text_secondary())
            .hover(|d| d.text_color(theme::accent()))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select(room_inst.clone(), channel, window, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(if connected {
                        // Connected: the colour flows from the waiting accent to green.
                        icon(assets::WAVEFORM, 14., state_color)
                            .with_spring(
                                "voice-state-icon",
                                gpui_kit::SpringAnimation::new(crate::theme::paced(
                                    crate::hover::HOVER_SPRING,
                                ))
                                .to(true)
                                .from(false),
                                |d, phase| {
                                    d.text_color(theme::mix(
                                        theme::accent(),
                                        theme::success(),
                                        phase.0.clamp(0., 1.),
                                    ))
                                },
                            )
                            .into_any_element()
                    } else {
                        // Still on its way: the icon breathes.
                        icon(assets::WAVEFORM, 14., state_color)
                            .with_animation(
                                "voice-connecting",
                                gpui_kit::Animation::new(Duration::from_millis(1200))
                                    .repeat()
                                    .with_easing(gpui_kit::pulsating_between(0.35, 1.)),
                                move |d, t| d.text_color(theme::alpha(state_color, t)),
                            )
                            .into_any_element()
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::caption_size())
                            .font_weight(FontWeight::MEDIUM)
                            .child(match status {
                                VoiceStatus::Connected { .. } => "Voice connected",
                                VoiceStatus::Connecting => "Voice connecting",
                                VoiceStatus::Reconnecting { .. } => "Voice reconnecting",
                                VoiceStatus::Disconnected => "Voice disconnected",
                            })
                            .with_spring(
                                "voice-state-text",
                                gpui_kit::SpringAnimation::new(crate::theme::paced(
                                    crate::hover::HOVER_SPRING,
                                ))
                                .to(connected),
                                move |d, phase| {
                                    let from = if connected {
                                        theme::accent()
                                    } else {
                                        state_color
                                    };
                                    d.text_color(theme::mix(
                                        from,
                                        state_color,
                                        phase.0.clamp(0., 1.),
                                    ))
                                },
                            ),
                    )
                    .children(ptt.map(|held| {
                        div()
                            .px(px(6.))
                            .rounded(theme::radius_small())
                            .border_1()
                            .border_color(theme::border())
                            .text_size(theme::caption_size())
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
                    .text_size(theme::caption_size())
                    .child(place),
            );
        let encryption = self.render_voice_encryption();
        // The connection line opens the network graph.
        let network = div()
            .id("voice-panel-network")
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(4.))
            .py(px(2.))
            .rounded(theme::radius_small())
            .cursor_pointer()
            .text_size(theme::caption_size())
            .text_color(theme::text_tertiary())
            .hover(|d| d.text_color(theme::accent()))
            .child(div().flex_1().min_w_0().truncate().child(quality))
            .child(self.render_spark())
            .child(self.popup_trigger())
            .on_click(cx.listener(|this, event: &gpui_kit::ClickEvent, _, cx| {
                if this.dismissed_by_this_press(&crate::modal::PopupKey::Network) {
                    // The press closed it: it stays closed.
                } else if this.network_open.is_some() && !crate::modal::closing("network-in") {
                    this.close_later("network-in", |s| s.network_open = None, cx);
                } else {
                    this.cancel_closing("network-in");
                    this.network_open = Some(this.popup_from(event.position()));
                }
                cx.notify();
            }));
        let share = self.share_voice_button("panel-share", cx);
        // The panel is narrow: its Leave keeps its square.
        let leave = self.leave_voice_button("panel-leave", false, cx);
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
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(div().flex_1().min_w_0().child(info))
                        // The share menu hugs both buttons, so it covers neither.
                        .child(
                            self.hug_target(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(share)
                                    .child(leave)
                                    .into_any_element(),
                            ),
                        ),
                )
                .child(encryption)
                .child(network)
                // Its height, for the empty panel that shrinks away after leaving.
                .child({
                    let seen = self.voice_panel_height.clone();
                    canvas(
                        move |bounds, _, _| seen.set(bounds.size.height.as_f32()),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0()
                })
                .relative()
                .overflow_hidden()
                // Joining: the panel opens upwards out of the user panel as it fades in.
                .with_animation(
                    "voice-panel-in",
                    gpui_kit::Animation::new(theme::fade() * 2)
                        .with_easing(gpui_kit::ease_out_quint()),
                    |d, t| d.max_h(px(VOICE_PANEL_MAX * t)).opacity(t),
                )
                .into_any_element(),
        )
    }

    /// Whether what is said here is safe, and who cannot be part of it.
    fn render_voice_encryption(&self) -> AnyElement {
        let state = self.core.voice_encryption();
        let names = |users: &[Id]| {
            let store = self.core.store();
            let inst = self
                .core
                .voice_channel()
                .map(|(i, _)| i)
                .unwrap_or_default();
            users
                .iter()
                .map(|u| {
                    store
                        .instance(&inst)
                        .map_or_else(|| u.to_string(), |s| s.display_name(None, *u))
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let (color, text) = match &state {
            Some(e) if !e.no_keys.is_empty() => (
                theme::warning(),
                format!("Cannot hear you (old version): {}", names(&e.no_keys)),
            ),
            Some(e) if e.ready && e.waiting.is_empty() => {
                (theme::text_tertiary(), "End-to-end encrypted".to_owned())
            }
            _ => (theme::text_tertiary(), "Securing the call…".to_owned()),
        };
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(4.))
            .text_size(theme::caption_size())
            .text_color(color)
            .child(icon(assets::LOCK, 12., color))
            .child(div().flex_1().min_w_0().truncate().child(text))
            .into_any_element()
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
        let count = samples.len();
        // The newest bar grows in each second, so the line reads as live.
        let newest = self.net.samples.back().map_or(0, |s| s.at_ms);
        div()
            .flex()
            .flex_shrink_0()
            .items_end()
            .gap(px(1.))
            .h(px(12.))
            .children(samples.into_iter().enumerate().map(move |(n, ping)| {
                let height = (ping as f32 / top * 12.).clamp(1., 12.);
                let bar = div().w(px(2.)).h(px(height)).bg(spark_color(ping));
                if n + 1 == count {
                    bar.with_animation(
                        SharedString::from(format!("spark-{newest}")),
                        gpui_kit::Animation::new(Duration::from_millis(300))
                            .with_easing(gpui_kit::ease_out_quint()),
                        move |d, t| d.h(px((height * t).max(1.))),
                    )
                    .into_any_element()
                } else {
                    bar.into_any_element()
                }
            }))
            .into_any_element()
    }

    /// The network details over the sidebar: a tile per measure with its value now, the chosen
    /// one's graph, and the raw numbers in one quiet line.
    pub fn render_network(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let at = self.network_open?;
        let link = self.net.link;
        let details = link.map_or_else(
            || "Waiting for the first numbers…".to_owned(),
            |l| {
                let lost = l.lost_packets as f32 * 100.0 / l.sent_packets.max(1) as f32;
                format!(
                    "{} · round trip {} ms · {} packets, {lost:.1} % lost",
                    l.server, l.rtt_ms, l.sent_packets
                )
            },
        );
        let range = self.net_range;
        let shown: Vec<NetSample> = {
            let skip = self.net.samples.len().saturating_sub(range);
            self.net.samples.iter().skip(skip).copied().collect()
        };
        let hovered = self.net_hover.and_then(|at| {
            let ago = ((1.0 - at) * (range - 1) as f32).round() as usize;
            shown.len().checked_sub(ago + 1).map(|i| (i, ago))
        });
        let graphs = net_graphs();
        let chosen = self.net_graph.min(graphs.len() - 1);
        let tiles = graphs
            .iter()
            .enumerate()
            .map(|(n, graph)| {
                let now = shown
                    .last()
                    .map_or_else(|| "—".to_owned(), |s| (graph.unit)((graph.value)(s)));
                let selected = n == chosen;
                div()
                    .id(("net-tile", n))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(theme::row_radius())
                    .cursor_pointer()
                    .border_1()
                    .when(!selected, |d| d.hover(|d| d.border_color(theme::border())))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(div().size(px(6.)).rounded(px(3.)).bg(graph.color))
                            .child(graph.name),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text())
                            .child(now),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.net_graph = n;
                        cx.notify();
                    }))
                    // Choosing a measure: its colour flows into the tile, out of the last one.
                    .with_spring(
                        ("net-tile-chosen", n),
                        gpui_kit::SpringAnimation::new(crate::theme::paced(
                            crate::hover::HOVER_SPRING,
                        ))
                        .to(selected),
                        {
                            let color = graph.color;
                            move |d, phase| {
                                let t = phase.0.clamp(0., 1.);
                                d.bg(theme::mix(theme::raised(), theme::alpha(color, 0.12), t))
                                    .border_color(theme::alpha(color, 0.6 * t))
                            }
                        },
                    )
            })
            .collect::<Vec<_>>();
        let graph = self.render_graph(&graphs[chosen], &shown, hovered, range, cx);
        let ranges = NET_RANGES.into_iter().map(|seconds| {
            crate::ui::chip(
                self,
                ("net-range", seconds),
                format!("{} min", seconds / 60),
                self.net_range == seconds,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.net_range = seconds;
                cx.notify();
            }))
        });
        let code = self.core.voice_encryption().map(|e| {
            format!(
                "Encryption code {} (the same for everyone in the call)",
                e.code
            )
        });
        Some(crate::modal::popup(
            "network-in",
            at,
            420.,
            self.viewport,
            div()
                .id("network")
                .occlude()
                .flex()
                .flex_col()
                .gap(px(12.))
                .w(px(420.))
                .max_h(self.viewport.height - px(16.))
                .overflow_y_scroll()
                .p(px(14.))
                .rounded(theme::radius_large())
                .bg(theme::sidebar())
                .border_1()
                .border_color(theme::border())
                .shadow_lg()
                .font_family(theme::font())
                .text_color(theme::text())
                .text_size(theme::secondary_size())
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if this.network_open.is_some() && !crate::modal::closing("network-in") {
                        this.note_dismissed(crate::modal::PopupKey::Network);
                    }
                    this.close_later("network-in", |s| s.network_open = None, cx);
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(icon(assets::WAVEFORM, 16., theme::success()))
                        .child(
                            div()
                                .flex_1()
                                .ml(px(4.))
                                .font_weight(FontWeight::MEDIUM)
                                .child("Connection"),
                        )
                        .children(ranges)
                        .child(
                            crate::ui::tip_button(
                                self,
                                "network-close",
                                assets::CLOSE,
                                "Close",
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_later("network-in", |s| s.network_open = None, cx);
                                cx.notify();
                            })),
                        ),
                )
                .child(div().flex().gap(px(6.)).children(tiles))
                .child(graph)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .pt(px(8.))
                        .border_t_1()
                        .border_color(theme::border())
                        .text_size(theme::caption_size())
                        .text_color(theme::text_tertiary())
                        .child(details)
                        .children(code),
                ),
        ))
    }

    /// Mute and deafen, as the user panel and the call bar show them (`prefix` keeps their
    /// animations apart).
    fn mute_deafen(&self, prefix: &'static str, cx: &mut Context<Self>) -> [AnyElement; 2] {
        let muted = self.core.self_mute() || self.core.self_deaf();
        let deafened = self.core.self_deaf();
        let (mute_id, deafen_id) = match prefix {
            "bar" => ("bar-mute", "bar-deafen"),
            _ => ("me-mute", "me-deafen"),
        };
        [
            self.voice_button(
                VoiceButton {
                    id: mute_id,
                    glyphs: (assets::MIC, assets::MIC_OFF),
                    look: Look::plain(theme::danger()),
                    on: muted,
                    tips: ("Mute", "Unmute"),
                    hover: OnHover::Deepen,
                },
                cx.listener(|this, _, _, cx| {
                    this.toggle_mute(cx);
                    cx.notify();
                }),
                cx,
            ),
            self.voice_button(
                VoiceButton {
                    id: deafen_id,
                    glyphs: (assets::HEADPHONES, assets::HEADPHONES_OFF),
                    look: Look::plain(theme::danger()),
                    on: deafened,
                    tips: ("Deafen", "Undeafen"),
                    hover: OnHover::Deepen,
                },
                cx.listener(|this, _, _, cx| {
                    this.toggle_deafen(cx);
                    cx.notify();
                }),
                cx,
            ),
        ]
    }

    /// The buttons next to my name: mute, deafen and settings.
    pub fn render_me_buttons(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut buttons: Vec<AnyElement> = self.mute_deafen("me", cx).into();
        buttons.push(self.voice_button(
            VoiceButton::new(
                "me-settings",
                assets::COG,
                Look::plain(theme::accent()),
                "Settings",
            ),
            cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
            cx,
        ));
        buttons
    }

    /// Share, which turns into a red stop button under the pointer while sharing.
    fn share_voice_button(&self, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        self.voice_button(
            VoiceButton {
                id,
                glyphs: (assets::MONITOR_SHARE, assets::MONITOR_SHARE),
                look: Look::plain(theme::accent()),
                on: self.core.sharing_screen(),
                tips: ("Share your screen", "Stop sharing"),
                hover: OnHover::Stop(assets::MONITOR_STOP),
            },
            cx.listener(move |this, _, _, cx| this.share_button(cx)),
            cx,
        )
    }

    /// Leave, which fills red under the pointer; `grow`: it also grows into a pill with its name.
    fn leave_voice_button(
        &self,
        id: &'static str,
        grow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.voice_button(
            VoiceButton {
                hover: OnHover::Fill { grow },
                ..VoiceButton::new(id, assets::HANG_UP, Look::negative(), "Leave")
            },
            cx.listener(|this, _, _, cx| this.leave_voice(cx)),
            cx,
        )
    }

    /// Floating over the room I am in: share on the left, mute and deafen together, leave on the
    /// right.
    fn render_call_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let [mute, deafen] = self.mute_deafen("bar", cx);
        div()
            .absolute()
            .bottom(px(20.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("call-bar")
                    .occlude()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(6.))
                    .rounded(theme::radius_large())
                    .bg(theme::raised())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .child(self.hug_target(self.share_voice_button("bar-share", cx)))
                    .child(
                        div()
                            .flex()
                            .rounded(theme::row_radius())
                            .bg(theme::selected())
                            .child(mute)
                            .child(deafen),
                    )
                    .child(self.leave_voice_button("bar-leave", true, cx)),
            )
            // It floats up into place when the room opens.
            .with_animation(
                "call-bar-in",
                gpui_kit::Animation::new(theme::fade() * 2).with_easing(gpui_kit::ease_out_quint()),
                |d, t| d.bottom(px(12. + 8. * t)).opacity(t),
            )
            .into_any_element()
    }

    /// Someone in a voice room, under its channel in the sidebar. `in_block`: inside a highlighted
    /// channel's block, whose bar on the left the row's hover must leave alone.
    #[allow(clippy::too_many_arguments)]
    pub fn render_voice_user(
        &self,
        inst: InstanceId,
        channel: Id,
        user: Id,
        name: String,
        muted: bool,
        deafened: bool,
        speaking: bool,
        nested: bool,
        _dm: bool,
        block: Option<crate::sidebar::RowState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let in_block = block.is_some();
        let key = format!("voice-user-{channel}-{user}");
        let (avatar, space) = {
            let store = self.core.store();
            let s = store.instance(&inst);
            let found = s.and_then(|s| s.user(user));
            (
                self.avatar(&inst, found, &name, 22.),
                s.and_then(|s| s.space_of(channel)),
            )
        };
        let target = Target::Member(inst.clone(), space, user);
        let profile = target.clone();
        let element = div()
            .id(SharedString::from(format!("voice-user-{user}")))
            .flex()
            .flex_shrink_0()
            .items_center()
            .relative()
            .gap(px(8.))
            .h(px(30.))
            .pl(px(if nested { 46. } else { 36. }))
            .pr(px(8.))
            // The pill starts after the bar and the name stays where it was.
            .when(in_block, |d| {
                d.ml(theme::indicator_width() + px(2.))
                    .mr(px(2.))
                    .pl(px(if nested { 44. } else { 34. }) - theme::indicator_width())
                    .pr(px(6.))
            })
            .rounded(theme::row_radius())
            .cursor_pointer()
            .text_size(theme::secondary_size())
            .text_color(if speaking {
                theme::text()
            } else {
                theme::text_secondary()
            })
            // In a highlighted block: a pill of the block's hue, and the name brightens.
            .map(|d| match block {
                Some(block) => d
                    .hover(|d| d.text_color(theme::text()))
                    .on_hover(self.track_hover(key.clone(), cx))
                    .child(self.block_hover_pill(&key, block)),
                None => d.hover(|d| d.text_color(theme::accent()).bg(theme::hover())),
            })
            .on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_profile(&profile, event.position(), cx);
                }),
            )
            .child(
                // The ring around the picture, in its shape (22 px and a 2 px border).
                div()
                    .flex_shrink_0()
                    .rounded(px(theme::avatar_radius(26.)))
                    .border_2()
                    .child(avatar)
                    .with_spring(
                        SharedString::from(format!("voice-ring-{channel}-{user}")),
                        gpui_kit::SpringAnimation::new(ring_spring(speaking)).to(speaking),
                        |d, phase| {
                            d.border_color(theme::alpha(theme::speaking(), phase.0.clamp(0., 1.)))
                        },
                    ),
            )
            .child(div().flex_1().min_w_0().truncate().child(name))
            .when(muted && !deafened, |d| {
                d.child(icon(assets::MIC_OFF, 14., theme::muted_voice()))
            })
            .when(deafened, |d| {
                d.child(icon(assets::HEADPHONES_OFF, 14., theme::muted_voice()))
            });
        self.with_menu(element, target, cx).into_any_element()
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
            min_layout(layout(shown.len(), area, TILE_GAP), area.0, TILE_GAP)
        };
        let overflows = enlarged.is_none()
            && shown.len().div_ceil(cols.max(1)) as f32 * (tile_size.1 + TILE_GAP) > area.1 + 1.;
        // A popped-out share's window says how large it shows instead.
        if let Some(latest) = self
            .screen_latest
            .as_ref()
            .filter(|_| self.popped_out.is_none())
        {
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
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .gap(px(12.))
            .p(px(16.))
            .when(in_call, |d| d.pb(px(84.)))
            .child(
                div()
                    .text_size(theme::caption_size())
                    .text_color(theme::text_tertiary())
                    .child(summary),
            )
            .child(
                div()
                    .id("voice-tiles")
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .items_center()
                    // Too many to fit at the smallest size: they scroll from the top.
                    .map(|d| {
                        if overflows {
                            d.justify_start()
                        } else {
                            d.justify_center()
                        }
                    })
                    .overflow_y_scroll()
                    .gap(px(TILE_GAP))
                    .child(measure)
                    .children(lines),
            )
            .when(in_call, |d| d.child(self.render_call_bar(cx)))
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
        let radius = theme::radius_large();
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
                let tile_hovered = self.is_hovered(&format!("tile-{}", p.user));
                div()
                    .relative()
                    .size_full()
                    .bg(gpui_kit::rgb(0))
                    // The first picture fades in over the black.
                    .child(
                        div()
                            .size_full()
                            .child(
                                img(frame.image.clone())
                                    .size_full()
                                    .object_fit(gpui_kit::ObjectFit::Contain),
                            )
                            .with_animation(
                                SharedString::from(format!("share-in-{}", p.user)),
                                gpui_kit::Animation::new(theme::fade() * 2)
                                    .with_easing(gpui_kit::ease_out_quint()),
                                |d, t| d.opacity(t),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom(px(8.))
                            .left(px(8.))
                            .px(px(8.))
                            .py(px(4.))
                            .rounded(theme::radius_small())
                            .bg(theme::scrim())
                            .child(name_row)
                            // Like the stop button, only while the pointer is over the share.
                            .with_spring(
                                SharedString::from(format!("share-name-show-{}", p.user)),
                                gpui_kit::SpringAnimation::new(crate::theme::paced(
                                    crate::hover::HOVER_SPRING,
                                ))
                                .to(tile_hovered),
                                |d, phase| d.opacity(phase.0.clamp(0., 1.)),
                            ),
                    )
                    // Stop watching shows while the pointer is over the share, so the screen
                    // stays clean.
                    .child(
                        div()
                            .absolute()
                            .top(px(8.))
                            .right(px(8.))
                            .flex()
                            .gap(px(4.))
                            .child({
                                let key = share_key.clone();
                                let title = format!("{}'s screen", p.name);
                                crate::ui::tip_button(
                                    self,
                                    SharedString::from(format!("pop-out-{}", p.user)),
                                    assets::media_icons::FULLSCREEN,
                                    "Open in its own window",
                                    cx,
                                )
                                .bg(theme::scrim())
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.pop_out_share(key.clone(), title.clone(), cx);
                                    },
                                ))
                            })
                            .child(
                                crate::ui::tip_button(
                                    self,
                                    SharedString::from(format!("stop-watching-{}", p.user)),
                                    assets::CLOSE,
                                    "Stop watching",
                                    cx,
                                )
                                .bg(theme::scrim())
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.watch_share(stop.clone(), false);
                                        cx.notify();
                                    },
                                )),
                            )
                            .with_spring(
                                SharedString::from(format!("stop-watching-show-{}", p.user)),
                                gpui_kit::SpringAnimation::new(crate::theme::paced(
                                    crate::hover::HOVER_SPRING,
                                ))
                                .to(tile_hovered),
                                |d, phase| d.opacity(phase.0.clamp(0., 1.)),
                            ),
                    )
                    .into_any_element()
            }
            None => {
                // Half the tile's height, never too small to see (#12).
                let side = (tile.1 * 0.5).clamp(40., AVATAR_MAX);
                let short = tile.1 < 130.;
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
                        .map(|v| format!("volume {}", hermes_client_core::audio::volume_label(v)))
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
                    .children(note.filter(|_| !short).map(|note| {
                        let note = div()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(note);
                        // Waiting for their first picture: the line breathes.
                        if sharing && watched && !own {
                            note.with_animation(
                                SharedString::from(format!("share-loading-{}", p.user)),
                                gpui_kit::Animation::new(Duration::from_millis(1400))
                                    .repeat()
                                    .with_easing(gpui_kit::pulsating_between(0.45, 1.)),
                                |d, t| d.opacity(t),
                            )
                            .into_any_element()
                        } else {
                            note.into_any_element()
                        }
                    }))
                    .children((sharing && own).then(|| self.render_preference_picker(cx)))
                    .into_any_element()
            }
        };
        let space = self.core.store().instance(inst).and_then(|s| {
            let channel = s.voice_states.get(&p.user)?.channel_id;
            s.space_of(channel)
        });
        let target = Target::Member(inst.clone(), space, p.user);
        let click_key = share_key.clone();
        let has_picture = picture.is_some();
        let user = p.user;
        // The ring thickens and turns green on a spring while they speak, over the content, so
        // nothing inside moves.
        let ring = div().absolute().inset_0().rounded(radius).with_spring(
            SharedString::from(format!("tile-ring-{user}")),
            gpui_kit::SpringAnimation::new(ring_spring(p.speaking)).to(p.speaking),
            |d, phase| {
                let t = phase.0.clamp(0., 1.);
                theme::border_width(d, px(1. + t)).border_color(theme::mix(
                    theme::border(),
                    theme::speaking(),
                    t,
                ))
            },
        );
        let (w, h) = tile;
        let tile = div()
            .id(SharedString::from(format!("tile-{user}")))
            .relative()
            .size_full()
            .rounded(radius)
            .overflow_hidden()
            .bg(theme::sidebar())
            .when((sharing && !own && !watched) || has_picture, |d| {
                d.cursor_pointer()
            })
            .on_hover(self.track_hover(format!("tile-{user}"), cx))
            // Someone who joins (or a share that is enlarged or put back) fades in.
            .child(div().size_full().child(content).with_animation(
                SharedString::from(format!("tile-in-{user}-{enlarged}")),
                gpui_kit::Animation::new(theme::fade() * 2).with_easing(gpui_kit::ease_out_quint()),
                |d, t| d.opacity(t),
            ))
            .child(ring)
            // A click on a share I do not watch starts watching it; on a share enlarges it, and
            // on the enlarged one goes back to everyone.
            .on_click(cx.listener(move |this, _, _, cx| {
                if sharing && !own && !watched {
                    this.watch_share(click_key.clone(), true);
                } else if has_picture {
                    this.enlarged_share = (!enlarged).then(|| click_key.clone());
                }
                cx.notify();
            }));
        // The tile's place stays put while the tile grows into it from a little smaller.
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .w(px(w))
            .h(px(h))
            .child(
                div()
                    .relative()
                    .child(self.with_menu(tile, target, cx))
                    .with_animation(
                        SharedString::from(format!("tile-grow-{user}-{enlarged}")),
                        gpui_kit::Animation::new(theme::fade() * 2)
                            .with_easing(gpui_kit::ease_out_quint()),
                        move |d, t| {
                            let scale = 0.96 + 0.04 * t;
                            d.w(px(w * scale)).h(px(h * scale))
                        },
                    ),
            )
            .into_any_element()
    }

    // ---- the volume menu ----------------------------------------------------------------------

    pub fn open_volume_menu(
        &mut self,
        inst: InstanceId,
        user: Id,
        at: crate::modal::PopupFrom,
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
                    .max(hermes_client_core::audio::MAX_VOLUME)
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
        self.cancel_closing("volume-menu-in");
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
                .text_size(theme::caption_size())
                .child(div().flex_1().child(name))
                .child(
                    div()
                        .text_color(theme::text_secondary())
                        .child(hermes_client_core::audio::volume_label(value)),
                )
        };
        Some(crate::modal::popup(
            "volume-menu-in",
            menu.at,
            240.,
            self.viewport,
            div()
                .id("volume-menu")
                .occlude()
                .flex()
                .flex_col()
                .gap(px(8.))
                .w(px(240.))
                .p(px(10.))
                .rounded(theme::row_radius())
                .bg(theme::sidebar())
                .border_1()
                .border_color(theme::border())
                .font_family(theme::font())
                .text_color(theme::text())
                .text_size(theme::secondary_size())
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.close_later("volume-menu-in", |s| s.volume_menu = None, cx);
                    cx.notify();
                }))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(menu.name.clone()),
                )
                .child(level("Volume", menu.volume))
                .child(Slider::new(&menu.volume_slider).cursor_pointer())
                .children(menu.share_volume.map(|v| level("Stream volume", v)))
                .children(
                    menu.share_slider
                        .as_ref()
                        .map(|s| Slider::new(s).cursor_pointer()),
                )
                .child(
                    Checkbox::new("volume-mute")
                        .cursor_pointer()
                        .label("Mute for me")
                        .checked(menu.muted)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            if let Some(menu) = &mut this.volume_menu {
                                menu.muted = *checked;
                                this.core.set_user_muted(&menu.inst, menu.user, *checked);
                            }
                            cx.notify();
                        })),
                ),
        ))
    }

    // ---- sharing --------------------------------------------------------------------------------

    /// The panel's share button: asks what to share first; a running share stops right away.
    fn share_button(&mut self, cx: &mut Context<Self>) {
        if self.core.sharing_screen() {
            self.stop_share(cx);
            return;
        }
        self.open_share_picker(cx);
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
            |this, screens, _, cx| {
                this.screen_list = screens;
                cx.notify();
            },
        );
    }

    /// What my share keeps when it cannot keep up; a running share follows at once.
    pub fn render_preference_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::screen::capture::{PREFERENCES, Preference};
        let current = Preference::from_config(self.config.voice.share_preference);
        div()
            .flex()
            .gap(px(4.))
            .children(PREFERENCES.into_iter().enumerate().map(|(n, preference)| {
                crate::ui::chip(
                    self,
                    ("preference", n),
                    preference.label(),
                    preference == current,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let value = preference.config_value();
                    this.set_config(|c| c.voice.share_preference = value, cx);
                    if let Some(capture) = &this.capture {
                        capture.set_preference(preference);
                    }
                    cx.notify();
                }))
            }))
            .into_any_element()
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
        // In the call: the voice panel's own button leaves it.
        if self.core.voice_channel() == Some((inst.clone(), channel)) {
            return None;
        }
        let joining = self.joining.is_some();
        let label = if joining {
            "Joining…"
        } else if matches!(self.page, Page::Chat(..)) {
            "Call"
        } else {
            "Join voice"
        };
        Some(
            Button::new("header-voice")
                .cursor_pointer()
                .label(label)
                .primary()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.join_voice(inst.clone(), channel, cx);
                }))
                .into_any_element(),
        )
    }
}

/// The colours of a voice button: its icon at rest and under the pointer, and while it is on.
#[derive(Clone, Copy)]
pub struct Look {
    rest: gpui_kit::Rgba,
    hover: gpui_kit::Rgba,
    on: gpui_kit::Rgba,
    /// A soft background of `on` under the pointer too.
    hover_bg: bool,
}

impl Look {
    /// Leaving a call: the red of a muted microphone, with a soft red behind it on hover.
    pub fn negative() -> Look {
        Look {
            rest: theme::danger(),
            hover: theme::danger(),
            on: theme::danger(),
            hover_bg: true,
        }
    }

    /// Grey, the accent on hover, `on` while it is on.
    pub fn plain(on: gpui_kit::Rgba) -> Look {
        Look {
            rest: theme::text_secondary(),
            hover: theme::accent(),
            on,
            hover_bg: false,
        }
    }
}

/// A little bouncy: a toggle overshoots once and settles in about a quarter second.
const TOGGLE_SPRING: gpui_kit::SpringConfig = gpui_kit::SpringConfig::new(420., 24., 1.);

/// Taller than the voice panel ever is: it opens towards this.
const VOICE_PANEL_MAX: f32 = 200.;

/// How much wider Leave grows under the pointer, for its name.
const LEAVE_LABEL: f32 = 48.;

/// A speaking ring lights up almost at once (about 85% in 50 ms) and fades a little slower
/// (about 120 ms). Both are critically damped (damping 2 * sqrt(stiffness)), so they never
/// overshoot, and not paced by the motion speed: the ring is live status, and a spring starting
/// from rest already eases in, which read as a delay. Speech detection holds 250 ms between
/// words, so it does not flicker.
fn ring_spring(speaking: bool) -> gpui_kit::SpringConfig {
    if speaking {
        gpui_kit::SpringConfig::new(4900., 140., 1.)
    } else {
        gpui_kit::SpringConfig::new(1225., 70., 1.)
    }
}

/// What the pointer does to a voice button, on top of its `Look`'s hover colour.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OnHover {
    /// Only the hover colour.
    Plain,
    /// While on, it becomes what a click does: this glyph in red ("stop" while sharing).
    Stop(&'static str),
    /// While on, it keeps its on look and the pointer only deepens its background (mute and
    /// deafen stay red and crossed out; the tooltip says what a click does). Previewing the off
    /// look read as a change that had not happened.
    Deepen,
    /// It fills with its colour and the icon tips over (leave). `grow`: it also widens into a
    /// pill with its name (the call bar); without, it keeps its square and has a tooltip.
    Fill { grow: bool },
}

/// An icon button of the voice and user panels.
pub struct VoiceButton {
    pub id: &'static str,
    /// The glyph while off, and while on; they crossfade.
    pub glyphs: (&'static str, &'static str),
    pub look: Look,
    pub on: bool,
    /// The tooltip while off, and while on.
    pub tips: (&'static str, &'static str),
    pub hover: OnHover,
}

impl VoiceButton {
    /// A button with one glyph and one tooltip.
    pub fn new(id: &'static str, glyph: &'static str, look: Look, tip: &'static str) -> Self {
        VoiceButton {
            id,
            glyphs: (glyph, glyph),
            look,
            on: false,
            tips: (tip, tip),
            hover: OnHover::Plain,
        }
    }
}

impl Shell {
    /// Draws a voice button. Turning on fades in a soft background of the `on` colour while the
    /// icon pops and its glyph crossfades; the pointer changes it on a spring of its own.
    pub fn voice_button(
        &self,
        button: VoiceButton,
        click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gpui_kit::{AnimationExt as _, SpringAnimation};
        let VoiceButton {
            id,
            glyphs,
            look,
            on,
            tips,
            hover,
        } = button;
        let hovered = self.is_hovered(id);
        let tip = SharedString::from(if on { tips.1 } else { tips.0 });
        // Leave grows into a pill with its name, so its width follows the inside.
        let grows = hover == OnHover::Fill { grow: true };
        div()
            .id(id)
            .flex_shrink_0()
            .h(px(32.))
            .when(!grows, |d| d.w(px(32.)))
            .cursor_pointer()
            .on_hover(self.track_hover(id, cx))
            .on_click(click)
            // Leave names itself as it grows; the others say what they do.
            .when(!grows, |d| {
                d.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
            })
            .with_spring(
                id,
                SpringAnimation::new(crate::theme::paced(TOGGLE_SPRING)).to(on),
                move |d, phase| {
                    let t = phase.0.clamp(0., 1.);
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .h_full()
                            .when(!grows, |d| d.w_full())
                            .rounded(theme::row_radius())
                            .with_spring(
                                SharedString::from(format!("{id}-hover")),
                                SpringAnimation::new(crate::theme::paced(
                                    crate::hover::HOVER_SPRING,
                                ))
                                .to(hovered),
                                move |d, phase| {
                                    let h = phase.0.clamp(0., 1.);
                                    paint_voice_button(d, glyphs, look, hover, t, h)
                                },
                            ),
                    )
                },
            )
            .into_any_element()
    }
}

/// A voice button at `t` of the way on and `h` of the way hovered.
fn paint_voice_button(
    d: gpui_kit::Div,
    (off_glyph, on_glyph): (&'static str, &'static str),
    look: Look,
    hover: OnHover,
    t: f32,
    h: f32,
) -> gpui_kit::Div {
    use gpui_kit::{Transformation, radians};
    // Biggest halfway through the change, either way.
    let pop = 1. + 0.22 * (std::f32::consts::PI * t).sin();
    let base = theme::mix(look.rest, look.on, t);
    let hovered_color = theme::mix(look.hover, look.on, t);
    // How far each glyph shows, the colour, the background and the tilt.
    let mut layers: Vec<(&'static str, f32)> = vec![(off_glyph, 1. - t), (on_glyph, t)];
    let mut color = theme::mix(base, hovered_color, h);
    let soft = |c: gpui_kit::Rgba, k: f32| theme::alpha(c, 0.16 * k);
    let mut bg = soft(look.on, if look.hover_bg { t.max(h) } else { t });
    let mut tilt = 0.;
    match hover {
        OnHover::Plain => {}
        OnHover::Stop(stop) => {
            let s = h * t;
            for layer in &mut layers {
                layer.1 *= 1. - s;
            }
            layers.push((stop, s));
            color = theme::mix(color, theme::danger(), s);
            bg = theme::mix(bg, soft(theme::danger(), 1.), s);
        }
        OnHover::Deepen => {
            bg = theme::alpha(look.on, 0.16 * t + 0.12 * h * t);
        }
        OnHover::Fill { grow } => {
            color = theme::mix(color, theme::on_accent(), h);
            bg = theme::alpha(look.on, 0.16 * t + h * (1. - 0.16 * t));
            tilt = -0.4 * h;
            if !grow {
                // Drawn like the other buttons below, in its square.
                let d = d.relative().bg(bg);
                return d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(off_glyph, 18., color).with_transformation(
                            Transformation::scale(size(pop, pop)).with_rotation(radians(tilt)),
                        )),
                );
            }
            // The handset keeps its square; the name slides out of it to the right.
            let glyph = icon(off_glyph, 18., color).with_transformation(
                Transformation::scale(size(pop, pop)).with_rotation(radians(tilt)),
            );
            return d
                .relative()
                .overflow_hidden()
                .justify_start()
                .w(px(32. + LEAVE_LABEL * h))
                .bg(bg)
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .size(px(32.))
                        .child(glyph),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .pr(px(10.))
                        .text_size(theme::caption_size())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::alpha(theme::on_accent(), h))
                        .child("Leave"),
                );
        }
    }
    // One layer per glyph, so a glyph that stays does not dim while its weights trade.
    let mut merged: Vec<(&'static str, f32)> = Vec::new();
    for (glyph, weight) in layers {
        match merged.iter_mut().find(|(g, _)| *g == glyph) {
            Some(layer) => layer.1 += weight,
            None => merged.push((glyph, weight)),
        }
    }
    let glyphs = merged
        .into_iter()
        .filter(|(_, weight)| *weight > 0.01)
        .map(|(glyph, weight)| {
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    icon(glyph, 18., theme::alpha(color, weight.min(1.))).with_transformation(
                        Transformation::scale(size(pop, pop)).with_rotation(radians(tilt)),
                    ),
                )
        });
    d.relative().bg(bg).children(glyphs)
}

/// One of the network graphs: what it shows and how.
struct Graph {
    name: &'static str,
    unit: fn(f32) -> String,
    /// The value scale reaches at least this, so a quiet connection does not look busy.
    floor: f32,
    color: gpui_kit::Rgba,
    value: fn(&NetSample) -> f32,
}

/// The measures of the connection popup, in the order of its tiles.
fn net_graphs() -> Vec<Graph> {
    vec![
        Graph {
            name: "Latency",
            unit: |v| format!("{v:.0} ms"),
            floor: 40.,
            color: theme::speaking(),
            value: |s| s.ping_ms as f32,
        },
        Graph {
            name: "Loss",
            unit: |v| format!("{v:.1} %"),
            floor: 2.,
            color: theme::danger(),
            value: |s| s.loss_pct,
        },
        Graph {
            name: "Download",
            unit: bitrate,
            floor: 100.,
            color: theme::link(),
            value: |s| s.down_kbit,
        },
        Graph {
            name: "Upload",
            unit: bitrate,
            floor: 100.,
            color: theme::accent(),
            value: |s| s.up_kbit,
        },
    ]
}

/// A sparkline bar's colour by its round trip: green while it is quick, amber when slow, red
/// when it would be heard.
fn spark_color(ping_ms: u32) -> gpui_kit::Rgba {
    match ping_ms {
        0..=119 => theme::speaking(),
        120..=249 => theme::warning(),
        _ => theme::danger(),
    }
}

fn bitrate(kbit: f32) -> String {
    if kbit >= 1000. {
        format!("{:.1} Mbit/s", kbit / 1000.)
    } else {
        format!("{kbit:.0} kbit/s")
    }
}

/// Minimum, average and maximum, with their unit once if they share it.
fn stats_line(low: String, average: String, high: String) -> String {
    let number = |text: &str| {
        text.rsplit_once(' ')
            .map(|(n, u)| (n.to_owned(), u.to_owned()))
    };
    match (number(&low), number(&average), number(&high)) {
        (Some((l, a_unit)), Some((a, b_unit)), Some((_, c_unit)))
            if a_unit == b_unit && b_unit == c_unit =>
        {
            format!("min {l} · avg {a} · max {high}")
        }
        _ => format!("min {low} · avg {average} · max {high}"),
    }
}

/// `seconds` as a time before now: `-1 min`, `-2 min 30 s`, `-30 s`.
fn time_ago(seconds: usize) -> String {
    match (seconds / 60, seconds % 60) {
        (0, s) => format!("-{s} s"),
        (m, 0) => format!("-{m} min"),
        (m, s) => format!("-{m} min {s} s"),
    }
}

/// The value scale's top: the next of 1, 2 or 5 times a power of ten at or above `value`.
fn nice_top(value: f32) -> f32 {
    let value = value.max(f32::MIN_POSITIVE);
    let power = 10_f32.powf(value.log10().floor());
    [1., 2., 5., 10.]
        .into_iter()
        .map(|step| step * power)
        .find(|top| *top >= value * 0.999)
        .unwrap_or(value)
}

/// At most `points` points of a graph, as `(sample index, value)`. A long range has more samples than
/// pixels, so each point takes the highest value it covers: a spike must not disappear.
fn thin(values: &[f32], points: usize) -> Vec<(f32, f32)> {
    if values.is_empty() {
        return Vec::new();
    }
    let points = points.clamp(1, values.len());
    (0..points)
        .map(|n| {
            let from = n * values.len() / points;
            let to = ((n + 1) * values.len() / points).max(from + 1);
            let top = values[from..to].iter().copied().fold(f32::MIN, f32::max);
            ((from + to - 1) as f32 / 2., top)
        })
        .collect()
}

impl Shell {
    /// A graph of the shown samples, the newest on the right: name, minimum, average and maximum, the
    /// latest value or the hovered one with its time, a value scale, and a time axis.
    fn render_graph(
        &self,
        graph: &Graph,
        samples: &[NetSample],
        hovered: Option<(usize, usize)>,
        range: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const HEIGHT: f32 = 80.;
        let values: Vec<f32> = samples.iter().map(graph.value).collect();
        let empty = values.is_empty();
        let unit = graph.unit;
        let top = nice_top(values.iter().copied().fold(graph.floor, f32::max));
        let stats = if values.is_empty() {
            "no data yet".to_owned()
        } else {
            let low = values.iter().copied().fold(f32::MAX, f32::min);
            let high = values.iter().copied().fold(f32::MIN, f32::max);
            let average = values.iter().sum::<f32>() / values.len() as f32;
            stats_line(unit(low), unit(average), unit(high))
        };
        let readout = match hovered {
            Some((index, ago)) => format!(
                "{} ({}) · {}",
                hermes_client_core::time::format(samples[index].at_ms, "%H:%M:%S", self.utc_offset),
                if ago == 0 {
                    "now".to_owned()
                } else {
                    time_ago(ago)
                },
                unit(values[index]),
            ),
            None => values.last().map_or_else(|| "—".to_owned(), |v| unit(*v)),
        };
        let bounds = std::rc::Rc::new(std::cell::Cell::new(gpui_kit::Bounds::default()));
        let seen = bounds.clone();
        let color = graph.color;
        let hover = self.net_hover;
        let chart = canvas(
            move |area, _, _| seen.set(area),
            move |area, (), window, _| {
                draw_graph(area, &values, top, range, color, hover, window);
            },
        )
        .absolute()
        .size_full();
        let scale = |text: String| {
            div()
                .absolute()
                .left(px(4.))
                .text_size(theme::caption_size())
                .text_color(theme::text_tertiary())
                .child(text)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(8.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(graph.name))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .when(hovered.is_none(), |d| d.child(stats)),
                    )
                    .child(div().flex_shrink_0().child(readout)),
            )
            .child(
                div()
                    .id(graph.name)
                    .relative()
                    .h(px(HEIGHT))
                    .overflow_hidden()
                    .rounded(theme::radius_small())
                    .bg(theme::code_background())
                    .child(chart)
                    .child(scale(unit(top)).top(px(1.)))
                    .child(scale(unit(top / 2.)).top(px(HEIGHT / 2. + 1.)))
                    // Nothing measured yet: say so where the line will be.
                    .when(empty, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(theme::caption_size())
                                .text_color(theme::text_tertiary())
                                .child("Measuring… the first numbers come in a second")
                                .with_animation(
                                    "graph-waiting",
                                    gpui_kit::Animation::new(Duration::from_millis(1400))
                                        .repeat()
                                        .with_easing(gpui_kit::pulsating_between(0.45, 1.)),
                                    |d, t| d.opacity(t),
                                ),
                        )
                    })
                    .on_mouse_move(cx.listener(
                        move |this, event: &gpui_kit::MouseMoveEvent, _, cx| {
                            let area: gpui_kit::Bounds<Pixels> = bounds.get();
                            if area.size.width > Pixels::ZERO {
                                let along = (event.position.x - area.left()) / area.size.width;
                                this.net_hover = Some(along.clamp(0., 1.));
                                cx.notify();
                            }
                        },
                    ))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if !*hovered {
                            this.net_hover = None;
                            cx.notify();
                        }
                    })),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_size(theme::caption_size())
                    .text_color(theme::text_tertiary())
                    .child(time_ago(range))
                    .child(time_ago(range / 2))
                    .child("now"),
            )
            .into_any_element()
    }
}

/// Draws a graph's grid, its filled line, and the line under the pointer.
fn draw_graph(
    area: gpui_kit::Bounds<Pixels>,
    values: &[f32],
    top: f32,
    range: usize,
    color: gpui_kit::Rgba,
    hover: Option<f32>,
    window: &mut Window,
) {
    let grid = theme::alpha(theme::text_tertiary(), 0.25);
    let bottom = area.bottom();
    let middle = area.top() + area.size.height / 2.;
    for y in [area.top(), middle] {
        window.paint_quad(gpui_kit::fill(
            gpui_kit::Bounds::new(
                gpui_kit::point(area.left(), y),
                gpui_kit::size(area.size.width, gpui_kit::px(1.)),
            ),
            grid,
        ));
    }
    // The newest sample sits on the right edge, one second per step, so a graph that has only just
    // started fills up from the right.
    let step = area.size.width / (range.max(2) - 1) as f32;
    let newest = values.len().saturating_sub(1) as f32;
    let at = |index: f32, value: f32| {
        gpui_kit::point(
            area.right() - step * (newest - index),
            bottom - area.size.height * (value / top).clamp(0., 1.),
        )
    };
    // No more points than two per pixel of width at this scale.
    let points = thin(values, (f32::from(area.size.width) / 2.) as usize);
    if let (Some(first), Some(last)) = (points.first(), points.last()) {
        let mut stroke = gpui_kit::PathBuilder::stroke(gpui_kit::px(1.5));
        let mut area_fill = gpui_kit::PathBuilder::fill();
        area_fill.move_to(gpui_kit::point(at(first.0, first.1).x, bottom));
        stroke.move_to(at(first.0, first.1));
        for (index, value) in &points {
            let point = at(*index, *value);
            stroke.line_to(point);
            area_fill.line_to(point);
        }
        area_fill.line_to(gpui_kit::point(at(last.0, last.1).x, bottom));
        area_fill.close();
        if points.len() > 1 {
            if let Ok(path) = area_fill.build() {
                window.paint_path(path, theme::alpha(color, 0.2));
            }
            if let Ok(path) = stroke.build() {
                window.paint_path(path, color);
            }
        }
    }
    if let Some(along) = hover {
        window.paint_quad(gpui_kit::fill(
            gpui_kit::Bounds::new(
                gpui_kit::point(area.left() + area.size.width * along, area.top()),
                gpui_kit::size(gpui_kit::px(1.), area.size.height),
            ),
            theme::text_secondary(),
        ));
    }
}

/// The size of nothing, before the room was first measured.
pub fn no_area() -> Size<Pixels> {
    size(px(0.), px(0.))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_scales_are_round_numbers() {
        assert_eq!(nice_top(40.), 50.);
        assert_eq!(nice_top(50.), 50.);
        assert_eq!(nice_top(61.), 100.);
        assert_eq!(nice_top(2.), 2.);
        assert_eq!(nice_top(1300.), 2000.);
        assert!(nice_top(0.).is_finite());
    }

    #[test]
    fn thinning_keeps_spikes() {
        let mut values = vec![1.0; 900];
        values[451] = 80.0;
        let points = thin(&values, 200);
        assert_eq!(points.len(), 200);
        assert!(points.iter().any(|(_, v)| *v == 80.0), "the spike survives");
        assert_eq!(thin(&[3.0, 4.0], 200), [(0.0, 3.0), (1.0, 4.0)]);
        assert!(thin(&[], 200).is_empty());
    }

    #[test]
    fn stats_say_a_shared_unit_once() {
        let line = |a: &str, b: &str, c: &str| stats_line(a.into(), b.into(), c.into());
        assert_eq!(
            line("1.2 Mbit/s", "2.0 Mbit/s", "3.1 Mbit/s"),
            "min 1.2 · avg 2.0 · max 3.1 Mbit/s"
        );
        assert_eq!(
            line("900 kbit/s", "1.0 Mbit/s", "1.5 Mbit/s"),
            "min 900 kbit/s · avg 1.0 Mbit/s · max 1.5 Mbit/s"
        );
    }

    #[test]
    fn times_read_well() {
        assert_eq!(time_ago(30), "-30 s");
        assert_eq!(time_ago(60), "-1 min");
        assert_eq!(time_ago(450), "-7 min 30 s");
    }

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
        // Never smaller than TILE_MIN: fewer columns, and the rows scroll.
        let (cols, tile) = min_layout(layout(12, (500.0, 300.0), 12.0), 500.0, 12.0);
        assert_eq!(tile, TILE_MIN);
        assert_eq!(cols, 2, "two of 176 px fit across 500 px");
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
