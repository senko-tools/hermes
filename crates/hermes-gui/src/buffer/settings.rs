//! The Settings buffer (PLAN.md section 15.1): Account, Appearance, Chat, Voice & Audio,
//! Notifications, Keybinds and About (the last tabs are in `settings_tabs`). Every change is
//! written to `config.toml`.

use std::fmt;

use hermes_client_core::audio::AudioDevices;
use hermes_client_core::config::ClientConfig;
use iced::widget::{button, column, container, progress_bar, row, scrollable, slider, text_input};
use iced::{Element, Fill};

use super::page::{dropdown, group, group_with, nav_item, nav_title, page, setting, toggle};
use crate::font::{self, Fonts};
use crate::theme::{Theme, style};
use crate::widget::label;
use crate::widget::{ButtonKind, text_button};

/// The level meter and the voice activity slider share this range (dBFS).
pub const METER_MIN: f32 = -70.0;
pub const PTT_INPUT: &str = "settings-ptt-key";

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Theme(String),
    OpenThemesFolder,
    /// Copy the current theme to a new file in the themes folder, to edit it.
    CopyTheme,
    /// A family picked from the font list ([`CUSTOM_FONT`] shows the name field).
    Font(FontSlot, String),
    /// The custom family name field changed.
    FontText(FontSlot, String),
    /// Enter in the custom family name field: save it.
    SaveFont(FontSlot),
    FontSize(u16),
    /// The UI scale slider moved (applied on release, see [`Setting::UiScale`]).
    UiScale(f64),
    /// A UI scale preset: applied and saved at once.
    UiScalePreset(f64),
    SidebarPosition(Position),
    /// Back to the default sidebar and member list widths.
    ResetWidths,
    /// A slider was released: save what it changed.
    Save(Setting),
    Close,
    Tab(Tab),
    /// The device lists arrived.
    Devices(AudioDevices),
    RefreshDevices,
    InputDevice(Device),
    OutputDevice(Device),
    InputVolume(f64),
    OutputVolume(f64),
    VadSensitivity(i32),
    ReleaseDelay(u32),
    SoundsVolume(f64),
    Mode(Mode),
    PttKey(String),
    SavePttKey,
    NoiseSuppression(bool),
    AutoGain(bool),
    Sounds(bool),
    JoinOn(JoinOn),
    MicTest(bool),
    /// A voice slider was released: apply and save `[voice]`.
    SaveVoice,
    /// A boolean setting (`section.key`).
    Flag(&'static str, bool),
    GroupMinutes(u32),
    SaveGroupMinutes,
    TimestampFormat(String),
    SaveTimestampFormat,
    AccountInstance(String),
    EditProfile,
    PickAvatar,
    RemoveAvatar,
    ChangePassword,
    LoadSessions,
    Sessions(Result<Vec<hermes_client_core::proto::api::SessionInfo>, String>),
    RevokeSession(hermes_client_core::proto::id::Id),
    RevokeOthers,
    CopyDebugInfo,
    OpenLogs,
    OpenProjectPage,
    /// The screens that can be shared were listed.
    Screens(Vec<String>),
    ShareScreen(ShareScreen),
    ShareResolution(ShareResolution),
    ShareFps(ShareFps),
    SharePreference(crate::screen::capture::Preference),
    /// Who may start DMs with me on the Account tab's instance.
    DmFrom(DmFrom),
    AllowRequests(bool),
}

/// `PrivacySettings::dm_from`, as offered in the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmFrom(pub hermes_client_core::proto::models::DmFrom);

impl DmFrom {
    pub const ALL: [DmFrom; 3] = {
        use hermes_client_core::proto::models::DmFrom as D;
        [DmFrom(D::Everyone), DmFrom(D::Spaces), DmFrom(D::Nobody)]
    };
}

impl fmt::Display for DmFrom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use hermes_client_core::proto::models::DmFrom as D;
        f.write_str(match self.0 {
            D::Everyone => "Everyone",
            D::Spaces => "People in my spaces",
            D::Nobody => "Nobody",
        })
    }
}

/// A slider-driven setting, saved on release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    FontSize,
    UiScale,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Left,
    Right,
}

impl Position {
    pub const ALL: [Position; 2] = [Position::Left, Position::Right];

    pub fn from_config(value: &str) -> Position {
        if value.eq_ignore_ascii_case("right") {
            Position::Right
        } else {
            Position::Left
        }
    }

    pub fn as_config(self) -> &'static str {
        match self {
            Position::Left => "left",
            Position::Right => "right",
        }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Position::Left => "Left",
            Position::Right => "Right",
        })
    }
}

/// An audio device choice: `None` is the system default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device(pub Option<String>);

impl Device {
    pub fn from_config(value: &str) -> Device {
        Device((!value.is_empty() && value != "default").then(|| value.to_owned()))
    }

    pub fn as_config(&self) -> String {
        self.0.clone().unwrap_or_else(|| "default".into())
    }
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(name) => f.write_str(name),
            None => f.write_str("System default"),
        }
    }
}

/// `voice.mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    VoiceActivity,
    PushToTalk,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::VoiceActivity, Mode::PushToTalk];

    pub fn from_config(value: &str) -> Mode {
        if value == "push_to_talk" {
            Mode::PushToTalk
        } else {
            Mode::VoiceActivity
        }
    }

    pub fn as_config(self) -> &'static str {
        match self {
            Mode::VoiceActivity => "voice_activity",
            Mode::PushToTalk => "push_to_talk",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Mode::VoiceActivity => "Voice activity",
            Mode::PushToTalk => "Push-to-talk",
        })
    }
}

/// `voice.join_on`, as offered in the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinOn(pub crate::voice::JoinOn);

impl JoinOn {
    pub const ALL: [JoinOn; 2] = [
        JoinOn(crate::voice::JoinOn::DoubleClick),
        JoinOn(crate::voice::JoinOn::SingleClick),
    ];

    pub fn as_config(self) -> &'static str {
        match self.0 {
            crate::voice::JoinOn::DoubleClick => "double_click",
            crate::voice::JoinOn::SingleClick => "single_click",
        }
    }
}

impl fmt::Display for JoinOn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            crate::voice::JoinOn::DoubleClick => "Double-click",
            crate::voice::JoinOn::SingleClick => "Single click",
        })
    }
}

/// What the Voice & Audio tab shows besides `[voice]`.
pub struct VoiceView<'a> {
    /// `None` until listed.
    pub devices: Option<&'a AudioDevices>,
    /// Microphone level, dBFS.
    pub level: f32,
    pub mic_test: bool,
    /// The PTT key being typed (`None`: show the saved one).
    pub ptt_draft: Option<&'a str>,
    /// The screens that can be shared, primary first (listed with the devices).
    pub screens: &'a [String],
}

/// `voice.share_screen`, as offered in the settings: a screen's system name, or empty for the
/// primary one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareScreen(pub String);

impl fmt::Display for ShareScreen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            f.write_str("Primary screen")
        } else {
            // Windows names screens like "\\.\DISPLAY2".
            f.write_str(self.0.trim_start_matches(['\\', '.']))
        }
    }
}

/// `voice.share_resolution`, as offered in the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareResolution(pub &'static str);

impl ShareResolution {
    pub fn from_config(value: &str) -> ShareResolution {
        let all = crate::screen::capture::RESOLUTIONS;
        ShareResolution(all.into_iter().find(|r| *r == value).unwrap_or(all[0]))
    }
}

impl fmt::Display for ShareResolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            "native" => "Full size (up to 4K)",
            other => other,
        })
    }
}

/// `voice.share_fps`, as offered in the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareFps(pub u32);

impl fmt::Display for ShareFps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} fps", self.0)
    }
}

/// The config key and TOML value a [`Setting`] is saved as.
pub fn saved_value(config: &ClientConfig, setting: Setting) -> (&'static str, String) {
    match setting {
        Setting::FontSize => ("font.size", config.font.size.to_string()),
        Setting::UiScale => ("font.ui_scale", format!("{:.2}", config.font.ui_scale)),
    }
}

/// Which font a setting is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontSlot {
    /// `font.family`: all text.
    Ui,
    /// `font.mono_family`: code.
    Mono,
}

impl FontSlot {
    pub fn key(self) -> &'static str {
        match self {
            FontSlot::Ui => "font.family",
            FontSlot::Mono => "font.mono_family",
        }
    }
}

/// The font list entry that lets you type the name of an installed font.
pub const CUSTOM_FONT: &str = "Custom…";

/// A font row: the bundled families plus [`CUSTOM_FONT`], and a name field for a custom one.
fn font_picker<'a>(
    slot: FontSlot,
    family: &'a str,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let mut choices: Vec<String> = font::CHOICES.iter().map(|c| (*c).to_owned()).collect();
    choices.push(CUSTOM_FONT.to_owned());
    let custom = !font::CHOICES.contains(&family);
    let selected = if custom {
        CUSTOM_FONT.to_owned()
    } else {
        family.to_owned()
    };
    let picker = dropdown(
        choices,
        Some(selected),
        move |c| Message::Font(slot, c),
        fonts,
        theme,
    );
    if !custom {
        return picker.into();
    }
    column![
        picker,
        text_input("Installed font, e.g. Segoe UI", family)
            .padding(style::INPUT_PAD)
            .style(move |_, st| style::input(theme, st))
            .on_input(move |t| Message::FontText(slot, t))
            .on_submit(Message::SaveFont(slot))
            .font(fonts.ui)
            .size(fonts.size),
    ]
    .spacing(style::GAP_S)
    .into()
}

/// UI scales offered as one-click choices.
pub const SCALE_PRESETS: [f64; 5] = [0.9, 1.0, 1.1, 1.25, 1.5];

#[allow(clippy::too_many_arguments)]
pub fn view<'a>(
    config: &'a ClientConfig,
    themes: &'a [(String, Result<Theme, String>)],
    tab: Tab,
    ui_scale_draft: Option<f64>,
    return_to: &str,
    voice: &VoiceView<'a>,
    account: &crate::buffer::settings_tabs::AccountView<'a>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    use crate::buffer::settings_tabs as more;
    use crate::widget::icon as i;

    let tab_button = |glyph: char, name: &'static str, t: Tab| {
        nav_item(glyph, name, tab == t, Message::Tab(t), fonts, theme)
    };
    let nav = column![
        nav_title("User settings", fonts, theme),
        tab_button(i::ACCOUNT, "Account", Tab::Account),
        tab_button(i::PALETTE, "Appearance", Tab::Appearance),
        tab_button(i::CHAT, "Chat", Tab::Chat),
        tab_button(i::MIC, "Voice & Audio", Tab::Voice),
        tab_button(i::BELL, "Notifications", Tab::Notifications),
        tab_button(i::KEYBOARD, "Keybinds", Tab::Keybinds),
        tab_button(i::INFO, "About", Tab::About),
    ]
    .spacing(2);
    let content = match tab {
        Tab::Account => more::account(account, fonts, theme),
        Tab::Appearance => appearance(config, themes, ui_scale_draft, fonts, theme),
        Tab::Chat => more::chat(config, account.utc_offset, fonts, theme),
        Tab::Voice => voice_tab(config, voice, fonts, theme),
        Tab::Notifications => more::notifications(config, fonts, theme),
        Tab::Keybinds => more::keybinds(config, fonts, theme),
        Tab::About => more::about(fonts, theme),
    };
    crate::buffer::page_frame(
        nav.into(),
        scrollable(
            container(content)
                // Room on the right for the scrollbar, which is drawn over the content.
                .padding(
                    iced::Padding::ZERO
                        .top(style::GAP_XL)
                        .bottom(style::HEADER)
                        .right(style::GAP_L),
                )
                .width(Fill),
        )
        .style(move |_, st| style::scrollbar(theme, st))
        .height(Fill)
        .into(),
        Message::Close,
        return_to,
        fonts,
        theme,
    )
}

fn appearance<'a>(
    config: &'a ClientConfig,
    themes: &'a [(String, Result<Theme, String>)],
    ui_scale_draft: Option<f64>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let cards = row(themes.iter().map(|(name, loaded)| {
        crate::widget::theme_card::card(
            name,
            loaded,
            *name == config.general.theme,
            Message::Theme(name.clone()),
            fonts,
            theme,
        )
    }))
    .spacing(style::GAP)
    .wrap()
    .vertical_spacing(style::GAP);
    let scale = ui_scale_draft.unwrap_or(config.font.ui_scale);
    let saved_scale = config.font.ui_scale;
    let preset = |value: f64| {
        let selected = (saved_scale - value).abs() < 0.001 && ui_scale_draft.is_none();
        button(
            container(label(format!("{:.0}%", value * 100.0), fonts))
                .center_x(Fill)
                .center_y(style::CONTROL_H),
        )
        .padding(0)
        .width(Fill)
        .style(move |_, s| style::tab(theme, s, selected))
        .on_press(Message::UiScalePreset(value))
        .into()
    };
    let presets = row(SCALE_PRESETS.iter().map(|v| preset(*v))).spacing(style::GAP_XS);
    page(
        "Appearance",
        None,
        vec![
            group_with(
                "Theme",
                vec![
                    text_button("Open themes folder", ButtonKind::Secondary, fonts, theme)
                        .on_press(Message::OpenThemesFolder)
                        .into(),
                    text_button("Copy current theme", ButtonKind::Secondary, fonts, theme)
                        .on_press(Message::CopyTheme)
                        .into(),
                ],
                vec![
                    cards.into(),
                    crate::widget::caption(
                        "Your own themes are .toml files in the themes folder: copy one to start, \
                         and changes show up as soon as you save. Keys you leave out come from \
                         the dark theme.",
                        fonts,
                    )
                    .color(theme.text.tertiary.0)
                    .into(),
                ],
                fonts,
                theme,
            ),
            group(
                "Text",
                vec![
                    setting(
                        "Font",
                        "Used for all text. Pick Custom… to use any installed font (Enter saves).",
                        font_picker(FontSlot::Ui, &config.font.family, fonts, theme),
                        fonts,
                        theme,
                    ),
                    setting(
                        "Code font",
                        "Used for code blocks and inline code.",
                        font_picker(FontSlot::Mono, &config.font.mono_family, fonts, theme),
                        fonts,
                        theme,
                    ),
                    setting(
                        "Font size",
                        &format!("{} px", config.font.size),
                        slider(font::MIN_SIZE..=24, config.font.size, Message::FontSize)
                            .on_release(Message::Save(Setting::FontSize)),
                        fonts,
                        theme,
                    ),
                ],
                fonts,
                theme,
            ),
            group(
                "Layout",
                vec![
                    setting(
                        "UI scale",
                        &if ui_scale_draft.is_some() {
                            format!("{:.0} %, applied when you let go", scale * 100.0)
                        } else {
                            format!(
                                "{:.0} % on top of the system scaling. Ctrl+= and Ctrl+- \
                                 zoom, Ctrl+0 resets.",
                                scale * 100.0
                            )
                        },
                        column![
                            presets,
                            slider(font::MIN_SCALE..=font::MAX_SCALE, scale, Message::UiScale)
                                .step(0.05)
                                .on_release(Message::Save(Setting::UiScale)),
                        ]
                        .spacing(style::GAP_S),
                        fonts,
                        theme,
                    ),
                    setting(
                        "Sidebar position",
                        "",
                        dropdown(
                            Position::ALL,
                            Some(Position::from_config(&config.sidebar.position)),
                            Message::SidebarPosition,
                            fonts,
                            theme,
                        ),
                        fonts,
                        theme,
                    ),
                    setting(
                        "Panel widths",
                        &format!(
                            "Sidebar {} px, member list {} px. Drag their inner edge to resize; \
                             a double-click on it resets.",
                            config.sidebar.width, config.sidebar.member_list_width
                        ),
                        text_button("Reset", ButtonKind::Secondary, fonts, theme)
                            .on_press(Message::ResetWidths),
                        fonts,
                        theme,
                    ),
                ],
                fonts,
                theme,
            ),
        ],
        fonts,
        theme,
    )
}

/// The hint under the push-to-talk key field.
fn ptt_hint(text: &str) -> &'static str {
    if !text.trim().is_empty() && crate::ptt::virtual_keys(text).is_none() {
        "Unknown key. Examples: Mouse4, Mouse5, CapsLock, F13, V, Ctrl+Space."
    } else if cfg!(windows) {
        "Works in every app: Mouse4, Mouse5, CapsLock, F1–F24, letters, Ctrl+Space… Enter saves."
    } else {
        "On this system it only works while the Hermes window is focused. Enter saves."
    }
}

fn voice_tab<'a>(
    config: &'a ClientConfig,
    voice: &VoiceView<'a>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let v = &config.voice;
    let devices = |names: Option<&Vec<String>>| {
        let mut list = vec![Device(None)];
        list.extend(names.into_iter().flatten().map(|n| Device(Some(n.clone()))));
        list
    };
    let mode = Mode::from_config(&v.mode);
    let level = voice.level.clamp(METER_MIN, 0.0);
    let open = level >= v.vad_sensitivity as f32;
    let meter = progress_bar(METER_MIN..=0.0, level)
        .girth(6)
        .style(move |_| iced::widget::progress_bar::Style {
            background: theme.surface.hover.0.into(),
            bar: if open {
                theme.voice.speaking_ring.0
            } else {
                theme.text.tertiary.0
            }
            .into(),
            border: iced::Border::default().rounded(3),
        });
    let mic_test = voice.mic_test;

    let device_group = group(
        "Devices",
        vec![
            setting(
                "Microphone",
                "",
                dropdown(
                    devices(voice.devices.map(|d| &d.inputs)),
                    Some(Device::from_config(&v.input_device)),
                    Message::InputDevice,
                    fonts,
                    theme,
                ),
                fonts,
                theme,
            ),
            setting(
                "Speakers",
                "",
                dropdown(
                    devices(voice.devices.map(|d| &d.outputs)),
                    Some(Device::from_config(&v.output_device)),
                    Message::OutputDevice,
                    fonts,
                    theme,
                ),
                fonts,
                theme,
            ),
            setting(
                "Device list",
                "Plugged in a headset? List the devices again.",
                text_button("Refresh devices", ButtonKind::Secondary, fonts, theme)
                    .on_press(Message::RefreshDevices),
                fonts,
                theme,
            ),
        ],
        fonts,
        theme,
    );
    let volume_group = group(
        "Volume",
        vec![
            setting(
                "Microphone volume",
                &format!("{:.0} %", v.input_volume * 100.0),
                slider(0.0..=2.0, v.input_volume, Message::InputVolume)
                    .step(0.05)
                    .on_release(Message::SaveVoice),
                fonts,
                theme,
            ),
            setting(
                "Output volume",
                &format!("{:.0} %", v.output_volume * 100.0),
                slider(0.0..=2.0, v.output_volume, Message::OutputVolume)
                    .step(0.05)
                    .on_release(Message::SaveVoice),
                fonts,
                theme,
            ),
        ],
        fonts,
        theme,
    );
    let mut input_rows = vec![setting(
        "Input mode",
        "Voice activity sends while you talk; push-to-talk while a key is held.",
        dropdown(Mode::ALL, Some(mode), Message::Mode, fonts, theme),
        fonts,
        theme,
    )];
    if mode == Mode::PushToTalk {
        let ptt_text = voice.ptt_draft.unwrap_or(v.ptt_key.as_str());
        input_rows.push(setting(
            "Push-to-talk key",
            ptt_hint(ptt_text),
            text_input("e.g. Mouse4", ptt_text)
                .padding(style::INPUT_PAD)
                .style(move |_, st| style::input(theme, st))
                .id(PTT_INPUT)
                .on_input(Message::PttKey)
                .on_submit(Message::SavePttKey)
                .font(fonts.ui)
                .size(fonts.size),
            fonts,
            theme,
        ));
        input_rows.push(setting(
            "Release delay",
            &format!("{} ms", v.ptt_release_delay_ms),
            slider(0..=1000, v.ptt_release_delay_ms, Message::ReleaseDelay)
                .step(10u32)
                .on_release(Message::SaveVoice),
            fonts,
            theme,
        ));
    }
    input_rows.push(setting(
        "Voice activity sensitivity",
        &format!(
            "Opens above {} dBFS · microphone now {:.0} dBFS",
            v.vad_sensitivity, level
        ),
        column![
            meter,
            slider(
                METER_MIN as i32..=0,
                v.vad_sensitivity.clamp(METER_MIN as i32, 0),
                Message::VadSensitivity
            )
            .on_release(Message::SaveVoice),
        ]
        .spacing(style::GAP_XS),
        fonts,
        theme,
    ));
    input_rows.push(setting(
        "Mic test",
        "Hear your processed microphone (use headphones).",
        text_button(
            if mic_test { "Stop test" } else { "Start test" },
            if mic_test {
                ButtonKind::Danger
            } else {
                ButtonKind::Secondary
            },
            fonts,
            theme,
        )
        .on_press(Message::MicTest(!mic_test)),
        fonts,
        theme,
    ));
    let processing_group = group(
        "Processing",
        vec![
            toggle(
                "Noise suppression",
                if v.noise_suppression {
                    "Removes background noise like fans and keyboards."
                } else {
                    "Without it, headphones are recommended."
                },
                v.noise_suppression,
                Message::NoiseSuppression,
                fonts,
                theme,
            ),
            toggle(
                "Automatic gain",
                "Evens out how loud you are.",
                v.auto_gain,
                Message::AutoGain,
                fonts,
                theme,
            ),
        ],
        fonts,
        theme,
    );
    let sounds_group = group(
        "Sounds and joining",
        vec![
            toggle(
                "Play sounds",
                "When people join or leave, and when you mute.",
                v.sounds,
                Message::Sounds,
                fonts,
                theme,
            ),
            setting(
                "Sound volume",
                &format!("{:.0} %", v.sounds_volume * 100.0),
                slider(0.0..=1.0, v.sounds_volume, Message::SoundsVolume)
                    .step(0.05)
                    .on_release(Message::SaveVoice),
                fonts,
                theme,
            ),
            setting(
                "Join a voice channel with",
                "With double-click, a single click only opens the channel's voice room.",
                dropdown(
                    JoinOn::ALL,
                    Some(JoinOn(crate::voice::JoinOn::from_config(&v.join_on))),
                    Message::JoinOn,
                    fonts,
                    theme,
                ),
                fonts,
                theme,
            ),
        ],
        fonts,
        theme,
    );
    let mut screens = vec![ShareScreen(String::new())];
    screens.extend(voice.screens.iter().skip(1).cloned().map(ShareScreen));
    let screen_group = group(
        "Screen sharing",
        vec![
            setting(
                "Screen to share",
                "Only people in the call see it. Start and stop it with Share screen in the \
                 voice room.",
                dropdown(
                    screens,
                    Some(ShareScreen(v.share_screen.clone())),
                    Message::ShareScreen,
                    fonts,
                    theme,
                ),
                fonts,
                theme,
            ),
            setting(
                "Resolution",
                "The largest size your screen is sent at. Larger is sharper but uses more CPU \
                 and bandwidth. Applies the next time you share.",
                dropdown(
                    crate::screen::capture::RESOLUTIONS.map(ShareResolution),
                    Some(ShareResolution::from_config(&v.share_resolution)),
                    Message::ShareResolution,
                    fonts,
                    theme,
                ),
                fonts,
                theme,
            ),
            setting(
                "Frame rate",
                "Frames per second. More is smoother (video, games) but uses more bandwidth. \
                 Applies the next time you share.",
                dropdown(
                    crate::screen::capture::FRAME_RATES.map(ShareFps),
                    Some(ShareFps(v.share_fps)),
                    Message::ShareFps,
                    fonts,
                    theme,
                ),
                fonts,
                theme,
            ),
            setting(
                "When it cannot keep up",
                "What your share keeps when your connection or computer falls behind. \
                 Smoothness keeps the frame rate and lowers sharpness, then size; Clarity keeps \
                 sharp, full-size pictures and lowers the frame rate (and spends more bandwidth \
                 on every picture); Mixed is in between. Also on your tile while you share.",
                crate::voice::preference_picker(
                    crate::screen::capture::Preference::from_config(&v.share_preference),
                    Message::SharePreference,
                    fonts,
                    theme,
                ),
                fonts,
                theme,
            ),
        ],
        fonts,
        theme,
    );
    page(
        "Voice & Audio",
        None,
        vec![
            device_group,
            volume_group,
            group("Input", input_rows, fonts, theme),
            processing_group,
            sounds_group,
            screen_group,
        ],
        fonts,
        theme,
    )
}
