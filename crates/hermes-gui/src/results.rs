//! Message lists with jump-to (PLAN.md section 15.1): Search (FTS over a space or a whole
//! instance), Mentions (recent mentions on every instance) and a channel's pinned messages.
//! Clicking a result opens its channel at that message.

use std::collections::HashMap;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Entity, FontWeight, SharedString, Subscription, Window, div};
use hermes_client_core::proto::api::{SearchHas, SearchQuery, SearchSort};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Message as ChatMessage;
use hermes_client_core::{InstanceId, time, view};

use crate::shell::{Page, Shell};
use crate::theme::{self, px};

pub const PAGE: u32 = 25;
/// How long typing pauses before the search runs by itself.
const TYPING_PAUSE: std::time::Duration = std::time::Duration::from_millis(350);
/// How much text is shown around the first match.
const SNIPPET_CHARS: usize = 220;
/// How many mentions are loaded per instance.
const MENTIONS: u32 = 50;

/// Which list a results page shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// Search in a space, or (`None`) in every channel and DM of the instance.
    Search(InstanceId, Option<Id>),
    Mentions,
    Pins(InstanceId, Id),
    /// My saved messages on every instance (kept on this device).
    Saved,
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
    /// Best matches first: kept in the server's order, later pages by `next_offset`.
    pub ranked: bool,
    pub next_offset: u32,
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
        if !self.ranked {
            list.sort_by_key(|(_, m)| std::cmp::Reverse(m.created_at()));
        }
        self.items = Remote::Ready(list);
    }
}

/// A results page: its list and, for a search, its field.
pub struct ResultsPage {
    pub list: Items,
    input: Option<Entity<InputState>>,
    _input_events: Option<Subscription>,
    /// The order a search asks for.
    sort: SearchSort,
    /// Counts what was typed: a search waits for a pause in typing.
    typed: u64,
}

/// Loaded results and, for a search, the total number of matches.
type ResultPage = (Vec<(InstanceId, ChatMessage)>, Option<u64>);

/// What a search's words ask for besides words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Filters {
    words: String,
    author: Option<Id>,
    channel: Option<Id>,
    /// Only messages older than this.
    before: Option<Id>,
    /// Only messages newer than this.
    after: Option<Id>,
    has: Option<SearchHas>,
    /// `from:` and `in:` that match nobody here: searched as words, and said so.
    unresolved: Vec<String>,
}

impl Filters {
    fn any(&self) -> bool {
        self.author.is_some()
            || self.channel.is_some()
            || self.before.is_some()
            || self.after.is_some()
            || self.has.is_some()
    }
}

/// Whether an opened message passes the filters the server may not know (an older one ignores
/// `after` and `has`, and encrypted messages are only known once opened).
fn passes(m: &ChatMessage, f: &Filters) -> bool {
    let has = match f.has {
        None => true,
        Some(SearchHas::File) => !m.attachments.is_empty(),
        Some(SearchHas::Image) => m
            .attachments
            .iter()
            .any(|a| a.content_type.starts_with("image/")),
        Some(SearchHas::Link) => m.content.contains("http://") || m.content.contains("https://"),
    };
    has && f.after.is_none_or(|after| m.id > after) && f.before.is_none_or(|before| m.id < before)
}

/// Takes `from:name`, `in:channel`, `has:file|image|link` and `before:`, `after:` or
/// `during:YYYY-MM-DD` out of a search. What matches nothing stays in the words.
fn filters(
    s: &hermes_client_core::store::InstanceStore,
    query: &str,
    space: Option<Id>,
    utc_offset: i64,
) -> Filters {
    let mut f = Filters::default();
    let (mut author, mut channel) = (None, None);
    let mut rest = Vec::new();
    let id_at = |ms: i64| Id::min_at_unix_ms(u64::try_from(ms).unwrap_or(0));
    for word in query.split_whitespace() {
        if let Some(kind) = word.strip_prefix("has:")
            && let Some(has) = SearchHas::from_name(kind)
        {
            f.has = Some(has);
            continue;
        }
        if let Some((name, date)) = word.split_once(':')
            && let Some((start, end)) = time::parse_day(date, utc_offset)
        {
            match name {
                "before" => {
                    f.before = Some(id_at(start));
                    continue;
                }
                "after" => {
                    f.after = Some(Id(id_at(end).0.saturating_sub(1)));
                    continue;
                }
                "during" | "on" => {
                    f.after = Some(Id(id_at(start).0.saturating_sub(1)));
                    f.before = Some(id_at(end));
                    continue;
                }
                _ => {}
            }
        }
        if let Some(name) = word.strip_prefix("from:") {
            let name = name.trim_start_matches('@').to_lowercase();
            let found = s
                .users
                .values()
                .find(|u| u.username.to_lowercase() == name || u.handle().to_lowercase() == name);
            if let Some(user) = found {
                author = Some(user.id);
                continue;
            }
            f.unresolved.push(format!(
                "Nobody called \u{201c}{name}\u{201d} is known here."
            ));
        } else if let Some(name) = word.strip_prefix("in:") {
            let name = name.trim_start_matches('#').to_lowercase();
            let found = space.and_then(|id| s.spaces.get(&id)).and_then(|state| {
                state
                    .channels
                    .values()
                    .find(|c| c.name.as_deref().is_some_and(|n| n.to_lowercase() == name))
            });
            if let Some(found) = found {
                channel = Some(found.id);
                continue;
            }
            f.unresolved.push(if space.is_none() {
                "in: works when searching one server.".to_owned()
            } else {
                format!("No channel called \u{201c}{name}\u{201d} here.")
            });
        }
        rest.push(word);
    }
    f.words = rest.join(" ");
    f.author = author;
    f.channel = channel;
    f
}

impl Shell {
    /// Shows a results page, loading it unless it is a search or loaded already.
    pub fn open_results(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        if !self.results.contains_key(&key) {
            let (input, events) = if matches!(key, Key::Search(..)) {
                let input = cx
                    .new(|cx| InputState::new(window, cx).placeholder("Search messages… (Enter)"));
                let event_key = key.clone();
                let events =
                    cx.subscribe_in(&input, window, move |this, _, event, _, cx| match event {
                        InputEvent::PressEnter { .. } => {
                            this.load_results(event_key.clone(), false, cx);
                        }
                        InputEvent::Change => this.search_after_pause(event_key.clone(), cx),
                        _ => {}
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
                    sort: SearchSort::Newest,
                    typed: 0,
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

    /// Searches once typing stops for a moment.
    fn search_after_pause(&mut self, key: Key, cx: &mut Context<Self>) {
        let Some(page) = self.results.get_mut(&key) else {
            return;
        };
        page.typed += 1;
        let typed = page.typed;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TYPING_PAUSE).await;
            let _ = this.update(cx, |this, cx| {
                if this.results.get(&key).is_some_and(|p| p.typed == typed) {
                    this.load_results(key, false, cx);
                }
            });
        })
        .detach();
    }

    fn set_search_sort(&mut self, key: Key, sort: SearchSort, cx: &mut Context<Self>) {
        let Some(page) = self.results.get_mut(&key) else {
            return;
        };
        if page.sort == sort {
            return;
        }
        page.sort = sort;
        self.load_results(key, false, cx);
    }

    /// Puts a suggested filter in place of the one being typed.
    fn complete_filter(
        &mut self,
        key: &Key,
        filter: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.results.get(key).and_then(|p| p.input.clone()) else {
            return;
        };
        let text = input.read(cx).value().to_string();
        let kept = text
            .rsplit_once(char::is_whitespace)
            .map_or("", |(before, _)| before);
        let value = if kept.trim().is_empty() {
            format!("{filter} ")
        } else {
            format!("{} {filter} ", kept.trim_end())
        };
        input.update(cx, |input, cx| {
            input.set_value(value, window, cx);
            input.focus(window, cx);
        });
        self.search_after_pause(key.clone(), cx);
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
        let sort = page.sort;
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
                let ranked = sort == SearchSort::Relevance;
                let offset = if append { state.next_offset } else { 0 };
                if !append {
                    state.ranked = ranked;
                    state.next_offset = 0;
                }
                let before = if append && !ranked {
                    state
                        .items
                        .ready()
                        .and_then(|l| l.last())
                        .map(|(_, m)| m.id)
                } else {
                    None
                };
                let f = self.core.store().instance(&inst).map_or_else(
                    || Filters {
                        words: query.clone(),
                        ..Filters::default()
                    },
                    |s| filters(s, &query, space, self.utc_offset),
                );
                if f.words.is_empty() && !f.any() {
                    state.items = Remote::Idle;
                    cx.notify();
                    return;
                }
                // A later page goes on from the last one shown, never past the date asked for.
                let before = match (before, f.before) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                let request = SearchQuery {
                    q: f.words.clone(),
                    space_id: space,
                    channel_id: f.channel,
                    author_id: f.author,
                    before,
                    after: f.after,
                    has: f.has,
                    limit: Some(PAGE),
                    sort: ranked.then_some(SearchSort::Relevance),
                    offset: (ranked && offset > 0).then_some(offset),
                };
                let core = self.core.clone();
                let done = key.clone();
                self.run(
                    cx,
                    async move {
                        let rest = core.rest(&inst).map_err(|e| e.to_string())?;
                        let mut res = core
                            .search_with(&rest, &inst, &request)
                            .await
                            .map_err(|e| e.to_string())?;
                        core.open_messages(&inst, &mut res.messages).await;
                        res.messages.retain(|m| passes(m, &f));
                        Ok::<ResultPage, String>((
                            res.messages
                                .into_iter()
                                .map(|m| (inst.clone(), m))
                                .collect(),
                            (!append).then_some(res.total),
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
                        let mut list = rest.pins(channel).await.map_err(|e| e.to_string())?;
                        core.open_messages(&inst, &mut list).await;
                        Ok::<ResultPage, String>((
                            list.into_iter().map(|m| (inst.clone(), m)).collect(),
                            None,
                        ))
                    },
                    move |this, result, _, cx| this.results_loaded(&done, append, result, cx),
                );
            }
            // Read from this device when drawn.
            Key::Saved => {
                state.items = Remote::Ready(Vec::new());
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
                            let mut list = rest
                                .mentions(None, MENTIONS)
                                .await
                                .map_err(|e| e.to_string())?;
                            core.open_messages(&inst, &mut list).await;
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
                if state.ranked {
                    state.next_offset += PAGE;
                }
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
            .children(page.input.as_ref().map(|input| Input::new(input).large()))
            .children(self.search_aids(&key, page, cx));

        let mut list: Vec<AnyElement> = Vec::new();
        let muted = |text: String| {
            div()
                .text_size(theme::secondary_size())
                .text_color(theme::text_tertiary())
                .child(text)
                .into_any_element()
        };
        if key == Key::Saved {
            list.extend(self.saved_rows(cx));
        } else {
            match &state.items {
            Remote::Idle => list.push(muted(if matches!(key, Key::Search(..)) {
                "Type to search: every word must appear somewhere in the message. Narrow it with \
                 from:, in:, has:, before:, after: or during:."
                    .to_owned()
            } else {
                "Loading…".to_owned()
            })),
            Remote::Loading => list.push(theme::skeleton("results", 5)),
            Remote::Failed(err) => list.push(
                div()
                    .text_size(theme::secondary_size())
                    .text_color(theme::danger())
                    .child(format!("Could not load: {err}"))
                    .into_any_element(),
            ),
            Remote::Ready(items) if items.is_empty() => list.push(match &key {
                Key::Search(..) => theme::empty_state(
                    "search",
                    crate::assets::SEARCH,
                    "Nothing found",
                    format!("Nothing found for “{}”.", state.searched),
                ),
                Key::Mentions => theme::empty_state(
                    "mentions",
                    crate::assets::AT,
                    "No mentions",
                    "Nobody mentioned you yet.",
                ),
                Key::Pins(..) => theme::empty_state(
                    "pins",
                    crate::assets::PIN,
                    "Nothing pinned",
                    "Right-click a message and choose Pin.",
                ),
                Key::Saved => div().into_any_element(),
            }),
            Remote::Ready(items) => {
                if let (Key::Search(..), Some(total)) = (&key, state.total) {
                    list.push(muted(format!(
                        "{total} result{}",
                        if total == 1 { "" } else { "s" }
                    )));
                }
                let words = search_words(&state.searched);
                let lines = self.result_lines(&key, items, &words);
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

    /// My saved messages on every instance, the latest saved first; each opens where it was
    /// said and can be let go.
    fn saved_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let store = self.core.store();
        let mut saved: Vec<(InstanceId, hermes_client_core::channel_keys::SavedMessage)> = self
            .core
            .instance_ids()
            .into_iter()
            .flat_map(|inst| {
                self.core
                    .saved_messages(&inst)
                    .into_iter()
                    .map(move |s| (inst.clone(), s))
            })
            .collect();
        saved.sort_by_key(|(_, s)| std::cmp::Reverse(s.saved_at));
        if saved.is_empty() {
            return vec![theme::empty_state(
                "saved",
                crate::assets::BOOKMARK,
                "Nothing saved",
                "Right-click a message and choose Save message.",
            )];
        }
        let many = self.core.instance_ids().len() > 1;
        saved
            .into_iter()
            .enumerate()
            .map(|(n, (inst, item))| {
                let place = store
                    .instance(&inst)
                    .and_then(|s| Some(view::channel_title(s, s.channel(item.channel)?)))
                    .unwrap_or_else(|| "a channel".into());
                let place = if many {
                    format!("{place} \u{b7} {inst}")
                } else {
                    place
                };
                let when = time::format(
                    item.message.timestamp_ms() as i64,
                    "%Y-%m-%d %H:%M",
                    self.utc_offset,
                );
                let (remove_inst, message) = (inst.clone(), item.message);
                let line = ResultLine {
                    inst,
                    channel: item.channel,
                    message: item.message,
                    author: item.author,
                    when,
                    place,
                    text: item.text,
                    marks: Vec::new(),
                };
                div()
                    .flex()
                    .items_start()
                    .gap(px(4.))
                    .child(div().flex_1().min_w_0().child(result_row(n, line, cx)))
                    .child(
                        crate::channel::tip_button(
                            ("saved-remove", n),
                            crate::assets::CLOSE,
                            "Remove from saved",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.core.set_saved(&remove_inst, message, None);
                            cx.notify();
                        })),
                    )
                    .into_any_element()
            })
            .collect()
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
            Key::Saved => (
                "Saved messages".into(),
                "Kept on this device, sealed like your drafts".into(),
            ),
            Key::Pins(inst, channel) => {
                let name = store
                    .instance(inst)
                    .and_then(|s| Some(view::channel_title(s, s.channel(*channel)?)))
                    .unwrap_or_default();
                ("Pinned messages".into(), name)
            }
        }
    }

    /// Under the search field: the order, the filters understood, what matched nobody, and
    /// filters to finish the one being typed.
    fn search_aids(
        &self,
        key: &Key,
        page: &ResultsPage,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Key::Search(inst, space) = key else {
            return None;
        };
        let input = page.input.as_ref()?;
        let typed = input.read(cx).value().to_string();
        let store = self.core.store();
        let s = store.instance(inst)?;
        let ranks = s.has_feature(hermes_client_core::proto::compat::features::SEARCH_RANK);
        let f = filters(s, &typed, *space, self.utc_offset);
        let suggestions = filter_suggestions(s, *space, &typed);
        drop(store);

        let mut row: Vec<AnyElement> = Vec::new();
        if ranks {
            for (n, (sort, name)) in [
                (SearchSort::Newest, "Newest"),
                (SearchSort::Relevance, "Best match"),
            ]
            .into_iter()
            .enumerate()
            {
                let sort_key = key.clone();
                row.push(
                    crate::voice::chip(("search-sort", n), name, page.sort == sort)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_search_sort(sort_key.clone(), sort, cx);
                        }))
                        .into_any_element(),
                );
            }
        }
        // The filters understood, as they were typed.
        let understood: Vec<String> = typed
            .split_whitespace()
            .filter(|w| is_filter_word(w) && !f.words.split_whitespace().any(|kept| kept == *w))
            .map(str::to_owned)
            .collect();
        if !understood.is_empty() && !row.is_empty() {
            row.push(
                div()
                    .w(px(1.))
                    .h(px(16.))
                    .bg(theme::border())
                    .into_any_element(),
            );
        }
        for (n, word) in understood.into_iter().enumerate() {
            row.push(
                div()
                    .id(("search-filter", n))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(theme::radius_small())
                    .bg(theme::accent_soft())
                    .text_size(theme::caption_size())
                    .text_color(theme::accent())
                    .child(word)
                    .into_any_element(),
            );
        }
        for (n, suggestion) in suggestions.into_iter().enumerate() {
            let complete_key = key.clone();
            let label = suggestion.clone();
            row.push(
                div()
                    .id(("search-suggest", n))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(theme::radius_small())
                    .border_1()
                    .border_color(theme::border())
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::hover()).text_color(theme::accent()))
                    .text_size(theme::caption_size())
                    .text_color(theme::text_secondary())
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.complete_filter(&complete_key, suggestion.clone(), window, cx);
                    }))
                    .into_any_element(),
            );
        }
        let warnings = f.unresolved.into_iter().map(|text| {
            div()
                .text_size(theme::caption_size())
                .text_color(theme::warning())
                .child(text)
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.))
                        .children(row),
                )
                .children(warnings)
                .into_any_element(),
        )
    }

    fn result_lines(
        &self,
        key: &Key,
        items: &[(InstanceId, ChatMessage)],
        words: &[String],
    ) -> Vec<ResultLine> {
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
                let text = view::plain_text(s, space, &m.content, 4000);
                let (text, marks) = if text.is_empty() && !m.attachments.is_empty() {
                    (
                        format!("[{} attachment(s)]", m.attachments.len()),
                        Vec::new(),
                    )
                } else {
                    snippet(&text, words)
                };
                Some(ResultLine {
                    inst: inst.clone(),
                    channel: m.channel_id,
                    message: m.id,
                    author: s.display_name(space, m.author.id),
                    when: time::format(m.created_at() as i64, "%Y-%m-%d %H:%M", self.utc_offset),
                    place,
                    text,
                    marks,
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
    /// Where the searched words are in `text`.
    marks: Vec<std::ops::Range<usize>>,
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
        marks,
    } = line;
    let marked = gpui_kit::HighlightStyle {
        background_color: Some(theme::alpha(theme::accent(), 0.25).into()),
        font_weight: Some(FontWeight::BOLD),
        ..gpui_kit::HighlightStyle::default()
    };
    let text = gpui_kit::StyledText::new(text)
        .with_highlights(marks.into_iter().map(move |range| (range, marked)));
    div()
        .id(("result", n))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(2.))
        .px(px(8.))
        .py(px(6.))
        .rounded(theme::radius_small())
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
                        .min_w_0()
                        .truncate()
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
                        .max_w(px(220.))
                        .truncate()
                        .child(place),
                ),
        )
        .child(div().text_size(theme::secondary_size()).child(text))
        .into_any_element()
}

/// Whether `word` is one of the search filters (`from:`, `in:`, `has:`, the dates).
fn is_filter_word(word: &str) -> bool {
    [
        "from:", "in:", "has:", "before:", "after:", "during:", "on:",
    ]
    .iter()
    .any(|p| word.len() > p.len() && word.to_lowercase().starts_with(p))
}

/// The words a search looks for (filters left out), lower case and without punctuation.
fn search_words(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .filter(|w| !is_filter_word(w))
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Filters finishing the last word typed: names after `from:`, channels after `in:`, kinds after
/// `has:`; the filters themselves while nothing is typed.
fn filter_suggestions(
    s: &hermes_client_core::store::InstanceStore,
    space: Option<Id>,
    typed: &str,
) -> Vec<String> {
    const MAX: usize = 6;
    if typed.trim().is_empty() {
        let mut all = vec!["from:", "has:", "before:", "during:"];
        if space.is_some() {
            all.insert(1, "in:");
        }
        return all.into_iter().map(str::to_owned).collect();
    }
    if typed.ends_with(char::is_whitespace) {
        return Vec::new();
    }
    let last = typed.split_whitespace().last().unwrap_or_default();
    let lower = last.to_lowercase();
    if let Some(start) = lower.strip_prefix("from:") {
        let start = start.trim_start_matches('@');
        let mut names: Vec<String> = s
            .users
            .values()
            .map(|u| u.username.clone())
            .filter(|n| n.to_lowercase().starts_with(start) && n.to_lowercase() != start)
            .collect();
        names.sort();
        names.truncate(MAX);
        return names.into_iter().map(|n| format!("from:{n}")).collect();
    }
    if let Some(start) = lower.strip_prefix("in:") {
        let start = start.trim_start_matches('#');
        let Some(state) = space.and_then(|id| s.spaces.get(&id)) else {
            return Vec::new();
        };
        let mut names: Vec<String> = state
            .channels
            .values()
            .filter_map(|c| c.name.clone())
            .filter(|n| n.to_lowercase().starts_with(start) && n.to_lowercase() != start)
            .collect();
        names.sort();
        names.truncate(MAX);
        return names.into_iter().map(|n| format!("in:{n}")).collect();
    }
    if let Some(start) = lower.strip_prefix("has:") {
        return SearchHas::ALL
            .into_iter()
            .map(SearchHas::as_str)
            .filter(|h| h.starts_with(start) && *h != start)
            .map(|h| format!("has:{h}"))
            .collect();
    }
    Vec::new()
}

/// The part of `text` around the first searched word (at most [`SNIPPET_CHARS`]), and where each
/// searched word is in it.
fn snippet(text: &str, words: &[String]) -> (String, Vec<std::ops::Range<usize>>) {
    const BEFORE: usize = 60;
    let lower = text.to_lowercase();
    // Lower case of some letters is longer: then no marks, from the start.
    let same = lower.len() == text.len();
    let first = if same {
        words.iter().filter_map(|w| lower.find(w.as_str())).min()
    } else {
        None
    };
    let mut start = match first {
        Some(at) if at > BEFORE => text.floor_char_boundary(at - BEFORE),
        _ => 0,
    };
    // Start at a word.
    if start > 0
        && let Some(space) = text[start..].find(char::is_whitespace)
        && start + space < first.unwrap_or(0)
    {
        start += space + 1;
    }
    let end = text.floor_char_boundary((start + SNIPPET_CHARS).min(text.len()));
    let prefix = if start > 0 { "\u{2026}" } else { "" };
    let mut out = format!("{prefix}{}", &text[start..end]);
    if end < text.len() {
        out.push('\u{2026}');
    }
    if !same {
        return (out, Vec::new());
    }
    let body = &lower[start..end];
    let mut marks: Vec<std::ops::Range<usize>> = Vec::new();
    for word in words {
        let mut from = 0;
        while let Some(at) = body[from..].find(word.as_str()) {
            let begin = prefix.len() + from + at;
            marks.push(begin..begin + word.len());
            from += at + word.len().max(1);
        }
    }
    marks.sort_by_key(|r| r.start);
    // Overlapping marks become one.
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for mark in marks {
        match merged.last_mut() {
            Some(last) if mark.start <= last.end => last.end = last.end.max(mark.end),
            _ => merged.push(mark),
        }
    }
    (out, merged)
}

/// The pages kept while the app runs.
pub type ResultsPages = HashMap<Key, ResultsPage>;

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_client_core::proto::models::{MessageKind, User};

    #[test]
    fn snippets_show_and_mark_the_words() {
        let words = search_words("Pizza from:bob \"friday!\"");
        assert_eq!(words, ["pizza", "friday"]);
        let (text, marks) = snippet("We had PIZZA on Friday, pizza again.", &words);
        assert_eq!(text, "We had PIZZA on Friday, pizza again.");
        let marked: Vec<&str> = marks.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(marked, ["PIZZA", "Friday", "pizza"]);
        // Far into a long text: the snippet starts near the word.
        let long = format!("{} needle here", "word ".repeat(100));
        let (text, marks) = snippet(&long, &["needle".to_owned()]);
        assert!(text.starts_with('\u{2026}'));
        assert_eq!(&text[marks[0].clone()], "needle");
        assert!(text.chars().count() <= SNIPPET_CHARS + 2);
    }

    #[test]
    fn filters_are_suggested_while_typed() {
        let s = hermes_client_core::store::InstanceStore::default();
        assert!(filter_suggestions(&s, None, "").contains(&"from:".to_owned()));
        assert!(!filter_suggestions(&s, None, "").contains(&"in:".to_owned()));
        assert_eq!(filter_suggestions(&s, None, "cats has:i"), ["has:image"]);
        assert!(filter_suggestions(&s, None, "cats has:image ").is_empty());
        let f = filters(&s, "from:nobody soup", None, 0);
        assert_eq!(f.unresolved.len(), 1);
    }

    #[test]
    fn search_words_give_filters() {
        let s = hermes_client_core::store::InstanceStore::default();
        let f = filters(&s, "pizza has:image during:2026-10-31 HAS:nope", None, 0);
        assert_eq!(f.words, "pizza HAS:nope");
        assert_eq!(f.has, Some(SearchHas::Image));
        let (start, end) = time::parse_day("2026-10-31", 0).unwrap();
        assert_eq!(f.before, Some(Id::min_at_unix_ms(end as u64)));
        assert!(f.after.unwrap() < Id::min_at_unix_ms(start as u64));
        let f = filters(&s, "before:2026-13-01", None, 0);
        assert_eq!(f.words, "before:2026-13-01", "not a date: kept as words");
        assert!(!f.any());

        let mut m = msg(1);
        m.content = "see https://example.org".into();
        let link = Filters {
            has: Some(SearchHas::Link),
            ..Filters::default()
        };
        assert!(passes(&m, &link));
        let file = Filters {
            has: Some(SearchHas::File),
            ..Filters::default()
        };
        assert!(!passes(&m, &file));
    }

    fn msg(id: u64) -> ChatMessage {
        ChatMessage {
            id: Id(id << 22),
            channel_id: Id(1),
            author: User {
                id: Id(2),
                username: "a".into(),
                discriminator: None,
                display_name: None,
                avatar: None,
                status_text: None,
                banner: None,
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
            e2ee: None,
            lock: Default::default(),
            message_key: None,
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
