//! Jumping to a message (search results, mentions, pins, reply previews) and loading newer
//! messages after a jump.

use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;
use hermes_client_core::store::HistoryAnchor;
use iced::Task;
use iced::widget::pane_grid::{self, Axis};
use iced::widget::{operation, scrollable};

use super::App;
use crate::buffer::{Buffer, channel};
use crate::message::Message;

impl App {
    /// Opens `channel` and scrolls to `message`, highlighted. A pane already showing the channel
    /// is reused; a results buffer (search, mentions, pins) stays open next to it.
    pub(super) fn jump_to(&mut self, inst: InstanceId, channel: Id, message: Id) -> Task<Message> {
        let buffer = Buffer::Channel {
            inst: inst.clone(),
            channel,
        };
        let existing = self
            .panes
            .state
            .iter()
            .find(|(_, p)| p.buffer == buffer)
            .map(|(id, _)| *id);
        let pane = match existing {
            Some(pane) => pane,
            None => {
                let focused_is_list = self.panes.focused().is_some_and(|p| p.buffer.is_results());
                if focused_is_list {
                    let other = self
                        .panes
                        .state
                        .iter()
                        .map(|(id, _)| *id)
                        .find(|id| *id != self.panes.focus);
                    match other {
                        Some(pane) => pane,
                        None => {
                            self.panes.split(self.panes.focus, Axis::Vertical);
                            self.panes.focus
                        }
                    }
                } else {
                    self.panes.focus
                }
            }
        };
        self.panes.focus = pane;
        if existing.is_none() {
            let marker = self
                .core
                .store()
                .instance(&inst)
                .and_then(|s| s.read_states.get(&channel).map(|r| r.last_read_id));
            self.panes.open_with_marker(buffer, marker);
        }
        self.composers.entry((inst.clone(), channel)).or_default();
        self.core
            .set_visible_channels(self.panes.visible_channels());
        if let Some(p) = self.panes.state.get_mut(pane) {
            p.channel.jump = Some(message);
            p.channel.highlight = Some(message);
        }
        self.load_jump(pane, inst, channel, message)
    }

    /// Loads the page around `message` unless it is loaded already, then scrolls to it.
    pub(super) fn load_jump(
        &mut self,
        pane: pane_grid::Pane,
        inst: InstanceId,
        channel: Id,
        message: Id,
    ) -> Task<Message> {
        let loaded = self
            .core
            .store()
            .instance(&inst)
            .and_then(|s| s.window(channel))
            .is_some_and(|w| w.loaded && w.get(message).is_some());
        if loaded {
            return self.scroll_to_jump(pane);
        }
        let core = self.core.clone();
        Task::perform(
            async move {
                core.load_history(&inst, channel, HistoryAnchor::Around(message))
                    .await
            },
            move |result| match result {
                Ok(()) => Message::Channel(pane, channel::Message::JumpLoaded),
                Err(err) => Message::Done(Err(format!("could not load the message: {err}"))),
            },
        )
    }

    /// Scrolls a pane to its pending jump target; done once the target row was measured.
    pub(super) fn scroll_to_jump(&mut self, pane: pane_grid::Pane) -> Task<Message> {
        let Some(p) = self.panes.state.get(pane) else {
            return Task::none();
        };
        let (Buffer::Channel { inst, channel }, Some(target)) = (&p.buffer, p.channel.jump) else {
            return Task::none();
        };
        let ctx = channel::Context {
            core: &self.core,
            config: &self.config,
            fonts: &self.fonts,
            theme: &self.theme,
            utc_offset: self.utc_offset,
            images: &self.images,
            composer: None,
            menu_message: None,
        };
        let Some((offset, measured)) =
            channel::jump_offset(&ctx, inst, *channel, &p.channel, target)
        else {
            return Task::none();
        };
        let id = p.channel.scroll_id.clone();
        if measured && let Some(p) = self.panes.state.get_mut(pane) {
            p.channel.jump = None;
        }
        operation::scroll_to(id, scrollable::AbsoluteOffset { x: 0.0, y: offset })
    }

    /// At the bottom of a window that does not reach the present (after a jump): the next page.
    pub(super) fn load_newer(
        &mut self,
        pane: pane_grid::Pane,
        inst: InstanceId,
        channel: Id,
    ) -> Task<Message> {
        let newest = {
            let store = self.core.store();
            let Some(window) = store.instance(&inst).and_then(|s| s.window(channel)) else {
                return Task::none();
            };
            if !window.loaded || window.reached_end {
                return Task::none();
            }
            match window.messages.last() {
                Some(m) => m.id,
                None => return Task::none(),
            }
        };
        if let Some(p) = self.panes.state.get_mut(pane) {
            p.channel.loading_newer = true;
        }
        let core = self.core.clone();
        Task::perform(
            async move {
                core.load_history(&inst, channel, HistoryAnchor::After(newest))
                    .await
            },
            move |result| {
                if let Err(err) = result {
                    tracing::warn!("newer messages: {err}");
                }
                Message::Channel(pane, channel::Message::NewerLoaded)
            },
        )
    }
}
