//! Message lists with jump-to (PLAN.md section 15.1): Search (FTS over a space or a whole
//! instance), Mentions (recent mentions on every instance) and a channel's pinned messages.
//! Clicking a result opens its channel at that message.

use std::collections::HashMap;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Entity, FontWeight, SharedString, Subscription, Window, div};
use hermes_client_core::proto::api::SearchQuery;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Message as ChatMessage;
use hermes_client_core::{InstanceId, time, view};

use crate::shell::{Page, Shell};
use crate::theme::{self, px};

pub const PAGE: u32 = 25;
/// How many mentions are loaded per instance.
const MENTIONS: u32 = 50;

/// Which list a results page shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// Search in a space, or (`None`) in every channel and DM of the instance.
    Search(InstanceId, Option<Id>),
    Mentions,
    Pins(InstanceId, Id),
}

/// A list loaded over REST.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Remote<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Remote<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Remote::Ready(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Items {
    /// The query the results are for.
    pub searched: String,
    pub items: Remote<Vec<(InstanceId, ChatMessage)>>,
    /// All search matches (the list holds the loaded pages).
    pub total: Option<u64>,
    pub loading_more: bool,
}

impl Items {
    /// More search results exist than are loaded.
    pub fn has_more(&self) -> bool {
        match (self.total, self.items.ready()) {
            (Some(total), Some(items)) => (items.len() as u64) < total,
            _ => false,
        }
    }

    /// Adds results, newest first, without duplicates.
    pub fn add(&mut self, append: bool, items: Vec<(InstanceId, ChatMessage)>) {
        let mut list = match (append, std::mem::take(&mut self.items)) {
            (true, Remote::Ready(old)) => old,
            _ => Vec::new(),
        };
        for item in items {
            if !list.iter().any(|(i, m)| *i == item.0 && m.id == item.1.id) {
                list.push(item);
            }
        }
        list.sort_by_key(|(_, m)| std::cmp::Reverse(m.created_at()));
        self.items = Remote::Ready(list);
    }
}

/// A results page: its list and, for a search, its field.
pub struct ResultsPage {
    pub list: Items,
    input: Option<Entity<InputState>>,
    _input_events: Option<Subscription>,
}

/// Loaded results and, for a search, the total number of matches.
type ResultPage = (Vec<(InstanceId, ChatMessage)>, Option<u64>);

impl Shell {
    /// Shows a results page, loading it unless it is a search or loaded already.
    pub fn open_results(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        if !self.results.contains_key(&key) {
            let (input, events) = if matches!(key, Key::Search(..)) {
                let input = cx
                    .new(|cx| InputState::new(window, cx).placeholder("Search messages… (Enter)"));
                let event_key = key.clone();
                let events = cx.subscribe_in(&input, window, move |this, _, event, _, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.load_results(event_key.clone(), false, cx);
                    }
                });
                (Some(input), Some(events))
            } else {
                (None, None)
            };
            self.results.insert(
                key.clone(),
                ResultsPage {
                    list: Items::default(),
                    input,
                    _input_events: events,
                },
            );
        }
        self.previous_page = Some(self.page.clone());
        self.page = Page::Results(key.clone());
        self.update_visible_channels();
        if let Some(input) = self.results.get(&key).and_then(|r| r.input.clone()) {
            input.update(cx, |input, cx| input.focus(window, cx));
        } else if !matches!(
            self.results.get(&key).map(|r| &r.list.items),
            Some(Remote::Ready(_) | Remote::Loading)
        ) {
            self.load_results(key, false, cx);
        }
        cx.notify();
    }

    /// Search in the channel's space, or the whole instance for a DM.
    pub fn search_from(
        &mut self,
        inst: InstanceId,
        channel: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let space = self
            .core
            .store()
            .instance(&inst)
            .and_then(|s| s.space_of(channel));
        self.open_results(Key::Search(inst, space), window, cx);
    }

    fn load_results(&mut self, key: Key, append: bool, cx: &mut Context<Self>) {
        let query = self
            .results
            .get(&key)
            .and_then(|r| r.input.as_ref())
            .map(|input| input.read(cx).value().trim().to_owned())
            .unwrap_or_default();
        let Some(page) = self.results.get_mut(&key) else {
            return;
        };
        let state = &mut page.list;
        if append {
            state.loading_more = true;
        } else {
            state.items = Remote::Loading;
            state.total = None;
        }
        match key.clone() {
            Key::Search(inst, space) => {
                if query.is_empty() {
                    state.items = Remote::Idle;
                    cx.notify();
                    return;
                }
                state.searched = query.clone();
                let before = if append {
                    state
                        .items
                        .ready()
                        .and_then(|l| l.last())
                        .map(|(_, m)| m.id)
                } else {
                    None
                };
                let request = SearchQuery {
                    q: query,
                    space_id: space,
                    before,
                    limit: Some(PAGE),
                    ..SearchQuery::default()
                };
                let core = self.core.clone();
                let done = key.clone();
                self.run(
                    cx,
                    async move {
                        let rest = core.rest(&inst).map_err(|e| e.to_string())?;
                        let res = rest.search(&request).await.map_err(|e| e.to_string())?;
                        Ok::<ResultPage, String>((
                            res.messages
                                .into_iter()
                                .map(|m| (inst.clone(), m))
                                .collect(),
                            Some(res.total),
                        ))
                    },
                    move |this, result, _, cx| this.results_loaded(&done, append, result, cx),
                );
            }
            Key::Pins(inst, channel) => {
                let core = self.core.clone();
                let done = key.clone();
                self.run(
                    cx,
                    async move {
                        let rest = core.rest(&inst).map_err(|e| e.to_string())?;
                        let list = rest.pins(channel).await.map_err(|e| e.to_string())?;
                        Ok::<ResultPage, String>((
                            list.into_iter().map(|m| (inst.clone(), m)).collect(),
                            None,
                        ))
                    },
                    move |this, result, _, cx| this.results_loaded(&done, append, result, cx),
                );
            }
            Key::Mentions => {
                let instances = self.core.instance_ids();
                if instances.is_empty() {
                    state.items = Remote::Ready(Vec::new());
                    return;
                }
                // Every instance appends to the same list.
                for inst in instances {
                    let core = self.core.clone();
                    let done = key.clone();
                    self.run(
                        cx,
                        async move {
                            let rest = core.rest(&inst).map_err(|e| e.to_string())?;
                            let list = rest
                                .mentions(None, MENTIONS)
                                .await
                                .map_err(|e| e.to_string())?;
                            Ok::<ResultPage, String>((
                                list.into_iter().map(|m| (inst.clone(), m)).collect(),
                                None,
                            ))
                        },
                        move |this, result, _, cx| this.results_loaded(&done, true, result, cx),
                    );
                }
            }
        }
        cx.notify();
    }

    fn results_loaded(
        &mut self,
        key: &Key,
        append: bool,
        result: Result<ResultPage, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.results.get_mut(key).map(|r| &mut r.list) else {
            return;
        };
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
                self.error(format!("could not load more: {err}"), cx);
            }
            Err(err) => state.items = Remote::Failed(err),
        }
    }

    pub fn close_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.previous_page.take() {
            Some(Page::Chat(inst, channel)) => self.open_chat(inst, channel, window, cx),
            Some(page) if !matches!(page, Page::Results(_)) => {
                self.page = page;
                self.update_visible_channels();
            }
            _ => self.open_friends(cx),
        }
        cx.notify();
    }

    pub fn render_results(&self, key: Key, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (title, scope) = self.results_title(&key);
        let Some(page) = self.results.get(&key) else {
            return div().into_any_element();
        };
        let state = &page.list;
        let refresh_key = key.clone();
        let header = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(8.))
            .px(px(16.))
            .py(px(10.))
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(theme::body_size())
                            .font_weight(FontWeight::MEDIUM)
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(scope),
                    )
                    .child(
                        Button::new("results-refresh")
                            .cursor_pointer()
                            .label("Refresh")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.load_results(refresh_key.clone(), false, cx);
                            })),
                    )
                    .child(
                        Button::new("results-close")
                            .cursor_pointer()
                            .label("Close")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_results(window, cx)),
                            ),
                    ),
            )
            .children(page.input.as_ref().map(|input| Input::new(input).large()));

        let mut list: Vec<AnyElement> = Vec::new();
        let muted = |text: String| {
            div()
                .text_size(theme::secondary_size())
                .text_color(theme::text_tertiary())
                .child(text)
                .into_any_element()
        };
        match &state.items {
            Remote::Idle => list.push(muted(if matches!(key, Key::Search(..)) {
                "Type words to find and press Enter. Every word must appear (as the start of a \
                 word)."
                    .to_owned()
            } else {
                "Loading…".to_owned()
            })),
            Remote::Loading => list.push(muted("Loading…".to_owned())),
            Remote::Failed(err) => list.push(
                div()
                    .text_size(theme::secondary_size())
                    .text_color(theme::danger())
                    .child(format!("Could not load: {err}"))
                    .into_any_element(),
            ),
            Remote::Ready(items) if items.is_empty() => list.push(muted(match &key {
                Key::Search(..) => format!("Nothing found for “{}”.", state.searched),
                Key::Mentions => "Nobody mentioned you yet.".to_owned(),
                Key::Pins(..) => {
                    "Nothing is pinned here. Right-click a message and choose Pin.".to_owned()
                }
            })),
            Remote::Ready(items) => {
                if let (Key::Search(..), Some(total)) = (&key, state.total) {
                    list.push(muted(format!(
                        "{total} result{}",
                        if total == 1 { "" } else { "s" }
                    )));
                }
                let lines = self.result_lines(&key, items);
                for (n, line) in lines.into_iter().enumerate() {
                    list.push(result_row(n, line, cx));
                }
                if state.has_more() {
                    let more_key = key.clone();
                    list.push(
                        Button::new("results-more")
                            .cursor_pointer()
                            .label(if state.loading_more {
                                "Loading…"
                            } else {
                                "Load more"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.load_results(more_key.clone(), true, cx);
                            }))
                            .into_any_element(),
                    );
                }
            }
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(
                div()
                    .id("results-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(px(6.))
                    .p(px(12.))
                    .overflow_y_scroll()
                    .children(list),
            )
            .into_any_element()
    }

    /// The heading and scope of a results page.
    fn results_title(&self, key: &Key) -> (String, String) {
        let store = self.core.store();
        match key {
            Key::Search(inst, space) => {
                let scope = space
                    .and_then(|sp| store.instance(inst)?.spaces.get(&sp))
                    .map_or_else(
                        || format!("everywhere on {inst}"),
                        |sp| sp.space.name.clone(),
                    );
                ("Search".into(), scope)
            }
            Key::Mentions => ("Mentions".into(), "Recent messages that mention you".into()),
            Key::Pins(inst, channel) => {
                let name = store
                    .instance(inst)
                    .and_then(|s| Some(view::channel_title(s, s.channel(*channel)?)))
                    .unwrap_or_default();
                ("Pinned messages".into(), name)
            }
        }
    }

    fn result_lines(&self, key: &Key, items: &[(InstanceId, ChatMessage)]) -> Vec<ResultLine> {
        let store = self.core.store();
        items
            .iter()
            .filter_map(|(inst, m)| {
                let s = store.instance(inst)?;
                let space = s.space_of(m.channel_id);
                let place = s
                    .channel(m.channel_id)
                    .map(|c| view::channel_title(s, c))
                    .unwrap_or_else(|| "a channel".into());
                let place = match (key, space.and_then(|sp| s.spaces.get(&sp))) {
                    (Key::Mentions, Some(sp)) => format!("{place} · {}", sp.space.name),
                    (Key::Mentions, None) => format!("{place} · {inst}"),
                    _ => place,
                };
                let text = view::plain_text(s, space, &m.content, 300);
                Some(ResultLine {
                    inst: inst.clone(),
                    channel: m.channel_id,
                    message: m.id,
                    author: s.display_name(space, m.author.id),
                    when: time::format(m.created_at() as i64, "%Y-%m-%d %H:%M", self.utc_offset),
                    place,
                    text: if text.is_empty() && !m.attachments.is_empty() {
                        format!("[{} attachment(s)]", m.attachments.len())
                    } else {
                        text
                    },
                })
            })
            .collect()
    }
}

struct ResultLine {
    inst: InstanceId,
    channel: Id,
    message: Id,
    author: String,
    when: String,
    place: String,
    text: String,
}

fn result_row(n: usize, line: ResultLine, cx: &mut Context<Shell>) -> AnyElement {
    let ResultLine {
        inst,
        channel,
        message,
        author,
        when,
        place,
        text,
    } = line;
    div()
        .id(("result", n))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(2.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|d| d.bg(theme::hover()))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.jump_to(inst.clone(), channel, message, window, cx);
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(theme::secondary_size())
                        .font_weight(FontWeight::BOLD)
                        .child(SharedString::from(author)),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(theme::caption_size())
                        .text_color(theme::text_tertiary())
                        .child(when),
                )
                .child(
                    div()
                        .text_size(theme::caption_size())
                        .text_color(theme::text_secondary())
                        .child(place),
                ),
        )
        .child(div().text_size(theme::secondary_size()).child(text))
        .into_any_element()
}

/// The pages kept while the app runs.
pub type ResultsPages = HashMap<Key, ResultsPage>;

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_client_core::proto::models::{MessageKind, User};

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
        let mut state = Items::default();
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
