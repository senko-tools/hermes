//! iced styles built from a Hermes [`Theme`]. Widgets without a function here use iced's own
//! styles, which follow the palette from [`iced_theme`].

use iced::widget::{button, container, rule, scrollable, text_editor, text_input};
use iced::{Background, Border, Color, Padding, Shadow, Vector};

use super::{Button, Theme};
use crate::app::Level;

// Shared measurements, so rows, buttons and inputs line up across every screen.

/// Gaps and paddings, on a 4 px grid. Literal values are only for 1–3 px optical tweaks.
pub const GAP_XS: f32 = 4.0;
pub const GAP_S: f32 = 8.0;
pub const GAP: f32 = 12.0;
pub const GAP_L: f32 = 16.0;
pub const GAP_XL: f32 = 24.0;

/// Height of every text button, text input and drop-down.
pub const CONTROL_H: f32 = 32.0;
/// Padding of a button whose content is a row (icon and text): `CONTROL_H` tall with the
/// default 13 px text. Plain text buttons use `widget::text_button`, which is centered exactly.
pub const BUTTON_PAD: Padding = Padding {
    top: 7.5,
    bottom: 7.5,
    left: 14.0,
    right: 14.0,
};
/// Padding of a text input, matching the buttons' height.
pub const INPUT_PAD: Padding = Padding {
    top: 7.5,
    bottom: 7.5,
    left: 10.0,
    right: 10.0,
};
/// Side of an icon button, and the icon inside it.
pub const ICON_BUTTON: f32 = 32.0;
pub const ICON: f32 = 18.0;
/// Icons inside rows (sidebar, menus, settings nav).
pub const ROW_ICON: f32 = 16.0;
/// Padding of a sidebar, list or menu row: `CONTROL_H` tall with one line of text.
pub const ROW_PAD: Padding = Padding {
    top: 7.5,
    bottom: 7.5,
    left: 8.0,
    right: 8.0,
};
/// Height of an on/off switch.
pub const TOGGLER: f32 = 20.0;
/// Height of the pane header and the sidebar's top bar.
pub const HEADER: f32 = 48.0;

/// Corner radius of buttons, inputs and rows (`shape.radius`).
pub fn radius(theme: &Theme) -> f32 {
    f32::from(theme.shape.radius)
}

/// Corner radius of cards, dialogs and the composer (`shape.radius_large`).
pub fn radius_large(theme: &Theme) -> f32 {
    f32::from(theme.shape.radius_large)
}

/// Corner radius of small things: badges, chips.
pub fn radius_small(theme: &Theme) -> f32 {
    (radius(theme) * 2.0 / 3.0).round()
}

/// The iced theme for `theme`: drives the default look of every widget.
pub fn iced_theme(theme: &Theme) -> iced::Theme {
    iced::Theme::custom(
        theme.name.clone(),
        iced::theme::Palette {
            background: theme.surface.main.0,
            text: theme.text.primary.0,
            primary: theme.accent.base.0,
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
        background: Some(theme.surface.main.0.into()),
        ..container::Style::default()
    }
}

pub fn sidebar(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(theme.text.primary.0),
        background: Some(theme.surface.sidebar.0.into()),
        ..container::Style::default()
    }
}

/// The pane header and the sidebar's top bar: a hairline below, drawn as a hard shadow so the
/// other sides stay borderless.
pub fn pane_header(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(theme.text.primary.0),
        background: Some(theme.pane.header_bg.0.into()),
        shadow: Shadow {
            color: theme.general.border.0,
            offset: Vector::new(0.0, 1.0),
            blur_radius: 0.0,
        },
        ..container::Style::default()
    }
}

/// A bar at the bottom of the sidebar (voice panel, user panel).
pub fn sidebar_footer(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(theme.text.primary.0),
        background: Some(theme.surface.deep.0.into()),
        ..container::Style::default()
    }
}

/// A floating panel: menus, tooltips, the message toolbar, autocomplete.
pub fn popover(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(theme.text.primary.0),
        background: Some(theme.surface.overlay.0.into()),
        border: Border {
            color: theme.general.border.0,
            width: 1.0,
            radius: radius(theme).into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 14.0,
        },
        ..container::Style::default()
    }
}

/// A dialog card over the dimmed window.
pub fn card(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(theme.text.primary.0),
        background: Some(theme.surface.main.0.into()),
        border: Border {
            color: theme.general.border.0,
            width: 1.0,
            radius: radius_large(theme).into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
            offset: Vector::new(0.0, 8.0),
            blur_radius: 32.0,
        },
        ..container::Style::default()
    }
}

/// A grouped block on a page (settings sections, welcome actions).
pub fn panel(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(theme.text.primary.0),
        background: Some(theme.surface.raised.0.into()),
        border: Border {
            color: theme.general.border.0,
            width: 1.0,
            radius: radius_large(theme).into(),
        },
        ..container::Style::default()
    }
}

/// The message composer: a rounded field holding the attach button and the editor.
pub fn composer(theme: &Theme, focused: bool) -> container::Style {
    container::Style {
        text_color: Some(theme.text.primary.0),
        background: Some(theme.surface.raised.0.into()),
        border: Border {
            color: if focused {
                theme.accent.focus.0.scale_alpha(0.6)
            } else {
                theme.general.border.0
            },
            width: 1.0,
            radius: radius_large(theme).into(),
        },
        ..container::Style::default()
    }
}

/// A text field.
pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let accent = theme.accent.base.0;
    let border = match status {
        text_input::Status::Focused { .. } => theme.accent.focus.0,
        text_input::Status::Hovered => mix(theme.general.border.0, theme.text.tertiary.0, 0.5),
        _ => theme.general.border.0,
    };
    text_input::Style {
        background: Background::Color(theme.surface.raised.0),
        border: Border {
            color: border,
            width: 1.0,
            radius: radius(theme).into(),
        },
        icon: theme.text.tertiary.0,
        placeholder: theme.text.tertiary.0,
        value: theme.text.primary.0,
        selection: accent.scale_alpha(0.35),
    }
}

/// A drop-down, framed like [`input`].
pub fn pick_list(
    theme: &Theme,
    status: iced::widget::pick_list::Status,
) -> iced::widget::pick_list::Style {
    use iced::widget::pick_list::Status;
    let border = match status {
        Status::Opened { .. } => theme.accent.focus.0,
        Status::Hovered => mix(theme.general.border.0, theme.text.tertiary.0, 0.5),
        Status::Active => theme.general.border.0,
    };
    iced::widget::pick_list::Style {
        text_color: theme.text.primary.0,
        placeholder_color: theme.text.tertiary.0,
        handle_color: theme.text.secondary.0,
        background: Background::Color(theme.surface.raised.0),
        border: Border {
            color: border,
            width: 1.0,
            radius: radius(theme).into(),
        },
    }
}

/// The list a drop-down opens.
pub fn menu(theme: &Theme) -> iced::overlay::menu::Style {
    let popover = popover(theme);
    iced::overlay::menu::Style {
        background: Background::Color(theme.surface.overlay.0),
        border: popover.border,
        text_color: theme.text.primary.0,
        selected_text_color: theme.text.primary.0,
        selected_background: Background::Color(theme.sidebar.selected.0),
        shadow: popover.shadow,
    }
}

/// The editor inside [`composer`]: no frame of its own.
pub fn editor(theme: &Theme, _status: text_editor::Status) -> text_editor::Style {
    text_editor::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
        placeholder: theme.text.tertiary.0,
        value: theme.text.primary.0,
        selection: theme.accent.base.0.scale_alpha(0.35),
    }
}

/// A thin scrollbar without a rail that only darkens while used.
pub fn scrollbar(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let mut style = scrollable::default(&iced_theme(theme), status);
    let active = matches!(
        status,
        scrollable::Status::Hovered { .. } | scrollable::Status::Dragged { .. }
    );
    let scroller = if active {
        mix(theme.general.scrollbar.0, theme.text.tertiary.0, 0.4)
    } else {
        theme.general.scrollbar.0
    };
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: scroller.into(),
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
        },
    };
    style.vertical_rail = rail;
    style.horizontal_rail = rail;
    style
}

/// A borderless button that only shows its background on hover (header and toolbar
/// actions). `active` keeps it highlighted, in the accent color.
pub fn ghost(theme: &Theme, status: button::Status, active: bool) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: (hovered || active).then_some(Background::Color(theme.surface.hover.0)),
        text_color: if active {
            theme.accent.base.0
        } else if hovered {
            theme.icons.active.0
        } else {
            theme.icons.default.0
        },
        border: Border {
            radius: radius(theme).into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

/// `a` moved towards `b` by `t` (0–1).
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// A `0xRRGGBB` role color.
pub fn rgb(color: u32) -> Color {
    Color::from_rgb8((color >> 16) as u8, (color >> 8) as u8, color as u8)
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
        background: Some(theme.surface.raised.0.into()),
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
    filled(theme, &theme.buttons.primary, status)
}

pub fn danger(theme: &Theme, status: button::Status) -> button::Style {
    filled(theme, &theme.buttons.danger, status)
}

/// A tab header; `selected` uses the primary colors.
pub fn tab(theme: &Theme, status: button::Status, selected: bool) -> button::Style {
    if selected {
        filled(theme, &theme.buttons.primary, status)
    } else {
        filled(theme, &theme.buttons.secondary, status)
    }
}

pub fn secondary(theme: &Theme, status: button::Status) -> button::Style {
    filled(theme, &theme.buttons.secondary, status)
}

/// A flat row in the sidebar; `selected` keeps the highlight.
pub fn sidebar_item(theme: &Theme, status: button::Status, selected: bool) -> button::Style {
    let background = if selected {
        Some(theme.sidebar.selected.0)
    } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
        Some(theme.surface.hover.0)
    } else {
        None
    };
    button::Style {
        background: background.map(Background::Color),
        text_color: theme.text.primary.0,
        border: Border {
            radius: radius(theme).into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

fn filled(theme: &Theme, colors: &Button, status: button::Status) -> button::Style {
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
            radius: radius(theme).into(),
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

/// The dot color of someone's status, as others see it.
pub fn presence(theme: &Theme, status: hermes_client_core::proto::models::Status) -> Color {
    use hermes_client_core::proto::models::Status;
    match status {
        Status::Online => theme.presence.online.0,
        Status::Idle => theme.presence.idle.0,
        Status::Dnd => theme.presence.dnd.0,
        Status::Offline => theme.presence.offline.0,
    }
}

/// The dot color of the status I chose (invisible looks offline).
pub fn my_presence(
    theme: &Theme,
    status: hermes_client_core::proto::models::PresenceStatus,
) -> Color {
    use hermes_client_core::proto::models::PresenceStatus;
    match status {
        PresenceStatus::Online => theme.presence.online.0,
        PresenceStatus::Idle => theme.presence.idle.0,
        PresenceStatus::Dnd => theme.presence.dnd.0,
        PresenceStatus::Invisible => theme.presence.offline.0,
    }
}

/// A row whose right-click menu is open: highlighted and outlined in the accent color, so it is
/// clear what the menu acts on.
pub fn menu_open(theme: &Theme) -> button::Style {
    button::Style {
        background: Some(Background::Color(theme.sidebar.selected.0)),
        text_color: theme.text.primary.0,
        border: Border {
            color: theme.accent.base.0.scale_alpha(0.7),
            width: 1.0,
            radius: radius(theme).into(),
        },
        ..button::Style::default()
    }
}

/// An on/off switch: the accent color when on.
pub fn toggler(
    theme: &Theme,
    status: iced::widget::toggler::Status,
) -> iced::widget::toggler::Style {
    use iced::widget::toggler::Status;
    let (on, hovered) = match status {
        Status::Active { is_toggled } => (is_toggled, false),
        Status::Hovered { is_toggled } => (is_toggled, true),
        Status::Disabled { is_toggled } => (is_toggled, false),
    };
    let track = if on {
        theme.accent.base.0
    } else if hovered {
        mix(
            theme.buttons.secondary.background.0,
            theme.text.tertiary.0,
            0.4,
        )
    } else {
        theme.buttons.secondary.background.0
    };
    iced::widget::toggler::Style {
        background: Background::Color(track),
        background_border_width: 0.0,
        background_border_color: Color::TRANSPARENT,
        foreground: Background::Color(if on {
            theme.buttons.primary.text.0
        } else {
            theme.text.secondary.0
        }),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: Some(theme.text.primary.0),
        border_radius: None,
        padding_ratio: 0.18,
    }
}
