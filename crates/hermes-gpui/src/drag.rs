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
        use gpui_kit::prelude::*;
        gpui_kit::div()
            .px(crate::theme::px(8.))
            .py(crate::theme::px(4.))
            .rounded(crate::theme::px(4.))
            .bg(crate::theme::selected())
            .text_color(crate::theme::text())
            .text_size(crate::theme::secondary_size())
            .font_family(crate::theme::font())
            .child(self.0.clone())
    }
}
