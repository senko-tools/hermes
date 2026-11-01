//! Message lists with jump-to (PLAN.md section 15.1): the Search buffer (FTS over a space or a
//! whole instance), the Mentions buffer (recent mentions on every instance) and a channel's
//! pinned messages. Clicking a result opens its channel at that message.

use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Message as ChatMessage;
use hermes_client_core::{Core, InstanceId, time, view};
use iced::widget::{Space, button, column, container, row, scrollable, text_input};
use iced::{Alignment, Element, Fill};

use super::space_settings::Remote;
use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::{heading, label, small};

pub const SEARCH_INPUT: &str = "search-input";
pub const PAGE: u32 = 25;

/// Which list a results buffer shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// Search in a space, or (`None`) in every channel and DM of the instance.
    Search(InstanceId, Option<Id>),
    Mentions,
    Pins(InstanceId, Id),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub query: String,
    /// The query the results are for.
    pub searched: String,
    pub items: Remote<Vec<(InstanceId, ChatMessage)>>,
    /// All search matches (the list holds the loaded pages).
    pub total: Option<u64>,
    pub loading_more: bool,
}

impl State {
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

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Query(String),
    Submit,
    Open(InstanceId, Id, Id),
    More,
    Refresh,
    Close,
    /// A page arrived: append?, results, total (search only).
    Loaded(bool, Result<Page, String>),
}

/// Loaded results and, for a search, the total number of matches.
pub type Page = (Vec<(InstanceId, ChatMessage)>, Option<u64>);

/// The header title of a results buffer.
pub fn title(core: &Core, key: &Key) -> (String, String) {
    let store = core.store();
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

pub fn view<'a>(
    core: &Core,
    key: &Key,
    state: &'a State,
    utc_offset: i64,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let (heading_text, scope) = title(core, key);
    let mut header = column![
        row![
            heading(heading_text, fonts),
            small(scope, fonts).color(theme.text.tertiary.0),
            Space::new().width(Fill),
            button(small("Refresh", fonts))
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::Refresh),
            button(small("Close", fonts))
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::Close),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
    ]
    .spacing(8);
    if matches!(key, Key::Search(..)) {
        header = header.push(
            text_input("Search messages… (Enter)", &state.query)
                .id(SEARCH_INPUT)
                .on_input(Message::Query)
                .on_submit(Message::Submit)
                .font(fonts.ui)
                .size(fonts.size)
                .padding(8),
        );
    }
    let header = container(header)
        .padding([8, 12])
        .width(Fill)
        .style(move |_| style::pane_header(theme));

    let mut list = column![].spacing(6).padding(12);
    match &state.items {
        Remote::Idle => {
            let hint = if matches!(key, Key::Search(..)) {
                "Type words to find and press Enter. Every word must appear (as the start of a \
                 word)."
            } else {
                "Loading…"
            };
            list = list.push(label(hint, fonts).color(theme.text.tertiary.0));
        }
        Remote::Loading => list = list.push(label("Loading…", fonts).color(theme.text.tertiary.0)),
        Remote::Failed(err) => {
            list =
                list.push(label(format!("Could not load: {err}"), fonts).color(theme.text.error.0));
        }
        Remote::Ready(items) if items.is_empty() => {
            let text = match key {
                Key::Search(..) => format!("Nothing found for “{}”.", state.searched),
                Key::Mentions => "Nobody mentioned you yet.".to_owned(),
                Key::Pins(..) => {
                    "Nothing is pinned here. Right-click a message and choose Pin.".to_owned()
                }
            };
            list = list.push(label(text, fonts).color(theme.text.tertiary.0));
        }
        Remote::Ready(items) => {
            if let (Key::Search(..), Some(total)) = (key, state.total) {
                list = list.push(
                    small(
                        format!("{total} result{}", if total == 1 { "" } else { "s" }),
                        fonts,
                    )
                    .color(theme.text.tertiary.0),
                );
            }
            let store = core.store();
            for (inst, m) in items {
                let Some(s) = store.instance(inst) else {
                    continue;
                };
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
                let when = time::format(m.created_at() as i64, "%Y-%m-%d %H:%M", utc_offset);
                let text = view::plain_text(s, space, &m.content, 300);
                let text = if text.is_empty() && !m.attachments.is_empty() {
                    format!("[{} attachment(s)]", m.attachments.len())
                } else {
                    text
                };
                list = list.push(
                    button(
                        column![
                            row![
                                label(s.display_name(space, m.author.id), fonts).font(fonts.bold()),
                                small(when, fonts).color(theme.text.tertiary.0),
                                Space::new().width(Fill),
                                small(place, fonts).color(theme.text.secondary.0),
                            ]
                            .spacing(8)
                            .align_y(Alignment::Center),
                            label(text, fonts),
                        ]
                        .spacing(2),
                    )
                    .padding([6, 10])
                    .width(Fill)
                    .style(move |_, st| style::sidebar_item(theme, st, false))
                    .on_press(Message::Open(inst.clone(), m.channel_id, m.id)),
                );
            }
            if state.has_more() {
                list = list.push(
                    button(small(
                        if state.loading_more {
                            "Loading…"
                        } else {
                            "Load more"
                        },
                        fonts,
                    ))
                    .style(move |_, s| style::secondary(theme, s))
                    .on_press_maybe((!state.loading_more).then_some(Message::More)),
                );
            }
        }
    }
    column![header, scrollable(list.width(Fill)).height(Fill)].into()
}
