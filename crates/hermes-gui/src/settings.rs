//! The settings page, shown over the whole window: its tabs in sections (You, App, Voice &
//! video, Advanced), a search over every setting, and long groups that fold. Every change is
//! written to `config.toml`.

use std::cell::{Cell, RefCell};

use gpui_kit::AnimationExt as _;
use gpui_kit::component::IndexPath;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Entity, FontWeight, SharedString, Subscription, Window, div};
use hermes_client_core::audio::{AudioDevices, Sound};
use hermes_client_core::config::{self, ClientConfig};
use hermes_client_core::proto::api::{SessionInfo, UpdateMeRequest, UpdatePrivacyRequest};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::DmFrom;
use hermes_client_core::{InstanceId, time};

use crate::assets::{self, icon};
use crate::keybinds::{ACTIONS, Action, Keybinds};
use crate::results::Remote;
use crate::screen::capture::{FRAME_RATES, PREFERENCES, Preference, ShareResolution};
use crate::shell::Shell;
use crate::theme::{self, Theme, px};

/// The level meter and the voice activity slider share this range (dBFS).
pub const METER_MIN: f32 = -70.0;
/// UI scales offered as one-click choices.
const SCALE_PRESETS: [f64; 5] = [0.9, 1.0, 1.1, 1.25, 1.5];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    Account,
    Privacy,
    #[default]
    Appearance,
    Chat,
    Notifications,
    Window,
    Keybinds,
    Voice,
    ScreenShare,
    Diagnostics,
    Admin,
    About,
}

/// A tab in the navigation: the tab, its icon and its name.
type NavTab = (Tab, &'static str, &'static str);

/// The navigation: sections of tabs. About stands apart at the bottom.
const SECTIONS: [(&str, &[NavTab]); 4] = [
    (
        "You",
        &[
            (Tab::Account, assets::ACCOUNT, "Account"),
            (Tab::Privacy, assets::LOCK, "Privacy & safety"),
        ],
    ),
    (
        "App",
        &[
            (Tab::Appearance, assets::PALETTE, "Appearance"),
            (Tab::Chat, assets::CHAT, "Chat"),
            (Tab::Notifications, assets::BELL, "Notifications"),
            (Tab::Window, assets::COG, "Window & startup"),
            (Tab::Keybinds, assets::KEYBOARD, "Keybinds"),
        ],
    ),
    (
        "Voice & video",
        &[
            (Tab::Voice, assets::MIC, "Voice & Audio"),
            (Tab::ScreenShare, assets::MONITOR_SHARE, "Screen sharing"),
        ],
    ),
    (
        "Advanced",
        &[
            (Tab::Diagnostics, assets::PULSE, "Diagnostics"),
            (Tab::Admin, assets::SERVER, "Instance"),
        ],
    ),
];
const ABOUT: NavTab = (Tab::About, assets::INFO, "About");

/// Every tab in the navigation's order, with its name.
fn all_tabs() -> impl Iterator<Item = (Tab, &'static str)> {
    SECTIONS
        .iter()
        .flat_map(|(_, tabs)| tabs.iter())
        .chain(std::iter::once(&ABOUT))
        .map(|(tab, _, name)| (*tab, *name))
}

/// The keybind groups, in the order shown. Pane keys come with split panes.
const KEY_GROUPS: [(&str, &[Action]); 3] = [
    (
        "Navigation",
        &[
            Action::CommandBar,
            Action::Search,
            Action::NextUnread,
            Action::PrevUnread,
            Action::NextChannel,
            Action::PrevChannel,
            Action::MarkRead,
            Action::ToggleMemberList,
            Action::ToggleSidebar,
        ],
    ),
    ("Voice", &[Action::ToggleMute, Action::ToggleDeafen]),
    (
        "App",
        &[
            Action::Settings,
            Action::ZoomIn,
            Action::ZoomOut,
            Action::ZoomReset,
        ],
    ),
];

/// What a slider of the page changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SliderKind {
    FontSize,
    UiScale,
    GroupMinutes,
    InputVolume,
    OutputVolume,
    Vad,
    VadHold,
    AgcTarget,
    Ducking,
    ReleaseDelay,
    SoundsVolume,
    MediaVolume,
    ScrollSpeed,
}

/// How often the level meter can redraw the window at most. Every frame redraws the whole window,
/// so this is not the display's rate: drawing at 144 Hz all the time cost about 10 % CPU and GPU.
const METER_FPS: f32 = 60.0;
/// The bar has to move this far for a redraw (under a pixel on a setting row), so a settled
/// meter costs nothing.
const METER_STEP_DB: f32 = 0.1;

/// Where the level meter's bar goes next: it follows the level quickly upwards and falls slowly, as
/// meters do, in dB and `seconds` after it was last drawn.
pub fn meter_step(shown: f32, level: f32, seconds: f32) -> f32 {
    const RISE: f32 = 0.03;
    const FALL: f32 = 0.25;
    let time = if level > shown { RISE } else { FALL };
    shown + (level - shown) * (1.0 - (-seconds / time).exp())
}

#[cfg(test)]
#[test]
fn the_meter_rises_quickly_and_falls_slowly() {
    let frame = 1.0 / 60.0;
    let rise = meter_step(-60.0, -20.0, frame) + 60.0;
    let fall = -20.0 - meter_step(-20.0, -60.0, frame);
    assert!(rise > 4.0 * fall, "rise {rise}, fall {fall}");
    assert!((meter_step(-30.0, -30.0, frame) + 30.0).abs() < 1e-6);
    assert!((meter_step(-60.0, -20.0, 10.0) + 20.0).abs() < 0.01);
}

/// A choice of the audio device dropdowns; `value` is what the config stores.
#[derive(Clone)]
struct DeviceChoice {
    value: String,
    name: SharedString,
}

/// A switch row for a `bool` in the config, naming the field once so what the switch shows and
/// what it changes cannot differ: `flag!(self, cx, "Name", "Hint", buffer.compact)`.
macro_rules! flag {
    ($shell:expr, $cx:expr, $name:expr, $hint:expr, $($field:ident).+) => {
        $shell.flag(
            $name,
            $hint,
            $shell.config.$($field).+,
            stringify!($($field).+),
            |c: &mut ClientConfig, on: bool| c.$($field).+ = on,
            $cx,
        )
    };
}

impl SelectItem for DeviceChoice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &String {
        &self.value
    }
}

/// The installed fonts, with the one in use even if it is not installed any more.
fn font_choices(names: &[String], current: &str) -> (Vec<DeviceChoice>, usize) {
    let mut all: Vec<String> = names.to_vec();
    if !all.iter().any(|n| n == current) {
        all.insert(0, current.to_owned());
    }
    let selected = all.iter().position(|n| n == current).unwrap_or(0);
    let choices = all
        .into_iter()
        .map(|name| DeviceChoice {
            name: SharedString::from(name.clone()),
            value: name,
        })
        .collect();
    (choices, selected)
}

/// The choices for the device saved as `current`: the system default, what is plugged in, and the
/// saved device even when it is not there, so the dropdown says what is really set.
fn device_choices(names: &[String], current: &str) -> (Vec<DeviceChoice>, usize) {
    let current = if current.is_empty() {
        "default"
    } else {
        current
    };
    let mut choices = vec![DeviceChoice {
        value: "default".to_owned(),
        name: "System default".into(),
    }];
    choices.extend(names.iter().map(|name| DeviceChoice {
        value: name.clone(),
        name: name.clone().into(),
    }));
    if !choices.iter().any(|c| c.value == current) {
        choices.push(DeviceChoice {
            value: current.to_owned(),
            name: format!("{current} (not connected)").into(),
        });
    }
    let selected = choices
        .iter()
        .position(|c| c.value == current)
        .unwrap_or_default();
    (choices, selected)
}

/// The controls of the settings page that hold state of their own.
pub struct SettingsPage {
    pub tab: Tab,
    pub account_inst: Option<InstanceId>,
    pub sessions: Remote<Vec<SessionInfo>>,
    /// Counts the session list loads; only the newest one's answer is shown.
    sessions_load: u64,
    /// The action waiting for a key press, by its config name.
    pub capturing_key: Option<&'static str>,
    /// Whether this account has two-factor login, once known.
    two_factor: Option<bool>,
    admin_users: Remote<Vec<hermes_client_core::proto::api::AdminUser>>,
    admin_log: Remote<Vec<hermes_client_core::proto::api::AdminLogEntry>>,
    admin_settings: Option<hermes_client_core::proto::api::AdminSettings>,
    /// The audio device dropdowns, once the Voice & Audio tab has listed the devices.
    device_selects: Option<DeviceSelects>,
    pub mic_test: bool,
    /// Held while the Voice tab and its level meter show.
    fine_timers: Option<crate::desktop::window::FineTimers>,
    font: Entity<SelectState<Vec<DeviceChoice>>>,
    mono_font: Entity<SelectState<Vec<DeviceChoice>>>,
    time_format: Entity<InputState>,
    date_format: Entity<InputState>,
    /// Custom was chosen for the time or date: its pattern field shows.
    custom_time: bool,
    custom_date: bool,
    /// Searching every setting by name and hint.
    search: Entity<InputState>,
    ptt_key: Entity<InputState>,
    sliders: Vec<(SliderKind, Entity<SliderState>)>,
    _events: Vec<Subscription>,
}

struct DeviceSelects {
    input: Entity<SelectState<Vec<DeviceChoice>>>,
    output: Entity<SelectState<Vec<DeviceChoice>>>,
    _events: [Subscription; 2],
}

impl Shell {
    /// Moves the level meter at [`METER_FPS`] while it shows, redrawing only when the bar would
    /// visibly move: a window redraw is what costs, not the meter.
    pub fn start_meter_timer(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut wait = std::time::Duration::from_secs_f32(1.0 / METER_FPS);
            loop {
                cx.background_executor().timer(wait).await;
                let alive = this.update(cx, |this, cx| {
                    let showing = this
                        .settings
                        .as_ref()
                        .is_some_and(|page| page.tab == Tab::Voice);
                    let now = std::time::Instant::now();
                    let seconds = now.duration_since(this.meter_drawn).as_secs_f32();
                    this.meter_drawn = now;
                    if !showing {
                        return false;
                    }
                    let next = meter_step(this.meter_shown, this.input_level, seconds);
                    let moved = (next - this.meter_shown).abs();
                    this.meter_shown = next;
                    let opened = this.meter_open != this.input_open;
                    this.meter_open = this.input_open;
                    if moved >= METER_STEP_DB || opened {
                        cx.notify();
                    }
                    true
                });
                let Ok(showing) = alive else {
                    return;
                };
                // Hardly anything to do while the Voice tab is closed.
                wait = std::time::Duration::from_secs_f32(if showing {
                    1.0 / METER_FPS
                } else {
                    0.25
                });
            }
        })
        .detach();
    }

    /// Opens the settings over the whole window.
    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        let voice = self.config.voice.clone();
        let font = self.config.font.clone();
        let text_field = |value: String,
                          placeholder: &'static str,
                          window: &mut Window,
                          cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = InputState::new(window, cx).placeholder(placeholder);
                input.set_value(value, window, cx);
                input
            })
        };
        let mut installed = cx.text_system().all_font_names();
        installed.sort();
        installed.dedup();
        let mut font_select = |current: &str, field: fn(&mut ClientConfig) -> &mut String| {
            let (choices, selected) = font_choices(&installed, current);
            let state =
                cx.new(|cx| SelectState::new(choices, Some(IndexPath::new(selected)), window, cx));
            let events = cx.subscribe(
                &state,
                move |this, _, event: &SelectEvent<Vec<DeviceChoice>>, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event {
                        let value = value.clone();
                        this.set_config(|c| *field(c) = value, cx);
                        this.reload_theme(cx);
                        this.remeasure_chats();
                    }
                },
            );
            (state, events)
        };
        let (font_input, font_events) = font_select(&font.family, |c| &mut c.font.family);
        let (mono_input, mono_events) = font_select(&font.mono_family, |c| &mut c.font.mono_family);
        let time_format = text_field(
            self.config.buffer.timestamp_format.clone(),
            "%H:%M",
            window,
            cx,
        );
        let date_format = text_field(
            self.config.buffer.date_format.clone(),
            "%B %e, %Y",
            window,
            cx,
        );
        let ptt_key = text_field(voice.ptt_key.clone(), "e.g. Mouse4", window, cx);
        let search = text_field(String::new(), "Search settings", window, cx);
        let mut events = vec![
            cx.subscribe_in(&search, window, |this, _, event, window, cx| match event {
                InputEvent::Change => cx.notify(),
                // Enter opens the first thing found.
                InputEvent::PressEnter { .. } => {
                    if let Some((tab, found)) = this.settings_found(cx).into_iter().next() {
                        this.open_found(tab, found, window, cx);
                    }
                }
                _ => {}
            }),
            cx.subscribe_in(&date_format, window, |this, input, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let value = input.read(cx).value().to_string();
                    this.set_pattern(Pattern::Date, value, true, window, cx);
                    this.info("Date format saved", cx);
                }
            }),
            font_events,
            mono_events,
            cx.subscribe_in(&time_format, window, |this, input, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let value = input.read(cx).value().to_string();
                    this.set_pattern(Pattern::Time, value, true, window, cx);
                    this.info("Time format saved", cx);
                }
            }),
            cx.subscribe_in(&ptt_key, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let text = input.read(cx).value().trim().to_owned();
                    if !text.is_empty() && crate::ptt::virtual_keys(&text).is_none() {
                        this.error(format!("{text:?} is not a key Hermes knows"), cx);
                    } else {
                        this.set_config(|c| c.voice.ptt_key = text, cx);
                        this.info("Push-to-talk key saved", cx);
                    }
                }
            }),
        ];
        let table = [
            (SliderKind::FontSize, 8., 32., 1., f32::from(font.size)),
            (
                SliderKind::UiScale,
                theme::MIN_SCALE as f32,
                theme::MAX_SCALE as f32,
                0.05,
                font.ui_scale as f32,
            ),
            (
                SliderKind::GroupMinutes,
                0.,
                30.,
                1.,
                self.config.buffer.group_minutes.min(30) as f32,
            ),
            (
                SliderKind::InputVolume,
                0.,
                hermes_client_core::audio::MAX_INPUT_VOLUME,
                0.05,
                voice.input_volume as f32,
            ),
            (
                SliderKind::OutputVolume,
                0.,
                hermes_client_core::audio::MAX_VOLUME,
                0.05,
                voice.output_volume as f32,
            ),
            (
                SliderKind::Vad,
                METER_MIN,
                0.,
                1.,
                (voice.vad_sensitivity as f32).clamp(METER_MIN, 0.),
            ),
            (
                SliderKind::ReleaseDelay,
                0.,
                1000.,
                10.,
                voice.ptt_release_delay_ms as f32,
            ),
            (
                SliderKind::VadHold,
                100.,
                1000.,
                10.,
                (voice.vad_hold_ms as f32).clamp(100., 1000.),
            ),
            (
                SliderKind::Ducking,
                3.,
                30.,
                1.,
                (voice.ducking_db as f32).clamp(3., 30.),
            ),
            (
                SliderKind::AgcTarget,
                -30.,
                -6.,
                1.,
                (voice.agc_target_db as f32).clamp(-30., -6.),
            ),
            (
                SliderKind::SoundsVolume,
                0.,
                1.,
                0.05,
                voice.sounds_volume as f32,
            ),
            (
                SliderKind::MediaVolume,
                0.,
                1.,
                0.05,
                (voice.media_volume as f32).clamp(0., 1.),
            ),
            (
                SliderKind::ScrollSpeed,
                0.5,
                3.,
                0.05,
                self.config.general.scroll_speed as f32,
            ),
        ];
        let mut sliders = Vec::new();
        for (kind, min, max, step, value) in table {
            let state = cx.new(|_| {
                SliderState::new()
                    .min(min)
                    .max(max)
                    .step(step)
                    .default_value(value)
            });
            events.push(cx.subscribe_in(
                &state,
                window,
                move |this, _, event: &SliderEvent, window, cx| match event {
                    SliderEvent::Change(value) => {
                        this.slider_moved(kind, value.end(), cx);
                    }
                    SliderEvent::Release(value) => {
                        this.slider_moved(kind, value.end(), cx);
                        this.slider_released(kind, window, cx);
                    }
                },
            ));
            sliders.push((kind, state));
        }
        self.settings = Some(SettingsPage {
            tab: self.settings_tab,
            account_inst: None,
            sessions: Remote::Idle,
            sessions_load: 0,
            capturing_key: None,
            two_factor: None,
            admin_users: Remote::Idle,
            admin_log: Remote::Idle,
            admin_settings: None,
            device_selects: None,
            mic_test: false,
            fine_timers: None,
            font: font_input,
            mono_font: mono_input,
            time_format,
            custom_time: false,
            custom_date: false,
            date_format,
            search,
            ptt_key,
            sliders,
            _events: events,
        });
        let tab = self.settings_tab;
        self.settings_open_tab(tab, cx);
        cx.notify();
    }

    pub fn close_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = self.settings.take() {
            self.settings_tab = page.tab;
            if page.mic_test {
                self.core.set_mic_test(false);
            }
        }
        // Diagnostics still listening finish first; they turn the meter off themselves.
        let listening = self
            .diagnostics
            .as_ref()
            .is_some_and(crate::diagnostics::Run::listening);
        if !listening {
            self.core.set_level_meter(false);
        }
        cx.notify();
    }

    pub(crate) fn settings_open_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        let Some(page) = &mut self.settings else {
            return;
        };
        page.tab = tab;
        reset_page_scroll();
        page.fine_timers = (tab == Tab::Voice).then(crate::desktop::window::FineTimers::new);
        // Diagnostics listen to the microphone for a moment of their own.
        let listening = self
            .diagnostics
            .as_ref()
            .is_some_and(crate::diagnostics::Run::listening);
        self.core.set_level_meter(tab == Tab::Voice || listening);
        match tab {
            Tab::Account => self.load_sessions(cx),
            Tab::Admin => self.load_admin(cx),
            Tab::Voice => self.load_devices(cx),
            _ => {}
        }
        cx.notify();
    }

    /// The instance the Account tab shows.
    fn account_instance(&self) -> Option<InstanceId> {
        let ids = self.core.instance_ids();
        self.settings
            .as_ref()
            .and_then(|p| p.account_inst.clone())
            .filter(|i| ids.contains(i))
            .or_else(|| ids.first().cloned())
    }

    fn load_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(inst) = self.account_instance() else {
            return;
        };
        let Some(page) = &mut self.settings else {
            return;
        };
        page.sessions = Remote::Loading;
        page.two_factor = None;
        page.sessions_load += 1;
        let load = page.sessions_load;
        let twofa_core = self.core.clone();
        let twofa_inst = inst.clone();
        self.run(
            cx,
            async move {
                let rest = twofa_core.rest(&twofa_inst).ok()?;
                rest.two_factor().await.ok().map(|s| s.enabled)
            },
            move |this, enabled, _, cx| {
                if let Some(page) = &mut this.settings
                    && page.sessions_load == load
                {
                    page.two_factor = enabled;
                }
                cx.notify();
            },
        );
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                core.rest(&inst)?
                    .sessions()
                    .await
                    .map_err(|e| e.to_string())
            },
            move |this, result, _, _| {
                // An older load (another instance picked since, or a reload after signing a
                // device out) is of no use any more.
                if let Some(page) = &mut this.settings
                    && page.sessions_load == load
                {
                    page.sessions = match result {
                        Ok(list) => Remote::Ready(list),
                        Err(err) => Remote::Failed(err),
                    };
                }
            },
        );
    }

    /// Lists the audio devices and screens in the background (it can take a moment).
    fn load_devices(&mut self, cx: &mut Context<Self>) {
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(move || core.list_audio_devices())
                    .await
                    .unwrap_or_default()
            },
            |this, devices, window, cx| {
                let selects = this.device_selects(&devices, window, cx);
                if let Some(page) = &mut this.settings {
                    page.device_selects = Some(selects);
                }
            },
        );
        self.load_screens(cx);
    }

    fn device_selects(
        &self,
        devices: &AudioDevices,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DeviceSelects {
        let voice = &self.config.voice;
        let mut select = |names: &[String], current: &str, input: bool| {
            let (choices, selected) = device_choices(names, current);
            let state =
                cx.new(|cx| SelectState::new(choices, Some(IndexPath::new(selected)), window, cx));
            let events = cx.subscribe(
                &state,
                move |this, _, event: &SelectEvent<Vec<DeviceChoice>>, cx| {
                    let SelectEvent::Confirm(Some(value)) = event else {
                        return;
                    };
                    let value = value.clone();
                    this.change_voice(cx, move |voice| {
                        if input {
                            voice.input_device = value;
                        } else {
                            voice.output_device = value;
                        }
                    });
                },
            );
            (state, events)
        };
        let (input, input_events) = select(&devices.inputs, &voice.input_device, true);
        let (output, output_events) = select(&devices.outputs, &voice.output_device, false);
        DeviceSelects {
            input,
            output,
            _events: [input_events, output_events],
        }
    }

    fn slider_moved(&mut self, kind: SliderKind, value: f32, cx: &mut Context<Self>) {
        let rounded = |v: f32| f64::from((v * 100.0).round() / 100.0);
        let voice = &mut self.config.voice;
        match kind {
            SliderKind::FontSize => self.config.font.size = value as u16,
            SliderKind::UiScale => self.ui_scale_draft = Some(rounded(value)),
            SliderKind::GroupMinutes => self.config.buffer.group_minutes = value as u32,
            SliderKind::InputVolume => voice.input_volume = rounded(value) as _,
            SliderKind::OutputVolume => voice.output_volume = rounded(value) as _,
            SliderKind::Vad => voice.vad_sensitivity = value as i32,
            SliderKind::ReleaseDelay => voice.ptt_release_delay_ms = value as u32,
            SliderKind::VadHold => voice.vad_hold_ms = value as u32,
            SliderKind::AgcTarget => voice.agc_target_db = value as i32,
            SliderKind::Ducking => voice.ducking_db = value as u32,
            SliderKind::SoundsVolume => voice.sounds_volume = rounded(value) as _,
            SliderKind::MediaVolume => voice.media_volume = rounded(value) as _,
            SliderKind::ScrollSpeed => self.config.general.scroll_speed = rounded(value) as _,
        }
        cx.notify();
    }

    fn slider_released(&mut self, kind: SliderKind, window: &mut Window, cx: &mut Context<Self>) {
        match kind {
            SliderKind::FontSize => {
                let size = self.config.font.size;
                self.set_config(|c| c.font.size = size, cx);
                self.reload_theme(cx);
                self.remeasure_chats();
            }
            SliderKind::UiScale => {
                if let Some(scale) = self.ui_scale_draft.take() {
                    self.set_ui_scale(scale, window, cx);
                }
            }
            SliderKind::GroupMinutes => {
                let minutes = self.config.buffer.group_minutes;
                self.set_config(|c| c.buffer.group_minutes = minutes, cx);
            }
            SliderKind::MediaVolume => {
                let volume = (self.config.voice.media_volume * 1000.0).round() / 1000.0;
                self.set_config(|c| c.voice.media_volume = volume, cx);
            }
            SliderKind::ScrollSpeed => {
                let speed = (self.config.general.scroll_speed * 100.0).round() / 100.0;
                self.set_config(|c| c.general.scroll_speed = speed, cx);
            }
            _ => {
                let voice = self.config.voice.clone();
                self.save_voice(&voice, cx);
            }
        }
    }

    /// Applies and saves a UI scale (rounded to whole percents, within the allowed range).
    pub fn set_ui_scale(&mut self, scale: f64, window: &mut Window, cx: &mut Context<Self>) {
        self.ui_scale_draft = None;
        let scale = ((scale * 100.0).round() / 100.0).clamp(theme::MIN_SCALE, theme::MAX_SCALE);
        self.set_config(|c| c.font.ui_scale = scale, cx);
        self.reload_theme(cx);
        window.set_rem_size(gpui_kit::px(16. * scale as f32));
        self.remeasure_chats();
        cx.notify();
    }

    /// Applies `[voice]` to the audio engine and writes it to `config.toml`.
    fn save_voice(&mut self, voice: &hermes_client_core::config::Voice, cx: &mut Context<Self>) {
        let settings = hermes_client_core::voice::audio_settings(voice);
        if let Err(err) = self.core.set_audio_settings(settings) {
            self.error(format!("setting not saved: {err}"), cx);
        }
        self.apply_config(cx);
    }

    fn change_voice(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut hermes_client_core::config::Voice),
    ) {
        let mut voice = self.config.voice.clone();
        change(&mut voice);
        self.save_voice(&voice, cx);
    }

    /// Loads the configured theme again (from disk if it is a user theme) and applies it.
    pub fn reload_theme(&mut self, cx: &mut Context<Self>) {
        let dir = self.core.paths().map(|p| p.dir.join(theme::THEMES_DIR));
        let loaded = Theme::load(&self.config.general.theme, dir.as_deref());
        self.themes = theme::available(dir.as_deref());
        self.theme_previews = self
            .themes
            .iter()
            .map(|name| {
                let preview = Theme::load(name, dir.as_deref());
                let failed = preview
                    .warnings
                    .into_iter()
                    .find(|w| w.contains("not loaded") || w.contains("not found"));
                (name.clone(), failed.map_or(Ok(preview.theme), Err))
            })
            .collect();
        theme::set_active(loaded.theme, &self.config.font);
        theme::apply_to_kit(cx);
        if let Some(warning) = loaded.warnings.into_iter().next() {
            self.warn(warning, cx);
        }
        cx.notify();
    }

    /// Copies the current theme to a new file in the themes folder, to edit it.
    fn copy_theme(&mut self, cx: &mut Context<Self>) -> Result<String, String> {
        let dir = self
            .core
            .paths()
            .map(|p| p.dir.join(theme::THEMES_DIR))
            .ok_or("no config directory")?;
        let name = self.config.general.theme.clone();
        let text = match std::fs::read_to_string(dir.join(format!("{name}.toml"))) {
            Ok(text) => text,
            Err(_) => theme::BUILTIN
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, text)| (*text).to_owned())
                .ok_or_else(|| format!("theme `{name}` not found"))?,
        };
        let mut copy = format!("{name}-custom");
        let mut n = 2;
        while dir.join(format!("{copy}.toml")).exists() {
            copy = format!("{name}-custom-{n}");
            n += 1;
        }
        let path = dir.join(format!("{copy}.toml"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        self.reload_theme(cx);
        self.info(
            format!(
                "Copied to {}: edit it and the changes show up at once",
                path.display()
            ),
            cx,
        );
        Ok(copy)
    }

    fn set_theme(&mut self, name: String, cx: &mut Context<Self>) {
        self.set_config(|c| c.general.theme = name, cx);
        self.reload_theme(cx);
    }

    /// Theme files changed on disk, or the config: redo the look.
    pub fn start_theme_watcher(&self, cx: &mut Context<Self>) {
        let Some(dir) = self.core.paths().map(|p| p.dir.join(theme::THEMES_DIR)) else {
            return;
        };
        // One slot: a reload already waiting covers every change after it, so a burst of file
        // events (an editor saving, a folder copied in) cannot pile up.
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let watcher = std::fs::create_dir_all(&dir)
            .map_err(notify::Error::io)
            .and_then(|()| {
                use notify::Watcher as _;
                let mut watcher =
                    notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                        if res.is_ok_and(|e| !e.kind.is_access()) {
                            let _ = tx.try_send(());
                        }
                    })?;
                watcher.watch(&dir, notify::RecursiveMode::NonRecursive)?;
                Ok(watcher)
            });
        let watcher = match watcher {
            Ok(watcher) => watcher,
            Err(err) => {
                tracing::warn!("theme hot reload unavailable: {err}");
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let _watcher = watcher;
            while rx.recv().await.is_some() {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
                while rx.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.reload_theme(cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Version, system and connection state, for bug reports (no tokens or message content).
    fn debug_info(&self) -> String {
        let mut out = format!(
            "Hermes {} ({} {})\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        for inst in self.core.instance_ids() {
            out.push_str(&format!("instance {inst}: {:?}\n", self.core.status(&inst)));
        }
        out.push_str(&format!("voice: {:?}\n", self.core.voice_status()));
        out.push_str(&format!(
            "theme {}, font size {}, ui scale {}\n",
            self.config.general.theme, self.config.font.size, self.config.font.ui_scale
        ));
        if let Some(paths) = self.core.paths() {
            out.push_str(&format!("config dir: {}\n", paths.dir.display()));
        }
        out
    }

    fn pick_avatar(&mut self, inst: InstanceId, cx: &mut Context<Self>) {
        self.pick_picture(crate::crop::CropFor::Avatar(inst), cx);
    }

    fn revoke_sessions(&mut self, ids: Vec<Id>, cx: &mut Context<Self>) {
        let Some(inst) = self.account_instance() else {
            return;
        };
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                let rest = core.rest(&inst)?;
                for id in ids {
                    rest.delete_session(id).await?;
                }
                Ok::<(), String>(())
            },
            |this, result, _, cx| match result {
                Ok(()) => this.load_sessions(cx),
                Err(err) => this.error(format!("could not sign out: {err}"), cx),
            },
        );
    }

    fn set_privacy(&mut self, req: UpdatePrivacyRequest, cx: &mut Context<Self>) {
        if let Some(inst) = self.account_instance() {
            self.call_status(
                &inst,
                "Privacy saved",
                move |r| async move { r.update_privacy(&req).await },
                cx,
            );
        }
    }

    // ---- drawing ------------------------------------------------------------------------------

    /// The page: navigation on the left, the tab on the right.
    pub fn render_settings(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let page = self.settings.as_ref()?;
        let admin = self.is_instance_admin();
        let item = |(tab, glyph, name): &NavTab, cx: &mut Context<Self>| {
            let tab = *tab;
            crate::ui::nav_item(
                self,
                glyph,
                name,
                page.tab == tab,
                format!("settings-tab-{name}"),
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.settings_open_tab(tab, cx)))
            .into_any_element()
        };
        let mut nav: Vec<AnyElement> = vec![
            div()
                .flex_shrink_0()
                .mb(px(4.))
                .child(
                    Input::new(&page.search)
                        .prefix(icon(assets::SEARCH, 16., theme::text_tertiary()))
                        .cleanable(true),
                )
                .into_any_element(),
        ];
        for (title, tabs) in SECTIONS {
            let tabs: Vec<_> = tabs
                .iter()
                .filter(|(tab, ..)| *tab != Tab::Admin || admin)
                .collect();
            if tabs.is_empty() {
                continue;
            }
            nav.push(nav_section(title));
            nav.extend(tabs.into_iter().map(|t| item(t, cx)));
        }
        // About stays at the bottom, under a line.
        nav.push(div().flex_1().min_h(px(12.)).into_any_element());
        nav.push(
            div()
                .flex_shrink_0()
                .h(px(1.))
                .mx(px(8.))
                .mb(px(6.))
                .bg(theme::border())
                .into_any_element(),
        );
        nav.push(item(&ABOUT, cx));
        let query = page.search.read(cx).value().trim().to_owned();
        let content = if !query.is_empty() {
            self.search_results(&query, cx)
        } else {
            match page.tab {
                Tab::Account => self.tab_account(page, cx),
                Tab::Privacy => self.tab_privacy(cx),
                Tab::Appearance => self.tab_appearance(page, cx),
                Tab::Chat => self.tab_chat(page, cx),
                Tab::Notifications => self.tab_notifications(cx),
                Tab::Window => self.tab_window(cx),
                Tab::Keybinds => self.tab_keybinds(page, cx),
                Tab::Voice => self.tab_voice(page, cx),
                Tab::ScreenShare => self.tab_screen_share(cx),
                Tab::Diagnostics => self.tab_diagnostics(cx),
                Tab::Admin => self.tab_admin(page, cx),
                Tab::About => self.tab_about(cx),
            }
        };
        let key = if query.is_empty() {
            format!("settings-{:?}", page.tab)
        } else {
            "settings-search".to_owned()
        };
        Some(frame(
            "User settings".into(),
            nav,
            theme::enter(SharedString::from(key), content),
            cx.listener(|this, _, _, cx| this.close_settings(cx)),
        ))
    }

    fn tab_account(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let instances = self.core.instance_ids();
        if instances.is_empty() {
            return page_body(
                "Account",
                None,
                vec![muted(
                    "Add an instance first: your account lives on a server.",
                )],
            );
        }
        let selected = self.account_instance();
        let me = {
            let store = self.core.store();
            let s = selected.as_ref().and_then(|i| store.instance(i));
            s.and_then(|s| s.me.clone())
        };
        let mut account_rows = Vec::new();
        if instances.len() > 1 {
            let chips = instances.iter().enumerate().map(|(n, inst)| {
                let chosen = Some(inst) == selected.as_ref();
                let inst = inst.clone();
                crate::ui::chip(self, ("account-inst", n), inst.clone(), chosen, cx).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if let Some(page) = &mut this.settings {
                            page.account_inst = Some(inst.clone());
                        }
                        this.load_sessions(cx);
                    }),
                )
            });
            account_rows.push(setting(
                "Instance",
                "Each server has its own account.",
                wrap().children(chips),
            ));
        }
        let Some(me) = me else {
            account_rows.push(muted("Not signed in to this instance."));
            return page_body("Account", None, vec![group("Account", account_rows)]);
        };
        let inst = selected.clone().unwrap_or_default();
        let mut hint = format!(
            "{} (@{}){}",
            me.user.name(),
            me.user.username,
            me.user
                .status_text
                .as_deref()
                .map(|s| format!(" · {s}"))
                .unwrap_or_default()
        );
        if me.instance_admin {
            hint.push_str(" · instance admin");
        }
        let username_inst = inst.clone();
        let copy_handle = me.user.handle();
        account_rows.push(setting(
            "Username",
            &format!("You are {copy_handle}. Friends add you by this name."),
            wrap()
                .child(
                    Button::new("account-username-copy")
                        .cursor_pointer()
                        .label(self.copy_label("account-username-copy", "Copy"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.copy_from(
                                "account-username-copy",
                                copy_handle.clone(),
                                "Username",
                                cx,
                            );
                        })),
                )
                .child(
                    Button::new("account-username")
                        .cursor_pointer()
                        .label("Change username…")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_form_username(username_inst.clone(), window, cx);
                        })),
                ),
        ));
        let profile_inst = inst.clone();
        account_rows.push(setting(
            "Profile",
            &hint,
            Button::new("account-profile")
                .cursor_pointer()
                .primary()
                .label("Edit profile…")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_form_profile(profile_inst.clone(), window, cx);
                })),
        ));
        let has_avatar = me.user.avatar.is_some();
        let (pick_inst, remove_inst) = (inst.clone(), inst.clone());
        let avatar_hint = format!(
            "{} {}",
            if has_avatar {
                "Shown next to your messages."
            } else {
                "No picture yet; your initials are shown."
            },
            crate::crop::CropFor::Avatar(inst.clone()).rules()
        );
        account_rows.push(setting(
            "Avatar",
            &avatar_hint,
            div()
                .flex()
                .gap(px(8.))
                .child(
                    Button::new("account-avatar")
                        .cursor_pointer()
                        .label(if has_avatar { "Change…" } else { "Upload…" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pick_avatar(pick_inst.clone(), cx);
                        })),
                )
                .when(has_avatar, |d| {
                    d.child(
                        Button::new("account-avatar-remove")
                            .cursor_pointer()
                            .label("Remove")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.call_status(
                                    &remove_inst,
                                    "Avatar removed",
                                    |r| async move {
                                        r.update_me(&UpdateMeRequest {
                                            avatar_id: Some(None),
                                            ..UpdateMeRequest::default()
                                        })
                                        .await
                                    },
                                    cx,
                                );
                            })),
                    )
                }),
        ));
        let has_banner = me.user.banner.is_some();
        let (banner_inst, unbanner_inst) = (inst.clone(), inst.clone());
        let banner_hint = format!(
            "The wide picture at the top of your profile. {}",
            crate::crop::CropFor::Banner(inst.clone()).rules()
        );
        account_rows.push(setting(
            "Banner",
            &banner_hint,
            div()
                .flex()
                .gap(px(8.))
                .child(
                    Button::new("account-banner")
                        .cursor_pointer()
                        .label(if has_banner { "Change…" } else { "Upload…" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pick_picture(
                                crate::crop::CropFor::Banner(banner_inst.clone()),
                                cx,
                            );
                        })),
                )
                .when(has_banner, |d| {
                    d.child(
                        Button::new("account-banner-remove")
                            .cursor_pointer()
                            .label("Remove")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.call_status(
                                    &unbanner_inst,
                                    "Banner removed",
                                    |r| async move {
                                        r.update_me(&UpdateMeRequest {
                                            banner_id: Some(None),
                                            ..UpdateMeRequest::default()
                                        })
                                        .await
                                    },
                                    cx,
                                );
                            })),
                    )
                }),
        ));
        let password_inst = inst.clone();
        account_rows.push(setting(
            "Password",
            "Changing it signs out your other devices.",
            Button::new("account-password")
                .cursor_pointer()
                .label("Change password…")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_form_password(password_inst.clone(), window, cx);
                })),
        ));

        let mut device_rows: Vec<AnyElement> = Vec::new();
        match &page.sessions {
            Remote::Idle | Remote::Loading => device_rows.push(theme::skeleton("devices", 3)),
            Remote::Failed(err) => device_rows.push(
                div()
                    .text_size(theme::secondary_size())
                    .text_color(theme::danger())
                    .child(format!("Could not load: {err}"))
                    .into_any_element(),
            ),
            Remote::Ready(list) => {
                for s in list {
                    let seen = time::format(s.last_seen_at, "%Y-%m-%d %H:%M", self.utc_offset);
                    let id = s.id;
                    let control: AnyElement = if s.current {
                        muted("This device")
                    } else {
                        Button::new(SharedString::from(format!("session-{id}")))
                            .cursor_pointer()
                            .label("Sign out")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.revoke_sessions(vec![id], cx);
                            }))
                            .into_any_element()
                    };
                    device_rows.push(setting(
                        &s.device_name,
                        &format!("Last active {seen}"),
                        control,
                    ));
                }
            }
        }
        let others: Vec<Id> = page
            .sessions
            .ready()
            .map(|l| l.iter().filter(|s| !s.current).map(|s| s.id).collect())
            .unwrap_or_default();
        if !others.is_empty() {
            device_rows.push(
                Button::new("sessions-others")
                    .cursor_pointer()
                    .danger()
                    .label("Sign out all others")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.revoke_sessions(others.clone(), cx);
                    }))
                    .into_any_element(),
            );
        }
        page_body(
            "Account",
            None,
            vec![
                group("Account", account_rows),
                group("Signed-in devices", device_rows),
                self.security_group(page, &inst, cx),
            ],
        )
    }

    fn slider(&self, page: &SettingsPage, kind: SliderKind) -> AnyElement {
        page.sliders.iter().find(|(k, _)| *k == kind).map_or_else(
            || div().into_any_element(),
            |(_, state)| {
                div()
                    .w_full()
                    .child(Slider::new(state).cursor_pointer())
                    .into_any_element()
            },
        )
    }

    fn tab_appearance(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let config = &self.config;
        let cards = self
            .theme_previews
            .iter()
            .enumerate()
            .map(|(n, (name, loaded))| {
                let chosen = *name == config.general.theme;
                let value = name.clone();
                theme_card(n, name, loaded, chosen).on_click(cx.listener(move |this, _, _, cx| {
                    this.set_theme(value.clone(), cx);
                }))
            });
        let theme_group = group_with(
            "Theme",
            div()
                .flex()
                .gap(px(8.))
                .child(
                    Button::new("themes-folder")
                        .cursor_pointer()
                        .small()
                        .label("Open themes folder")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(paths) = this.core.paths() {
                                let dir = paths.dir.join(theme::THEMES_DIR);
                                let _ = std::fs::create_dir_all(&dir);
                                if let Err(err) = crate::open::path(&dir) {
                                    this.error(
                                        format!("could not open {}: {err}", dir.display()),
                                        cx,
                                    );
                                }
                            }
                        })),
                )
                .child(
                    Button::new("themes-copy")
                        .cursor_pointer()
                        .small()
                        .label("Copy current theme")
                        .on_click(cx.listener(|this, _, _, cx| match this.copy_theme(cx) {
                            Ok(name) => this.set_theme(name, cx),
                            Err(err) => this.error(format!("theme not copied: {err}"), cx),
                        })),
                )
                .into_any_element(),
            vec![
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .children(cards)
                    .into_any_element(),
                muted("Your own themes are .toml files in the themes folder."),
            ],
        );
        let shown_px = i32::from(config.font.size) + 3;
        let text_group = group(
            "Text",
            vec![
                setting(
                    "Font",
                    "Any font installed on this computer.",
                    Select::new(&page.font).cursor_pointer(),
                ),
                setting(
                    "Code font",
                    "For code blocks.",
                    Select::new(&page.mono_font).cursor_pointer(),
                ),
                setting(
                    "Size of titles",
                    "",
                    wrap().children(
                        [(2, "Small"), (4, "Normal"), (8, "Large")]
                            .into_iter()
                            .enumerate()
                            .map(|(n, (extra, name))| {
                                crate::ui::chip(
                                    self,
                                    ("heading-extra", n),
                                    name,
                                    config.font.heading_extra == extra,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.set_config(|c| c.font.heading_extra = extra, cx);
                                    },
                                ))
                            }),
                    ),
                ),
                setting(
                    "Font size",
                    &format!("{shown_px} px"),
                    self.slider(page, SliderKind::FontSize),
                ),
            ],
        );
        let scale = self.ui_scale_draft.unwrap_or(config.font.ui_scale);
        let presets = SCALE_PRESETS.iter().enumerate().map(|(n, value)| {
            let value = *value;
            let chosen =
                (config.font.ui_scale - value).abs() < 0.001 && self.ui_scale_draft.is_none();
            crate::ui::chip(
                self,
                ("scale-preset", n),
                format!("{:.0}%", value * 100.),
                chosen,
                cx,
            )
            .on_click(cx.listener(move |this, _, window, cx| this.set_ui_scale(value, window, cx)))
        });
        let position_chips = [
            (config::SidebarPosition::Left, "Left"),
            (config::SidebarPosition::Right, "Right"),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (value, name))| {
            crate::ui::chip(
                self,
                ("sidebar-position", n),
                name,
                config.sidebar.position == value,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_config(|c| c.sidebar.position = value, cx);
            }))
        });
        let layout_group = group(
            "Layout",
            vec![
                setting(
                    "UI scale",
                    &if self.ui_scale_draft.is_some() {
                        format!("{:.0} %, applied when you let go", scale * 100.)
                    } else {
                        format!("{:.0} % · Ctrl+= and Ctrl+- zoom", scale * 100.)
                    },
                    div()
                        .flex()
                        .flex_col()
                        .w_full()
                        .gap(px(8.))
                        .child(wrap().children(presets))
                        .child(self.slider(page, SliderKind::UiScale)),
                ),
                setting("Sidebar position", "", wrap().children(position_chips)),
                setting(
                    "Panel widths",
                    &format!(
                        "Sidebar {} px, member list {} px",
                        config.sidebar.width, config.sidebar.member_list_width
                    ),
                    Button::new("reset-widths")
                        .cursor_pointer()
                        .label("Reset")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_config(
                                |c| {
                                    c.sidebar.width = 240;
                                    c.sidebar.member_list_width = 232;
                                },
                                cx,
                            );
                        })),
                ),
            ],
        );
        page_body(
            "Appearance",
            None,
            vec![
                theme_group,
                text_group,
                layout_group,
                self.motion_group(page, cx),
            ],
        )
    }

    /// A switch row for a `bool` in the config; use [`flag!`], which names the field once.
    fn flag(
        &self,
        name: &'static str,
        hint: &'static str,
        on: bool,
        id: &'static str,
        set: fn(&mut ClientConfig, bool),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        setting(
            name,
            hint,
            Switch::new(SharedString::from(id))
                .cursor_pointer()
                .checked(on)
                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                    let on = *on;
                    this.set_config(|c| set(c, on), cx);
                })),
        )
    }

    fn tab_chat(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.config.buffer;
        let preview = time::format(
            hermes_client_core::now_ms(),
            &b.timestamp_format,
            self.utc_offset,
        );
        let date_preview = time::format(
            hermes_client_core::now_ms(),
            &b.date_format,
            self.utc_offset,
        );
        page_body(
            "Chat",
            None,
            vec![
                group(
                    "Messages",
                    vec![
                        flag!(self, cx, "Show avatars", "", buffer.show_avatars),
                        flag!(self, cx, "Show image previews", "", buffer.image_previews),
                        flag!(self, cx, "Show who is typing", "", buffer.show_typing),
                        flag!(
                            self,
                            cx,
                            "Format messages",
                            "Off: bold, code blocks and the like show as they were typed.",
                            buffer.markdown
                        ),
                        flag!(
                            self,
                            cx,
                            "Link previews",
                            "Show the title and picture of linked pages. This computer fetches \
                            them itself, so the site sees your address (the Hermes server never \
                            fetches them).",
                            buffer.link_previews
                        ),
                        flag!(
                            self,
                            cx,
                            "Preview while writing",
                            "The message shows formatted above the message box. Also the eye \
                            button next to it.",
                            buffer.composer_preview
                        ),
                        flag!(
                            self,
                            cx,
                            "Enter sends",
                            "Shift+Enter starts a new line.",
                            buffer.enter_sends
                        ),
                    ],
                ),
                group(
                    "Layout",
                    vec![
                        setting(
                            "Group messages",
                            &format!("Within {} minutes", b.group_minutes),
                            self.slider(page, SliderKind::GroupMinutes),
                        ),
                        setting(
                            "Quick reactions",
                            &self.config.buffer.quick_reactions,
                            Button::new("edit-reactions")
                                .cursor_pointer()
                                .label("Change")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_form_quick_reactions(window, cx);
                                })),
                        ),
                        setting(
                            "Status line stays",
                            &format!("{} seconds", self.config.general.status_seconds),
                            wrap().children([3, 5, 10].into_iter().enumerate().map(|(n, secs)| {
                                crate::ui::chip(
                                    self,
                                    ("status-seconds", n),
                                    format!("{secs} s"),
                                    self.config.general.status_seconds == secs,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.set_config(|c| c.general.status_seconds = secs, cx);
                                    },
                                ))
                            })),
                        ),
                        flag!(
                            self,
                            cx,
                            "Compact messages",
                            "Time, a small picture, the name and the text on one line.",
                            buffer.compact
                        ),
                        self.pattern_row(page, Pattern::Time, &format!("Now: {preview}."), cx),
                        self.pattern_row(
                            page,
                            Pattern::Date,
                            &format!("On the line between days. Now: {date_preview}."),
                            cx,
                        ),
                    ],
                ),
                self.sidebar_group(cx),
                self.files_group(page, cx),
            ],
        )
    }

    fn motion_group(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let g = &self.config.general;
        group(
            "Motion",
            vec![
                flag!(
                    self,
                    cx,
                    "Reduce motion",
                    "No sliding or fading.",
                    general.reduce_motion
                ),
                setting(
                    "Motion speed",
                    "How long things take to slide and fade.",
                    wrap().children(theme::MOTION_SPEEDS.into_iter().enumerate().map(
                        |(n, (value, label, _))| {
                            crate::ui::chip(
                                self,
                                ("settings-motion", n),
                                label,
                                g.motion_speed == value,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.set_config(|c| c.general.motion_speed = value, cx);
                                },
                            ))
                        },
                    )),
                ),
                flag!(
                    self,
                    cx,
                    "Smooth scrolling",
                    "The mouse wheel glides instead of jumping.",
                    general.smooth_scroll
                ),
                setting(
                    "Scroll speed",
                    &format!("{:.2} x", g.scroll_speed),
                    self.slider(page, SliderKind::ScrollSpeed),
                ),
                flag!(
                    self,
                    cx,
                    "Animate pictures",
                    "GIFs move while the window has the focus.",
                    general.animate_pictures
                ),
            ],
        )
    }

    fn sidebar_group(&self, cx: &mut Context<Self>) -> AnyElement {
        group(
            "Sidebar",
            vec![
                flag!(self, cx, "Show unread dots", "", sidebar.show_unread_dots),
                flag!(
                    self,
                    cx,
                    "Show mention counts",
                    "",
                    sidebar.show_mention_counts
                ),
                flag!(
                    self,
                    cx,
                    "Show who is in voice channels",
                    "",
                    sidebar.show_voice_users
                ),
            ],
        )
    }

    fn files_group(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let g = &self.config.general;
        let folder = if g.download_dir.is_empty() {
            "The Downloads folder".to_owned()
        } else {
            g.download_dir.clone()
        };
        group(
            "Files",
            vec![
                setting(
                    "Media volume",
                    &format!("{:.0} %", self.config.voice.media_volume * 100.),
                    self.slider(page, SliderKind::MediaVolume),
                ),
                setting(
                    "Download folder",
                    &folder,
                    wrap()
                        .child(
                            Button::new("download-choose")
                                .cursor_pointer()
                                .label("Choose…")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.choose_download_dir(cx)),
                                ),
                        )
                        .child(
                            Button::new("download-reset")
                                .cursor_pointer()
                                .label("Reset")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_config(|c| c.general.download_dir.clear(), cx);
                                })),
                        ),
                ),
                setting(
                    "Size of pictures and videos",
                    &format!("{} %", self.config.buffer.media_scale),
                    wrap().children(
                        [(75, "Small"), (100, "Normal"), (150, "Large")]
                            .into_iter()
                            .enumerate()
                            .map(|(n, (scale, name))| {
                                crate::ui::chip(
                                    self,
                                    ("media-scale", n),
                                    name,
                                    self.config.buffer.media_scale == scale,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.set_config(|c| c.buffer.media_scale = scale, cx);
                                    },
                                ))
                            }),
                    ),
                ),
                setting(
                    "Saved on this device",
                    "Thumbnails come back as needed, the search index fills up again.",
                    wrap()
                        .child(
                            Button::new("clear-thumbs")
                                .cursor_pointer()
                                .label("Clear pictures")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.clear_pictures();
                                    this.info("Pictures cleared", cx);
                                })),
                        )
                        .child(
                            Button::new("clear-index")
                                .cursor_pointer()
                                .label("Clear search index")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    for inst in this.core.instance_ids() {
                                        this.core.clear_search_index(&inst);
                                    }
                                    this.info("Search index cleared", cx);
                                })),
                        ),
                ),
                flag!(
                    self,
                    cx,
                    "Ask where to save",
                    "Choose a place for each download.",
                    general.ask_where_to_save
                ),
            ],
        )
    }

    fn choose_download_dir(&mut self, cx: &mut Context<Self>) {
        self.run(
            cx,
            async {
                rfd::AsyncFileDialog::new()
                    .pick_folder()
                    .await
                    .map(|folder| folder.path().display().to_string())
            },
            |this, picked, _, cx| {
                if let Some(folder) = picked {
                    this.set_config(|c| c.general.download_dir = folder, cx);
                }
            },
        );
    }

    fn is_instance_admin(&self) -> bool {
        let store = self.core.store();
        self.account_instance()
            .and_then(|inst| store.instance(&inst).and_then(|s| s.me.clone()))
            .is_some_and(|me| me.instance_admin)
    }

    fn security_group(
        &self,
        page: &SettingsPage,
        inst: &InstanceId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = page.two_factor == Some(true);
        let (form_inst, export_inst, delete_inst) = (inst.clone(), inst.clone(), inst.clone());
        group(
            "Security and your data",
            vec![
                setting(
                    "Two-factor login",
                    if on {
                        "On: signing in also needs a code."
                    } else {
                        "Signing in can also need a code from an authenticator app."
                    },
                    Button::new("two-factor")
                        .cursor_pointer()
                        .label(if on { "Turn off" } else { "Turn on" })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if on {
                                this.open_form_two_factor_disable(form_inst.clone(), window, cx);
                            } else {
                                this.open_form_two_factor_begin(form_inst.clone(), window, cx);
                            }
                        })),
                ),
                setting(
                    "Connect when Hermes starts",
                    "",
                    Switch::new("autoconnect")
                        .cursor_pointer()
                        .checked(
                            self.config
                                .instances
                                .iter()
                                .find(|e| e.id == *inst)
                                .is_none_or(|e| e.autoconnect),
                        )
                        .on_click(cx.listener({
                            let inst = inst.clone();
                            move |this, on: &bool, _, cx| {
                                if let Err(err) = this.core.set_autoconnect(&inst, *on) {
                                    this.error(err.to_string(), cx);
                                }
                                this.apply_config(cx);
                            }
                        })),
                ),
                setting(
                    "Your data",
                    "A file with your profile, servers and messages as the server holds them.",
                    Button::new("export-account")
                        .cursor_pointer()
                        .label("Export…")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.export_account(export_inst.clone(), cx);
                        })),
                ),
                setting(
                    "Delete account",
                    "This cannot be undone.",
                    Button::new("delete-account")
                        .cursor_pointer()
                        .danger()
                        .label("Delete…")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_form_delete_account(delete_inst.clone(), window, cx);
                        })),
                ),
            ],
        )
    }

    fn export_account(&mut self, inst: InstanceId, cx: &mut Context<Self>) {
        let core = self.core.clone();
        self.run_status(cx, async move {
            let file = rfd::AsyncFileDialog::new()
                .set_file_name("hermes-export.json")
                .save_file()
                .await;
            let Some(file) = file else {
                return Ok("Export cancelled".to_owned());
            };
            let text = core.rest(&inst)?.export_account().await?;
            let path = file.path().to_path_buf();
            let written = path.clone();
            tokio::task::spawn_blocking(move || std::fs::write(written, text))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            Ok(format!("Saved to {}", path.display()))
        });
    }

    fn load_admin(&mut self, cx: &mut Context<Self>) {
        let Some(inst) = self.account_instance() else {
            return;
        };
        if let Some(page) = &mut self.settings {
            page.admin_users = Remote::Loading;
            page.admin_log = Remote::Loading;
        }
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                let rest = core.rest(&inst)?;
                let users = rest.admin_users(None, 200).await?;
                let log = rest.admin_log().await?;
                let settings = rest.admin_settings().await?;
                Ok::<_, String>((users, log, settings))
            },
            |this, result, _, cx| {
                if let Some(page) = &mut this.settings {
                    match result {
                        Ok((users, log, settings)) => {
                            page.admin_users = Remote::Ready(users);
                            page.admin_log = Remote::Ready(log);
                            page.admin_settings = Some(settings);
                        }
                        Err(err) => {
                            page.admin_users = Remote::Failed(err.clone());
                            page.admin_log = Remote::Failed(err);
                        }
                    }
                }
                cx.notify();
            },
        );
    }

    fn tab_admin(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        use hermes_client_core::proto::api::{AdminSettings, RegistrationMode as Mode};
        let Some(inst) = self.account_instance() else {
            return muted("Add an instance first.");
        };
        let current = page.admin_settings.map(|s| s.registration_mode);
        let chips = [
            (Mode::Open, "Open"),
            (Mode::InviteOnly, "Invite only"),
            (Mode::Closed, "Closed"),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (mode, name))| {
            let inst = inst.clone();
            crate::ui::chip(self, ("registration", n), name, current == Some(mode), cx).on_click(
                cx.listener(move |this, _, _, cx| {
                    let core = this.core.clone();
                    let inst = inst.clone();
                    this.run(
                        cx,
                        async move {
                            core.rest(&inst)?
                                .set_admin_settings(&AdminSettings {
                                    registration_mode: mode,
                                })
                                .await
                                .map_err(|e| e.to_string())
                        },
                        |this, result, _, cx| match result {
                            Ok(_) => this.load_admin(cx),
                            Err(err) => this.error(err, cx),
                        },
                    );
                }),
            )
        });
        let mut users: Vec<AnyElement> = Vec::new();
        match &page.admin_users {
            Remote::Idle | Remote::Loading => users.push(theme::skeleton("users", 4)),
            Remote::Failed(err) => users.push(muted(err.clone())),
            Remote::Ready(list) => {
                for entry in list {
                    let id = entry.user.id;
                    let flags = match (entry.instance_admin, entry.disabled) {
                        (true, _) => "admin",
                        (_, true) => "disabled",
                        _ => "",
                    };
                    let disabled = entry.disabled;
                    let admin = entry.instance_admin;
                    let (disable_inst, admin_inst) = (inst.clone(), inst.clone());
                    users.push(setting(
                        &entry.user.handle(),
                        flags,
                        wrap()
                            .child(
                                Button::new(SharedString::from(format!("admin-disable-{id}")))
                                    .cursor_pointer()
                                    .label(if disabled { "Enable" } else { "Disable" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.admin_patch(
                                            disable_inst.clone(),
                                            id,
                                            Some(!disabled),
                                            None,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!("admin-admin-{id}")))
                                    .cursor_pointer()
                                    .label(if admin { "Remove admin" } else { "Make admin" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.admin_patch(
                                            admin_inst.clone(),
                                            id,
                                            None,
                                            Some(!admin),
                                            cx,
                                        );
                                    })),
                            ),
                    ));
                }
            }
        }
        let mut log: Vec<AnyElement> = Vec::new();
        match &page.admin_log {
            Remote::Idle | Remote::Loading => log.push(theme::skeleton("log", 5)),
            Remote::Failed(err) => log.push(muted(err.clone())),
            Remote::Ready(list) if list.is_empty() => log.push(muted("Nothing yet.")),
            Remote::Ready(list) => {
                for entry in list {
                    log.push(muted(format!(
                        "{} · {} · {}",
                        time::format(entry.created_at, "%Y-%m-%d %H:%M", self.utc_offset),
                        entry.action,
                        entry.details
                    )));
                }
            }
        }
        page_body(
            "Instance",
            None,
            vec![
                group(
                    "Who can make an account",
                    vec![
                        setting(
                            "Registration",
                            "Takes effect at once, and over the config file.",
                            wrap().children(chips),
                        ),
                        setting(
                            "Channels must be encrypted",
                            "Set in the server's config file.",
                            muted(
                                if self
                                    .core
                                    .store()
                                    .instance(&inst)
                                    .and_then(|s| {
                                        s.instance.as_ref().map(|i| i.limits.channels_encrypted)
                                    })
                                    .unwrap_or(false)
                                {
                                    "Yes"
                                } else {
                                    "No"
                                },
                            ),
                        ),
                    ],
                ),
                group("Accounts", users),
                group("Log", log),
            ],
        )
    }

    fn admin_patch(
        &mut self,
        inst: InstanceId,
        user: Id,
        disabled: Option<bool>,
        instance_admin: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                core.rest(&inst)?
                    .admin_update_user(
                        user,
                        &hermes_client_core::proto::api::AdminUserPatch {
                            disabled,
                            instance_admin,
                            tagless: None,
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())
            },
            |this, result, _, cx| match result {
                Ok(()) => this.load_admin(cx),
                Err(err) => this.error(err, cx),
            },
        );
    }

    /// The account's server-side privacy and this device's keys, for the Privacy tab; empty when
    /// not signed in.
    fn account_privacy_groups(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(inst) = self.account_instance() else {
            return Vec::new();
        };
        let privacy = {
            let store = self.core.store();
            match store.instance(&inst) {
                Some(s) if s.me.is_some() => s.privacy,
                _ => return Vec::new(),
            }
        };
        let many = self.core.instance_ids().len() > 1;
        let dm_chips = [
            (DmFrom::Everyone, "Everyone"),
            (DmFrom::Spaces, "People in my servers"),
            (DmFrom::Nobody, "Nobody"),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (value, name))| {
            crate::ui::chip(self, ("dm-from", n), name, privacy.dm_from == value, cx).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.set_privacy(
                        UpdatePrivacyRequest {
                            dm_from: Some(value),
                            ..UpdatePrivacyRequest::default()
                        },
                        cx,
                    );
                }),
            )
        });
        let privacy_rows = vec![
            setting(
                "Direct messages from",
                "Who can start a conversation with you.",
                wrap().children(dm_chips),
            ),
            setting(
                "Message requests",
                "Messages from anyone else wait in Requests. Off: they cannot message you.",
                Switch::new("allow-requests")
                    .cursor_pointer()
                    .checked(privacy.message_requests)
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.set_privacy(
                            UpdatePrivacyRequest {
                                message_requests: Some(*on),
                                ..UpdatePrivacyRequest::default()
                            },
                            cx,
                        );
                    })),
            ),
            setting(
                "Show when I was last online",
                "On your profile, for people who share a server with you and your friends.",
                Switch::new("show-last-online")
                    .cursor_pointer()
                    .checked(privacy.show_last_online)
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.set_privacy(
                            UpdatePrivacyRequest {
                                show_last_online: Some(*on),
                                ..UpdatePrivacyRequest::default()
                            },
                            cx,
                        );
                    })),
            ),
        ];
        let title = if many {
            format!("Who can reach you on {inst}")
        } else {
            "Who can reach you".to_owned()
        };
        vec![
            group(&title, privacy_rows),
            group(
                "End-to-end encryption",
                vec![setting(
                    "This device",
                    "Only your and their devices can read DMs, files and calls.",
                    muted(
                        self.core
                            .my_device_id(&inst)
                            .map_or_else(String::new, |d| d.chars().take(16).collect::<String>()),
                    ),
                )],
            ),
        ]
    }

    fn tab_privacy(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut groups = self.account_privacy_groups(cx);
        let device = group(
            "What this device tells the server",
            vec![
                flag!(
                    self,
                    cx,
                    "Show others when I am typing",
                    "",
                    privacy.send_typing
                ),
                flag!(
                    self,
                    cx,
                    "Sync what I have read",
                    "Off: the server is not told, and your other devices will not know either.",
                    privacy.send_read_marks
                ),
                setting(
                    "Close the microphone while muted",
                    "The system then shows it as unused. Unmuting takes a moment longer.",
                    Switch::new("close-mic")
                        .cursor_pointer()
                        .checked(self.config.voice.close_mic_when_muted)
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            let on = *on;
                            this.change_voice(cx, |voice| voice.close_mic_when_muted = on);
                        })),
                ),
                flag!(
                    self,
                    cx,
                    "Search on this device only",
                    "Older unencrypted messages are then not found.",
                    privacy.local_search_only
                ),
                setting(
                    "Warn about unencrypted chats",
                    "A popup when opening a chat the server can read. The note above \
                        the message box is always there.",
                    self.warn_unencrypted_choices("settings-warn-unencrypted", cx),
                ),
            ],
        );
        // Before the keys: what the server is told comes with who can reach you.
        let at = groups.len().min(1);
        groups.insert(at, device);
        page_body("Privacy & safety", None, groups)
    }

    fn tab_window(&self, cx: &mut Context<Self>) -> AnyElement {
        let g = &self.config.general;
        page_body(
            "Window & startup",
            None,
            vec![group(
                "Window",
                vec![
                    flag!(
                        self,
                        cx,
                        "Close to the tray",
                        "Closing the window keeps Hermes running in the tray.",
                        general.close_to_tray
                    ),
                    setting(
                        "Start when I sign in",
                        "Hermes opens by itself when you sign in to this computer.",
                        Switch::new("start-on-login")
                            .cursor_pointer()
                            .checked(g.start_on_login)
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                let on = *on;
                                match crate::desktop::set_start_on_login(on) {
                                    Ok(()) => {
                                        this.set_config(|c| c.general.start_on_login = on, cx)
                                    }
                                    Err(err) => this.error(err, cx),
                                }
                            })),
                    ),
                    setting(
                        "Go idle after",
                        "With nothing touched for this long.",
                        wrap().children(
                            [(0, "Never"), (5, "5 min"), (10, "10 min"), (30, "30 min")]
                                .into_iter()
                                .enumerate()
                                .map(|(n, (minutes, name))| {
                                    crate::ui::chip(
                                        self,
                                        ("idle-after", n),
                                        name,
                                        g.auto_idle_minutes == minutes,
                                        cx,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.set_config(
                                                |c| c.general.auto_idle_minutes = minutes,
                                                cx,
                                            );
                                        },
                                    ))
                                }),
                        ),
                    ),
                    flag!(
                        self,
                        cx,
                        "Start minimized to the tray",
                        "",
                        general.start_minimized
                    ),
                    flag!(
                        self,
                        cx,
                        "Check for updates on start",
                        "",
                        general.check_for_updates
                    ),
                    flag!(
                        self,
                        cx,
                        "Install updates by themselves",
                        "A newer version found on start is downloaded (only what changed) \
                             and Hermes restarts into it.",
                        general.auto_update
                    ),
                    setting(
                        "Check for updates now",
                        "",
                        Button::new("update-now")
                            .cursor_pointer()
                            .label("Check")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.check_for_update_now(cx);
                            })),
                    ),
                ],
            )],
        )
    }

    fn tab_screen_share(&self, cx: &mut Context<Self>) -> AnyElement {
        let v = &self.config.voice;
        // Before the list has loaded (or where capture is unavailable): the primary screen only.
        let screens: Vec<(String, String)> = if self.screen_list.is_empty() {
            vec![(String::new(), "Screen 1 (primary)".to_owned())]
        } else {
            self.screen_list
                .iter()
                .map(|s| (s.value.clone(), s.label()))
                .collect()
        };
        let screen_chips = screens.into_iter().enumerate().map(|(n, (screen, name))| {
            crate::ui::chip(
                self,
                ("settings-screen", n),
                name,
                screen == v.share_screen,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let screen = screen.clone();
                this.set_config(|c| c.voice.share_screen = screen, cx);
            }))
        });
        let res_chips = ShareResolution::ALL
            .iter()
            .copied()
            .enumerate()
            .map(|(n, value)| {
                let name = match value {
                    ShareResolution::Native => "Native",
                    _ => value.as_str(),
                };
                crate::ui::chip(
                    self,
                    ("settings-res", n),
                    name,
                    v.share_resolution == value,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_config(|c| c.voice.share_resolution = value, cx);
                }))
            });
        let fps_chips = FRAME_RATES.into_iter().enumerate().map(|(n, fps)| {
            crate::ui::chip(
                self,
                ("settings-fps", n),
                format!("{fps} fps"),
                v.share_fps == fps,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_config(|c| c.voice.share_fps = fps, cx);
            }))
        });
        let current = Preference::from_config(v.share_preference);
        let pref_chips = PREFERENCES.into_iter().enumerate().map(|(n, preference)| {
            crate::ui::chip(
                self,
                ("settings-pref", n),
                preference.label(),
                preference == current,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_config(|c| c.voice.share_preference = preference.config_value(), cx);
                if let Some(capture) = &this.capture {
                    capture.set_preference(preference);
                }
            }))
        });
        let sharing = group(
            "What you share",
            vec![
                setting("Screen to share", "", wrap().children(screen_chips)),
                setting(
                    "Resolution",
                    "From your next share",
                    wrap().children(res_chips),
                ),
                setting(
                    "Frame rate",
                    "From your next share",
                    wrap().children(fps_chips),
                ),
                setting(
                    "Share sound",
                    "Others hear what your computer plays, but not this call. From your next \
                    share; needs Windows 10 version 2004 or later.",
                    Switch::new("share-sound")
                        .cursor_pointer()
                        .checked(v.share_sound)
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.set_config(|c| c.voice.share_sound = *on, cx);
                        })),
                ),
                setting(
                    "When it cannot keep up",
                    "What your share keeps when it falls behind",
                    wrap().children(pref_chips),
                ),
            ],
        );
        page_body(
            "Screen sharing",
            Some("These are also chosen in the share picker each time you share."),
            vec![sharing],
        )
    }

    fn tab_voice(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let v = &self.config.voice;
        let device_select = |state: Option<&Entity<SelectState<Vec<DeviceChoice>>>>| {
            state.map_or_else(
                || muted("Listing devices…"),
                |state| Select::new(state).cursor_pointer().into_any_element(),
            )
        };
        let selects = page.device_selects.as_ref();
        let devices = group(
            "Devices",
            vec![
                setting("Microphone", "", device_select(selects.map(|s| &s.input))),
                setting("Speakers", "", device_select(selects.map(|s| &s.output))),
                setting(
                    "Device list",
                    "",
                    Button::new("devices-refresh")
                        .cursor_pointer()
                        .label("Refresh devices")
                        .on_click(cx.listener(|this, _, _, cx| this.load_devices(cx))),
                ),
            ],
        );
        let volumes = group(
            "Volume",
            vec![
                setting(
                    "Microphone volume",
                    &hermes_client_core::audio::volume_label(v.input_volume as f32),
                    self.slider(page, SliderKind::InputVolume),
                ),
                setting(
                    "Output volume",
                    &hermes_client_core::audio::volume_label(v.output_volume as f32),
                    self.slider(page, SliderKind::OutputVolume),
                ),
            ],
        );
        let ptt_mode = v.mode == config::VoiceMode::PushToTalk;
        let mode_chips = [
            (config::VoiceMode::VoiceActivity, "Voice activity"),
            (config::VoiceMode::PushToTalk, "Push-to-talk"),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (value, name))| {
            crate::ui::chip(self, ("voice-mode", n), name, v.mode == value, cx).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.change_voice(cx, move |voice| voice.mode = value);
                }),
            )
        });
        // The bar moves smoothly; it is green while the microphone would go through, as the
        // input mode decides: above the line, or while push-to-talk is held.
        let level = self.meter_shown.clamp(METER_MIN, 0.);
        let open = self.meter_open;
        let fill = (level - METER_MIN) / -METER_MIN;
        let meter = div()
            .w_full()
            .h(px(6.))
            .rounded(px(3.))
            .bg(theme::hover())
            .child(
                div()
                    .h_full()
                    .rounded(px(3.))
                    .w(gpui_kit::relative(fill))
                    // Green while the voice would go through, blending so it does not flicker.
                    .with_spring(
                        "meter-open",
                        gpui_kit::SpringAnimation::new(crate::theme::paced(
                            gpui_kit::SpringConfig::new(260., 34., 1.),
                        ))
                        .to(open),
                        |d, phase| {
                            d.bg(theme::mix(
                                theme::text_tertiary(),
                                theme::speaking(),
                                phase.0.clamp(0., 1.),
                            ))
                        },
                    ),
            );
        let mut input_rows = vec![setting("Input mode", "", wrap().children(mode_chips))];
        if ptt_mode {
            input_rows.push(setting(
                "Push-to-talk key",
                if cfg!(windows) {
                    "e.g. Mouse4, F13, Ctrl+Space. Enter saves."
                } else {
                    "Only while Hermes is focused. Enter saves."
                },
                Input::new(&page.ptt_key),
            ));
            input_rows.push(setting(
                "Release delay",
                &format!("{} ms", v.ptt_release_delay_ms),
                self.slider(page, SliderKind::ReleaseDelay),
            ));
        }
        if ptt_mode {
            // Sensitivity does nothing here: the key decides.
            input_rows.push(setting(
                "Microphone level",
                &format!("Green while your push-to-talk key is held · now {level:.0} dBFS"),
                meter,
            ));
        } else {
            input_rows.push(setting(
                "Voice activity sensitivity",
                &format!("Opens above {} dBFS · now {level:.0}", v.vad_sensitivity),
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(px(6.))
                    .child(meter)
                    .child(self.slider(page, SliderKind::Vad)),
            ));
        }
        if !ptt_mode {
            input_rows.push(setting(
                "Stay open after speaking",
                &format!("{} ms, so the ends of words are not cut off", v.vad_hold_ms),
                self.slider(page, SliderKind::VadHold),
            ));
            input_rows.push(setting(
                "Only loudness opens the microphone",
                "Noise suppression's voice detection can't open it below the line.",
                Switch::new("vad-level-only")
                    .cursor_pointer()
                    .checked(v.vad_level_only)
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        let on = *on;
                        this.change_voice(cx, move |voice| voice.vad_level_only = on);
                    })),
            ));
        }
        let mic_test = page.mic_test;
        input_rows.push(setting(
            "Mic test",
            "Hear yourself (use headphones).",
            Button::new("mic-test")
                .cursor_pointer()
                .label(if mic_test { "Stop test" } else { "Start test" })
                .when(mic_test, |b| b.danger())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(page) = &mut this.settings {
                        page.mic_test = !mic_test;
                    }
                    this.core.set_mic_test(!mic_test);
                    cx.notify();
                })),
        ));
        let voice_switch = |id: &'static str,
                            on: bool,
                            set: fn(&mut hermes_client_core::config::Voice, bool),
                            cx: &mut Context<Self>| {
            Switch::new(id)
                .cursor_pointer()
                .checked(on)
                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                    let on = *on;
                    this.change_voice(cx, move |voice| set(voice, on));
                }))
        };
        let processing = self.fold_group(
            "processing",
            "Processing",
            vec![
                setting(
                    "Rebuild lost sound",
                    "Sends a little extra, so a dropped packet can be recovered.",
                    Switch::new("opus-fec")
                        .cursor_pointer()
                        .checked(v.opus_fec)
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            let on = *on;
                            this.change_voice(cx, |voice| voice.opus_fec = on);
                        })),
                ),
                setting(
                    "Send nothing during silence",
                    "Saves bandwidth.",
                    Switch::new("opus-dtx")
                        .cursor_pointer()
                        .checked(v.opus_dtx)
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            let on = *on;
                            this.change_voice(cx, |voice| voice.opus_dtx = on);
                        })),
                ),
                setting(
                    "Noise suppression",
                    if v.noise_suppression {
                        "Removes fans, keyboards and the like."
                    } else {
                        "Without it, headphones are recommended."
                    },
                    voice_switch(
                        "noise",
                        v.noise_suppression,
                        |v, on| v.noise_suppression = on,
                        cx,
                    ),
                ),
                setting(
                    "Echo cancellation",
                    "For speakers or a laptop's own microphone: others don't hear \
                    themselves back. Not needed with headphones.",
                    voice_switch(
                        "echo",
                        v.echo_cancellation,
                        |v, on| v.echo_cancellation = on,
                        cx,
                    ),
                ),
                setting(
                    "Automatic gain",
                    "Evens out how loud you are.",
                    voice_switch("gain", v.auto_gain, |v, on| v.auto_gain = on, cx),
                ),
                setting(
                    "Lower other sound while someone speaks",
                    "Screen share sound, videos and audio files step back for voices.",
                    voice_switch("ducking", v.ducking, |v, on| v.ducking = on, cx),
                ),
                setting(
                    "Lowered by",
                    &format!("{} dB", v.ducking_db),
                    self.slider(page, SliderKind::Ducking),
                ),
                setting(
                    "Automatic gain level",
                    &format!("Aims for {} dBFS. Higher is louder.", v.agc_target_db),
                    self.slider(page, SliderKind::AgcTarget),
                ),
            ],
            cx,
        );
        let join_chips = [
            (config::JoinOn::DoubleClick, "Double-click"),
            (config::JoinOn::SingleClick, "Single click"),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (value, name))| {
            crate::ui::chip(self, ("join-on", n), name, v.join_on == value, cx).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.set_config(|c| c.voice.join_on = value, cx);
                }),
            )
        })
        .collect::<Vec<_>>();
        let sounds = self.fold_group(
            "sounds",
            "Sounds and joining",
            vec![
                setting(
                    "Play sounds",
                    "When people join or leave, and when you mute.",
                    voice_switch("sounds", v.sounds, |v, on| v.sounds = on, cx),
                ),
                setting(
                    "Sound volume",
                    &format!("{:.0} %", v.sounds_volume * 100.),
                    self.slider(page, SliderKind::SoundsVolume),
                ),
                setting("Join a voice channel with", "", wrap().children(join_chips)),
            ],
            cx,
        );
        page_body(
            "Voice & Audio",
            None,
            vec![
                devices,
                volumes,
                group("Input", input_rows),
                processing,
                sounds,
            ],
        )
    }

    fn tab_notifications(&self, cx: &mut Context<Self>) -> AnyElement {
        let n = &self.config.notifications;
        page_body(
            "Notifications",
            Some("Per server and channel: right-click it, Notifications."),
            vec![group(
                "Desktop notifications",
                vec![
                    flag!(self, cx, "Show notifications", "", notifications.enabled),
                    flag!(
                        self,
                        cx,
                        "Show the message text",
                        "Off: only who wrote, and where.",
                        notifications.show_preview
                    ),
                    flag!(self, cx, "Play a sound", "", notifications.sound),
                    setting(
                        "Test sounds",
                        "Messages and DMs play the softer one, mentions the ping.",
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(
                                Button::new("test-sound-message")
                                    .cursor_pointer()
                                    .label("Message")
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.core.play_sound(Sound::Message);
                                    })),
                            )
                            .child(
                                Button::new("test-sound-mention")
                                    .cursor_pointer()
                                    .label("Mention")
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.core.play_sound(Sound::Ping);
                                    })),
                            ),
                    ),
                    setting(
                        "Quiet hours",
                        &if n.quiet_from.is_empty() {
                            "None".to_owned()
                        } else {
                            format!("{} to {}", n.quiet_from, n.quiet_to)
                        },
                        Button::new("quiet-hours")
                            .cursor_pointer()
                            .label("Change")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_form_quiet_hours(window, cx);
                            })),
                    ),
                    setting(
                        "Words that notify me",
                        &if n.keywords.is_empty() {
                            "None".to_owned()
                        } else {
                            n.keywords.clone()
                        },
                        Button::new("keywords")
                            .cursor_pointer()
                            .label("Change")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_form_keywords(window, cx);
                            })),
                    ),
                    flag!(self, cx, "Pop up for DMs", "", notifications.toast_dms),
                    flag!(
                        self,
                        cx,
                        "Pop up for mentions",
                        "",
                        notifications.toast_mentions
                    ),
                    flag!(
                        self,
                        cx,
                        "Pop up for other messages",
                        "",
                        notifications.toast_other
                    ),
                    flag!(self, cx, "Sound for DMs", "", notifications.sound_dms),
                    flag!(
                        self,
                        cx,
                        "Sound for mentions",
                        "",
                        notifications.sound_mentions
                    ),
                    flag!(
                        self,
                        cx,
                        "Sound for other messages",
                        "",
                        notifications.sound_other
                    ),
                    flag!(
                        self,
                        cx,
                        "Flash the taskbar button",
                        "",
                        notifications.flash_taskbar
                    ),
                ],
            )],
        )
    }

    /// A key pressed while a keybind waits for one. Whether it was taken.
    pub fn capture_key(&mut self, keystroke: &gpui_kit::Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(page) = self.settings.as_mut() else {
            return false;
        };
        let Some(name) = page.capturing_key else {
            return false;
        };
        page.capturing_key = None;
        if keystroke.key == "escape" {
            cx.notify();
            return true;
        }
        let m = keystroke.modifiers;
        let mut combo = String::new();
        for (on, part) in [(m.control, "Ctrl+"), (m.shift, "Shift+"), (m.alt, "Alt+")] {
            if on {
                combo.push_str(part);
            }
        }
        combo.push_str(&keystroke.key.to_uppercase());
        let taken = ACTIONS
            .iter()
            .filter(|(_, other, ..)| *other != name)
            .find(|(action, ..)| {
                Keybinds::combo_text(&self.config.keys, *action).is_some_and(|text| text == combo)
            })
            .map(|(.., title)| *title);
        if let Some(title) = taken {
            self.warn(format!("{combo} was already used for {title}"), cx);
        }
        self.set_config(|c| _ = c.keys.insert(name.to_owned(), combo), cx);
        true
    }

    fn tab_keybinds(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let mut groups: Vec<AnyElement> = KEY_GROUPS
            .iter()
            .map(|(title, actions)| {
                let rows = actions
                    .iter()
                    .filter_map(|action| ACTIONS.iter().find(|(a, ..)| a == action))
                    .map(|(action, name, default, title)| {
                        let combo =
                            Keybinds::combo_text(&self.config.keys, *action).unwrap_or_default();
                        let (name, default): (&'static str, &'static str) = (name, default);
                        let waiting = page.capturing_key == Some(name);
                        setting(
                            title,
                            if waiting {
                                "Press the keys; Escape cancels."
                            } else {
                                ""
                            },
                            wrap()
                                .child(key_caps(&combo))
                                .child(
                                    Button::new(SharedString::from(format!("bind-{name}")))
                                        .cursor_pointer()
                                        .label(if waiting { "Waiting…" } else { "Change" })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(page) = &mut this.settings {
                                                page.capturing_key = Some(name);
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("unbind-{name}")))
                                        .cursor_pointer()
                                        .label("Reset")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.set_config(
                                                |c| {
                                                    c.keys.insert(
                                                        name.to_owned(),
                                                        default.to_owned(),
                                                    );
                                                },
                                                cx,
                                            );
                                        })),
                                ),
                        )
                    })
                    .collect();
                group(title, rows)
            })
            .collect();
        groups.push(group(
            "Everywhere",
            vec![muted(
                "Right-click anything for more. Drag channels and roles to reorder them.",
            )],
        ));
        groups.push(group(
            "These do not change",
            vec![
                setting("Close what is on top", "", key_caps("Escape")),
                setting("Copy what is selected", "", key_caps("Ctrl+C")),
                setting("Open the menu", "", key_caps("Shift+F10")),
                setting("Edit your last message", "", key_caps("Up")),
            ],
        ));
        page_body(
            "Keybinds",
            Some("Click Change, then press the keys."),
            groups,
        )
    }

    /// Every setting of every tab (name, hint, group), in the order shown: the tabs are built
    /// with their rows noted instead of drawn.
    fn settings_index(&self, cx: &mut Context<Self>) -> Vec<(Tab, Found)> {
        let Some(page) = self.settings.as_ref() else {
            return Vec::new();
        };
        let admin = self.is_instance_admin();
        let mut out = Vec::new();
        for (tab, _) in all_tabs() {
            if (tab == Tab::Admin && !admin) || tab == Tab::Diagnostics {
                continue;
            }
            RECORDING.with(|r| *r.borrow_mut() = Some(Recording::default()));
            let _ = match tab {
                Tab::Account => self.tab_account(page, cx),
                Tab::Privacy => self.tab_privacy(cx),
                Tab::Appearance => self.tab_appearance(page, cx),
                Tab::Chat => self.tab_chat(page, cx),
                Tab::Notifications => self.tab_notifications(cx),
                Tab::Window => self.tab_window(cx),
                Tab::Keybinds => self.tab_keybinds(page, cx),
                Tab::Voice => self.tab_voice(page, cx),
                Tab::ScreenShare => self.tab_screen_share(cx),
                Tab::Admin => self.tab_admin(page, cx),
                Tab::About => self.tab_about(cx),
                Tab::Diagnostics => div().into_any_element(),
            };
            let found = RECORDING
                .with(|r| r.borrow_mut().take())
                .unwrap_or_default();
            out.extend(found.found.into_iter().map(|f| (tab, f)));
        }
        out
    }

    /// The settings matching what is typed in the search: every word somewhere in the tab,
    /// group, name or hint; those with all words in the name first.
    fn settings_found(&self, cx: &mut Context<Self>) -> Vec<(Tab, Found)> {
        let Some(page) = self.settings.as_ref() else {
            return Vec::new();
        };
        let query = page.search.read(cx).value().trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let words: Vec<&str> = query.split_whitespace().collect();
        let mut found: Vec<(bool, Tab, Found)> = self
            .settings_index(cx)
            .into_iter()
            .filter_map(|(tab, f)| {
                let name = f.name.to_lowercase();
                let all = format!(
                    "{} {} {name} {}",
                    tab_name(tab).to_lowercase(),
                    f.group.to_lowercase(),
                    f.hint.to_lowercase()
                );
                words
                    .iter()
                    .all(|w| all.contains(w))
                    .then(|| (!words.iter().all(|w| name.contains(w)), tab, f))
            })
            .collect();
        found.sort_by_key(|(weaker, ..)| *weaker);
        found.into_iter().map(|(_, tab, f)| (tab, f)).collect()
    }

    fn search_results(&self, query: &str, cx: &mut Context<Self>) -> AnyElement {
        let found = self.settings_found(cx);
        let count = match found.len() {
            0 => format!("Nothing found for \u{201c}{query}\u{201d}."),
            1 => "1 setting".to_owned(),
            n => format!("{n} settings"),
        };
        let rows = found
            .into_iter()
            .enumerate()
            .map(|(n, (tab, f))| {
                let place = format!("{} \u{203a} {}", tab_name(tab), f.group);
                let (name, hint) = (f.name.clone(), f.hint.clone());
                div()
                    .id(("settings-found", n))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(theme::row_radius())
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::hover()))
                    .child(div().text_size(theme::body_size()).child(name))
                    .child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::accent())
                            .child(place),
                    )
                    .when(!hint.is_empty(), |d| d.child(muted(hint)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_found(tab, f.clone(), window, cx);
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let list = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(2.))
            .max_w(px(760.))
            .children(rows)
            .into_any_element();
        page_body("Search", Some(&count), vec![list])
    }

    /// Goes to a setting found by the search: its tab opens on its group (unfolded), and the row
    /// lights up for a moment.
    fn open_found(&mut self, tab: Tab, found: Found, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(page) = &self.settings {
            page.search
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        if let Some(key) = found.fold
            && self.config.general.folded_settings.iter().any(|k| k == key)
        {
            self.set_folded(key, false, cx);
        }
        self.settings_open_tab(tab, cx);
        LIT.with(|l| *l.borrow_mut() = Some((found.name, std::time::Instant::now())));
        SCROLL_TO.with(|s| s.set(Some(found.index)));
        cx.notify();
    }

    fn set_folded(&mut self, key: &str, folded: bool, cx: &mut Context<Self>) {
        let mut keys = self.config.general.folded_settings.clone();
        keys.retain(|k| k != key);
        if folded {
            keys.push(key.to_owned());
        }
        self.set_config(|c| c.general.folded_settings = keys, cx);
    }

    /// A group of settings that folds into its title (remembered under `key`): for long groups
    /// most people never change.
    fn fold_group(
        &self,
        key: &'static str,
        title: &str,
        rows: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let folded = self.config.general.folded_settings.iter().any(|k| k == key);
        let count = rows.len();
        let toggle = div()
            .id(SharedString::from(format!("fold-{key}")))
            .flex()
            .items_center()
            .gap(px(4.))
            .cursor_pointer()
            .text_size(theme::caption_size())
            .text_color(theme::text_tertiary())
            .hover(|d| d.text_color(theme::accent()))
            .child(if folded {
                format!("Show {count}")
            } else {
                "Hide".to_owned()
            })
            .child(crate::ui::chevron(
                icon(assets::CHEVRON_DOWN, 14., theme::text_tertiary()),
                format!("fold-chevron-{key}"),
                !folded,
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_folded(key, !folded, cx);
            }))
            .into_any_element();
        let note_fold = || {
            RECORDING.with(|r| {
                if let Some(r) = r.borrow_mut().as_mut() {
                    r.fold_last(key);
                }
            });
        };
        if folded && !recording() {
            return group_with(title, toggle, Vec::new());
        }
        let header = group_header(title, toggle);
        // Opening grows the card from nothing, so the groups below glide down.
        let card = crate::channel::grow_in(
            &self.grow_heights,
            SharedString::from(format!("fold-open-{key}")),
            group_card(rows).into_any_element(),
        );
        let shown = group_column(header, Some(card));
        note_fold();
        shown
    }

    fn tab_about(&self, cx: &mut Context<Self>) -> AnyElement {
        let header = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .size(px(56.))
                    .rounded(theme::radius_large())
                    .bg(theme::accent_soft())
                    .child(icon(assets::FEATHER, 28., theme::accent())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(theme::heading_size())
                            .font_weight(FontWeight::MEDIUM)
                            .child("Hermes"),
                    )
                    .child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(format!(
                                "Version {} \u{b7} {} {}",
                                env!("CARGO_PKG_VERSION"),
                                std::env::consts::OS,
                                std::env::consts::ARCH
                            )),
                    )
                    .child(
                        div()
                            .text_size(theme::secondary_size())
                            .text_color(theme::text_secondary())
                            .child("Self-hostable chat with text, voice and screen sharing."),
                    ),
            )
            .into_any_element();
        let licence = setting(
            "Licence",
            "The app is GPL-3.0-or-later; the server is AGPL-3.0-or-later.",
            div(),
        );
        let debug = setting(
            "Debug info",
            "Version, system and connection state for bug reports. No tokens or messages.",
            Button::new("about-debug")
                .cursor_pointer()
                .label(self.copy_label("about-debug", "Copy"))
                .on_click(cx.listener(|this, _, _, cx| {
                    let text = this.debug_info();
                    this.copy_from("about-debug", text, "Debug info", cx);
                })),
        );
        let logs = setting(
            "Logs",
            "Dated log files from this device.",
            Button::new("about-logs")
                .cursor_pointer()
                .label("Open folder")
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(paths) = this.core.paths() {
                        let dir = paths.dir.join("logs");
                        let _ = std::fs::create_dir_all(&dir);
                        if let Err(err) = crate::open::path(&dir) {
                            this.error(format!("could not open {}: {err}", dir.display()), cx);
                        }
                    }
                })),
        );
        let project = setting(
            "Project page",
            "Source code, issues and releases.",
            Button::new("about-project")
                .cursor_pointer()
                .label("Open")
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Err(err) = crate::open::url(env!("CARGO_PKG_REPOSITORY")) {
                        this.error(format!("could not open the browser: {err}"), cx);
                    }
                })),
        );
        page_body(
            "About",
            None,
            vec![
                group("Hermes", vec![header, licence]),
                group("Help", vec![debug, logs, project]),
            ],
        )
    }
}

/// The date formats offered as chips on the day dividers.
const DATE_FORMATS: [&str; 5] = [
    "%B %e, %Y",
    "%e %B %Y",
    "%A, %e %B %Y",
    "%Y-%m-%d",
    "%d.%m.%Y",
];

/// The two settings that are a time pattern: message times and the day lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pattern {
    Time,
    Date,
}

/// The time patterns offered as chips.
const TIME_FORMATS: [(&str, &str); 2] = [("%H:%M", "24 hour"), ("%I:%M %p", "12 hour")];

impl Shell {
    /// "Time" or "Date on day lines": preset chips and Custom, which shows the pattern field
    /// under them. A preset also goes into the field, so the two never disagree (#11).
    fn pattern_row(
        &self,
        page: &SettingsPage,
        which: Pattern,
        hint: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let b = &self.config.buffer;
        let (name, current, input, custom_chosen) = match which {
            Pattern::Time => (
                "Time",
                &b.timestamp_format,
                &page.time_format,
                page.custom_time,
            ),
            Pattern::Date => (
                "Date on day lines",
                &b.date_format,
                &page.date_format,
                page.custom_date,
            ),
        };
        let presets: Vec<(String, String)> = match which {
            Pattern::Time => TIME_FORMATS
                .iter()
                .map(|(f, n)| ((*f).to_owned(), (*n).to_owned()))
                .collect(),
            Pattern::Date => DATE_FORMATS
                .iter()
                .map(|f| {
                    let shown = time::format(hermes_client_core::now_ms(), f, self.utc_offset);
                    ((*f).to_owned(), shown)
                })
                .collect(),
        };
        let custom = custom_chosen || !presets.iter().any(|(f, _)| f == current);
        let id = match which {
            Pattern::Time => "time-format",
            Pattern::Date => "date-format",
        };
        let mut chips: Vec<AnyElement> = presets
            .into_iter()
            .enumerate()
            .map(|(n, (format, shown))| {
                let chosen = !custom && *current == format;
                crate::ui::chip(self, (id, n), shown, chosen, cx)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_pattern(which, format.clone(), false, window, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        chips.push(
            crate::ui::chip(self, (id, 99usize), "Custom", custom, cx)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(page) = this.settings.as_mut() {
                        match which {
                            Pattern::Time => page.custom_time = true,
                            Pattern::Date => page.custom_date = true,
                        }
                    }
                    cx.notify();
                }))
                .into_any_element(),
        );
        let field = custom.then(|| {
            let help = match which {
                Pattern::Time => "%H hours, %M minutes, %I and %p for 12 hours. Enter saves.",
                Pattern::Date => "%e day, %B month, %A weekday, %Y year. Enter saves.",
            };
            crate::channel::grow_in(
                &self.grow_heights,
                SharedString::from(format!("{id}-custom")),
                div()
                    .w_full()
                    .flex()
                    .justify_end()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .pt(px(8.))
                            .w(px(300.))
                            .child(Input::new(input))
                            .child(muted(help)),
                    )
                    .into_any_element(),
            )
        });
        setting(
            name,
            hint,
            div()
                .flex()
                .flex_col()
                .items_end()
                .child(wrap().justify_end().children(chips))
                .children(field),
        )
    }

    /// Saves a time or date pattern and shows it in the pattern field; `custom` keeps the
    /// field open (it came from the field itself).
    fn set_pattern(
        &mut self,
        which: Pattern,
        format: String,
        custom: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(page) = self.settings.as_mut() {
            let input = match which {
                Pattern::Time => {
                    page.custom_time = custom;
                    page.time_format.clone()
                }
                Pattern::Date => {
                    page.custom_date = custom;
                    page.date_format.clone()
                }
            };
            if !custom {
                let shown = format.clone();
                input.update(cx, |input, cx| input.set_value(shown, window, cx));
            }
        }
        self.set_config(
            |c| match which {
                Pattern::Time => c.buffer.timestamp_format = format,
                Pattern::Date => c.buffer.date_format = format,
            },
            cx,
        );
    }
}

/// A tab's name in the navigation.
fn tab_name(tab: Tab) -> &'static str {
    all_tabs()
        .find(|(t, _)| *t == tab)
        .map_or("", |(_, name)| name)
}

/// A setting as the search finds it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Found {
    pub group: String,
    /// The group's place among the page's groups, for scrolling to it.
    pub index: usize,
    pub name: String,
    pub hint: String,
    /// The group folds under this key.
    pub fold: Option<&'static str>,
}

/// What [`setting`] and [`group_with`] noted while a tab was built for the search.
#[derive(Default)]
struct Recording {
    /// Rows of the group being built (its rows come before its title).
    rows: Vec<(String, String)>,
    groups: usize,
    found: Vec<Found>,
}

impl Recording {
    fn row(&mut self, name: &str, hint: &str) {
        self.rows.push((name.to_owned(), hint.to_owned()));
    }

    fn group(&mut self, title: &str) {
        let index = self.groups;
        self.groups += 1;
        for (name, hint) in self.rows.drain(..) {
            self.found.push(Found {
                group: title.to_owned(),
                index,
                name,
                hint,
                fold: None,
            });
        }
    }

    /// The group just noted folds under `key`.
    fn fold_last(&mut self, key: &'static str) {
        let last = self.groups.saturating_sub(1);
        for f in self.found.iter_mut().filter(|f| f.index == last) {
            f.fold = Some(key);
        }
    }
}

thread_local! {
    /// Set while the search notes every tab's rows.
    static RECORDING: RefCell<Option<Recording>> = const { RefCell::new(None) };
    /// The row the search went to, lit for a moment.
    static LIT: RefCell<Option<(String, std::time::Instant)>> = const { RefCell::new(None) };
    /// The settings page's scroll; the search moves it to a group.
    static PAGE_SCROLL: gpui_kit::ScrollHandle = gpui_kit::ScrollHandle::new();
    /// The group a page scrolls to when it is next drawn.
    static SCROLL_TO: Cell<Option<usize>> = const { Cell::new(None) };
}

fn recording() -> bool {
    RECORDING.with(|r| r.borrow().is_some())
}

/// How long a row found by the search stays lit.
const LIT_FOR: std::time::Duration = std::time::Duration::from_millis(1800);

/// Starts a page at its top (a new tab does not keep the last one's scroll).
pub(crate) fn reset_page_scroll() {
    PAGE_SCROLL.with(|h| h.set_offset(gpui_kit::point(px(0.), px(0.))));
}

/// A section's caption in the navigation.
fn nav_section(title: &str) -> AnyElement {
    div()
        .flex_shrink_0()
        .px(px(8.))
        .pt(px(12.))
        .pb(px(4.))
        .text_size(theme::caption_size())
        .text_color(theme::text_tertiary())
        .child(title.to_uppercase())
        .into_any_element()
}

// ---- parts, also used by the space settings ---------------------------------------------------

/// Chips side by side, wrapping; they line up on the right, where a setting's control goes.
pub fn wrap() -> gpui_kit::Div {
    div().flex().flex_wrap().justify_end().w_full().gap(px(4.))
}

pub fn muted(text: impl Into<SharedString>) -> AnyElement {
    div()
        .text_size(theme::secondary_size())
        .text_color(theme::text_tertiary())
        .child(text.into())
        .into_any_element()
}

/// A page over the whole window: navigation, content and a way back.
pub fn frame(
    title: String,
    nav: Vec<AnyElement>,
    content: AnyElement,
    close: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    div()
        .id("page")
        .flex()
        .flex_1()
        .min_h_0()
        .size_full()
        .child(
            div()
                .id("page-nav")
                .flex()
                .flex_col()
                .flex_shrink_0()
                .gap(px(2.))
                .w(px(240.))
                .h_full()
                .p(px(12.))
                .overflow_y_scroll()
                .bg(theme::sidebar())
                // The whole line goes back, not just the arrow.
                .child(
                    div()
                        .id("page-close")
                        .group("page-close")
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(8.))
                        .h(px(30.))
                        .mb(px(8.))
                        .px(px(6.))
                        .cursor_pointer()
                        .text_size(theme::caption_size())
                        .text_color(theme::text_tertiary())
                        .hover(|d| d.text_color(theme::accent()))
                        .child(
                            icon(assets::ARROW_LEFT, 18., theme::text_tertiary())
                                .group_hover("page-close", |s| s.text_color(theme::accent())),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .on_click(close),
                )
                .children(nav),
        )
        .child(div().flex_1().min_w_0().h_full().child(content))
        .into_any_element()
}

/// A tab's content: title, optional text and groups, scrolling.
pub fn page_body(title: &str, text: Option<&str>, groups: Vec<AnyElement>) -> AnyElement {
    let scroll = PAGE_SCROLL.with(|h| h.clone());
    if !recording()
        && let Some(index) = SCROLL_TO.with(Cell::take)
    {
        // The title and the text come before the groups.
        scroll.scroll_to_item(index + 1 + usize::from(text.is_some()));
    }
    div()
        .id("page-body")
        .track_scroll(&scroll)
        .flex()
        .flex_col()
        .gap(px(20.))
        .size_full()
        .p(px(24.))
        .overflow_y_scroll()
        .child(
            div()
                .text_size(theme::heading_size())
                .font_weight(FontWeight::MEDIUM)
                .child(SharedString::from(title.to_owned())),
        )
        .children(text.map(|t| muted(SharedString::from(t.to_owned()))))
        .children(groups)
        .into_any_element()
}

pub fn group(title: &str, rows: Vec<AnyElement>) -> AnyElement {
    group_with(title, div().into_any_element(), rows)
}

/// A titled group of settings, with something on the title's right.
pub fn group_with(title: &str, actions: AnyElement, rows: Vec<AnyElement>) -> AnyElement {
    let header = group_header(title, actions);
    group_column(
        header,
        (!rows.is_empty()).then(|| group_card(rows).into_any_element()),
    )
}

/// A group's title line, with something on its right.
fn group_header(title: &str, actions: AnyElement) -> gpui_kit::Div {
    RECORDING.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.group(title);
        }
    });
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .flex_1()
                .text_size(theme::caption_size())
                .text_color(theme::text_tertiary())
                .child(title.to_uppercase()),
        )
        .child(actions)
}

/// A group's rows on their card, a thin line between each two.
fn group_card(rows: Vec<AnyElement>) -> gpui_kit::Div {
    let count = rows.len();
    let mut children: Vec<AnyElement> = Vec::with_capacity(count * 2);
    for (n, row) in rows.into_iter().enumerate() {
        children.push(row);
        if n + 1 < count {
            children.push(
                div()
                    .h(px(1.))
                    .flex_shrink_0()
                    .bg(theme::border())
                    .into_any_element(),
            );
        }
    }
    div()
        .flex()
        .flex_col()
        .px(px(14.))
        .py(px(4.))
        .rounded(theme::radius_large())
        .bg(theme::sidebar())
        .children(children)
}

/// The title line over the card (none while folded).
fn group_column(header: gpui_kit::Div, card: Option<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(10.))
        .max_w(px(760.))
        .child(header)
        .children(card)
        .into_any_element()
}

/// How long a hint may be and still show in full under its setting; a longer one shows its
/// start and the rest in the (i) next to the name.
const HINT_CHARS: usize = 72;

/// A row: the name and a one-line hint on the left, the control on the right. A hint too long
/// for one line puts all of it in the (i) beside the name.
pub fn setting(name: &str, hint: &str, control: impl IntoElement) -> AnyElement {
    RECORDING.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.row(name, hint);
        }
    });
    // Just found by the search: it glows and fades back.
    let lit = LIT.with(|l| {
        l.borrow()
            .as_ref()
            .is_some_and(|(found, at)| found == name && at.elapsed() < LIT_FOR)
    });
    let long = hint.chars().count() > HINT_CHARS;
    let full_hint = SharedString::from(hint.to_owned());
    let info = long.then(|| {
        let tip = full_hint.clone();
        div()
            .id(SharedString::from(format!("hint-{name}")))
            .flex_shrink_0()
            .child(icon(assets::INFO, 14., theme::text_tertiary()))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
    });
    let row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(12.))
        .py(px(10.))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(220.))
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(theme::body_size())
                        .child(SharedString::from(name.to_owned()))
                        .children(info),
                )
                .when(!hint.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(theme::secondary_size())
                            .text_color(theme::text_tertiary())
                            .truncate()
                            .child(full_hint),
                    )
                }),
        )
        .child(
            // At least wide enough for a slider, which has no width of its own.
            div()
                .flex()
                .flex_col()
                .items_end()
                .flex_shrink_0()
                .min_w(px(240.))
                .max_w_full()
                .child(control),
        );
    if !lit {
        return row.into_any_element();
    }
    row.rounded(theme::radius_small())
        .with_animation(
            SharedString::from(format!("lit-{name}")),
            gpui_kit::Animation::new(LIT_FOR).with_easing(gpui_kit::ease_in_out),
            |d, t| d.bg(theme::alpha(theme::accent(), 0.22 * (1. - t))),
        )
        .into_any_element()
}

/// A rounded bar standing in for a line of text in a theme's miniature.
fn preview_bar(width: f32, height: f32, color: gpui_kit::Rgba) -> gpui_kit::Div {
    div()
        .flex_shrink_0()
        .w(px(width))
        .h(px(height))
        .rounded(px(height / 2.))
        .bg(color)
}

/// A sidebar row in a theme's miniature, with its state bar and background if it has them.
fn preview_row(
    state: Option<(gpui_kit::Rgba, gpui_kit::Rgba)>,
    indicator: f32,
    text: gpui_kit::Rgba,
    width: f32,
) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .w_full()
        .py(px(2.))
        .when_some(state, |d, (_, background)| d.bg(background))
        .child(
            div()
                .flex_shrink_0()
                .w(px(indicator))
                .h(px(10.))
                .when_some(state, |d, (bar, _)| d.bg(bar)),
        )
        .child(preview_bar(width, 4., text))
}

/// A line of chat in a theme's miniature.
fn preview_line(
    avatar: gpui_kit::Rgba,
    first: (f32, gpui_kit::Rgba),
    second: (f32, gpui_kit::Rgba),
) -> gpui_kit::Div {
    div()
        .flex()
        .gap(px(5.))
        .child(
            div()
                .flex_shrink_0()
                .size(px(10.))
                .rounded(px(5.))
                .bg(avatar),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(preview_bar(first.0, 4., first.1))
                .child(preview_bar(second.0, 4., second.1)),
        )
}

/// A theme drawn in miniature: sidebar with a selected and an unread row, chat lines and the input.
fn theme_preview(t: &Theme) -> gpui_kit::Div {
    let indicator = f32::from(t.shape.indicator_width).max(1.);
    let corner = f32::from(t.shape.radius).min(4.);
    let sidebar = div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(4.))
        .w(px(52.))
        .h_full()
        .p(px(6.))
        .bg(t.surface.sidebar.0)
        .child(preview_bar(28., 5., t.accent.highlight.0))
        .child(preview_row(None, indicator, t.text.secondary.0, 22.))
        .child(preview_row(
            Some((t.channel.selected_border.0, t.channel.selected_bg.0)),
            indicator,
            t.text.primary.0,
            26.,
        ))
        .child(preview_row(
            Some((t.channel.unread_border.0, t.channel.unread_bg.0)),
            indicator,
            t.text.primary.0,
            18.,
        ))
        .child(preview_row(None, indicator, t.text.tertiary.0, 24.));
    let chat = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .h_full()
        .gap(px(6.))
        .p(px(8.))
        .bg(t.surface.main.0)
        .child(preview_line(
            t.accent.muted.0,
            (40., t.text.primary.0),
            (60., t.text.secondary.0),
        ))
        .child(preview_line(
            t.presence.online.0,
            (30., t.text.primary.0),
            (48., t.text.secondary.0),
        ))
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .gap(px(4.))
                .child(
                    div()
                        .flex_1()
                        .h(px(12.))
                        .rounded(px(corner))
                        .bg(t.surface.raised.0),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .w(px(18.))
                        .h(px(12.))
                        .rounded(px(corner))
                        .bg(t.accent.base.0),
                ),
        );
    div().flex().size_full().child(sidebar).child(chat)
}

/// A card for a theme: its miniature (or that its file did not load) and its name. The theme in use
/// is outlined in the accent colour.
fn theme_card(
    n: usize,
    name: &str,
    loaded: &Result<Theme, String>,
    chosen: bool,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    const WIDTH: f32 = 160.;
    const PREVIEW_HEIGHT: f32 = 96.;
    let picture = match loaded {
        Ok(t) => theme_preview(t),
        Err(_) => div()
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .text_size(theme::caption_size())
            .text_color(theme::danger())
            .child("not loaded"),
    };
    div()
        .id(("theme-card", n))
        .group("theme-card")
        .flex()
        .flex_col()
        .gap(px(6.))
        .w(px(WIDTH))
        .cursor_pointer()
        .child(
            div()
                .relative()
                .w_full()
                .h(px(PREVIEW_HEIGHT))
                .overflow_hidden()
                .rounded(theme::row_radius())
                .child(picture)
                // The ring lies over the preview, so choosing never shrinks it; it thickens and
                // takes the accent on a spring.
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(theme::row_radius())
                        .when(!chosen, |d| {
                            d.group_hover("theme-card", |d| d.border_color(theme::text_tertiary()))
                        })
                        .with_spring(
                            ("theme-card-ring", n),
                            gpui_kit::SpringAnimation::new(crate::theme::paced(
                                crate::hover::HOVER_SPRING,
                            ))
                            .to(chosen),
                            |d, phase| {
                                let t = phase.0.clamp(0., 1.);
                                theme::border_width(d, px(1. + t)).border_color(theme::mix(
                                    theme::border(),
                                    theme::accent(),
                                    t,
                                ))
                            },
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(6.))
                .text_color(if chosen {
                    theme::text()
                } else {
                    theme::text_secondary()
                })
                .child(name.to_owned())
                .when(chosen, |d| {
                    d.child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::accent())
                            .child("in use"),
                    )
                }),
        )
}

/// `Ctrl+Shift+M` as separate key caps.
fn key_caps(combo: &str) -> AnyElement {
    if combo.is_empty() {
        return muted("Not bound");
    }
    // A combo ending in `+` (Ctrl++) has `+` as its key.
    let mut keys: Vec<String> = combo.split('+').map(str::to_owned).collect();
    if combo.ends_with('+') {
        keys.retain(|k| !k.is_empty());
        keys.push("+".into());
    }
    div()
        .flex()
        .gap(px(4.))
        .children(keys.into_iter().map(|key| {
            div()
                .px(px(8.))
                .py(px(2.))
                .rounded(theme::radius_small())
                .border_1()
                .border_color(theme::border())
                .bg(theme::raised())
                .text_size(theme::caption_size())
                .child(key)
        }))
        .into_any_element()
}
