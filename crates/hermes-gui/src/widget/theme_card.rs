//! A theme drawn in miniature (sidebar with a selected and an unread row, chat lines, an accent
//! button), for picking themes by look.

use iced::widget::{Space, button, column, container, row, stack};
use iced::{Alignment, Border, Color, Element, Fill};

use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::{caption, icon, label, tip};

pub const WIDTH: f32 = 160.0;
const PREVIEW_H: f32 = 96.0;

/// A rounded bar of `color`, standing in for a line of text.
fn bar<'a, M: 'a>(width: f32, height: f32, color: Color) -> Element<'a, M> {
    container(Space::new().width(width).height(height))
        .style(move |_| container::Style {
            background: Some(color.into()),
            border: Border {
                radius: (height / 2.0).into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

/// The miniature of `t`.
fn preview<'a, M: 'a>(t: &Theme) -> Element<'a, M> {
    let t = t.clone();
    let row_of = |state_bar: Option<Color>, bg: Color, text: Color, width: f32| -> Element<'a, M> {
        let indicator = f32::from(t.shape.indicator_width).max(1.0);
        container(
            row![
                bar(indicator, 10.0, state_bar.unwrap_or(Color::TRANSPARENT)),
                bar(width, 4.0, text),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .width(Fill)
        .padding([2, 0])
        .style(move |_| container::Style {
            background: Some(bg.into()),
            ..container::Style::default()
        })
        .into()
    };
    let sidebar = container(
        column![
            bar(28.0, 5.0, t.accent.highlight.0),
            row_of(None, Color::TRANSPARENT, t.text.secondary.0, 22.0),
            row_of(
                Some(t.channel.selected_border.0),
                t.channel.selected_bg.0,
                t.text.primary.0,
                26.0
            ),
            row_of(
                Some(t.channel.unread_border.0),
                t.channel.unread_bg.0,
                t.text.primary.0,
                18.0
            ),
            row_of(None, Color::TRANSPARENT, t.text.tertiary.0, 24.0),
        ]
        .spacing(4),
    )
    .padding(6)
    .width(52)
    .height(Fill)
    .style({
        let bg = t.surface.sidebar.0;
        move |_| container::Style {
            background: Some(bg.into()),
            ..container::Style::default()
        }
    });
    let line = |avatar: Color, w1: f32, w2: f32| -> Element<'a, M> {
        row![
            container(Space::new().width(10).height(10)).style(move |_| container::Style {
                background: Some(avatar.into()),
                border: Border {
                    radius: 5.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            }),
            column![
                bar(w1, 4.0, t.text.primary.0),
                bar(w2, 4.0, t.text.secondary.0)
            ]
            .spacing(3),
        ]
        .spacing(5)
        .into()
    };
    let radius = f32::from(t.shape.radius);
    let accent = t.accent.base.0;
    let composer_bg = t.surface.raised.0;
    let chat = container(
        column![
            line(t.accent.muted.0, 40.0, 60.0),
            line(t.presence.online.0, 30.0, 48.0),
            Space::new().height(Fill),
            row![
                container(Space::new().width(Fill).height(12)).style(move |_| container::Style {
                    background: Some(composer_bg.into()),
                    border: Border {
                        radius: radius.min(4.0).into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                }),
                container(Space::new().width(18).height(12)).style(move |_| container::Style {
                    background: Some(accent.into()),
                    border: Border {
                        radius: radius.min(4.0).into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                }),
            ]
            .spacing(4),
        ]
        .spacing(6),
    )
    .padding(8)
    .width(Fill)
    .height(Fill)
    .style({
        let bg = t.surface.main.0;
        move |_| container::Style {
            background: Some(bg.into()),
            ..container::Style::default()
        }
    });
    row![sidebar, chat].height(PREVIEW_H).into()
}

/// A card for theme `name`: its miniature (or what is wrong with its file) and its name; the
/// current theme is outlined in the accent color.
pub fn card<'a, M: Clone + 'a>(
    name: &'a str,
    loaded: &'a Result<Theme, String>,
    selected: bool,
    on_press: M,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, M> {
    let picture: Element<'a, M> = match loaded {
        Ok(t) => preview(t),
        Err(err) => tip(
            container(icon::icon(icon::INFO, 24.0).color(theme.text.warning.0))
                .center_x(Fill)
                .center_y(PREVIEW_H),
            err,
            fonts,
            theme,
        ),
    };
    let border = if selected {
        theme.accent.base.0
    } else {
        theme.general.border.0
    };
    let radius = style::radius(theme);
    // The outline is a layer of its own, so the picture does not paint over it.
    let outline =
        container(Space::new().width(Fill).height(PREVIEW_H)).style(move |_| container::Style {
            border: Border {
                color: border,
                width: if selected { 2.0 } else { 1.0 },
                radius: radius.into(),
            },
            ..container::Style::default()
        });
    let framed = stack![container(picture).clip(true), outline];
    let mut title = row![label(name, fonts).color(if selected {
        theme.text.primary.0
    } else {
        theme.text.secondary.0
    })]
    .spacing(style::GAP_XS)
    .align_y(Alignment::Center);
    if selected {
        title = title.push(caption("in use", fonts).color(theme.accent.base.0));
    }
    button(column![framed, title].spacing(style::GAP_S))
        .padding(0)
        .width(WIDTH)
        .style(move |_, _| button::Style::default())
        .on_press(on_press)
        .into()
}
