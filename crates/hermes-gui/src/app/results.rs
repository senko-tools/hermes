//! The Search, Mentions and Pins buffers: opening them, loading pages and jumping to results.

use hermes_client_core::InstanceId;
use hermes_client_core::proto::api::SearchQuery;
use hermes_client_core::proto::id::Id;
use iced::Task;
use iced::widget::operation;
use iced::widget::pane_grid::Axis;

use super::App;
use crate::buffer::Buffer;
use crate::buffer::results::{self, Key, Message as RMsg, PAGE};
use crate::buffer::space_settings::Remote;
use crate::message::Message;

/// How many mentions are loaded per instance.
const MENTIONS: u32 = 50;

impl App {
    /// Shows a results buffer: in a pane that shows it already, else next to the focused channel
    /// (so the channel stays visible), else in the focused pane.
    pub(super) fn open_results(&mut self, key: Key) -> Task<Message> {
        let buffer = Buffer::Results(key.clone());
        let existing = self
            .panes
            .state
            .iter()
            .find(|(_, p)| p.buffer == buffer)
            .map(|(id, _)| *id);
        match existing {
            Some(pane) => self.panes.focus = pane,
            None => {
                if self.panes.focused_channel().is_some() {
                    self.panes.split(self.panes.focus, Axis::Vertical);
                }
                self.panes.open(buffer);
            }
        }
        self.core
            .set_visible_channels(self.panes.visible_channels());
        let state = self.results.entry(key.clone()).or_default();
        match key {
            Key::Search(..) => operation::focus(iced::widget::Id::new(results::SEARCH_INPUT)),
            _ if matches!(state.items, Remote::Ready(_) | Remote::Loading) => Task::none(),
            _ => self.load_results(key, false),
        }
    }

    /// Search in the channel's space, or the whole instance for a DM.
    pub(super) fn search_from(&mut self, inst: InstanceId, channel: Id) -> Task<Message> {
        let space = self
            .core
            .store()
            .instance(&inst)
            .and_then(|s| s.space_of(channel));
        self.open_results(Key::Search(inst, space))
    }

    fn load_results(&mut self, key: Key, append: bool) -> Task<Message> {
        let state = self.results.entry(key.clone()).or_default();
        if append {
            state.loading_more = true;
        } else {
            state.items = Remote::Loading;
            state.total = None;
        }
        let wrap = {
            let key = key.clone();
            move |r| Message::Results(key.clone(), RMsg::Loaded(append, r))
        };
        match key {
            Key::Search(inst, space) => {
                let q = state.query.trim().to_owned();
                if q.is_empty() {
                    state.items = Remote::Idle;
                    return Task::none();
                }
                state.searched = q.clone();
                let before = if append {
                    state
                        .items
                        .ready()
                        .and_then(|l| l.last())
                        .map(|(_, m)| m.id)
                } else {
                    None
                };
                let query = SearchQuery {
                    q,
                    space_id: space,
                    before,
                    limit: Some(PAGE),
                    ..SearchQuery::default()
                };
                let i = inst.clone();
                self.call(
                    &inst,
                    move |r| async move { r.search(&query).await },
                    move |r| {
                        wrap(r.map(|res| {
                            (
                                res.messages.into_iter().map(|m| (i.clone(), m)).collect(),
                                Some(res.total),
                            )
                        }))
                    },
                )
            }
            Key::Pins(inst, channel) => {
                let i = inst.clone();
                self.call(
                    &inst,
                    move |r| async move { r.pins(channel).await },
                    move |r| {
                        wrap(
                            r.map(|list| {
                                (list.into_iter().map(|m| (i.clone(), m)).collect(), None)
                            }),
                        )
                    },
                )
            }
            Key::Mentions => {
                let instances = self.core.instance_ids();
                if instances.is_empty() {
                    state.items = Remote::Ready(Vec::new());
                    return Task::none();
                }
                let tasks: Vec<_> = instances
                    .into_iter()
                    .map(|inst| {
                        let wrap = wrap.clone();
                        let i = inst.clone();
                        self.call(
                            &inst,
                            move |r| async move { r.mentions(None, MENTIONS).await },
                            move |r| {
                                // Every instance appends to the same list.
                                let mut msg = wrap(r.map(|list| {
                                    (list.into_iter().map(|m| (i.clone(), m)).collect(), None)
                                }));
                                if let Message::Results(_, RMsg::Loaded(append, _)) = &mut msg {
                                    *append = true;
                                }
                                msg
                            },
                        )
                    })
                    .collect();
                Task::batch(tasks)
            }
        }
    }

    pub(super) fn on_results(&mut self, key: Key, message: RMsg) -> Task<Message> {
        let state = self.results.entry(key.clone()).or_default();
        match message {
            RMsg::Query(q) => state.query = q,
            RMsg::Submit | RMsg::Refresh => return self.load_results(key, false),
            RMsg::More => return self.load_results(key, true),
            RMsg::Close => {
                self.panes
                    .clear_where(|b| *b == Buffer::Results(key.clone()));
            }
            RMsg::Open(inst, channel, message) => return self.jump_to(inst, channel, message),
            RMsg::Loaded(append, result) => {
                state.loading_more = false;
                match result {
                    Ok((items, total)) => {
                        // Mentions: the first instance's answer replaces "Loading…".
                        let append = append && matches!(state.items, Remote::Ready(_));
                        state.add(append, items);
                        if total.is_some() {
                            state.total = total;
                        }
                    }
                    Err(err) if append && matches!(state.items, Remote::Ready(_)) => {
                        self.error(format!("could not load more: {err}"));
                    }
                    Err(err) => state.items = Remote::Failed(err),
                }
            }
        }
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::results::State;
    use hermes_client_core::proto::models::{Message as ChatMessage, MessageKind, User};

    fn msg(id: u64) -> ChatMessage {
        ChatMessage {
            id: Id(id << 22),
            channel_id: Id(1),
            author: User {
                id: Id(2),
                username: "a".into(),
                display_name: None,
                avatar: None,
                status_text: None,
            },
            content: String::new(),
            reply_to: None,
            edited_at: None,
            deleted: false,
            pinned: false,
            kind: MessageKind::Default,
            attachments: vec![],
            reactions: vec![],
            mentions: Default::default(),
            nonce: None,
        }
    }

    #[test]
    fn results_merge_newest_first_without_duplicates() {
        let mut state = State::default();
        state.add(false, vec![("a".into(), msg(1)), ("a".into(), msg(3))]);
        state.add(true, vec![("b".into(), msg(2)), ("a".into(), msg(3))]);
        let ids: Vec<u64> = state
            .items
            .ready()
            .unwrap()
            .iter()
            .map(|(_, m)| m.id.0 >> 22)
            .collect();
        assert_eq!(ids, [3, 2, 1]);
        state.total = Some(5);
        assert!(state.has_more());
        state.add(false, vec![]);
        assert_eq!(state.items, Remote::Ready(vec![]));
    }
}
