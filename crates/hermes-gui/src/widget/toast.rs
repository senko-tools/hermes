//! A notification card at the top right of the window ("Created #general", errors): it drops
//! in as it appears, floats over the content instead of pushing it down, and closes with its X
//! (info ones also on their own after a few seconds).

use iced::widget::{Space, button, container, row, stack};
use iced::{Alignment, Border, Element, Fill, Shadow, Vector};

use crate::app::{Level, Status};
use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::icon::{self, icon};
use crate::widget::label;

const WIDTH: f32 = 380.0;

/// `t` is how far its opening animation is (0–1).
pub fn view<'a, M: Clone + 'a>(
    status: &Status,
    t: f32,
    on_close: M,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, M> {
    let (glyph, color) = match status.level {
        Level::Info => (icon::CHECK, theme.accent.base.0),
        Level::Warning => (icon::INFO, theme.text.warning.0),
        Level::Error => (icon::INFO, theme.text.error.0),
    };
    let close = button(icon(icon::CLOSE, 14.0))
        .padding(4)
        .style(move |_, s| style::ghost(theme, s, false))
        .on_press(on_close);
    let body = row![
        icon(glyph, style::ICON).color(color),
        label(status.text.clone(), fonts)
            .color(theme.text.primary.0)
            .width(Fill),
        close,
    ]
    .spacing(style::GAP)
    .align_y(Alignment::Center);
    let radius = style::radius(theme);
    let card = container(body)
        .padding([style::GAP, style::GAP])
        .width(WIDTH)
        .style(move |_| container::Style {
            background: Some(theme.surface.overlay.0.into()),
            border: Border {
                color: theme.general.border.0,
                width: 1.0,
                radius: radius.into(),
            },
            shadow: Shadow {
                color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.35 * t),
                offset: Vector::new(0.0, 4.0),
                blur_radius: 14.0,
            },
            ..container::Style::default()
        });
    // A bar in the level's color along the left edge, like a sidebar row's state.
    let bar = container(Space::new().width(3).height(Fill)).style(move |_| container::Style {
        background: Some(color.into()),
        border: Border {
            radius: iced::border::Radius::default()
                .top_left(radius)
                .bottom_left(radius),
            ..Border::default()
        },
        ..container::Style::default()
    });
    // Top right, just under the headers (clear of the composer and "Jump to present"),
    // dropping in 12 px as it appears.
    container(stack![card, bar])
        .width(Fill)
        .height(Fill)
        .align_right(Fill)
        .align_top(Fill)
        .padding(iced::Padding {
            top: style::HEADER + style::GAP_S + 12.0 * (t - 1.0),
            right: style::GAP_L,
            bottom: 0.0,
            left: 0.0,
        })
        .into()
}
