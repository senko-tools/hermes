//! The Settings buffer (PLAN.md section 15.1): Account, Appearance, Chat, Voice & Audio,
//! Notifications, Keybinds and About (the last tabs are in `settings_tabs`). Every change is
//! written to `config.toml`.

use std::fmt;

use hermes_client_core::audio::AudioDevices;
use hermes_client_core::config::ClientConfig;
use iced::widget::{
    button, column, container, pick_list, progress_bar, row, rule, scrollable, slider, space,
    text_input,
};
use iced::{Alignment, Element, Fill, Length};

use crate::font::{self, Fonts};
use crate::theme::{Theme, style};
use crate::widget::{heading, label, small, strong};

pub const MIN_SIDEBAR_WIDTH: u16 = 160;
pub const MAX_SIDEBAR_WIDTH: u16 = 480;
/// The level meter and the voice activity slider share this range (dBFS).
pub const METER_MIN: f32 = -70.0;
pub const PTT_INPUT: &str = "settings-ptt-key";

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Theme(String),
    /// A family picked from the font list ([`CUSTOM_FONT`] shows the name field).
    Font(FontSlot, String),
    /// The custom family name field changed.
    FontText(FontSlot, String),
    /// Enter in the custom family name field: save it.
    SaveFont(FontSlot),
    FontSize(u16),
    UiScale(f64),
    SidebarPosition(Position),
    SidebarWidth(u16),
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
}

/// A slider-driven setting, saved on release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    FontSize,
    UiScale,
    SidebarWidth,
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
}

/// The config key and TOML value a [`Setting`] is saved as.
pub fn saved_value(config: &ClientConfig, setting: Setting) -> (&'static str, String) {
    match setting {
        Setting::FontSize => ("font.size", config.font.size.to_string()),
        Setting::UiScale => ("font.ui_scale", format!("{:.2}", config.font.ui_scale)),
        Setting::SidebarWidth => ("sidebar.width", config.sidebar.width.to_string()),
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
    let picker = pick_list(choices, Some(selected), move |c| Message::Font(slot, c))
        .padding(style::INPUT_PAD)
        .style(move |_, st| style::pick_list(theme, st))
        .menu_style(move |_| style::menu(theme))
        .font(fonts.ui)
        .text_size(fonts.size)
        .width(240);
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
            .size(fonts.size)
            .width(220),
    ]
    .spacing(style::GAP)
    .into()
}

pub fn view<'a>(
    config: &'a ClientConfig,
    themes: &'a [String],
    tab: Tab,
    voice: &VoiceView<'a>,
    account: &crate::buffer::settings_tabs::AccountView<'a>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    use crate::buffer::settings_tabs as more;

    let tab_button = |glyph: char, name: &'static str, t: Tab| {
        let selected = tab == t;
        button(
            row![
                crate::widget::icon::icon(glyph, 16.0).color(if selected {
                    theme.buttons.primary.background.0
                } else {
                    theme.text.tertiary.0
                }),
                label(name, fonts).color(if selected {
                    theme.text.primary.0
                } else {
                    theme.text.secondary.0
                }),
            ]
            .spacing(style::GAP_L)
            .align_y(Alignment::Center),
        )
        .padding([8, 12])
        .width(Fill)
        .style(move |_, s| style::sidebar_item(theme, s, selected))
        .on_press(Message::Tab(t))
    };
    use crate::widget::icon as i;
    let nav = column![
        container(
            iced::widget::text("USER SETTINGS")
                .font(fonts.bold())
                .size(fonts.small() - 0.5)
                .color(theme.text.tertiary.0)
        )
        .padding([4, 12]),
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
        Tab::Appearance => appearance(config, themes, fonts, theme),
        Tab::Chat => more::chat(config, fonts, theme),
        Tab::Voice => voice_tab(config, voice, fonts, theme),
        Tab::Notifications => more::notifications(config, fonts, theme),
        Tab::Keybinds => more::keybinds(config, fonts, theme),
        Tab::About => more::about(fonts, theme),
    };
    crate::buffer::page_frame(
        nav.into(),
        scrollable(container(content).padding([48, 0]).width(Fill))
            .style(move |_, st| style::scrollbar(theme, st))
            .height(Fill)
            .into(),
        Message::Close,
        fonts,
        theme,
    )
}

fn appearance<'a>(
    config: &'a ClientConfig,
    themes: &'a [String],
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let current_theme = themes.iter().find(|t| **t == config.general.theme).cloned();
    column![
        heading("Appearance", fonts),
        setting(
            "Theme",
            "Built-in themes, plus any .toml file in the themes folder of the config directory.",
            pick_list(themes, current_theme, Message::Theme)
                .padding(style::INPUT_PAD)
                .style(move |_, st| style::pick_list(theme, st))
                .menu_style(move |_| style::menu(theme))
                .font(fonts.ui)
                .text_size(fonts.size)
                .width(220),
            fonts,
            theme,
        ),
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
                .on_release(Message::Save(Setting::FontSize))
                .width(220),
            fonts,
            theme,
        ),
        setting(
            "UI scale",
            &format!(
                "{:.0} % (on top of the system scaling)",
                fonts.scale * 100.0
            ),
            slider(
                font::MIN_SCALE..=font::MAX_SCALE,
                config.font.ui_scale,
                Message::UiScale
            )
            .step(0.05)
            .on_release(Message::Save(Setting::UiScale))
            .width(220),
            fonts,
            theme,
        ),
        setting(
            "Sidebar position",
            "",
            pick_list(
                Position::ALL,
                Some(Position::from_config(&config.sidebar.position)),
                Message::SidebarPosition
            )
            .padding(style::INPUT_PAD)
            .style(move |_, st| style::pick_list(theme, st))
            .menu_style(move |_| style::menu(theme))
            .font(fonts.ui)
            .text_size(fonts.size)
            .width(220),
            fonts,
            theme,
        ),
        setting(
            "Sidebar width",
            &format!("{} px", config.sidebar.width),
            slider(
                MIN_SIDEBAR_WIDTH..=MAX_SIDEBAR_WIDTH,
                config
                    .sidebar
                    .width
                    .clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH),
                Message::SidebarWidth
            )
            .step(10u16)
            .on_release(Message::Save(Setting::SidebarWidth))
            .width(220),
            fonts,
            theme,
        ),
    ]
    .spacing(14)
    .max_width(720)
    .into()
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
    let check = |on: bool, text: &'static str, f: fn(bool) -> Message| {
        iced::widget::toggler(on)
            .label(text)
            .size(20)
            .spacing(style::GAP_L)
            .style(move |_, st| style::toggler(theme, st))
            .on_toggle(f)
            .text_size(fonts.size)
            .font(fonts.ui)
    };
    let picker_width = 260;
    let mode = Mode::from_config(&v.mode);
    let level = voice.level.clamp(METER_MIN, 0.0);
    let open = level >= v.vad_sensitivity as f32;
    let meter = progress_bar(METER_MIN..=0.0, level)
        .girth(8)
        .style(move |_| iced::widget::progress_bar::Style {
            background: theme.sidebar.hover.0.into(),
            bar: if open {
                theme.voice.speaking_ring.0
            } else {
                theme.text.tertiary.0
            }
            .into(),
            border: iced::Border::default().rounded(4),
        });
    let mic_test = voice.mic_test;

    let mut section = column![
        heading("Devices", fonts),
        setting(
            "Microphone",
            "",
            pick_list(
                devices(voice.devices.map(|d| &d.inputs)),
                Some(Device::from_config(&v.input_device)),
                Message::InputDevice
            )
            .padding(style::INPUT_PAD)
            .style(move |_, st| style::pick_list(theme, st))
            .menu_style(move |_| style::menu(theme))
            .font(fonts.ui)
            .text_size(fonts.size)
            .width(picker_width),
            fonts,
            theme,
        ),
        setting(
            "Speakers",
            "",
            pick_list(
                devices(voice.devices.map(|d| &d.outputs)),
                Some(Device::from_config(&v.output_device)),
                Message::OutputDevice
            )
            .padding(style::INPUT_PAD)
            .style(move |_, st| style::pick_list(theme, st))
            .menu_style(move |_| style::menu(theme))
            .font(fonts.ui)
            .text_size(fonts.size)
            .width(picker_width),
            fonts,
            theme,
        ),
        row![
            space::horizontal(),
            button(small("Refresh devices", fonts))
                .padding(style::BUTTON_PAD)
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::RefreshDevices),
        ],
        setting(
            "Microphone volume",
            &format!("{:.0} %", v.input_volume * 100.0),
            slider(0.0..=2.0, v.input_volume, Message::InputVolume)
                .step(0.05)
                .on_release(Message::SaveVoice)
                .width(picker_width),
            fonts,
            theme,
        ),
        setting(
            "Output volume",
            &format!("{:.0} %", v.output_volume * 100.0),
            slider(0.0..=2.0, v.output_volume, Message::OutputVolume)
                .step(0.05)
                .on_release(Message::SaveVoice)
                .width(picker_width),
            fonts,
            theme,
        ),
        strong("Input", fonts),
        setting(
            "Input mode",
            "Voice activity sends while you talk; push-to-talk while a key is held.",
            pick_list(Mode::ALL, Some(mode), Message::Mode)
                .padding(style::INPUT_PAD)
                .style(move |_, st| style::pick_list(theme, st))
                .menu_style(move |_| style::menu(theme))
                .font(fonts.ui)
                .text_size(fonts.size)
                .width(picker_width),
            fonts,
            theme,
        ),
    ]
    .spacing(14)
    .max_width(720);
    if mode == Mode::PushToTalk {
        let ptt_text = voice.ptt_draft.unwrap_or(v.ptt_key.as_str());
        section = section
            .push(setting(
                "Push-to-talk key",
                ptt_hint(ptt_text),
                text_input("e.g. Mouse4", ptt_text)
                    .padding(style::INPUT_PAD)
                    .style(move |_, st| style::input(theme, st))
                    .id(PTT_INPUT)
                    .on_input(Message::PttKey)
                    .on_submit(Message::SavePttKey)
                    .font(fonts.ui)
                    .size(fonts.size)
                    .width(picker_width),
                fonts,
                theme,
            ))
            .push(setting(
                "Release delay",
                &format!("{} ms", v.ptt_release_delay_ms),
                slider(0..=1000, v.ptt_release_delay_ms, Message::ReleaseDelay)
                    .step(10u32)
                    .on_release(Message::SaveVoice)
                    .width(picker_width),
                fonts,
                theme,
            ));
    }
    section = section
        .push(setting(
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
            .spacing(6)
            .width(picker_width),
            fonts,
            theme,
        ))
        .push(setting(
            "Mic test",
            "Hear your processed microphone (use headphones).",
            button(label(
                if mic_test { "Stop test" } else { "Start test" },
                fonts,
            ))
            .style(move |_, s| {
                if mic_test {
                    style::danger(theme, s)
                } else {
                    style::secondary(theme, s)
                }
            })
            .on_press(Message::MicTest(!mic_test)),
            fonts,
            theme,
        ))
        .push(setting(
            "Processing",
            if v.noise_suppression {
                ""
            } else {
                "Without noise suppression, headphones are recommended."
            },
            column![
                check(
                    v.noise_suppression,
                    "Noise suppression",
                    Message::NoiseSuppression
                ),
                check(v.auto_gain, "Automatic gain", Message::AutoGain),
            ]
            .spacing(6),
            fonts,
            theme,
        ))
        .push(strong("Other", fonts))
        .push(setting(
            "Sounds",
            &format!(
                "Join, leave and mute sounds at {:.0} %",
                v.sounds_volume * 100.0
            ),
            column![
                check(v.sounds, "Play sounds", Message::Sounds),
                slider(0.0..=1.0, v.sounds_volume, Message::SoundsVolume)
                    .step(0.05)
                    .on_release(Message::SaveVoice),
            ]
            .spacing(6)
            .width(picker_width),
            fonts,
            theme,
        ))
        .push(setting(
            "Join a voice channel with",
            "With double-click, a single click only opens the channel's chat.",
            pick_list(
                JoinOn::ALL,
                Some(JoinOn(crate::voice::JoinOn::from_config(&v.join_on))),
                Message::JoinOn,
            )
            .padding(style::INPUT_PAD)
            .style(move |_, st| style::pick_list(theme, st))
            .menu_style(move |_| style::menu(theme))
            .font(fonts.ui)
            .text_size(fonts.size)
            .width(picker_width),
            fonts,
            theme,
        ));
    section.into()
}

pub(crate) fn setting<'a>(
    name: &'a str,
    hint: &str,
    control: impl Into<Element<'a, Message>>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    column![
        rule::horizontal(1).style(move |_| style::rule(theme)),
        row![
            column![
                label(name, fonts).font(fonts.medium()),
                small(hint.to_owned(), fonts).color(theme.text.tertiary.0)
            ]
            .spacing(3)
            .width(Length::Fill),
            control.into(),
        ]
        .spacing(16)
        .align_y(Alignment::Center),
    ]
    .spacing(14)
    .into()
}
