//! A right-click menu drawn at a point over the whole window. A click outside closes it.

use iced::widget::{Space, button, column, container, mouse_area, pin, row, rule, stack};
use iced::{Element, Fill, Point, Size};

use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::icon::{self, icon};
use crate::widget::label;

pub const WIDTH: f32 = 220.0;

pub struct Item<M> {
    pub label: String,
    pub message: M,
    pub danger: bool,
    /// A check mark in front (the current choice).
    pub checked: bool,
    /// Chosen with the arrow keys.
    pub selected: bool,
}

/// `base` with the menu over it at `at`, moved inside `window` if it would leave it. `None`
/// items are separators.
pub fn overlay<'a, M: Clone + 'a>(
    base: Element<'a, M>,
    items: Vec<Option<Item<M>>>,
    at: Point,
    window: Size,
    fonts: &Fonts,
    theme: &'a Theme,
    on_close: M,
) -> Element<'a, M> {
    let row_height = fonts.size * 1.3 + 14.0 + 2.0;
    let separators = items.iter().filter(|i| i.is_none()).count() as f32;
    let height = (items.len() as f32 - separators) * row_height + separators * 9.0 + 8.0;
    let x = at.x.min(window.width - WIDTH - 4.0).max(0.0);
    let y = at.y.min(window.height - height - 4.0).max(0.0);

    let mut list = column![].spacing(0);
    for item in items {
        let Some(item) = item else {
            list = list.push(
                container(rule::horizontal(1).style(move |_| style::rule(theme))).padding([4, 6]),
            );
            continue;
        };
        let danger = item.danger;
        let selected = item.selected;
        list = list.push(
            button(
                row![
                    container(if item.checked {
                        Element::from(icon(icon::CHECK, 14.0))
                    } else {
                        Element::from(Space::new())
                    })
                    .width(16),
                    label(item.label, fonts),
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center),
            )
            .width(Fill)
            .padding([7, 10])
            .style(move |_, s| {
                let mut st = style::sidebar_item(theme, s, selected);
                if danger {
                    st.text_color = theme.text.error.0;
                }
                st
            })
            .on_press(item.message),
        );
    }
    let menu = container(list)
        .padding(5)
        .width(WIDTH)
        .style(move |_| style::popover(theme));
    let catcher = mouse_area(container("").width(Fill).height(Fill))
        .on_press(on_close.clone())
        .on_right_press(on_close);
    stack![base, catcher, pin(menu).x(x).y(y)].into()
}
