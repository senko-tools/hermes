//! The settings page (PLAN.md section 15.1), shown over the whole window: Account, Appearance,
//! Chat, Voice & Audio, Notifications, Keybinds and About. Every change is written to
//! `config.toml`.

use gpui_kit::component::IndexPath;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Entity, FontWeight, SharedString, Subscription, Window, div};
use hermes_client_core::audio::AudioDevices;
use hermes_client_core::proto::api::{SessionInfo, UpdateMeRequest, UpdatePrivacyRequest};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::DmFrom;
use hermes_client_core::{CoreError, InstanceId, time};

use crate::assets::{self, icon};
use crate::channel::icon_button;
use crate::keybinds::{ACTIONS, Action, Keybinds};
use crate::results::Remote;
use crate::screen::capture::{FRAME_RATES, PREFERENCES, Preference, RESOLUTIONS};
use crate::shell::Shell;
use crate::theme::{self, Theme, px};
use crate::voice::chip;

/// The level meter and the voice activity slider share this range (dBFS).
pub const METER_MIN: f32 = -70.0;
/// UI scales offered as one-click choices.
const SCALE_PRESETS: [f64; 5] = [0.9, 1.0, 1.1, 1.25, 1.5];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    Account,
    #[default]
    Appearance,
    Chat,
    Voice,
    Notifications,
    Keybinds,
    About,
}

const TABS: [(Tab, &str, &str); 7] = [
    (Tab::Account, assets::ACCOUNT, "Account"),
    (Tab::Appearance, assets::PALETTE, "Appearance"),
    (Tab::Chat, assets::CHAT, "Chat"),
    (Tab::Voice, assets::MIC, "Voice & Audio"),
    (Tab::Notifications, assets::BELL, "Notifications"),
    (Tab::Keybinds, assets::KEYBOARD, "Keybinds"),
    (Tab::About, assets::INFO, "About"),
];

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
    ReleaseDelay,
    SoundsVolume,
}

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

impl SelectItem for DeviceChoice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &String {
        &self.value
    }
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
    /// The audio device dropdowns, once the Voice & Audio tab has listed the devices.
    device_selects: Option<DeviceSelects>,
    pub mic_test: bool,
    font: Entity<InputState>,
    mono_font: Entity<InputState>,
    time_format: Entity<InputState>,
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
    /// Moves the level meter towards the level and asks for another frame while it shows.
    pub fn animate_meter(&mut self, window: &mut Window) {
        let showing = self
            .settings
            .as_ref()
            .is_some_and(|page| page.tab == Tab::Voice);
        if !showing {
            return;
        }
        let now = std::time::Instant::now();
        let seconds = now.duration_since(self.meter_drawn).as_secs_f32();
        self.meter_drawn = now;
        self.meter_shown = meter_step(self.meter_shown, self.input_level, seconds);
        window.request_animation_frame();
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
        let font_input = text_field(
            font.family.clone(),
            "Installed font, e.g. Segoe UI",
            window,
            cx,
        );
        let mono_input = text_field(
            font.mono_family.clone(),
            "Installed font, e.g. Consolas",
            window,
            cx,
        );
        let time_format = text_field(
            self.config.buffer.timestamp_format.clone(),
            "%H:%M",
            window,
            cx,
        );
        let ptt_key = text_field(voice.ptt_key.clone(), "e.g. Mouse4", window, cx);
        let mut events = vec![
            cx.subscribe_in(&font_input, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let value = input.read(cx).value().trim().to_owned();
                    this.persist("font.family", &toml_string(value), cx);
                    this.reload_theme(cx);
                }
            }),
            cx.subscribe_in(&mono_input, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let value = input.read(cx).value().trim().to_owned();
                    this.persist("font.mono_family", &toml_string(value), cx);
                    this.reload_theme(cx);
                }
            }),
            cx.subscribe_in(&time_format, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let value = input.read(cx).value().to_string();
                    this.persist("buffer.timestamp_format", &toml_string(value), cx);
                    this.info("Time format saved", cx);
                }
            }),
            cx.subscribe_in(&ptt_key, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let text = input.read(cx).value().trim().to_owned();
                    if !text.is_empty() && crate::ptt::virtual_keys(&text).is_none() {
                        this.error(format!("{text:?} is not a key Hermes knows"), cx);
                    } else {
                        this.persist("voice.ptt_key", &toml_string(text), cx);
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
                2.,
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
                SliderKind::SoundsVolume,
                0.,
                1.,
                0.05,
                voice.sounds_volume as f32,
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
            device_selects: None,
            mic_test: false,
            font: font_input,
            mono_font: mono_input,
            time_format,
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
        self.core.set_level_meter(false);
        cx.notify();
    }

    fn settings_open_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        let Some(page) = &mut self.settings else {
            return;
        };
        page.tab = tab;
        self.core.set_level_meter(tab == Tab::Voice);
        match tab {
            Tab::Account => self.load_sessions(cx),
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
        if let Some(page) = &mut self.settings {
            page.sessions = Remote::Loading;
        }
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                core.rest(&inst)
                    .map_err(|e| e.to_string())?
                    .sessions()
                    .await
                    .map_err(|e| e.to_string())
            },
            |this, result, _, _| {
                if let Some(page) = &mut this.settings {
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
            SliderKind::SoundsVolume => voice.sounds_volume = rounded(value) as _,
        }
        cx.notify();
    }

    fn slider_released(&mut self, kind: SliderKind, window: &mut Window, cx: &mut Context<Self>) {
        match kind {
            SliderKind::FontSize => {
                let value = self.config.font.size.to_string();
                self.persist("font.size", &value, cx);
                self.reload_theme(cx);
                self.remeasure_chats();
            }
            SliderKind::UiScale => {
                if let Some(scale) = self.ui_scale_draft.take() {
                    self.set_ui_scale(scale, window, cx);
                }
            }
            SliderKind::GroupMinutes => {
                let value = self.config.buffer.group_minutes.to_string();
                self.persist("buffer.group_minutes", &value, cx);
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
        self.persist("font.ui_scale", &format!("{scale:.2}"), cx);
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
        self.persist("general.theme", &toml_string(name), cx);
        self.reload_theme(cx);
    }

    /// Theme files changed on disk, or the config: redo the look.
    pub fn start_theme_watcher(&self, cx: &mut Context<Self>) {
        let Some(dir) = self.core.paths().map(|p| p.dir.join(theme::THEMES_DIR)) else {
            return;
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let watcher = std::fs::create_dir_all(&dir)
            .map_err(notify::Error::io)
            .and_then(|()| {
                use notify::Watcher as _;
                let mut watcher =
                    notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                        if res.is_ok_and(|e| !e.kind.is_access()) {
                            let _ = tx.send(());
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
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                let Some(file) = rfd::AsyncFileDialog::new()
                    .set_title("Choose a picture")
                    .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"])
                    .pick_file()
                    .await
                else {
                    return Ok(None);
                };
                let avatar = core.upload(&inst, file.path()).await?;
                core.rest(&inst)?
                    .update_me(&UpdateMeRequest {
                        avatar_id: Some(Some(avatar.id)),
                        ..UpdateMeRequest::default()
                    })
                    .await?;
                Ok::<_, CoreError>(Some(()))
            },
            |this, result, _, cx| match result {
                Ok(Some(())) => this.info("Avatar changed", cx),
                Ok(None) => {}
                Err(err) => this.error(format!("could not change it: {err}"), cx),
            },
        );
    }

    fn revoke_sessions(&mut self, ids: Vec<Id>, cx: &mut Context<Self>) {
        let Some(inst) = self.account_instance() else {
            return;
        };
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                let rest = core.rest(&inst).map_err(|e| e.to_string())?;
                for id in ids {
                    rest.delete_session(id).await.map_err(|e| e.to_string())?;
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
        let nav: Vec<AnyElement> = TABS
            .iter()
            .map(|(tab, glyph, name)| {
                let tab = *tab;
                nav_item(glyph, name, page.tab == tab, format!("settings-tab-{name}"))
                    .on_click(cx.listener(move |this, _, _, cx| this.settings_open_tab(tab, cx)))
                    .into_any_element()
            })
            .collect();
        let content = match page.tab {
            Tab::Account => self.tab_account(page, cx),
            Tab::Appearance => self.tab_appearance(page, cx),
            Tab::Chat => self.tab_chat(page, cx),
            Tab::Voice => self.tab_voice(page, cx),
            Tab::Notifications => self.tab_notifications(cx),
            Tab::Keybinds => self.tab_keybinds(),
            Tab::About => self.tab_about(cx),
        };
        Some(frame(
            "User settings".into(),
            nav,
            content,
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
        let (me, privacy) = {
            let store = self.core.store();
            let s = selected.as_ref().and_then(|i| store.instance(i));
            (
                s.and_then(|s| s.me.clone()),
                s.map(|s| s.privacy).unwrap_or_default(),
            )
        };
        let mut account_rows = Vec::new();
        if instances.len() > 1 {
            let chips = instances.iter().enumerate().map(|(n, inst)| {
                let chosen = Some(inst) == selected.as_ref();
                let inst = inst.clone();
                chip(("account-inst", n), inst.clone(), chosen).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if let Some(page) = &mut this.settings {
                            page.account_inst = Some(inst.clone());
                        }
                        this.load_sessions(cx);
                    },
                ))
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
            hint.push_str(
                ". You are an admin of this instance: manage users with the hermes-server user \
                 commands or the API (/admin/users).",
            );
        }
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
        account_rows.push(setting(
            "Avatar",
            if has_avatar {
                "Shown next to your messages."
            } else {
                "No picture yet; your initials are shown."
            },
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

        let dm_chips = [
            (DmFrom::Everyone, "Everyone"),
            (DmFrom::Spaces, "People in my spaces"),
            (DmFrom::Nobody, "Nobody"),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (value, name))| {
            chip(("dm-from", n), name, privacy.dm_from == value).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.set_privacy(
                        UpdatePrivacyRequest {
                            dm_from: Some(value),
                            ..UpdatePrivacyRequest::default()
                        },
                        cx,
                    );
                },
            ))
        });
        let privacy_rows = vec![
            setting(
                "Direct messages from",
                "Who can start a conversation with you on this instance. Conversations you \
                 already have stay open.",
                wrap().children(dm_chips),
            ),
            setting(
                "Message requests",
                "Messages from everyone else wait in Requests: you are notified once per \
                 request, and they stay quiet until you accept. Off: they cannot message you \
                 at all.",
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
        ];

        let mut device_rows: Vec<AnyElement> = Vec::new();
        match &page.sessions {
            Remote::Idle | Remote::Loading => device_rows.push(muted("Loading…")),
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
                group("Privacy", privacy_rows),
                group("Signed-in devices", device_rows),
            ],
        )
    }

    fn slider(&self, page: &SettingsPage, kind: SliderKind) -> AnyElement {
        page.sliders.iter().find(|(k, _)| *k == kind).map_or_else(
            || div().into_any_element(),
            |(_, state)| Slider::new(state).cursor_pointer().into_any_element(),
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
                muted(
                    "Your own themes are .toml files in the themes folder: copy one to start, and \
                     changes show up as soon as you save. Keys you leave out come from the dark \
                     theme.",
                ),
            ],
        );
        let shown_px = i32::from(config.font.size) + 3;
        let text_group = group(
            "Text",
            vec![
                setting(
                    "Font",
                    "Used for all text. Any installed font's name works; Enter saves.",
                    Input::new(&page.font),
                ),
                setting(
                    "Code font",
                    "Used for code blocks and inline code. Enter saves.",
                    Input::new(&page.mono_font),
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
            chip(("scale-preset", n), format!("{:.0}%", value * 100.), chosen).on_click(
                cx.listener(move |this, _, window, cx| this.set_ui_scale(value, window, cx)),
            )
        });
        let position_chips = [("left", "Left"), ("right", "Right")]
            .into_iter()
            .enumerate()
            .map(|(n, (value, name))| {
                chip(
                    ("sidebar-position", n),
                    name,
                    config.sidebar.position.eq_ignore_ascii_case(value),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.persist("sidebar.position", &toml_string(value.to_owned()), cx);
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
                        format!(
                            "{:.0} % on top of the system scaling. Ctrl+= and Ctrl+- zoom, Ctrl+0 \
                             resets.",
                            scale * 100.
                        )
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
                        "Sidebar {} px, member list {} px. Drag the sidebar's edge to resize; a \
                         double-click on it resets.",
                        config.sidebar.width, config.sidebar.member_list_width
                    ),
                    Button::new("reset-widths")
                        .cursor_pointer()
                        .label("Reset")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.persist("sidebar.width", "240", cx);
                            this.persist("sidebar.member_list_width", "232", cx);
                        })),
                ),
            ],
        );
        page_body(
            "Appearance",
            None,
            vec![theme_group, text_group, layout_group],
        )
    }

    fn flag(
        &self,
        name: &'static str,
        hint: &'static str,
        on: bool,
        key: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        setting(
            name,
            hint,
            Switch::new(SharedString::from(key))
                .cursor_pointer()
                .checked(on)
                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                    this.persist(key, if *on { "true" } else { "false" }, cx);
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
        page_body(
            "Chat",
            None,
            vec![
                group(
                    "Messages",
                    vec![
                        self.flag(
                            "Show avatars",
                            "",
                            b.show_avatars,
                            "buffer.show_avatars",
                            cx,
                        ),
                        self.flag(
                            "Show image previews",
                            "Images in messages are shown, not just linked.",
                            b.image_previews,
                            "buffer.image_previews",
                            cx,
                        ),
                        self.flag(
                            "Show who is typing",
                            "",
                            b.show_typing,
                            "buffer.show_typing",
                            cx,
                        ),
                        self.flag(
                            "Enter sends",
                            "Shift+Enter starts a new line.",
                            b.enter_sends,
                            "buffer.enter_sends",
                            cx,
                        ),
                    ],
                ),
                group(
                    "Layout",
                    vec![
                        setting(
                            "Group messages",
                            &format!(
                                "Messages by the same person within {} minutes share one name line.",
                                b.group_minutes
                            ),
                            self.slider(page, SliderKind::GroupMinutes),
                        ),
                        setting(
                            "Time format",
                            &format!(
                                "strftime style: %H:%M is 18:05, %I:%M %p is 06:05 PM. Now: {preview}. Enter saves."
                            ),
                            Input::new(&page.time_format),
                        ),
                    ],
                ),
            ],
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
                    "Plugged in a headset? List the devices again.",
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
                    &format!("{:.0} %", v.input_volume * 100.),
                    self.slider(page, SliderKind::InputVolume),
                ),
                setting(
                    "Output volume",
                    &format!("{:.0} %", v.output_volume * 100.),
                    self.slider(page, SliderKind::OutputVolume),
                ),
            ],
        );
        let ptt_mode = v.mode == "push_to_talk";
        let mode_chips = [
            ("voice_activity", "Voice activity"),
            ("push_to_talk", "Push-to-talk"),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (value, name))| {
            chip(("voice-mode", n), name, v.mode == value).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.change_voice(cx, move |voice| voice.mode = value.to_owned());
                },
            ))
        });
        // The bar moves smoothly; whether the gate is open is the real level's business.
        let level = self.meter_shown.clamp(METER_MIN, 0.);
        let open = self.input_level >= v.vad_sensitivity as f32;
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
                    .bg(if open {
                        theme::speaking()
                    } else {
                        theme::text_tertiary()
                    }),
            );
        let mut input_rows = vec![setting(
            "Input mode",
            "Voice activity sends while you talk; push-to-talk while a key is held.",
            wrap().children(mode_chips),
        )];
        if ptt_mode {
            input_rows.push(setting(
                "Push-to-talk key",
                if cfg!(windows) {
                    "Works in every app: Mouse4, Mouse5, CapsLock, F1–F24, letters, Ctrl+Space… Enter saves."
                } else {
                    "On this system it only works while the Hermes window is focused. Enter saves."
                },
                Input::new(&page.ptt_key),
            ));
            input_rows.push(setting(
                "Release delay",
                &format!("{} ms", v.ptt_release_delay_ms),
                self.slider(page, SliderKind::ReleaseDelay),
            ));
        }
        input_rows.push(setting(
            "Voice activity sensitivity",
            &format!(
                "Opens above {} dBFS · microphone now {level:.0} dBFS",
                v.vad_sensitivity
            ),
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(px(6.))
                .child(meter)
                .child(self.slider(page, SliderKind::Vad)),
        ));
        let mic_test = page.mic_test;
        input_rows.push(setting(
            "Mic test",
            "Hear your processed microphone (use headphones).",
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
        let processing = group(
            "Processing",
            vec![
                setting(
                    "Noise suppression",
                    if v.noise_suppression {
                        "Removes background noise like fans and keyboards."
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
                    "Automatic gain",
                    "Evens out how loud you are.",
                    voice_switch("gain", v.auto_gain, |v, on| v.auto_gain = on, cx),
                ),
            ],
        );
        let join_chips = [
            ("double_click", "Double-click"),
            ("single_click", "Single click"),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (value, name))| {
            chip(("join-on", n), name, v.join_on == value).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.persist("voice.join_on", &toml_string(value.to_owned()), cx);
                },
            ))
        })
        .collect::<Vec<_>>();
        let sounds = group(
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
                setting(
                    "Join a voice channel with",
                    "With double-click, a single click only opens the channel's voice room.",
                    wrap().children(join_chips),
                ),
            ],
        );
        let mut screens = vec![String::new()];
        screens.extend(self.screen_names.iter().skip(1).cloned());
        let screen_chips = screens.into_iter().enumerate().map(|(n, screen)| {
            let name = if screen.is_empty() {
                "Primary screen".to_owned()
            } else {
                screen.trim_start_matches(['\\', '.']).to_owned()
            };
            chip(("settings-screen", n), name, screen == v.share_screen).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.persist("voice.share_screen", &toml_string(screen.clone()), cx);
                },
            ))
        });
        let res_chips = RESOLUTIONS.into_iter().enumerate().map(|(n, value)| {
            let name = if value == "native" {
                "Full size (up to 4K)"
            } else {
                value
            };
            chip(("settings-res", n), name, v.share_resolution == value).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.persist("voice.share_resolution", &toml_string(value.to_owned()), cx);
                },
            ))
        });
        let fps_chips = FRAME_RATES.into_iter().enumerate().map(|(n, fps)| {
            chip(
                ("settings-fps", n),
                format!("{fps} fps"),
                v.share_fps == fps,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.persist("voice.share_fps", &fps.to_string(), cx);
            }))
        });
        let current = Preference::from_config(&v.share_preference);
        let pref_chips = PREFERENCES.into_iter().enumerate().map(|(n, preference)| {
            chip(
                ("settings-pref", n),
                preference.label(),
                preference == current,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.persist(
                    "voice.share_preference",
                    &toml_string(preference.config_value().to_owned()),
                    cx,
                );
                if let Some(capture) = &this.capture {
                    capture.set_preference(preference);
                }
            }))
        });
        let sharing = group(
            "Screen sharing",
            vec![
                setting(
                    "Screen to share",
                    "Only people in the call see it. Start and stop it with the share button in \
                     the voice panel.",
                    wrap().children(screen_chips),
                ),
                setting(
                    "Resolution",
                    "The largest size your screen is sent at. Larger is sharper but uses more CPU \
                     and bandwidth. Applies the next time you share.",
                    wrap().children(res_chips),
                ),
                setting(
                    "Frame rate",
                    "Frames per second. More is smoother (video, games) but uses more bandwidth. \
                     Applies the next time you share.",
                    wrap().children(fps_chips),
                ),
                setting(
                    "When it cannot keep up",
                    "What your share keeps when your connection or computer falls behind. \
                     Smoothness keeps the frame rate and lowers sharpness, then size; Clarity \
                     keeps sharp, full-size pictures and lowers the frame rate; Mixed is in \
                     between. Also on your tile while you share.",
                    wrap().children(pref_chips),
                ),
            ],
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
                sharing,
            ],
        )
    }

    fn tab_notifications(&self, cx: &mut Context<Self>) -> AnyElement {
        let n = &self.config.notifications;
        let g = &self.config.general;
        page_body(
            "Notifications",
            Some(
                "Which messages notify is set per space or channel: right-click it and pick \
                 \"Notify: …\" or Mute. By default spaces notify on @mentions and DMs on every \
                 message.",
            ),
            vec![
                group(
                    "Desktop notifications",
                    vec![
                        self.flag(
                            "Show notifications",
                            "",
                            n.enabled,
                            "notifications.enabled",
                            cx,
                        ),
                        self.flag(
                            "Show the message text",
                            "Off: only who wrote, and where.",
                            n.show_preview,
                            "notifications.show_preview",
                            cx,
                        ),
                        self.flag("Play a sound", "", n.sound, "notifications.sound", cx),
                        self.flag(
                            "Flash the taskbar button",
                            "",
                            n.flash_taskbar,
                            "notifications.flash_taskbar",
                            cx,
                        ),
                    ],
                ),
                group(
                    "Window",
                    vec![
                        self.flag(
                            "Close to the tray",
                            "Closing the window keeps Hermes running in the tray.",
                            g.close_to_tray,
                            "general.close_to_tray",
                            cx,
                        ),
                        self.flag(
                            "Start minimized to the tray",
                            "",
                            g.start_minimized,
                            "general.start_minimized",
                            cx,
                        ),
                        self.flag(
                            "Check for updates on start",
                            "",
                            g.check_for_updates,
                            "general.check_for_updates",
                            cx,
                        ),
                    ],
                ),
            ],
        )
    }

    fn tab_keybinds(&self) -> AnyElement {
        let mut groups: Vec<AnyElement> = KEY_GROUPS
            .iter()
            .map(|(title, actions)| {
                let rows = actions
                    .iter()
                    .filter_map(|action| ACTIONS.iter().find(|(a, ..)| a == action))
                    .map(|(action, _, _, title)| {
                        let combo =
                            Keybinds::combo_text(&self.config.keys, *action).unwrap_or_default();
                        setting(title, "", key_caps(&combo))
                    })
                    .collect();
                group(title, rows)
            })
            .collect();
        groups.push(group(
            "Everywhere",
            vec![muted(
                "Right-click channels, spaces, messages and people for more actions. Double-click \
                 a voice channel to join it. Drag channels and roles to reorder them. Escape \
                 closes menus and dialogs. Ctrl+K finds anything.",
            )],
        ));
        page_body(
            "Keybinds",
            Some(
                "Change them in the [keys] section of config.toml; changes apply at once. \
                 Examples: \"Ctrl+K\", \"Alt+Shift+Down\", \"Ctrl+\\\\\".",
            ),
            groups,
        )
    }

    fn tab_about(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .max_w(px(640.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(56.))
                            .rounded(px(12.))
                            .bg(theme::accent_soft())
                            .child(icon(assets::FEATHER, 28., theme::accent())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
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
                                    .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(theme::secondary_size())
                    .text_color(theme::text_secondary())
                    .child(
                        "A self-hostable chat with spaces, text and voice channels, DMs and \
                         calls. Licensed GPL-3.0-or-later; the server is AGPL-3.0-or-later.",
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(
                        Button::new("about-debug")
                            .cursor_pointer()
                            .label("Copy debug info")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let text = this.debug_info();
                                this.copy(text, "Debug info", cx);
                            })),
                    )
                    .child(
                        Button::new("about-logs")
                            .cursor_pointer()
                            .label("Open the logs folder")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(paths) = this.core.paths() {
                                    let dir = paths.dir.join("logs");
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
                        Button::new("about-project")
                            .cursor_pointer()
                            .label("Project page")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Err(err) = crate::open::url(env!("CARGO_PKG_REPOSITORY")) {
                                    this.error(format!("could not open the browser: {err}"), cx);
                                }
                            })),
                    ),
            )
            .into_any_element()
    }
}

// ---- parts, also used by the space settings ---------------------------------------------------

fn toml_string(value: String) -> String {
    toml::Value::String(value).to_string()
}

pub fn wrap() -> gpui_kit::Div {
    div().flex().flex_wrap().w_full().gap(px(4.))
}

pub fn muted(text: impl Into<SharedString>) -> AnyElement {
    div()
        .text_size(theme::secondary_size())
        .text_color(theme::text_tertiary())
        .child(text.into())
        .into_any_element()
}

/// An entry of a page's navigation column. The caller adds what a click does.
pub fn nav_item(
    glyph: &str,
    name: &str,
    selected: bool,
    id: String,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(SharedString::from(id))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(8.))
        .h(px(34.))
        .px(px(8.))
        .rounded(theme::row_radius())
        .cursor_pointer()
        .text_size(theme::body_size())
        .text_color(if selected {
            theme::text()
        } else {
            theme::text_secondary()
        })
        .when(selected, |d| d.bg(theme::selected()))
        .when(!selected, |d| d.hover(|d| d.bg(theme::hover())))
        .child(icon(glyph, 18., theme::text_tertiary()))
        .child(SharedString::from(name.to_owned()))
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
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .pb(px(8.))
                        .child(icon_button("page-close", assets::ARROW_LEFT).on_click(close))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(theme::caption_size())
                                .text_color(theme::text_tertiary())
                                .child(title),
                        ),
                )
                .children(nav),
        )
        .child(div().flex_1().min_w_0().h_full().child(content))
        .into_any_element()
}

/// A tab's content: title, optional text and groups, scrolling.
pub fn page_body(title: &str, text: Option<&str>, groups: Vec<AnyElement>) -> AnyElement {
    div()
        .id("page-body")
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
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(10.))
        .max_w(px(760.))
        .child(
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
                .child(actions),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .p(px(14.))
                .rounded(px(8.))
                .bg(theme::sidebar())
                .children(rows),
        )
        .into_any_element()
}

/// A row: name and hint on the left, the control on the right.
pub fn setting(name: &str, hint: &str, control: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(220.))
                .gap(px(2.))
                .child(
                    div()
                        .text_size(theme::body_size())
                        .child(SharedString::from(name.to_owned())),
                )
                .when(!hint.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(theme::secondary_size())
                            .text_color(theme::text_tertiary())
                            .child(SharedString::from(hint.to_owned())),
                    )
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_start()
                .flex_1()
                .min_w_0()
                .child(control),
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
                .w_full()
                .h(px(PREVIEW_HEIGHT))
                .overflow_hidden()
                .rounded(px(6.))
                .when(chosen, |d| d.border_2().border_color(theme::accent()))
                .when(!chosen, |d| {
                    d.border_1()
                        .border_color(theme::border())
                        .group_hover("theme-card", |d| d.border_color(theme::text_tertiary()))
                })
                .child(picture),
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
                .rounded(px(4.))
                .border_1()
                .border_color(theme::border())
                .bg(theme::raised())
                .text_size(theme::caption_size())
                .child(key)
        }))
        .into_any_element()
}
