//! iced styles built from a Hermes [`Theme`]. Widgets without a function here use iced's own
//! styles, which follow the palette from [`iced_theme`].

use iced::widget::{button, container, rule};
use iced::{Background, Border, Color};

use super::{Button, Theme};
use crate::app::Level;

/// The iced theme for `theme`: drives the default look of every widget.
pub fn iced_theme(theme: &Theme) -> iced::Theme {
    iced::Theme::custom(
        theme.name.clone(),
        iced::theme::Palette {
            background: theme.general.background.0,
            text: theme.text.primary.0,
            primary: theme.buttons.primary.background.0,
            success: theme.text.success.0,
            warning: theme.text.warning.0,
            danger: theme.text.error.0,
        },
    )
}

/// The window background.
pub fn app(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(theme.text.primary.0),
        background: Some(theme.general.background.0.into()),
        ..container::Style::default()
    }
}

pub fn sidebar(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(theme.text.primary.0),
        background: Some(theme.sidebar.background.0.into()),
        ..container::Style::default()
    }
}

pub fn pane_header(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(theme.text.primary.0),
        background: Some(theme.pane.header_bg.0.into()),
        border: Border {
            color: theme.general.border.0,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

/// A one-line message bar (warnings, errors).
pub fn status_bar(theme: &Theme, level: Level) -> container::Style {
    let color = match level {
        Level::Info => theme.text.info.0,
        Level::Warning => theme.text.warning.0,
        Level::Error => theme.text.error.0,
    };
    container::Style {
        text_color: Some(color),
        background: Some(theme.text.code_bg.0.into()),
        border: Border {
            color,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn rule(theme: &Theme) -> rule::Style {
    rule::Style {
        color: theme.general.horizontal_rule.0,
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

pub fn primary(theme: &Theme, status: button::Status) -> button::Style {
    filled(&theme.buttons.primary, status)
}

pub fn danger(theme: &Theme, status: button::Status) -> button::Style {
    filled(&theme.buttons.danger, status)
}

/// A tab header; `selected` uses the primary colors.
pub fn tab(theme: &Theme, status: button::Status, selected: bool) -> button::Style {
    if selected {
        filled(&theme.buttons.primary, status)
    } else {
        filled(&theme.buttons.secondary, status)
    }
}

pub fn secondary(theme: &Theme, status: button::Status) -> button::Style {
    filled(&theme.buttons.secondary, status)
}

/// A flat row in the sidebar; `selected` keeps the highlight.
pub fn sidebar_item(theme: &Theme, status: button::Status, selected: bool) -> button::Style {
    let background = if selected {
        Some(theme.sidebar.selected.0)
    } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
        Some(theme.sidebar.hover.0)
    } else {
        None
    };
    button::Style {
        background: background.map(Background::Color),
        text_color: theme.text.primary.0,
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

fn filled(colors: &Button, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => colors.background_hover.0,
        button::Status::Active => colors.background.0,
        button::Status::Disabled => fade(colors.background.0),
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: if status == button::Status::Disabled {
            fade(colors.text.0)
        } else {
            colors.text.0
        },
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

fn fade(color: Color) -> Color {
    Color {
        a: color.a * 0.5,
        ..color
    }
}
