//! A right-click menu drawn at a point over the whole window. A click outside closes it.

use iced::widget::{button, column, container, mouse_area, pin, stack};
use iced::{Border, Element, Fill, Point, Shadow, Size, Vector};

use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::label;

pub const WIDTH: f32 = 220.0;

pub struct Item<M> {
    pub label: &'static str,
    pub message: M,
    pub danger: bool,
}

/// `base` with the menu over it at `at`, moved inside `window` if it would leave it.
pub fn overlay<'a, M: Clone + 'a>(
    base: Element<'a, M>,
    items: Vec<Item<M>>,
    at: Point,
    window: Size,
    fonts: &Fonts,
    theme: &'a Theme,
    on_close: M,
) -> Element<'a, M> {
    let row_height = fonts.size + 14.0;
    let height = items.len() as f32 * row_height + 8.0;
    let x = at.x.min(window.width - WIDTH - 4.0).max(0.0);
    let y = at.y.min(window.height - height - 4.0).max(0.0);

    let mut list = column![].spacing(0);
    for item in items {
        let danger = item.danger;
        list = list.push(
            button(label(item.label, fonts).color(if danger {
                theme.text.error.0
            } else {
                theme.text.primary.0
            }))
            .width(Fill)
            .padding([5, 10])
            .style(move |_, s| style::sidebar_item(theme, s, false))
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
