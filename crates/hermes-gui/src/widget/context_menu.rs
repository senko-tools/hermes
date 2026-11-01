//! A right-click menu drawn at a point over the whole window. A click outside closes it.

use iced::widget::{Space, button, column, container, mouse_area, pin, row, rule, stack, text};
use iced::{Alignment, Color, Element, Fill, Point, Size};

use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::icon::{self, icon};
use crate::widget::label;

/// Narrowest and widest a menu gets; in between it fits its longest entry on one line.
pub const MIN_WIDTH: f32 = 220.0;
pub const MAX_WIDTH: f32 = 340.0;
/// Height of one entry.
const ROW: f32 = style::ROW_H;
const SEPARATOR: f32 = 9.0;
const HEADER: f32 = 24.0 + 2.0 * 8.0 + SEPARATOR;

/// A width that fits the longest of `labels` on one line (estimated from the text size).
fn width_for<'s>(labels: impl Iterator<Item = &'s str>, fonts: &Fonts) -> f32 {
    let longest = labels.map(|l| l.chars().count()).max().unwrap_or(0) as f32;
    // Icon, gaps, check mark and paddings around the label.
    let chrome = 18.0 + 16.0 + 2.0 * style::GAP + 2.0 * style::GAP_S + 2.0 * style::GAP_XS;
    (longest * fonts.size * 0.62 + chrome).clamp(MIN_WIDTH, MAX_WIDTH)
}

pub struct Item<M> {
    pub label: String,
    pub message: M,
    pub icon: char,
    /// Overrides the icon's color (status dots).
    pub icon_color: Option<Color>,
    pub danger: bool,
    /// A check mark at the end (the current choice).
    pub checked: bool,
    /// Chosen with the arrow keys.
    pub selected: bool,
}

/// What a menu is about, shown at its top: an icon (or an avatar) and a name.
pub struct Header<'a, M> {
    pub lead: Element<'a, M>,
    pub title: String,
}

/// `base` with the menu over it at `at`, moved inside `window` if it would leave it. `None`
/// items are separators.
#[allow(clippy::too_many_arguments)]
pub fn overlay<'a, M: Clone + 'a>(
    base: Element<'a, M>,
    header: Option<Header<'a, M>>,
    items: Vec<Option<Item<M>>>,
    at: Point,
    window: Size,
    fonts: &Fonts,
    theme: &'a Theme,
    on_close: M,
) -> Element<'a, M> {
    let separators = items.iter().filter(|i| i.is_none()).count() as f32;
    let height = (items.len() as f32 - separators) * ROW
        + separators * SEPARATOR
        + if header.is_some() { HEADER } else { 0.0 }
        + 10.0;
    let width = width_for(
        items
            .iter()
            .flatten()
            .map(|i| i.label.as_str())
            .chain(header.as_ref().map(|h| h.title.as_str())),
        fonts,
    );
    let x = at.x.min(window.width - width - 4.0).max(0.0);
    let y = at.y.min(window.height - height - 4.0).max(0.0);
    let separator = || {
        container(rule::horizontal(1).style(move |_| style::menu_separator(theme)))
            .padding([style::GAP_XS, style::GAP_S])
    };

    let mut list = column![].spacing(0);
    if let Some(header) = header {
        list = list
            .push(
                row![
                    container(header.lead).center_x(24).center_y(24),
                    text(header.title)
                        .font(fonts.bold())
                        .size(fonts.size)
                        .color(theme.text.primary.0)
                        .wrapping(text::Wrapping::None),
                ]
                .spacing(style::GAP_S)
                .align_y(Alignment::Center)
                .padding([style::GAP_S, style::GAP_S]),
            )
            .push(separator());
    }
    for item in items {
        let Some(item) = item else {
            list = list.push(separator());
            continue;
        };
        let danger = item.danger;
        let selected = item.selected;
        let icon_color = item.icon_color.unwrap_or(if danger {
            theme.text.error.0
        } else {
            theme.text.tertiary.0
        });
        let check: Element<'a, M> = if item.checked {
            icon(icon::CHECK, 16.0).color(theme.accent.base.0).into()
        } else {
            Space::new().into()
        };
        list = list.push(
            button(
                container(
                    row![
                        icon(item.icon, 18.0).color(icon_color),
                        label(item.label, fonts)
                            .wrapping(text::Wrapping::None)
                            .width(Fill),
                        check,
                    ]
                    .spacing(style::GAP)
                    .align_y(Alignment::Center),
                )
                .center_y(Fill),
            )
            .width(Fill)
            .height(ROW)
            .padding([0.0, style::GAP_S])
            .style(move |_, s| {
                let mut st = style::menu_item(theme, s, selected);
                if danger {
                    st.text_color = theme.text.error.0;
                }
                st
            })
            .on_press(item.message),
        );
    }
    let menu = container(list)
        .padding(style::GAP_XS)
        .width(width)
        .style(move |_| style::popover(theme));
    let catcher = mouse_area(container("").width(Fill).height(Fill))
        .on_press(on_close.clone())
        .on_right_press(on_close);
    stack![base, catcher, pin(menu).x(x).y(y)].into()
}
