//! The Settings buffer (PLAN.md section 15.1). Only the Appearance tab exists so far; the other
//! tabs come with the features they configure. Every change is written to `config.toml`.

use std::fmt;

use hermes_client_core::config::ClientConfig;
use iced::widget::{button, column, container, pick_list, row, rule, scrollable, slider, space};
use iced::{Alignment, Element, Fill, Length};

use crate::font::{self, Fonts};
use crate::theme::{Theme, style};
use crate::widget::{heading, label, small, strong};

pub const MIN_SIDEBAR_WIDTH: u16 = 160;
pub const MAX_SIDEBAR_WIDTH: u16 = 480;

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Theme(String),
    FontSize(u16),
    UiScale(f64),
    SidebarPosition(Position),
    SidebarWidth(u16),
    /// A slider was released: save what it changed.
    Save(Setting),
    Close,
}

/// A slider-driven setting, saved on release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    FontSize,
    UiScale,
    SidebarWidth,
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

/// The config key and TOML value a [`Setting`] is saved as.
pub fn saved_value(config: &ClientConfig, setting: Setting) -> (&'static str, String) {
    match setting {
        Setting::FontSize => ("font.size", config.font.size.to_string()),
        Setting::UiScale => ("font.ui_scale", format!("{:.2}", config.font.ui_scale)),
        Setting::SidebarWidth => ("sidebar.width", config.sidebar.width.to_string()),
    }
}

pub fn view<'a>(
    config: &'a ClientConfig,
    themes: &'a [String],
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let header = container(
        row![
            heading("Settings", fonts),
            space::horizontal(),
            button(label("Close", fonts))
                .style(|_, status| style::secondary(theme, status))
                .on_press(Message::Close),
        ]
        .align_y(Alignment::Center),
    )
    .padding([8, 12])
    .width(Fill)
    .style(|_| style::pane_header(theme));

    let current_theme = themes.iter().find(|t| **t == config.general.theme).cloned();
    let appearance = column![
        strong("Appearance", fonts),
        setting(
            "Theme",
            "Built-in themes, plus any .toml file in the themes folder of the config directory.",
            pick_list(themes, current_theme, Message::Theme)
                .font(fonts.ui)
                .text_size(fonts.size)
                .width(220),
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
    .max_width(720);

    column![
        header,
        scrollable(container(appearance).padding(20).width(Fill)).height(Fill),
    ]
    .into()
}

fn setting<'a>(
    name: &'a str,
    hint: &str,
    control: impl Into<Element<'a, Message>>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    column![
        rule::horizontal(1).style(move |_| style::rule(theme)),
        row![
            column![label(name, fonts), small(hint.to_owned(), fonts)]
                .spacing(2)
                .width(Length::Fill),
            control.into(),
        ]
        .spacing(16)
        .align_y(Alignment::Center),
    ]
    .spacing(10)
    .into()
}
