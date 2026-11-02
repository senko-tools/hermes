//! The voice UI's actions (PLAN.md sections 12.6, 15.1 and 15.4): joining and leaving, mute and
//! deafen, push-to-talk, the per-user volume menu and the incoming-call banner with its ring.

use std::time::{Duration, Instant};

use hermes_client_core::audio::Sound;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::{Core, InstanceId};
use iced::widget::{checkbox, column, container, mouse_area, pin, row, slider, stack};
use iced::{Alignment, Border, Element, Fill, Point, Shadow, Size, Task, Vector};

use crate::app::App;
use crate::buffer::{Buffer, settings};
use crate::font::Fonts;
use crate::message::Message;
use crate::theme::{Theme, style};
use crate::widget::{ButtonKind, text_button};
use crate::widget::{caption, strong};

/// An incoming call rings for at most this long.
pub const RING_FOR: Duration = Duration::from_secs(30);
/// One ring sound is about 1.6 s; it is repeated at this interval.
pub const RING_EVERY: Duration = Duration::from_millis(2000);
const MENU_WIDTH: f32 = 240.0;

#[derive(Debug, Clone, PartialEq)]
pub enum VoiceMsg {
    /// Join (or switch to) the voice room of a voice channel or DM. In a DM nobody is in,
    /// this starts a call (the others' clients ring).
    Join(InstanceId, Id),
    /// A voice channel was clicked with `voice.join_on = "single_click"`.
    SelectAndJoin(InstanceId, Id),
    Joined(InstanceId, Id, Result<(), String>),
    Leave,
    Left,
    ToggleMute,
    ToggleDeafen,
    /// Opens the VoiceRoom buffer of a channel in the focused pane.
    OpenRoom(InstanceId, Id),
    /// Right-click on a participant: opens the volume menu at the cursor.
    UserMenu(InstanceId, Id),
    UserVolume(f32),
    UserMuted(bool),
    CloseUserMenu,
    /// The push-to-talk key went down (`true`) or up.
    Ptt(bool),
    /// The global mute key was pressed.
    GlobalMute,
    AnswerCall,
    DismissCall,
    RingTick,
    /// Start or stop sharing my screen in my voice room.
    ToggleShare,
    /// Opens the menu that asks which screen to share and how (at the pointer).
    OpenShareMenu,
    CloseShareMenu,
    /// The share menu's Start sharing.
    StartShare,
    /// Shows one person's screen share alone in the voice room (`None`: everyone again).
    EnlargeShare(Option<(InstanceId, Id)>),
}

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
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub inst: InstanceId,
    pub channel: Id,
    pub caller: Id,
    pub since: Instant,
}

/// The open per-user volume menu.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeMenu {
    pub inst: InstanceId,
    pub user: Id,
    pub name: String,
    pub at: Point,
    pub volume: f32,
    pub muted: bool,
}

/// Joins a room; an empty DM room starts a call instead.
pub async fn join(core: Core, inst: InstanceId, channel: Id) -> Result<(), String> {
    core.join_or_call(&inst, channel)
        .await
        .map_err(|e| e.to_string())
}

impl App {
    pub fn on_voice(&mut self, message: VoiceMsg) -> Task<Message> {
        match message {
            VoiceMsg::Join(inst, channel) => {
                if self.core.voice_channel() == Some((inst.clone(), channel)) {
                    return Task::none();
                }
                self.joining = Some((inst.clone(), channel));
                let core = self.core.clone();
                return Task::perform(join(core, inst.clone(), channel), move |result| {
                    Message::Voice(VoiceMsg::Joined(inst.clone(), channel, result))
                });
            }
            VoiceMsg::SelectAndJoin(inst, channel) => {
                let select = self.update(Message::Select(inst.clone(), channel));
                return select.chain(self.on_voice(VoiceMsg::Join(inst, channel)));
            }
            VoiceMsg::Joined(inst, channel, result) => {
                if self.joining.as_ref() == Some(&(inst.clone(), channel)) {
                    self.joining = None;
                }
                match result {
                    Ok(()) => {
                        if self
                            .call
                            .as_ref()
                            .is_some_and(|c| c.inst == inst && c.channel == channel)
                        {
                            self.stop_ringing();
                        }
                        if !self.config.voice.noise_suppression && !self.headphones_hint_shown {
                            self.headphones_hint_shown = true;
                            self.info(
                                "Noise suppression is off: headphones are recommended (there is \
                                 no echo cancellation)."
                                    .into(),
                            );
                        }
                    }
                    Err(err) => self.error(format!("Could not join voice: {err}")),
                }
            }
            VoiceMsg::Leave => {
                let core = self.core.clone();
                return Task::perform(async move { core.leave_voice().await }, |()| {
                    Message::Voice(VoiceMsg::Left)
                });
            }
            VoiceMsg::Left => {}
            VoiceMsg::OpenShareMenu => {
                self.share_menu = Some(self.cursor.get());
                return self.load_devices();
            }
            VoiceMsg::CloseShareMenu => self.share_menu = None,
            VoiceMsg::StartShare => {
                self.share_menu = None;
                if self.capture.is_none() {
                    return self.on_voice(VoiceMsg::ToggleShare);
                }
            }
            VoiceMsg::ToggleShare => {
                if self.capture.take().is_some() {
                    self.core.stop_screen_share();
                } else if let Err(err) = self.core.start_screen_share() {
                    self.error(err.to_string());
                } else {
                    let v = &self.config.voice;
                    let screen = v.share_screen.clone();
                    let quality = crate::screen::capture::Quality::from_config(
                        &v.share_resolution,
                        v.share_fps,
                    );
                    match crate::screen::capture::Capture::start(
                        self.core.clone(),
                        &screen,
                        quality,
                    ) {
                        Ok(capture) => self.capture = Some(capture),
                        Err(err) => {
                            self.core.stop_screen_share();
                            self.error(format!("Could not share the screen: {err}"));
                        }
                    }
                }
            }
            VoiceMsg::ToggleMute | VoiceMsg::GlobalMute => {
                // Unmuting while deafened undeafens too (deafened implies muted).
                if self.core.self_deaf() {
                    self.core.set_self_deaf(false);
                    self.core.set_self_mute(false);
                } else {
                    self.core.set_self_mute(!self.core.self_mute());
                }
                self.update_tray_voice();
            }
            VoiceMsg::ToggleDeafen => {
                self.core.set_self_deaf(!self.core.self_deaf());
                self.update_tray_voice();
            }
            VoiceMsg::OpenRoom(inst, channel) => {
                self.panes.open(Buffer::VoiceRoom { inst, channel });
            }
            VoiceMsg::EnlargeShare(share) => self.enlarged_share = share,
            VoiceMsg::UserMenu(inst, user) => {
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
                self.volume_menu = Some(VolumeMenu {
                    volume: self.core.user_volume(&inst, user),
                    muted: self.core.user_muted(&inst, user),
                    inst,
                    user,
                    name,
                    at: self.cursor.get(),
                });
            }
            VoiceMsg::UserVolume(volume) => {
                if let Some(menu) = &mut self.volume_menu {
                    menu.volume = volume;
                    if let Err(err) = self.core.set_user_volume(&menu.inst, menu.user, volume) {
                        tracing::warn!("volume not saved: {err}");
                    }
                }
            }
            VoiceMsg::UserMuted(muted) => {
                if let Some(menu) = &mut self.volume_menu {
                    menu.muted = muted;
                    self.core.set_user_muted(&menu.inst, menu.user, muted);
                }
            }
            VoiceMsg::CloseUserMenu => self.volume_menu = None,
            VoiceMsg::Ptt(down) => {
                self.ptt_down = down;
                self.core.set_ptt(down);
            }
            VoiceMsg::AnswerCall => {
                if let Some(call) = self.call.clone() {
                    self.stop_ringing();
                    let select = self.update(Message::Select(call.inst.clone(), call.channel));
                    return select.chain(self.on_voice(VoiceMsg::Join(call.inst, call.channel)));
                }
            }
            VoiceMsg::DismissCall => self.stop_ringing(),
            VoiceMsg::RingTick => {
                if let Some(call) = &self.call {
                    if call.since.elapsed() >= RING_FOR {
                        self.stop_ringing();
                    } else {
                        self.core.play_sound(Sound::Ring);
                    }
                }
            }
        }
        Task::none()
    }

    /// An incoming call: show the banner and ring (unless I am in that room already).
    pub fn incoming_call(&mut self, inst: InstanceId, channel: Id, caller: Id) {
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

    /// The tray's Mute / Deafen check marks follow the core.
    pub fn update_tray_voice(&mut self) {
        if let Some(tray) = &mut self.tray {
            tray.set_voice(self.core.self_mute(), self.core.self_deaf());
        }
    }
}

/// The incoming-call bar shown at the top of the window.
pub fn banner<'a>(
    core: &Core,
    call: &Call,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let (caller, place) = {
        let store = core.store();
        let s = store.instance(&call.inst);
        let caller = s.map_or_else(
            || "Someone".to_owned(),
            |s| s.display_name(None, call.caller),
        );
        let place = s
            .and_then(|s| {
                let c = s.channel(call.channel)?;
                (c.kind == ChannelKind::GroupDm)
                    .then(|| hermes_client_core::view::channel_title(s, c))
            })
            .map(|title| format!(" in {title}"))
            .unwrap_or_default();
        (caller, place)
    };
    container(
        row![
            strong(format!("{caller} is calling{place}"), fonts).width(Fill),
            text_button("Join", ButtonKind::Primary, fonts, theme)
                .on_press(Message::Voice(VoiceMsg::AnswerCall)),
            text_button("Dismiss", ButtonKind::Secondary, fonts, theme)
                .on_press(Message::Voice(VoiceMsg::DismissCall)),
        ]
        .spacing(style::GAP_S)
        .align_y(Alignment::Center),
    )
    .padding([style::GAP_S, style::GAP_S])
    .width(Fill)
    .style(move |_| container::Style {
        text_color: Some(theme.buttons.primary.text.0),
        background: Some(theme.text.success.0.scale_alpha(0.25).into()),
        border: Border {
            color: theme.text.success.0,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

/// `base` with the per-user volume menu over it.
/// The share menu: which screen, how sharp and how smooth (each choice is saved as soon as it
/// is made, so the menu offers the last one next time), and Start sharing.
pub fn share_overlay<'a>(
    base: Element<'a, Message>,
    at: Point,
    config: &hermes_client_core::config::Voice,
    screens: &[String],
    window: Size,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    use crate::buffer::page::dropdown;
    use crate::screen::capture::{FRAME_RATES, RESOLUTIONS};

    let width = 280.0;
    let height = 290.0;
    // Above the pointer (the buttons sit at the bottom of the window).
    let x = at.x.min(window.width - width - 4.0).max(0.0);
    let y = (at.y - height).min(window.height - height - 4.0).max(0.0);
    let mut options = vec![settings::ShareScreen(String::new())];
    options.extend(screens.iter().skip(1).cloned().map(settings::ShareScreen));
    let field = |name: &'a str, input: Element<'a, Message>| {
        column![caption(name, fonts).color(theme.text.secondary.0), input].spacing(style::GAP_XS)
    };
    let card = container(
        column![
            strong("Share your screen", fonts),
            field(
                "Screen",
                dropdown(
                    options,
                    Some(settings::ShareScreen(config.share_screen.clone())),
                    |s| Message::Settings(settings::Message::ShareScreen(s)),
                    fonts,
                    theme,
                )
                .into(),
            ),
            field(
                "Resolution",
                dropdown(
                    RESOLUTIONS.map(settings::ShareResolution),
                    Some(settings::ShareResolution::from_config(
                        &config.share_resolution
                    )),
                    |r| Message::Settings(settings::Message::ShareResolution(r)),
                    fonts,
                    theme,
                )
                .into(),
            ),
            field(
                "Frame rate",
                dropdown(
                    FRAME_RATES.map(settings::ShareFps),
                    Some(settings::ShareFps(config.share_fps)),
                    |f| Message::Settings(settings::Message::ShareFps(f)),
                    fonts,
                    theme,
                )
                .into(),
            ),
            row![
                iced::widget::space::horizontal(),
                text_button("Cancel", ButtonKind::Secondary, fonts, theme)
                    .on_press(Message::Voice(VoiceMsg::CloseShareMenu)),
                text_button("Start sharing", ButtonKind::Primary, fonts, theme)
                    .on_press(Message::Voice(VoiceMsg::StartShare)),
            ]
            .spacing(style::GAP_S),
        ]
        .spacing(style::GAP),
    )
    .padding(style::GAP)
    .width(width)
    .style(move |_| container::Style {
        text_color: Some(theme.text.primary.0),
        background: Some(theme.surface.overlay.0.into()),
        border: Border {
            color: theme.general.border.0,
            width: 1.0,
            radius: 6.0.into(),
        },
        shadow: Shadow {
            color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.4),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        ..container::Style::default()
    });
    let close = Message::Voice(VoiceMsg::CloseShareMenu);
    let catcher = mouse_area(container("").width(Fill).height(Fill))
        .on_press(close.clone())
        .on_right_press(close);
    stack![base, catcher, pin(iced::widget::opaque(card)).x(x).y(y)].into()
}

pub fn volume_overlay<'a>(
    base: Element<'a, Message>,
    menu: &VolumeMenu,
    window: Size,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let height = 130.0;
    let x = menu.at.x.min(window.width - MENU_WIDTH - 4.0).max(0.0);
    let y = menu.at.y.min(window.height - height - 4.0).max(0.0);
    let card = container(
        column![
            strong(menu.name.clone(), fonts),
            row![
                caption("Volume", fonts).width(Fill),
                caption(format!("{:.0} %", menu.volume * 100.0), fonts)
                    .color(theme.text.secondary.0),
            ],
            slider(0.0..=2.0, menu.volume, |v| Message::Voice(
                VoiceMsg::UserVolume(v)
            ))
            .step(0.05_f32),
            checkbox(menu.muted)
                .label("Mute for me")
                .on_toggle(|on| Message::Voice(VoiceMsg::UserMuted(on)))
                .text_size(fonts.size)
                .font(fonts.ui),
        ]
        .spacing(style::GAP_S),
    )
    .padding(style::GAP_S)
    .width(MENU_WIDTH)
    .style(move |_| container::Style {
        text_color: Some(theme.text.primary.0),
        background: Some(theme.surface.overlay.0.into()),
        border: Border {
            color: theme.general.border.0,
            width: 1.0,
            radius: 6.0.into(),
        },
        shadow: Shadow {
            color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.4),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        ..container::Style::default()
    });
    let close = Message::Voice(VoiceMsg::CloseUserMenu);
    let catcher = mouse_area(container("").width(Fill).height(Fill))
        .on_press(close.clone())
        .on_right_press(close);
    stack![base, catcher, pin(iced::widget::opaque(card)).x(x).y(y)].into()
}
