//! What can be dragged: roles in the space settings and channels in the sidebar and the settings
//! (PLAN.md sections 15.1 and 15.4). gpui does the dragging; a row carries an [`Item`].

use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;

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
