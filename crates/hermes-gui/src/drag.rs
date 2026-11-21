//! What can be dragged: roles in the space settings and channels in the sidebar and the settings
//! (PLAN.md sections 15.1 and 15.4). gpui does the dragging; a row carries an [`Item`].

use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;
pub use hermes_client_core::view::Place;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Role,
    Channel,
}

/// A draggable row.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Item {
    pub inst: InstanceId,
    pub space: Id,
    pub kind: Kind,
    pub id: Id,
}

impl Item {
    /// Whether this row can be dropped on `target`: another row of the same list.
    pub fn can_drop_on(&self, target: &Item) -> bool {
        self != target
            && self.kind == target.kind
            && self.inst == target.inst
            && self.space == target.space
    }
}

/// What a channel list row is, for where a drop on it lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Channel,
    /// A category; `open` when its channels show under it.
    Category {
        open: bool,
    },
}

/// Where a channel dropped on a row lands, from how far down the row the pointer is (`frac`,
/// 0 at its top, 1 at its bottom): the upper half before it, the lower half after it. On a
/// category a channel goes into it, except near its top edge (before it) and, while it is
/// closed, near its bottom edge (after it and its channels).
pub fn place_for(frac: f32, target: Shape, dragging_category: bool) -> Place {
    match target {
        Shape::Category { open } if !dragging_category => {
            if frac < 0.3 {
                Place::Before
            } else if !open && frac > 0.7 {
                Place::After
            } else {
                Place::Into
            }
        }
        // A category lands after an open one's channels: not under its header.
        Shape::Category { open: true } => Place::Before,
        _ if frac < 0.5 => Place::Before,
        _ => Place::After,
    }
}

/// The little label following the pointer while dragging.
pub struct Ghost(pub String);

impl gpui_kit::Render for Ghost {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use crate::theme;
        use gpui_kit::AnimationExt as _;
        use gpui_kit::prelude::*;
        // A lifted card: raised, outlined and shadowed, a little see-through so the place it
        // goes shows under it. It pops up from the row as the drag starts.
        gpui_kit::div()
            .flex()
            .items_center()
            .gap(theme::px(6.))
            .px(theme::px(10.))
            .py(theme::px(5.))
            .rounded(theme::radius_small())
            .bg(theme::raised())
            .border_1()
            .border_color(theme::accent())
            .shadow_md()
            .text_color(theme::text())
            .text_size(theme::secondary_size())
            .font_family(theme::font())
            .child(crate::assets::icon(
                crate::assets::DRAG,
                14.,
                theme::accent(),
            ))
            .child(self.0.clone())
            .with_animation(
                "ghost-lift",
                gpui_kit::Animation::new(theme::fade()).with_easing(gpui_kit::ease_out_quint()),
                |d, t| d.opacity(0.6 + 0.32 * t),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pointer_half_decides_where_a_channel_lands() {
        assert_eq!(place_for(0.2, Shape::Channel, false), Place::Before);
        assert_eq!(place_for(0.8, Shape::Channel, false), Place::After);
        let (open, closed) = (
            Shape::Category { open: true },
            Shape::Category { open: false },
        );
        assert_eq!(place_for(0.1, open, false), Place::Before);
        assert_eq!(place_for(0.5, open, false), Place::Into);
        assert_eq!(place_for(0.9, open, false), Place::Into);
        assert_eq!(place_for(0.9, closed, false), Place::After);
        assert_eq!(place_for(0.9, open, true), Place::Before);
        assert_eq!(place_for(0.9, closed, true), Place::After);
    }
}
