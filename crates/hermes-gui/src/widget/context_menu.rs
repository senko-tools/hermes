//! A right-click menu drawn at a point over the whole window. A click outside closes it.

use iced::widget::{button, column, container, mouse_area, pin, rule, stack};
use iced::{Border, Element, Fill, Point, Shadow, Size, Vector};

use crate::font::Fonts;
use crate::theme::{Theme, style};
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
    let row_height = fonts.size + 14.0;
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
        let text = if item.checked {
            format!("✓ {}", item.label)
        } else {
            format!("   {}", item.label)
        };
        list = list.push(
            button(label(text, fonts).color(if danger {
                theme.text.error.0
            } else {
                theme.text.primary.0
            }))
            .width(Fill)
            .padding([5, 10])
            .style(move |_, s| style::sidebar_item(theme, s, selected))
            .on_press(item.message),
        );
    }
    let menu =
        container(list)
            .padding(4)
            .width(WIDTH)
            .style(move |_| iced::widget::container::Style {
                background: Some(theme.sidebar.background.0.into()),
                border: Border {
                    color: theme.general.border.0,
                    width: 1.0,
                    radius: 6.0.into(),
                },
                shadow: Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.4),
                    offset: Vector::new(0.0, 2.0),
                    blur_radius: 8.0,
                },
                ..iced::widget::container::Style::default()
            });
    let catcher = mouse_area(container("").width(Fill).height(Fill))
        .on_press(on_close.clone())
        .on_right_press(on_close);
    stack![base, catcher, pin(menu).x(x).y(y)].into()
}
