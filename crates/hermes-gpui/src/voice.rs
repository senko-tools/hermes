//! Voice: joining and leaving rooms, the room page, the panel over the user panel. (Grows in the
//! voice step.)

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, div, px};
use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;

use crate::shell::Shell;
use crate::theme;

impl Shell {
    pub fn join_voice(&mut self, _inst: InstanceId, _channel: Id, _cx: &mut Context<Self>) {}

    pub fn leave_voice(&mut self, _cx: &mut Context<Self>) {}

    pub fn toggle_mute(&mut self, _cx: &mut Context<Self>) {}

    pub fn toggle_deafen(&mut self, _cx: &mut Context<Self>) {}

    pub fn render_voice(
        &self,
        _inst: InstanceId,
        _channel: Id,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div().size_full()
    }

    pub fn render_voice_panel(&self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        None
    }

    pub fn render_me_buttons(&self, _cx: &mut Context<Self>) -> Vec<AnyElement> {
        Vec::new()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_voice_user(
        &self,
        _inst: InstanceId,
        _channel: Id,
        _user: Id,
        name: String,
        _muted: bool,
        _deafened: bool,
        _speaking: bool,
        _me: bool,
        nested: bool,
        _dm: bool,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(8.))
            .h(px(30.))
            .pl(px(if nested { 46. } else { 36. }))
            .text_size(px(theme::TEXT_SMALL))
            .text_color(theme::text_secondary())
            .child(name)
            .into_any_element()
    }
}
