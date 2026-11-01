//! Changing the width of the sidebar or the member list by dragging its inner edge. A press on
//! the edge starts a drag, the window follows the pointer until the button is released (then the
//! width is saved), and a double-click goes back to the default width.

use iced::widget::{Space, container, mouse_area, stack};
use iced::{Element, Fill};

use crate::message::Message;
use crate::theme::Theme;

/// Width of the grab strip along the edge.
const HANDLE: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Sidebar,
    Members,
}

impl Edge {
    /// Allowed widths, in logical pixels.
    pub fn range(self) -> (u16, u16) {
        match self {
            Edge::Sidebar => (160, 480),
            Edge::Members => (180, 360),
        }
    }

    pub fn default_width(self) -> u16 {
        match self {
            Edge::Sidebar => 240,
            Edge::Members => 232,
        }
    }

    /// The `config.toml` key of the width.
    pub fn key(self) -> &'static str {
        match self {
            Edge::Sidebar => "sidebar.width",
            Edge::Members => "sidebar.member_list_width",
        }
    }

    pub fn clamp(self, width: u16) -> u16 {
        let (min, max) = self.range();
        width.clamp(min, max)
    }
}

/// Which side of the panel its draggable edge is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// A drag in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resize {
    pub edge: Edge,
    start_x: f32,
    pub start_width: u16,
    /// The panel's edge is on its right, so moving right makes it wider.
    grows_right: bool,
}

impl Resize {
    pub fn new(edge: Edge, start_x: f32, start_width: u16, side: Side) -> Resize {
        Resize {
            edge,
            start_x,
            start_width,
            grows_right: side == Side::Right,
        }
    }

    /// The width with the pointer at `x`.
    pub fn width_at(&self, x: f32) -> u16 {
        let moved = x - self.start_x;
        let moved = if self.grows_right { moved } else { -moved };
        let (min, max) = self.edge.range();
        (f32::from(self.start_width) + moved)
            .round()
            .clamp(f32::from(min), f32::from(max)) as u16
    }
}

/// `panel` with a grab strip on its `side` edge; the strip shows the accent color while dragged.
pub fn with_handle<'a>(
    panel: impl Into<Element<'a, Message>>,
    edge: Edge,
    side: Side,
    dragging: bool,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let strip = mouse_area(
        container(Space::new())
            .width(HANDLE)
            .height(Fill)
            .style(move |_| container::Style {
                background: dragging.then(|| theme.accent.base.0.into()),
                ..container::Style::default()
            }),
    )
    .on_press(Message::ResizeStart(edge))
    .on_double_click(Message::ResizeReset(edge))
    .interaction(iced::mouse::Interaction::ResizingHorizontally);
    let strip = container(strip).width(Fill).height(Fill);
    let strip = match side {
        Side::Left => strip.align_left(Fill),
        Side::Right => strip.align_right(Fill),
    };
    stack![panel.into(), strip].into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_follows_the_pointer_within_range() {
        let drag = Resize::new(Edge::Sidebar, 240.0, 240, Side::Right);
        assert_eq!(drag.width_at(300.0), 300);
        assert_eq!(drag.width_at(5000.0), 480);
        assert_eq!(drag.width_at(-100.0), 160);

        // A right-hand sidebar (or the member list) has its edge on the left.
        let drag = Resize::new(Edge::Members, 900.0, 232, Side::Left);
        assert_eq!(drag.width_at(880.0), 252);
        assert_eq!(drag.width_at(1100.0), 180);
    }
}
