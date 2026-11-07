//! The chat of a channel or DM (PLAN.md section 15.2): a virtualized message list held at the
//! bottom, with grouping, day separators, the "New messages" divider, reply previews, pending
//! messages, attachments, reactions and the typing line.

use std::collections::{BTreeSet, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{RopeExt as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, ExternalPaths, FontStyle, FontWeight, HighlightStyle,
    InteractiveText, ListAlignment, ListScrollEvent, ListState, MouseButton, SharedString,
    StrikethroughStyle, StyledText, Subscription, UnderlineStyle, Window, div, img, list, px, rgba,
};
use hermes_client_core::commands::{self, SlashCommand};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{Attachment, Message as ChatMessage};
use hermes_client_core::proto::permissions::Permissions;
use hermes_client_core::store::{HistoryAnchor, InstanceStore, LocalId, PendingState};
use hermes_client_core::view::{self, Suggestion};
use hermes_client_core::{Draft, InstanceId, now_ms, time};

use crate::assets::{self, icon};
use crate::images::Img;
use crate::markup::{self, Block, Link, Piece};
use crate::menu_entries::{self, Action, Target};
use crate::rows::{self, MessageRow, PendingRow, Row, RowKey};
use crate::shell::{ChannelKey, Page, Shell};
use crate::theme;

/// Quick reactions offered by the palette.
pub const QUICK_REACTIONS: [&str; 8] = ["👍", "❤️", "😂", "😮", "😢", "🎉", "👀", "🔥"];
const MAX_IMAGE_WIDTH: f32 = 400.0;
const MAX_IMAGE_HEIGHT: f32 = 300.0;
/// Suggestions shown at most.
const MAX_SUGGESTIONS: usize = 8;
/// Older history loads when this many rows from the top are showing.
const LOAD_OLDER_WITHIN: usize = 15;
/// The avatar column, which grouped messages leave empty.
const GUTTER: f32 = theme::AVATAR + 16.0;
/// The hover group of a message row, for the time of a grouped message.
const ROW_GROUP: &str = "message-row";

/// A file being attached.
#[derive(Debug, Clone, PartialEq)]
pub struct Upload {
    pub local: u64,
    pub name: String,
    pub size: u64,
    pub state: UploadState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UploadState {
    Uploading,
    Ready(Attachment),
    Failed(String),
}

/// The full-size image viewer.
pub struct Viewer {
    pub inst: InstanceId,
    pub attachment: Attachment,
    pub image: Img,
}

/// What each channel remembers while the app runs: its list position and its draft.
pub struct ChatView {
    pub list: ListState,
    /// The rows in the list, kept in step with it.
    pub rows: Vec<Row>,
    /// What each row showed when last measured: a change means measuring it again.
    signatures: Vec<u64>,
    /// My read marker when the channel was opened (the divider position).
    pub read_marker: Option<Id>,
    pub loading_older: bool,
    pub loading_newer: bool,
    /// The message whose quick-reaction palette is open.
    pub palette: Option<Id>,
    /// Messages whose spoilers were clicked.
    pub revealed: HashSet<Id>,
    /// A message to scroll to once its page is loaded (search results, reply previews, pins).
    pub jump: Option<Id>,
    /// The message jumped to, drawn highlighted.
    pub highlight: Option<Id>,
    /// Messages selected to delete together (only ones I may delete).
    pub selected: BTreeSet<Id>,
    /// The message the last selecting click was on: Shift+click selects from there.
    pub anchor: Option<Id>,
    pub input: Entity<TextareaState>,
    pub reply_to: Option<Id>,
    /// Editing this message instead of sending a new one.
    pub editing: Option<Id>,
    pub uploads: Vec<Upload>,
    pub suggestions: Vec<Suggestion>,
    pub suggestion: usize,
    _input_events: Subscription,
}

impl ChatView {
    /// A click on message `id` (one of `selectable`, the loaded messages I may delete, oldest
    /// first): Ctrl, or any click while selecting, toggles it; Shift selects the range from the
    /// last clicked one. Without either, nothing is selected and nothing happens.
    pub fn click(&mut self, id: Id, ctrl: bool, shift: bool, selectable: &[Id]) {
        if !selectable.contains(&id) {
            return;
        }
        let anchor = self.anchor.filter(|a| selectable.contains(a));
        if shift && let Some(anchor) = anchor {
            let (from, to) = (anchor.min(id), anchor.max(id));
            self.selected.extend(
                selectable
                    .iter()
                    .copied()
                    .filter(|m| (from..=to).contains(m)),
            );
        } else if ctrl || shift || !self.selected.is_empty() {
            if !self.selected.remove(&id) {
                self.selected.insert(id);
            }
        } else {
            return;
        }
        self.anchor = Some(id);
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }

    /// Clears the draft, its reply or edit state, and its uploads.
    fn clear_mode(&mut self) {
        self.reply_to = None;
        self.editing = None;
        self.uploads.clear();
        self.suggestions.clear();
    }

    /// Attachments that finished uploading; `Err` while some are still uploading.
    fn ready_attachments(&self) -> Result<Vec<Attachment>, &'static str> {
        let mut out = Vec::new();
        for u in &self.uploads {
            match &u.state {
                UploadState::Ready(a) => out.push(a.clone()),
                UploadState::Uploading => return Err("Wait until the files are uploaded."),
                UploadState::Failed(_) => {}
            }
        }
        Ok(out)
    }
}

/// Whether an attachment is shown as an inline image.
pub fn is_image(a: &Attachment) -> bool {
    a.has_thumb && a.content_type.starts_with("image/")
}

/// The size an inline image is shown at: scaled into 400×300, never enlarged.
pub fn display_size(a: &Attachment) -> (f32, f32) {
    match (a.width, a.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => {
            let (w, h) = (w as f32, h as f32);
            let scale = (MAX_IMAGE_WIDTH / w).min(MAX_IMAGE_HEIGHT / h).min(1.0);
            (w * scale, h * scale)
        }
        _ => (300.0, 200.0),
    }
}

/// `1.4 MB`, `820 KB`, `12 B`.
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB * KB {
        format!("{:.1} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// `Bob is typing…`, `Bob and Carol are typing…`, `Several people are typing…`.
pub fn typing_text(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [a] => Some(format!("{a} is typing…")),
        [a, b] => Some(format!("{a} and {b} are typing…")),
        [a, b, c] => Some(format!("{a}, {b} and {c} are typing…")),
        _ => Some("Several people are typing…".into()),
    }
}

/// The color of a user's name: their highest colored role.
pub fn author_color(s: &InstanceStore, space: Id, user: Id) -> Option<u32> {
    let state = s.spaces.get(&space)?;
    let member = state.members.get(&user)?;
    state
        .roles
        .iter()
        .filter(|r| member.role_ids.contains(&r.id))
        .find_map(|r| r.color)
}

/// A day divider's text: `Today`, `Yesterday`, the weekday within the last week, else the
/// date (`October 31, 2026`). `day` and `today` count days since 1970-01-01.
pub fn day_label(day: i64, today: i64) -> String {
    const WEEKDAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    match today - day {
        0 => "Today".to_owned(),
        1 => "Yesterday".to_owned(),
        // 1970-01-01 was a Thursday.
        2..=6 => WEEKDAYS[(day + 4).rem_euclid(7) as usize].to_owned(),
        _ => {
            let date = time::format(day * 86_400_000, "%Y %m %d", 0);
            let mut parts = date.split(' ').map(|p| p.parse::<usize>().unwrap_or(1));
            let (y, m, d) = (
                parts.next().unwrap_or(1970),
                parts.next().unwrap_or(1),
                parts.next().unwrap_or(1),
            );
            format!("{} {d}, {y}", MONTHS[(m - 1).min(11)])
        }
    }
}

/// The token being typed before byte offset `cursor` of `text`: from the last whitespace to the
/// cursor, if it starts with `@`, `#`, `:` (two or more characters) or `/` (at the very start).
fn token_before(text: &str, cursor: usize) -> Option<(usize, String)> {
    let mut end = cursor.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let before = &text[..end];
    let start = before.rfind(char::is_whitespace).map_or(0, |i| {
        i + before[i..].chars().next().map_or(1, char::len_utf8)
    });
    let token = &before[start..];
    let ok = match token.chars().next()? {
        '@' | '#' => true,
        ':' => token.chars().count() >= 3,
        '/' => start == 0,
        _ => false,
    };
    ok.then(|| (start, token.to_owned()))
}

/// The length of the text two row lists share at the start and the end: `splice` then only touches
/// the part between, and the list keeps its scroll position.
fn common_ends(old: &[RowKey], new: &[RowKey]) -> (usize, usize) {
    let front = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let back = old[front..]
        .iter()
        .rev()
        .zip(new[front..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (front, back)
}

/// What a row's height depends on, besides the width.
fn signature(row: &Row, s: &InstanceStore, channel: Id) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let window = s.window(channel);
    match row {
        Row::Message(m) => {
            m.grouped.hash(&mut hash);
            if let Some(message) = window.and_then(|w| w.get(m.id)) {
                message.content.hash(&mut hash);
                message.edited_at.hash(&mut hash);
                message.attachments.len().hash(&mut hash);
                for r in &message.reactions {
                    r.emoji.hash(&mut hash);
                }
            }
        }
        Row::Pending(p) => {
            p.grouped.hash(&mut hash);
            if let Some(pending) = window.and_then(|w| w.pending(p.local_id)) {
                pending.content.hash(&mut hash);
                matches!(pending.state, PendingState::Failed(_)).hash(&mut hash);
            }
        }
        _ => {}
    }
    hash.finish()
}

impl Shell {
    // ---- opening ------------------------------------------------------------------------------

    /// The state of a channel's chat, created the first time it opens.
    fn chat_view(&mut self, key: &ChannelKey, window: &mut Window, cx: &mut Context<Self>) {
        if self.chats.contains_key(key) {
            return;
        }
        let marker = self
            .core
            .store()
            .instance(&key.0)
            .and_then(|s| s.read_states.get(&key.1).map(|r| r.last_read_id));
        let list = ListState::new(0, ListAlignment::Bottom, px(600.));
        list.set_follow_mode(gpui_kit::FollowMode::Tail);
        let scroll_key = key.clone();
        list.set_scroll_handler(cx.listener(move |this, event: &ListScrollEvent, _, cx| {
            this.chat_scrolled(&scroll_key, event, cx);
        }));
        let placeholder = {
            let store = self.core.store();
            let s = store.instance(&key.0);
            let channel = s.and_then(|s| s.channel(key.1));
            match (s, channel) {
                (Some(s), Some(c)) => {
                    let title = view::channel_title(s, c);
                    if c.kind == hermes_client_core::proto::models::ChannelKind::Dm {
                        format!("Message @{title}")
                    } else {
                        format!("Message {title}")
                    }
                }
                _ => "Message".to_owned(),
            }
        };
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .submit_on_enter(true)
                .placeholder(placeholder)
        });
        let event_key = key.clone();
        let events = cx.subscribe_in(
            &input,
            window,
            move |this, _, event, window, cx| match event {
                InputEvent::Change => this.composer_changed(&event_key, cx),
                InputEvent::PressEnter { shift, secondary } => {
                    this.composer_enter(&event_key, *shift, *secondary, window, cx);
                }
                _ => {}
            },
        );
        self.chats.insert(
            key.clone(),
            ChatView {
                list,
                rows: Vec::new(),
                signatures: Vec::new(),
                read_marker: marker,
                loading_older: false,
                loading_newer: false,
                palette: None,
                revealed: HashSet::new(),
                jump: None,
                highlight: None,
                selected: BTreeSet::new(),
                anchor: None,
                input,
                reply_to: None,
                editing: None,
                uploads: Vec::new(),
                suggestions: Vec::new(),
                suggestion: 0,
                _input_events: events,
            },
        );
    }

    /// Shows a channel's chat and loads its latest messages.
    pub fn open_chat(
        &mut self,
        inst: InstanceId,
        channel: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = (inst.clone(), channel);
        self.chat_view(&key, window, cx);
        self.page = Page::Chat(inst.clone(), channel);
        self.update_visible_channels();
        self.sync_rows(&key);
        self.focus_composer = true;
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.open_channel(&inst, channel).await },
            move |this, result, _, cx| {
                if let Err(err) = result {
                    this.error(format!("could not open the channel: {err}"), cx);
                }
                this.sync_rows(&key);
                this.mark_open_read(cx);
            },
        );
        cx.notify();
    }

    pub fn focus_open_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.open_channel().and_then(|key| self.chats.get(&key)) {
            view.input.update(cx, |input, cx| input.focus(window, cx));
        }
    }

    // ---- the list -----------------------------------------------------------------------------

    /// The store changed for a channel: brings its rows and list up to date.
    pub fn messages_changed(&mut self, inst: &str, channel: Id, cx: &mut Context<Self>) {
        let key = (inst.to_owned(), channel);
        self.sync_rows(&key);
        if self.open_channel().as_ref() == Some(&key) {
            self.mark_open_read(cx);
        }
    }

    /// Rebuilds a channel's rows and tells the list only what differs, which keeps its position.
    fn sync_rows(&mut self, key: &ChannelKey) {
        let store = self.core.store();
        let Some(view) = self.chats.get_mut(key) else {
            return;
        };
        let Some(s) = store.instance(&key.0) else {
            return;
        };
        let new = match s.window(key.1) {
            Some(window) => rows::build(
                s,
                window,
                &rows::Options {
                    group_minutes: self.config.buffer.group_minutes,
                    utc_offset: self.utc_offset,
                    read_marker: view.read_marker,
                },
            ),
            None => Vec::new(),
        };
        let old_keys: Vec<RowKey> = view.rows.iter().map(Row::key).collect();
        let new_keys: Vec<RowKey> = new.iter().map(Row::key).collect();
        let signatures: Vec<u64> = new.iter().map(|r| signature(r, s, key.1)).collect();
        let (front, back) = common_ends(&old_keys, &new_keys);
        if front + back != old_keys.len() || front + back != new_keys.len() {
            view.list
                .splice(front..old_keys.len() - back, new_keys.len() - front - back);
        }
        for (i, signature) in signatures.iter().enumerate().take(front) {
            if view.signatures[i] != *signature {
                view.list.remeasure_items(i..i + 1);
            }
        }
        for i in 0..back {
            let (old, new) = (old_keys.len() - 1 - i, new_keys.len() - 1 - i);
            if view.signatures[old] != signatures[new] {
                view.list.remeasure_items(new..new + 1);
            }
        }
        view.rows = new;
        view.signatures = signatures;
        if let Some(target) = view.jump
            && let Some(ix) = view
                .rows
                .iter()
                .position(|r| r.key() == RowKey::Message(target))
        {
            view.list.scroll_to_reveal_item(ix);
            view.jump = None;
        }
    }

    fn chat_scrolled(&mut self, key: &ChannelKey, event: &ListScrollEvent, cx: &mut Context<Self>) {
        let Some(view) = self.chats.get(key) else {
            return;
        };
        let near_top = event.visible_range.start < LOAD_OLDER_WITHIN;
        let at_end = event.visible_range.end >= event.count;
        if near_top && !view.loading_older {
            self.load_older(key, cx);
        }
        let jumping = self.chats.get(key).is_some_and(|v| v.jump.is_some());
        if at_end && !jumping {
            self.load_newer(key, cx);
            self.mark_open_read(cx);
        }
        // The "jump to present" button follows the position.
        cx.notify();
    }

    /// Loads the page before the oldest loaded message.
    fn load_older(&mut self, key: &ChannelKey, cx: &mut Context<Self>) {
        let oldest = {
            let store = self.core.store();
            let Some(window) = store.instance(&key.0).and_then(|s| s.window(key.1)) else {
                return;
            };
            if !window.loaded || window.reached_start {
                return;
            }
            match window.messages.first() {
                Some(m) => m.id,
                None => return,
            }
        };
        if let Some(view) = self.chats.get_mut(key) {
            view.loading_older = true;
        }
        let core = self.core.clone();
        let (inst, channel) = key.clone();
        let done = key.clone();
        self.run(
            cx,
            async move {
                core.load_history(&inst, channel, HistoryAnchor::Before(oldest))
                    .await
            },
            move |this, result, _, _| {
                if let Err(err) = result {
                    tracing::warn!("older messages: {err}");
                }
                if let Some(view) = this.chats.get_mut(&done) {
                    view.loading_older = false;
                }
            },
        );
    }

    /// At the bottom of a window that does not reach the present (after a jump): the next page.
    fn load_newer(&mut self, key: &ChannelKey, cx: &mut Context<Self>) {
        if self.chats.get(key).is_none_or(|v| v.loading_newer) {
            return;
        }
        let newest = {
            let store = self.core.store();
            let Some(window) = store.instance(&key.0).and_then(|s| s.window(key.1)) else {
                return;
            };
            if !window.loaded || window.reached_end {
                return;
            }
            match window.messages.last() {
                Some(m) => m.id,
                None => return,
            }
        };
        if let Some(view) = self.chats.get_mut(key) {
            view.loading_newer = true;
        }
        let core = self.core.clone();
        let (inst, channel) = key.clone();
        let done = key.clone();
        self.run(
            cx,
            async move {
                core.load_history(&inst, channel, HistoryAnchor::After(newest))
                    .await
            },
            move |this, result, _, _| {
                if let Err(err) = result {
                    tracing::warn!("newer messages: {err}");
                }
                if let Some(view) = this.chats.get_mut(&done) {
                    view.loading_newer = false;
                }
            },
        );
    }

    /// Marks the open channel read when its newest message is on screen and the window has the
    /// focus (section 15.2).
    pub fn mark_open_read(&self, _cx: &mut Context<Self>) {
        if !self.focused {
            return;
        }
        let Some((inst, channel)) = self.open_channel() else {
            return;
        };
        let Some(view) = self.chats.get(&(inst.clone(), channel)) else {
            return;
        };
        if view.list.is_scrolled_to_end() == Some(false) {
            return;
        }
        let last = {
            let store = self.core.store();
            let Some(s) = store.instance(&inst) else {
                return;
            };
            let Some(window) = s.window(channel) else {
                return;
            };
            if !window.reached_end {
                return;
            }
            let Some(last) = window
                .messages
                .iter()
                .rev()
                .find(|m| !m.deleted)
                .map(|m| m.id)
            else {
                return;
            };
            if s.read_states
                .get(&channel)
                .is_some_and(|r| r.last_read_id >= last)
            {
                return;
            }
            last
        };
        self.core.mark_read(&inst, channel, last);
    }

    /// Scrolls the open channel to `message`, loading its page if needed.
    pub fn jump_to(
        &mut self,
        inst: InstanceId,
        channel: Id,
        message: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = (inst.clone(), channel);
        if self.open_channel().as_ref() != Some(&key) {
            self.select(inst.clone(), channel, window, cx);
        }
        self.chat_view(&key, window, cx);
        if let Some(view) = self.chats.get_mut(&key) {
            view.jump = Some(message);
            view.highlight = Some(message);
        }
        let loaded = self
            .core
            .store()
            .instance(&inst)
            .and_then(|s| s.window(channel))
            .is_some_and(|w| w.loaded && w.get(message).is_some());
        if loaded {
            self.sync_rows(&key);
            return;
        }
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                core.load_history(&inst, channel, HistoryAnchor::Around(message))
                    .await
            },
            move |this, result, _, cx| {
                if let Err(err) = result {
                    this.error(format!("could not load the message: {err}"), cx);
                }
                this.sync_rows(&key);
            },
        );
    }

    /// Back to the newest messages.
    fn jump_to_present(&mut self, key: &ChannelKey, cx: &mut Context<Self>) {
        let live = self
            .core
            .store()
            .instance(&key.0)
            .and_then(|s| s.window(key.1).map(|w| w.reached_end))
            .unwrap_or(false);
        if let Some(view) = self.chats.get(key) {
            view.list.scroll_to_end();
        }
        if live {
            cx.notify();
            return;
        }
        let core = self.core.clone();
        let (inst, channel) = key.clone();
        let key = key.clone();
        self.run(
            cx,
            async move {
                core.load_history(&inst, channel, HistoryAnchor::Latest)
                    .await
            },
            move |this, result, _, cx| {
                if let Err(err) = result {
                    this.error(format!("could not load messages: {err}"), cx);
                }
                this.sync_rows(&key);
                if let Some(view) = this.chats.get(&key) {
                    view.list.scroll_to_end();
                }
            },
        );
    }

    // ---- the message box ----------------------------------------------------------------------

    fn composer_changed(&mut self, key: &ChannelKey, cx: &mut Context<Self>) {
        let Some(view) = self.chats.get(key) else {
            return;
        };
        let text = view.input.read(cx).value().to_string();
        if !text.trim().is_empty() && !text.starts_with('/') && view.editing.is_none() {
            self.core.typing(&key.0, key.1);
        }
        self.refresh_suggestions(key, cx);
    }

    /// Autocomplete for the token at the cursor.
    fn refresh_suggestions(&mut self, key: &ChannelKey, cx: &mut Context<Self>) {
        let Some(view) = self.chats.get(key) else {
            return;
        };
        let (text, cursor) = {
            let input = view.input.read(cx);
            (input.value().to_string(), input.cursor())
        };
        let suggestions = match token_before(&text, cursor) {
            Some((_, token)) => self
                .core
                .store()
                .instance(&key.0)
                .map(|s| view::complete(s, key.1, &token, MAX_SUGGESTIONS))
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let Some(view) = self.chats.get_mut(key) else {
            return;
        };
        if suggestions != view.suggestions {
            view.suggestion = 0;
        }
        view.suggestions = suggestions;
        cx.notify();
    }

    /// Replaces the token before the cursor with suggestion `n`.
    pub fn accept_suggestion(
        &mut self,
        key: &ChannelKey,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.chats.get_mut(key) else {
            return;
        };
        let Some(insert) = view.suggestions.get(n).map(|s| s.insert.clone()) else {
            return;
        };
        view.suggestions.clear();
        let input = view.input.clone();
        input.update(cx, |input, cx| {
            let text = input.value().to_string();
            if let Some((start, _)) = token_before(&text, input.cursor()) {
                let end = input.cursor();
                let new = format!("{}{insert}{}", &text[..start], &text[end..]);
                let cursor = start + insert.len();
                input.set_value(new, window, cx);
                let position = input.text().offset_to_position(cursor);
                input.set_cursor_position(position, window, cx);
            }
        });
        cx.notify();
    }

    fn composer_enter(
        &mut self,
        key: &ChannelKey,
        shift: bool,
        secondary: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let send = if self.config.buffer.enter_sends {
            !shift
        } else {
            shift || secondary
        };
        if send {
            self.submit(key.clone(), window, cx);
        } else if let Some(view) = self.chats.get(key) {
            view.input
                .update(cx, |input, cx| input.insert("\n", window, cx));
        }
    }

    fn set_composer_text(
        &mut self,
        key: &ChannelKey,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self.chats.get(key) {
            let text = text.to_owned();
            view.input.update(cx, |input, cx| {
                let end = text.len();
                input.set_value(text, window, cx);
                let position = input.text().offset_to_position(end);
                input.set_cursor_position(position, window, cx);
            });
        }
    }

    /// Inserts text at the cursor of a channel's message box and gives it the keyboard.
    pub fn insert_text(
        &mut self,
        key: &ChannelKey,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self.chats.get(key) {
            let text = text.to_owned();
            view.input.update(cx, |input, cx| {
                input.insert(text, window, cx);
                input.focus(window, cx);
            });
        }
    }

    pub fn start_edit(
        &mut self,
        key: &ChannelKey,
        message: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = self
            .core
            .store()
            .instance(&key.0)
            .and_then(|s| s.window(key.1))
            .and_then(|w| w.get(message))
            .map(|m| m.content.clone());
        let Some(content) = content else { return };
        if let Some(view) = self.chats.get_mut(key) {
            view.reply_to = None;
            view.suggestions.clear();
            view.editing = Some(message);
        }
        self.set_composer_text(key, &content, window, cx);
        cx.notify();
    }

    /// Up in an empty box: edit my last message.
    pub fn edit_last(&mut self, key: &ChannelKey, window: &mut Window, cx: &mut Context<Self>) {
        let last = {
            let store = self.core.store();
            let s = store.instance(&key.0);
            let me = s.and_then(|s| s.me_id());
            s.and_then(|s| s.window(key.1)).and_then(|w| {
                w.messages
                    .iter()
                    .rev()
                    .find(|m| Some(m.author.id) == me && !m.deleted)
                    .map(|m| m.id)
            })
        };
        if let Some(id) = last {
            self.start_edit(key, id, window, cx);
        }
    }

    /// Escape in the message box: closes the popup, else leaves reply/edit mode, else marks read.
    pub fn composer_cancel(
        &mut self,
        key: &ChannelKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.chats.get_mut(key) else {
            return;
        };
        if !view.suggestions.is_empty() {
            view.suggestions.clear();
        } else if view.editing.is_some() {
            view.editing = None;
            self.set_composer_text(key, "", window, cx);
        } else if view.reply_to.is_some() {
            view.reply_to = None;
        } else {
            self.run_action(
                Target::Channel(key.0.clone(), key.1),
                Action::MarkRead,
                window,
                cx,
            );
        }
        cx.notify();
    }

    /// Sends the draft (or saves the edit, or runs a slash command).
    fn submit(&mut self, key: ChannelKey, window: &mut Window, cx: &mut Context<Self>) {
        let (inst, channel) = key.clone();
        let Some(view) = self.chats.get(&key) else {
            return;
        };
        let text = view.input.read(cx).value().trim_end().to_owned();
        if let Some(message) = view.editing {
            if text.trim().is_empty() {
                self.error("A message cannot be empty; delete it instead.", cx);
                return;
            }
            if let Some(view) = self.chats.get_mut(&key) {
                view.clear_mode();
            }
            self.set_composer_text(&key, "", window, cx);
            let core = self.core.clone();
            self.run(
                cx,
                async move { core.edit_message(&inst, channel, message, &text).await },
                |this, result, _, cx| {
                    if let Err(err) = result {
                        this.error(format!("could not edit: {err}"), cx);
                    }
                },
            );
            return;
        }
        let attachments = match view.ready_attachments() {
            Ok(a) => a,
            Err(err) => {
                self.info(err, cx);
                return;
            }
        };
        if text.trim().is_empty() && attachments.is_empty() {
            return;
        }
        let command = if text.trim().is_empty() {
            SlashCommand::Send(String::new())
        } else {
            match commands::parse(&text) {
                Ok(c) => c,
                Err(err) => {
                    self.error(err.to_string(), cx);
                    return;
                }
            }
        };
        let reply_to = view.reply_to;
        if let Some(view) = self.chats.get_mut(&key) {
            view.clear_mode();
        }
        self.set_composer_text(&key, "", window, cx);
        if let Some(content) = command.message_text() {
            let draft = Draft {
                content,
                reply_to,
                attachments,
            };
            if let Err(err) = self.core.send_message(&inst, channel, draft) {
                self.error(err.to_string(), cx);
            }
            self.sync_rows(&key);
            // Sending scrolls to the newest message.
            if let Some(view) = self.chats.get(&key) {
                view.list.scroll_to_end();
            }
            return;
        }
        self.run_slash_command(inst, channel, command, window, cx);
    }

    fn run_slash_command(
        &mut self,
        inst: InstanceId,
        channel: Id,
        command: SlashCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let space = self
            .core
            .store()
            .instance(&inst)
            .and_then(|s| s.space_of(channel));
        match command {
            SlashCommand::Send(_) | SlashCommand::Me(_) => {}
            SlashCommand::Join(name) => {
                let found = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| view::find_voice_channel(s, Some(channel), &name));
                match found {
                    Some(room) => self.join_voice(inst, room, cx),
                    None => self.error(format!("No voice channel {name:?} here"), cx),
                }
            }
            SlashCommand::Leave => self.leave_voice(cx),
            SlashCommand::Mute => self.toggle_mute(cx),
            SlashCommand::Deafen => self.toggle_deafen(cx),
            SlashCommand::Status(status) => {
                self.core.set_presence(status);
                self.info(format!("Status: {status:?}"), cx);
            }
            SlashCommand::Nick(nick) => {
                let Some(space) = space else {
                    self.error("Nicknames are per space; open a space channel.", cx);
                    return;
                };
                let core = self.core.clone();
                self.run_status(cx, async move {
                    let req = hermes_client_core::proto::api::UpdateMemberRequest {
                        nickname: Some(nick),
                        role_ids: None,
                    };
                    let rest = core.rest(&inst).map_err(|e| e.to_string())?;
                    rest.update_member(space, None, &req)
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok("Nickname changed".to_owned())
                });
            }
            SlashCommand::Dm(name) => {
                let wanted = name.trim().trim_start_matches('@').to_lowercase();
                let user = self.core.store().instance(&inst).and_then(|s| {
                    let me = s.me_id();
                    s.users
                        .values()
                        .filter(|u| Some(u.id) != me)
                        .find(|u| u.username == wanted)
                        .or_else(|| {
                            s.users
                                .values()
                                .filter(|u| Some(u.id) != me)
                                .find(|u| u.name().to_lowercase() == wanted)
                        })
                        .map(|u| u.id)
                });
                match user {
                    Some(user) => self.message_user(inst, user, window, cx),
                    None => self.error(format!("No user {name:?} known here"), cx),
                }
            }
            SlashCommand::Invite => match space {
                Some(space) => {
                    self.run_action(Target::Space(inst, space), Action::InvitePeople, window, cx);
                }
                None => self.error("Invites are for spaces; open a space channel.", cx),
            },
        }
    }

    // ---- files --------------------------------------------------------------------------------

    /// Adds an upload chip and starts uploading `path`.
    pub fn start_upload(
        &mut self,
        key: &ChannelKey,
        path: std::path::PathBuf,
        name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.next_upload += 1;
        let local = self.next_upload;
        let name = name.unwrap_or_else(|| {
            path.file_name()
                .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned())
        });
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let Some(view) = self.chats.get_mut(key) else {
            return;
        };
        view.uploads.push(Upload {
            local,
            name,
            size,
            state: UploadState::Uploading,
        });
        let core = self.core.clone();
        let inst = key.0.clone();
        let key = key.clone();
        self.run(
            cx,
            async move { core.upload(&inst, &path).await.map_err(|e| e.to_string()) },
            move |this, result, _, _| {
                if let Some(upload) = this
                    .chats
                    .get_mut(&key)
                    .and_then(|v| v.uploads.iter_mut().find(|u| u.local == local))
                {
                    upload.state = match result {
                        Ok(attachment) => UploadState::Ready(attachment),
                        Err(err) => UploadState::Failed(err),
                    };
                }
            },
        );
        cx.notify();
    }

    fn pick_files(&mut self, key: ChannelKey, cx: &mut Context<Self>) {
        self.run(
            cx,
            async move {
                rfd::AsyncFileDialog::new()
                    .set_title("Attach files")
                    .pick_files()
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|f| f.path().to_path_buf())
                    .collect::<Vec<_>>()
            },
            move |this, paths, _, cx| {
                for path in paths.into_iter().filter(|p| p.is_file()) {
                    this.start_upload(&key, path, None, cx);
                }
                this.focus_composer = true;
            },
        );
    }

    /// Ctrl+V: an image on the clipboard is attached instead of pasted. `true` if one was.
    fn paste_image(&mut self, key: &ChannelKey, cx: &mut Context<Self>) -> bool {
        let Ok(mut clipboard) = arboard::Clipboard::new() else {
            return false;
        };
        let Ok(image) = clipboard.get_image() else {
            return false;
        };
        let Some(buffer) = image::RgbaImage::from_raw(
            image.width as u32,
            image.height as u32,
            image.bytes.into_owned(),
        ) else {
            self.error("could not read the pasted image", cx);
            return true;
        };
        let dir = std::env::temp_dir().join("hermes-gui");
        self.next_upload += 1;
        let path = dir.join(format!(
            "paste-{}-{}.png",
            std::process::id(),
            self.next_upload
        ));
        let saved = std::fs::create_dir_all(&dir)
            .map_err(|e| e.to_string())
            .and_then(|()| buffer.save(&path).map_err(|e| e.to_string()));
        match saved {
            Ok(()) => self.start_upload(key, path, Some("pasted image.png".into()), cx),
            Err(err) => self.error(format!("could not keep the pasted image: {err}"), cx),
        }
        true
    }

    /// Downloads an attachment into the Downloads folder (never overwriting a file).
    pub fn download(&self, inst: InstanceId, attachment: Attachment, cx: &mut Context<Self>) {
        let core = self.core.clone();
        self.run_status(cx, async move {
            let result: Result<String, String> = async {
                let bytes = core
                    .rest(&inst)
                    .map_err(|e| e.to_string())?
                    .download(attachment.id, &attachment.filename)
                    .await
                    .map_err(|e| e.to_string())?;
                let dir = crate::open::downloads_dir();
                let path = tokio::task::spawn_blocking(move || {
                    crate::open::save_new(&dir, &attachment.filename, &bytes)
                })
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
                Ok(format!("Saved to {}", path.display()))
            }
            .await;
            result.map_err(|e| format!("download failed: {e}"))
        });
    }

    fn open_image(&mut self, inst: InstanceId, attachment: Attachment, cx: &mut Context<Self>) {
        self.viewer = Some(Viewer {
            inst: inst.clone(),
            attachment: attachment.clone(),
            image: Img::Loading,
        });
        let core = self.core.clone();
        let id = attachment.id;
        self.run(
            cx,
            async move {
                let bytes = core
                    .rest(&inst)
                    .map_err(|e| e.to_string())?
                    .download(attachment.id, &attachment.filename)
                    .await
                    .map_err(|e| e.to_string())?;
                tokio::task::spawn_blocking(move || crate::images::decode(&bytes, None))
                    .await
                    .map_err(|e| e.to_string())?
            },
            move |this, result, _, _| {
                if let Some(viewer) = &mut this.viewer
                    && viewer.attachment.id == id
                {
                    viewer.image = match result {
                        Ok(image) => Img::Ready(image),
                        Err(err) => {
                            tracing::warn!("image {id}: {err}");
                            Img::Failed
                        }
                    };
                }
            },
        );
        cx.notify();
    }

    pub fn render_viewer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let viewer = self.viewer.as_ref()?;
        let content = match &viewer.image {
            Img::Ready(image) => img(image.clone())
                .max_w_full()
                .max_h_full()
                .object_fit(gpui_kit::ObjectFit::Contain)
                .into_any_element(),
            Img::Failed => "Image not available".into_any_element(),
            Img::Loading => "Loading…".into_any_element(),
        };
        Some(
            gpui_kit::deferred(
                div()
                    .id("viewer")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .flex_col()
                    .bg(theme::scrim_strong())
                    .occlude()
                    .font_family(theme::FONT)
                    .text_color(theme::text())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.viewer = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_end()
                            .gap(px(8.))
                            .p(px(12.))
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .child(viewer.attachment.filename.clone()),
                            )
                            .child(icon_button("viewer-download", assets::DOWNLOAD).on_click(
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    if let Some(viewer) = &this.viewer {
                                        this.download(
                                            viewer.inst.clone(),
                                            viewer.attachment.clone(),
                                            cx,
                                        );
                                    }
                                }),
                            ))
                            .child(icon_button("viewer-close", assets::CLOSE).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.viewer = None;
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .items_center()
                            .justify_center()
                            .p(px(16.))
                            .child(content),
                    ),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }

    // ---- message actions (from the menu) ------------------------------------------------------

    /// Menu actions on messages. `false` if this is not one.
    pub fn run_message_action(
        &mut self,
        target: &Target,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let core = self.core.clone();
        match (target.clone(), action) {
            (Target::Message(inst, channel, message), Action::CopyText) => {
                let text = core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.window(channel))
                    .and_then(|w| w.get(message))
                    .map(|m| m.content.clone());
                if let Some(text) = text {
                    self.copy(text, "Message text", cx);
                }
            }
            (Target::Message(_, _, message), Action::CopyMessageId) => {
                self.copy(message.to_string(), "Message ID", cx);
            }
            (Target::Message(inst, channel, message), Action::Pin | Action::Unpin) => {
                let pin = action == Action::Pin;
                self.call_status(
                    &inst,
                    if pin { "Pinned" } else { "Unpinned" },
                    move |r| async move {
                        if pin {
                            r.pin(channel, message).await
                        } else {
                            r.unpin(channel, message).await
                        }
                    },
                    cx,
                );
            }
            (Target::Message(inst, channel, message), Action::SelectMessage) => {
                if let Some(view) = self.chats.get_mut(&(inst, channel)) {
                    view.clear_selection();
                    view.selected.insert(message);
                    view.anchor = Some(message);
                }
            }
            (Target::Messages(inst, channel, messages), Action::DeleteMessage) => {
                if let Some(view) = self.chats.get_mut(&(inst.clone(), channel)) {
                    view.clear_selection();
                }
                self.run_status(cx, async move {
                    let total = messages.len();
                    let mut failed = 0;
                    let mut last_error = None;
                    for message in messages {
                        if let Err(err) = core.delete_message(&inst, channel, message).await {
                            failed += 1;
                            last_error = Some(err.to_string());
                        }
                    }
                    match last_error {
                        None => Ok(format!(
                            "Deleted {total} message{}",
                            if total == 1 { "" } else { "s" }
                        )),
                        Some(err) => Err(format!(
                            "could not delete {failed} of {total} messages: {err}"
                        )),
                    }
                });
            }
            (Target::Message(inst, channel, message), Action::DeleteMessage) => {
                self.run(
                    cx,
                    async move { core.delete_message(&inst, channel, message).await },
                    |this, result, _, cx| {
                        if let Err(err) = result {
                            this.error(format!("could not delete: {err}"), cx);
                        }
                    },
                );
            }
            (Target::Message(inst, channel, message), Action::Reply) => {
                let key = (inst.clone(), channel);
                self.select_if_hidden(&key, window, cx);
                if let Some(view) = self.chats.get_mut(&key) {
                    view.editing = None;
                    view.reply_to = Some(message);
                }
                self.focus_composer = true;
            }
            (Target::Message(inst, channel, message), Action::AddReaction) => {
                if let Some(view) = self.chats.get_mut(&(inst, channel)) {
                    view.palette = Some(message);
                }
            }
            (Target::Message(inst, channel, message), Action::Edit) => {
                self.start_edit(&(inst, channel), message, window, cx);
            }
            (Target::Message(inst, channel, message), Action::QuoteText) => {
                let quote = core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.window(channel)?.get(message).cloned())
                    .map(|m| {
                        let mut q: String = m.content.lines().map(|l| format!("> {l}\n")).collect();
                        q.push('\n');
                        q
                    });
                if let Some(quote) = quote {
                    let key = (inst, channel);
                    self.select_if_hidden(&key, window, cx);
                    self.insert_text(&key, &quote, window, cx);
                }
            }
            (Target::Member(inst, _, user), Action::Mention) => {
                let username = core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.user(user).map(|u| u.username.clone()));
                let open = self.open_channel().filter(|(i, _)| *i == inst);
                match (username, open) {
                    (Some(name), Some(key)) => {
                        self.insert_text(&key, &format!("@{name} "), window, cx);
                    }
                    _ => self.info("Open a channel first to mention someone", cx),
                }
            }
            _ => return false,
        }
        true
    }

    /// Shows a channel unless it already shows.
    fn select_if_hidden(&mut self, key: &ChannelKey, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_channel().as_ref() != Some(key) {
            self.select(key.0.clone(), key.1, window, cx);
        }
    }

    // ---- drawing ------------------------------------------------------------------------------

    pub fn render_chat(
        &self,
        inst: InstanceId,
        channel: Id,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let key = (inst.clone(), channel);
        let store = self.core.store();
        let s = store.instance(&inst);
        let window_state = s.and_then(|s| s.window(channel));
        let loaded = window_state.is_some_and(|w| w.loaded);
        let live = window_state.is_none_or(|w| w.reached_end);
        let Some(view) = self.chats.get(&key) else {
            return div().into_any_element();
        };
        let empty = view.rows.is_empty();
        let title = s
            .and_then(|s| s.channel(channel).map(|c| view::channel_title(s, c)))
            .unwrap_or_default();
        let typing = s.and_then(|s| self.typing_line(s, channel));
        let at_end = view.list.is_scrolled_to_end() != Some(false);
        let selected_count = view.selected.len();
        let can_send = s.is_some_and(|s| view::can(s, channel, Permissions::SEND_MESSAGES));
        let reply_name = view.reply_to.map(|id| {
            s.and_then(|s| {
                s.window(channel)?
                    .get(id)
                    .map(|m| s.display_name(s.space_of(channel), m.author.id))
            })
            .unwrap_or_else(|| "a message".into())
        });
        drop(store);

        let content: AnyElement = if !loaded {
            centered("Loading…").into_any_element()
        } else if empty {
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .flex_1()
                .min_h_0()
                .child(icon(assets::CHAT, 40., theme::accent()))
                .child(
                    div()
                        .text_size(px(theme::TEXT + 4.))
                        .child(format!("Welcome to {title}")),
                )
                .child(
                    div()
                        .text_size(px(theme::TEXT_SMALL))
                        .text_color(theme::text_secondary())
                        .child("This is the start of the conversation. Say hello!"),
                )
                .into_any_element()
        } else {
            let row_inst = inst.clone();
            let messages = list(
                view.list.clone(),
                cx.processor(move |this: &mut Shell, ix: usize, _, cx| {
                    this.render_row(&row_inst, channel, ix, cx)
                }),
            )
            .size_full();
            let jump_key = key.clone();
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(messages)
                .when(!at_end || !live, |d| {
                    d.child(
                        div().absolute().bottom(px(12.)).right(px(16.)).child(
                            Button::new("jump-present")
                                .primary()
                                .label("Jump to present")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.jump_to_present(&jump_key, cx);
                                })),
                        ),
                    )
                })
                .children(typing.map(|text| {
                    div()
                        .absolute()
                        .bottom(px(2.))
                        .left(px(16.))
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .bg(theme::background())
                        .text_size(px(theme::TEXT_TINY))
                        .text_color(theme::text_secondary())
                        .child(text)
                }))
                .into_any_element()
        };

        let bottom = if selected_count > 0 {
            self.render_selection_bar(&key, selected_count, cx)
        } else if !can_send {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(20.))
                .py(px(16.))
                .text_size(px(theme::TEXT_SMALL))
                .text_color(theme::text_tertiary())
                .child(icon(assets::LOCK, 14., theme::text_tertiary()))
                .child("You cannot send messages in this channel.")
                .into_any_element()
        } else {
            self.render_composer(&key, reply_name, cx)
        };

        let drop_key = key.clone();
        div()
            .flex()
            .flex_col()
            .size_full()
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                for path in paths.paths().iter().filter(|p| p.is_file()) {
                    this.start_upload(&drop_key, path.clone(), None, cx);
                }
            }))
            .child(content)
            .child(bottom)
            .into_any_element()
    }

    fn typing_line(&self, s: &InstanceStore, channel: Id) -> Option<String> {
        if !self.config.buffer.show_typing {
            return None;
        }
        let space = s.space_of(channel);
        let names: Vec<String> = s
            .typing(channel, now_ms())
            .into_iter()
            .map(|u| s.display_name(space, u))
            .collect();
        typing_text(&names)
    }

    fn render_selection_bar(
        &self,
        key: &ChannelKey,
        count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let what = if count == 1 {
            "1 message selected".to_owned()
        } else {
            format!("{count} messages selected")
        };
        let cancel_key = key.clone();
        let delete_key = key.clone();
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(12.))
            .mx(px(16.))
            .mb(px(20.))
            .mt(px(8.))
            .px(px(12.))
            .py(px(8.))
            .rounded(px(8.))
            .bg(theme::selected())
            .child(div().text_size(px(theme::TEXT)).child(what))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(theme::TEXT_TINY))
                    .text_color(theme::text_tertiary())
                    .child("Ctrl+click to add · Shift+click for a range"),
            )
            .child(
                Button::new("selection-cancel")
                    .label("Cancel")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(view) = this.chats.get_mut(&cancel_key) {
                            view.clear_selection();
                        }
                        cx.notify();
                    })),
            )
            .child(
                Button::new("selection-delete")
                    .danger()
                    .label("Delete")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Only what still exists and I may still delete.
                        let selectable = selectable_messages(this, &delete_key.0, delete_key.1);
                        if let Some(view) = this.chats.get_mut(&delete_key) {
                            view.selected.retain(|id| selectable.contains(id));
                            let ids: Vec<Id> = view.selected.iter().copied().collect();
                            if !ids.is_empty() {
                                this.confirm = Some((
                                    Target::Messages(delete_key.0.clone(), delete_key.1, ids),
                                    Action::DeleteMessage,
                                ));
                            }
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_composer(
        &self,
        key: &ChannelKey,
        reply_name: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(view) = self.chats.get(key) else {
            return div().into_any_element();
        };
        let mode = match (view.editing, reply_name) {
            (Some(_), _) => Some((assets::PENCIL, "Editing message".to_owned())),
            (None, Some(name)) => Some((assets::REPLY, format!("Replying to {name}"))),
            _ => None,
        };
        let popup = (!view.suggestions.is_empty()).then(|| {
            let rows = view.suggestions.iter().enumerate().map(|(n, suggestion)| {
                let accept = key.clone();
                div()
                    .id(("suggestion", n))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(px(theme::TEXT_SMALL))
                    .when(n == view.suggestion, |d| d.bg(theme::selected()))
                    .hover(|d| d.bg(theme::hover()))
                    .child(suggestion.label.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.accept_suggestion(&accept, n, window, cx);
                    }))
            });
            div()
                .absolute()
                .bottom_full()
                .left(px(16.))
                .right(px(16.))
                .mb(px(6.))
                .p(px(6.))
                .rounded(px(8.))
                .bg(theme::sidebar())
                .border_1()
                .border_color(theme::border())
                .children(rows)
        });
        let cancel_key = key.clone();
        let mode_bar = mode.map(|(glyph, text)| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .pb(px(6.))
                .mb(px(6.))
                .border_b_1()
                .border_color(theme::border())
                .child(icon(glyph, 16., theme::accent()))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(theme::TEXT_SMALL))
                        .text_color(theme::text_secondary())
                        .child(text),
                )
                .child(
                    icon_button("composer-cancel", assets::CLOSE).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.composer_cancel(&cancel_key, window, cx);
                        },
                    )),
                )
        });
        let upload_key = key.clone();
        let chips = view.uploads.iter().map(|upload| {
            let (state, color) = match &upload.state {
                UploadState::Uploading => ("uploading…".to_owned(), theme::text_tertiary()),
                UploadState::Ready(_) => (human_size(upload.size), theme::text_tertiary()),
                UploadState::Failed(e) => (format!("failed: {e}"), theme::danger()),
            };
            let local = upload.local;
            let remove = upload_key.clone();
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(8.))
                .py(px(4.))
                .rounded(px(4.))
                .bg(theme::selected())
                .text_size(px(theme::TEXT_TINY))
                .child(icon(assets::FILE, 14., theme::text_secondary()))
                .child(upload.name.clone())
                .child(div().text_color(color).child(state))
                .child(
                    icon_button(("upload-remove", local as usize), assets::CLOSE).on_click(
                        cx.listener(move |this, _, _, cx| {
                            if let Some(view) = this.chats.get_mut(&remove) {
                                view.uploads.retain(|u| u.local != local);
                            }
                            cx.notify();
                        }),
                    ),
                )
        });
        let uploads = (!view.uploads.is_empty()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .pb(px(6.))
                .children(chips)
        });
        let pick_key = key.clone();
        let paste_key = key.clone();
        let shell = cx.entity().downgrade();
        let input = Textarea::new(&view.input)
            .large()
            .appearance(false)
            .on_paste(move |_, _: &mut Window, cx: &mut App| {
                shell
                    .update(cx, |this, cx| this.paste_image(&paste_key, cx))
                    .unwrap_or(false)
            });
        div()
            .relative()
            .flex_shrink_0()
            .px(px(16.))
            .pt(px(8.))
            .pb(px(20.))
            .children(popup)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p(px(6.))
                    .rounded(px(8.))
                    .bg(theme::selected())
                    .children(mode_bar)
                    .children(uploads)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(icon_button("composer-attach", assets::ATTACH).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.pick_files(pick_key.clone(), cx);
                                }),
                            ))
                            .child(div().flex_1().min_w_0().child(input)),
                    ),
            )
            .into_any_element()
    }

    fn render_row(
        &mut self,
        inst: &InstanceId,
        channel: Id,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = (inst.clone(), channel);
        let Some(row) = self.chats.get(&key).and_then(|v| v.rows.get(ix)).cloned() else {
            return div().into_any_element();
        };
        if let Row::Message(m) = &row
            && self.config.buffer.image_previews
        {
            let wanted: Vec<Id> = {
                let store = self.core.store();
                store
                    .instance(inst)
                    .and_then(|s| s.window(channel)?.get(m.id).cloned())
                    .map(|msg| {
                        msg.attachments
                            .iter()
                            .filter(|a| is_image(a))
                            .map(|a| a.id)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            for id in wanted {
                self.load_thumbnail(inst, id, cx);
            }
        }
        let store = self.core.store();
        let Some(s) = store.instance(inst) else {
            return div().into_any_element();
        };
        match &row {
            Row::Older => div()
                .flex()
                .justify_center()
                .py(px(8.))
                .text_size(px(theme::TEXT_TINY))
                .text_color(theme::text_tertiary())
                .child("Loading older messages…")
                .into_any_element(),
            Row::Day { day, .. } => {
                let today = time::day(now_ms(), self.utc_offset);
                divider(
                    day_label(*day, today),
                    theme::text_tertiary(),
                    theme::border(),
                )
            }
            Row::NewMessages => divider("New messages".into(), theme::danger(), theme::danger()),
            Row::Message(m) => match s.window(channel).and_then(|w| w.get(m.id)) {
                Some(message) => self.render_message(s, inst, channel, m, message, cx),
                None => div().into_any_element(),
            },
            Row::Pending(p) => self.render_pending(s, inst, channel, p, cx),
        }
    }

    fn render_message(
        &self,
        s: &InstanceStore,
        inst: &InstanceId,
        channel: Id,
        m: &MessageRow,
        msg: &ChatMessage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = (inst.clone(), channel);
        let view = self.chats.get(&key);
        let space = s.space_of(channel);
        let id = msg.id;
        let time_text = time::format(
            msg.created_at() as i64,
            &self.config.buffer.timestamp_format,
            self.utc_offset,
        );
        let mut body: Vec<AnyElement> = Vec::new();
        if let Some(reply) = &m.reply {
            let excerpt = if reply.excerpt.is_empty() {
                "original message deleted".to_owned()
            } else {
                reply.excerpt.replace('\n', " ")
            };
            let (jump_inst, reply_id) = (inst.clone(), reply.id);
            body.push(
                div()
                    .id(SharedString::from(format!("reply-{id}")))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .text_size(px(theme::TEXT_TINY))
                    .text_color(theme::text_tertiary())
                    .child(icon(assets::REPLY, 12., theme::text_tertiary()))
                    .child(
                        div()
                            .text_color(theme::text_secondary())
                            .font_weight(FontWeight::MEDIUM)
                            .child(reply.author.clone()),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(excerpt))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.jump_to(jump_inst.clone(), channel, reply_id, window, cx);
                    }))
                    .into_any_element(),
            );
        }
        if !m.grouped {
            let name = s.display_name(space, msg.author.id);
            let color = space
                .and_then(|sp| author_color(s, sp, msg.author.id))
                .map_or(theme::accent(), theme::rgb_u32);
            let author = self.author_menu(
                div()
                    .id(SharedString::from(format!("author-{id}")))
                    .cursor_pointer()
                    .text_size(px(theme::TEXT))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color)
                    .child(name),
                inst,
                space,
                msg.author.id,
                cx,
            );
            body.push(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_baseline()
                    .child(author)
                    .child(
                        div()
                            .text_size(px(theme::TEXT_TINY))
                            .text_color(theme::text_tertiary())
                            .child(time_text.clone()),
                    )
                    .into_any_element(),
            );
        }
        let revealed = view.is_some_and(|v| v.revealed.contains(&id));
        let blocks = markup::blocks(&msg.content, s, space);
        let last = blocks.len().saturating_sub(1);
        let system = m.system;
        for (i, block) in blocks.into_iter().enumerate() {
            let edited = i == last && msg.edited_at.is_some();
            body.push(self.render_block(block, inst, channel, id, i, system, revealed, edited, cx));
        }
        if msg.content.is_empty() && msg.edited_at.is_some() {
            body.push(
                div()
                    .text_size(px(theme::TEXT_TINY))
                    .text_color(theme::text_tertiary())
                    .child("(edited)")
                    .into_any_element(),
            );
        }
        for attachment in &msg.attachments {
            body.push(self.render_attachment(inst, attachment, cx));
        }
        if !msg.reactions.is_empty() {
            let chips = msg.reactions.iter().enumerate().map(|(n, r)| {
                let (mine, emoji) = (r.me, r.emoji.clone());
                let (inst, emoji_id) =
                    (inst.clone(), SharedString::from(format!("react-{id}-{n}")));
                div()
                    .id(emoji_id)
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(10.))
                    .border_1()
                    .cursor_pointer()
                    .text_size(px(theme::TEXT_SMALL))
                    .when(mine, |d| {
                        d.bg(theme::accent_soft()).border_color(theme::accent())
                    })
                    .when(!mine, |d| {
                        d.bg(theme::sidebar())
                            .border_color(theme::border())
                            .hover(|d| d.bg(theme::hover()))
                    })
                    .child(r.emoji.clone())
                    .child(r.count.to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.react(inst.clone(), channel, id, emoji.clone(), !mine, cx);
                    }))
            });
            body.push(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .pt(px(4.))
                    .children(chips)
                    .into_any_element(),
            );
        }

        let gutter: AnyElement = if !m.grouped && self.config.buffer.show_avatars {
            // The user's current picture (the message keeps the one from when it was sent).
            let name = s.display_name(space, msg.author.id);
            let user = s.user(msg.author.id).unwrap_or(&msg.author);
            let avatar = self.avatar(inst, Some(user), &name, theme::AVATAR);
            self.author_menu(
                div()
                    .id(SharedString::from(format!("avatar-{id}")))
                    .cursor_pointer()
                    .child(avatar),
                inst,
                space,
                msg.author.id,
                cx,
            )
            .into_any_element()
        } else if m.grouped {
            // Always laid out and only made visible on hover, so hovering never resizes the row.
            div()
                .pt(px(3.))
                .text_size(px(theme::TEXT_TINY))
                .text_color(rgba(0))
                .group_hover(ROW_GROUP, |d| d.text_color(theme::text_tertiary()))
                .child(time_text)
                .into_any_element()
        } else {
            div().into_any_element()
        };

        let selected = view.is_some_and(|v| v.selected.contains(&id));
        let jumped = view.is_some_and(|v| v.highlight == Some(id));
        let palette_open = view.is_some_and(|v| v.palette == Some(id));
        let highlight = m.mentions_me;
        let click_inst = inst.clone();
        let target = Target::Message(inst.clone(), channel, id);
        let element = div()
            .id(SharedString::from(format!("message-{id}")))
            .group(ROW_GROUP)
            .relative()
            .flex()
            .w_full()
            .pt(px(if m.grouped { 1. } else { 8. }))
            .pb(px(if m.ends_group { 8. } else { 1. }))
            .pl(px(8.))
            .pr(px(8.))
            .rounded(px(4.))
            .when(selected, |d| d.bg(theme::accent_soft()))
            .when(jumped && !selected, |d| d.bg(theme::selected()))
            .when(highlight && !selected && !jumped, |d| {
                d.bg(theme::mention_soft())
                    .border_l_2()
                    .border_color(theme::accent())
            })
            .when(!selected && !jumped && !highlight, |d| {
                d.hover(|d| d.bg(theme::hover_soft()))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                let modifiers = window.modifiers();
                let selectable = selectable_messages(this, &click_inst, channel);
                if let Some(view) = this.chats.get_mut(&(click_inst.clone(), channel)) {
                    view.click(id, modifiers.control, modifiers.shift, &selectable);
                    if view.highlight == Some(id) {
                        view.highlight = None;
                    }
                }
                cx.notify();
            }))
            .child(div().flex_shrink_0().w(px(GUTTER)).child(gutter))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .flex_1()
                    .min_w_0()
                    .children(body),
            )
            .children(palette_open.then(|| self.render_palette(inst, channel, id, cx)));
        // The row fills the list's width; the margin keeps its highlight off the edges.
        div()
            .w_full()
            .px(px(8.))
            .child(self.with_menu(element, target, cx))
            .into_any_element()
    }

    /// `element` opens the person's menu on either click.
    fn author_menu(
        &self,
        element: gpui_kit::Stateful<gpui_kit::Div>,
        inst: &InstanceId,
        space: Option<Id>,
        user: Id,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let target = Target::Member(inst.clone(), space, user);
        let left = target.clone();
        self.with_menu(element, target, cx)
            .on_mouse_up(MouseButton::Right, |_, _, cx: &mut App| {
                cx.stop_propagation()
            })
            .on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_menu(left.clone(), event.position(), window, cx);
                }),
            )
    }

    pub fn react(
        &mut self,
        inst: InstanceId,
        channel: Id,
        message: Id,
        emoji: String,
        add: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self.chats.get_mut(&(inst.clone(), channel)) {
            view.palette = None;
        }
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                if add {
                    core.react(&inst, channel, message, &emoji).await
                } else {
                    core.unreact(&inst, channel, message, &emoji).await
                }
            },
            |this, result, _, cx| {
                if let Err(err) = result {
                    this.error(format!("reaction failed: {err}"), cx);
                }
            },
        );
    }

    fn render_palette(
        &self,
        inst: &InstanceId,
        channel: Id,
        message: Id,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let buttons = QUICK_REACTIONS.iter().enumerate().map(|(n, emoji)| {
            let inst = inst.clone();
            div()
                .id(("quick-reaction", n))
                .flex()
                .items_center()
                .justify_center()
                .size(px(30.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|d| d.bg(theme::hover()))
                .child(*emoji)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.react(
                        inst.clone(),
                        channel,
                        message,
                        (*emoji).to_owned(),
                        true,
                        cx,
                    );
                }))
        });
        let close_inst = inst.clone();
        div()
            .id("palette")
            .absolute()
            .top(px(-16.))
            .right(px(16.))
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(3.))
            .rounded(px(6.))
            .bg(theme::sidebar())
            .border_1()
            .border_color(theme::border())
            .occlude()
            .children(buttons)
            .child(
                icon_button("palette-close", assets::CLOSE).on_click(cx.listener(
                    move |this, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(view) = this.chats.get_mut(&(close_inst.clone(), channel)) {
                            view.palette = None;
                        }
                        cx.notify();
                    },
                )),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_block(
        &self,
        block: Block,
        inst: &InstanceId,
        channel: Id,
        message: Id,
        index: usize,
        system: bool,
        revealed: bool,
        edited: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let edited_mark = || {
            div()
                .text_size(px(theme::TEXT_TINY))
                .text_color(theme::text_tertiary())
                .child("(edited)")
        };
        match block {
            Block::Text(pieces) => {
                let mut pieces = pieces;
                if edited {
                    pieces.push(Piece {
                        text: " (edited)".into(),
                        italic: true,
                        ..Piece::default()
                    });
                }
                self.render_pieces(pieces, inst, channel, message, index, system, revealed, cx)
            }
            Block::Code { code, .. } => div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .p(px(8.))
                        .rounded(px(4.))
                        .bg(theme::code_background())
                        .border_1()
                        .border_color(theme::border())
                        .text_size(px(theme::TEXT_SMALL))
                        .whitespace_normal()
                        .child(code),
                )
                .when(edited, |d| d.child(edited_mark()))
                .into_any_element(),
            Block::Quote(pieces) => div()
                .flex()
                .gap(px(8.))
                .child(
                    div()
                        .flex_shrink_0()
                        .w(px(3.))
                        .rounded(px(1.))
                        .bg(theme::text_tertiary()),
                )
                .child(div().flex_1().min_w_0().child(
                    self.render_pieces(pieces, inst, channel, message, index, system, revealed, cx),
                ))
                .into_any_element(),
        }
    }

    /// Styled, clickable text. Links and spoilers act on release, as everything does.
    #[allow(clippy::too_many_arguments)]
    fn render_pieces(
        &self,
        pieces: Vec<Piece>,
        inst: &InstanceId,
        channel: Id,
        message: Id,
        index: usize,
        system: bool,
        revealed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut text = String::new();
        let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
        let mut links: Vec<(Range<usize>, Link)> = Vec::new();
        for piece in pieces {
            let start = text.len();
            text.push_str(&piece.text);
            let range = start..text.len();
            if range.is_empty() {
                continue;
            }
            highlights.push((range.clone(), piece_style(&piece, system, revealed)));
            let hidden = piece.spoiler && !revealed;
            match piece.link {
                Some(Link::Spoiler) if !hidden => {}
                Some(link) => links.push((range, link)),
                None => {}
            }
        }
        if text.is_empty() {
            return div().into_any_element();
        }
        let styled = StyledText::new(SharedString::from(text)).with_highlights(highlights);
        let body = div()
            .text_size(px(theme::TEXT))
            .when(system, |d| d.text_color(theme::text_secondary()));
        if links.is_empty() {
            return body.child(styled).into_any_element();
        }
        let ranges: Vec<Range<usize>> = links.iter().map(|(range, _)| range.clone()).collect();
        let shell = cx.entity().downgrade();
        let inst = inst.clone();
        let element_id = SharedString::from(format!("text-{message}-{index}"));
        body.child(InteractiveText::new(element_id, styled).on_click(
            ranges,
            move |clicked, window, cx| {
                cx.stop_propagation();
                let Some((_, link)) = links.get(clicked) else {
                    return;
                };
                let link = link.clone();
                let inst = inst.clone();
                let _ = shell.update(cx, |this, cx| {
                    this.follow_link(inst, channel, message, link, window, cx);
                });
            },
        ))
        .into_any_element()
    }

    fn follow_link(
        &mut self,
        inst: InstanceId,
        channel: Id,
        message: Id,
        link: Link,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match link {
            Link::Url(url) => {
                if let Err(err) = crate::open::url(&url) {
                    self.error(format!("could not open the link: {err}"), cx);
                }
            }
            Link::Channel(target) => self.select(inst, target, window, cx),
            Link::Spoiler => {
                if let Some(view) = self.chats.get_mut(&(inst, channel)) {
                    view.revealed.insert(message);
                }
                cx.notify();
            }
        }
    }

    fn render_attachment(
        &self,
        inst: &InstanceId,
        a: &Attachment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if is_image(a) && self.config.buffer.image_previews {
            let (w, h) = display_size(a);
            let picture = self.images.thumbs.get(&(inst.clone(), a.id));
            if let Some(Img::Ready(image)) = picture {
                let (open_inst, attachment) = (inst.clone(), a.clone());
                return div()
                    .id(SharedString::from(format!("image-{}", a.id)))
                    .mt(px(4.))
                    .w(px(w))
                    .h(px(h))
                    .rounded(px(6.))
                    .overflow_hidden()
                    .cursor_pointer()
                    .child(img(image.clone()).w(px(w)).h(px(h)).rounded(px(6.)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_image(open_inst.clone(), attachment.clone(), cx);
                    }))
                    .into_any_element();
            }
            let text = if matches!(picture, Some(Img::Failed)) {
                "Image not available"
            } else {
                "Loading…"
            };
            return div()
                .flex()
                .items_center()
                .justify_center()
                .mt(px(4.))
                .w(px(w))
                .h(px(h))
                .rounded(px(6.))
                .bg(theme::code_background())
                .text_size(px(theme::TEXT_TINY))
                .text_color(theme::text_tertiary())
                .child(text)
                .into_any_element();
        }
        let (down_inst, attachment) = (inst.clone(), a.clone());
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .mt(px(4.))
            .max_w(px(420.))
            .px(px(12.))
            .py(px(8.))
            .rounded(px(6.))
            .bg(theme::sidebar())
            .border_1()
            .border_color(theme::border())
            .child(icon(assets::FILE, 22., theme::accent()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(theme::TEXT_SMALL))
                            .text_color(theme::link())
                            .child(a.filename.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TEXT_TINY))
                            .text_color(theme::text_tertiary())
                            .child(human_size(a.size)),
                    ),
            )
            .child(
                icon_button(
                    SharedString::from(format!("download-{}", a.id)),
                    assets::DOWNLOAD,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.download(down_inst.clone(), attachment.clone(), cx);
                })),
            )
            .into_any_element()
    }

    fn render_pending(
        &self,
        s: &InstanceStore,
        inst: &InstanceId,
        channel: Id,
        p: &PendingRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(pending) = s.window(channel).and_then(|w| w.pending(p.local_id)) else {
            return div().into_any_element();
        };
        let (color, note) = match &pending.state {
            PendingState::Sending => (theme::text_tertiary(), None),
            PendingState::Queued => (theme::text_tertiary(), Some("waiting for connection")),
            PendingState::Failed(_) => (theme::danger(), Some("not sent")),
        };
        let me = s.me_id();
        let name = me.map(|id| s.display_name(None, id)).unwrap_or_default();
        let local_id: LocalId = p.local_id;
        let mut body = div().flex().flex_col().gap(px(2.)).flex_1().min_w_0();
        if !p.grouped {
            body = body.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_baseline()
                    .child(
                        div()
                            .text_size(px(theme::TEXT))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::accent())
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TEXT_TINY))
                            .text_color(theme::text_tertiary())
                            .child(time::format(
                                pending.created_at,
                                &self.config.buffer.timestamp_format,
                                self.utc_offset,
                            )),
                    ),
            );
        }
        body = body.child(
            div()
                .text_size(px(theme::TEXT))
                .text_color(color)
                .child(pending.content.clone()),
        );
        if let Some(note) = note {
            let mut line = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(theme::TEXT_TINY))
                .text_color(color)
                .child(note);
            if let PendingState::Failed(reason) = &pending.state {
                let (retry_inst, discard_inst) = (inst.clone(), inst.clone());
                line = line
                    .child(
                        div()
                            .text_color(theme::text_tertiary())
                            .child(reason.clone()),
                    )
                    .child(
                        Button::new(("pending-retry", local_id as usize))
                            .small()
                            .label("Retry")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Err(err) =
                                    this.core.retry_message(&retry_inst, channel, local_id)
                                {
                                    this.error(err.to_string(), cx);
                                }
                            })),
                    )
                    .child(
                        Button::new(("pending-discard", local_id as usize))
                            .small()
                            .danger()
                            .label("Delete")
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.core.discard_message(&discard_inst, channel, local_id);
                            })),
                    );
            }
            body = body.child(line);
        }
        let gutter = if !p.grouped && self.config.buffer.show_avatars {
            let user = me.and_then(|id| s.user(id));
            self.avatar(inst, user, &name, theme::AVATAR)
        } else {
            div().into_any_element()
        };
        div()
            .flex()
            .w_full()
            .pt(px(if p.grouped { 1. } else { 8. }))
            .pb(px(if p.ends_group { 8. } else { 1. }))
            .px(px(16.))
            .child(div().flex_shrink_0().w(px(GUTTER)).child(gutter))
            .child(body)
            .into_any_element()
    }
}

/// The loaded messages I may delete, oldest first (for selecting).
pub fn selectable_messages(this: &Shell, inst: &str, channel: Id) -> Vec<Id> {
    let store = this.core.store();
    let Some(s) = store.instance(inst) else {
        return Vec::new();
    };
    let Some(window) = s.window(channel) else {
        return Vec::new();
    };
    window
        .messages
        .iter()
        .filter(|m| !m.deleted && menu_entries::can_delete(s, channel, m.id))
        .map(|m| m.id)
        .collect()
}

fn piece_style(p: &Piece, system: bool, revealed: bool) -> HighlightStyle {
    let hidden = p.spoiler && !revealed;
    let mut style = HighlightStyle::default();
    if p.bold {
        style.font_weight = Some(FontWeight::BOLD);
    }
    if p.italic || system {
        style.font_style = Some(FontStyle::Italic);
    }
    if matches!(p.link, Some(Link::Url(_))) {
        style.color = Some(theme::link().into());
    }
    if p.code {
        style.background_color = Some(theme::code_background().into());
    }
    if p.mention {
        style.color = Some(p.color.map_or(theme::link(), theme::rgb_u32).into());
        style.background_color = Some(theme::mention_soft().into());
    }
    if p.underline && !hidden {
        style.underline = Some(UnderlineStyle {
            thickness: px(1.),
            ..UnderlineStyle::default()
        });
    }
    if p.strike && !hidden {
        style.strikethrough = Some(StrikethroughStyle {
            thickness: px(1.),
            ..StrikethroughStyle::default()
        });
    }
    if hidden {
        style.color = Some(theme::code_background().into());
        style.background_color = Some(theme::code_background().into());
    } else if p.spoiler {
        style.background_color = Some(theme::code_background().into());
    }
    style
}

fn centered(text: &'static str) -> gpui_kit::Div {
    div()
        .flex()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .text_size(px(theme::TEXT_SMALL))
        .text_color(theme::text_tertiary())
        .child(text)
}

/// A centered label on a hairline (day dividers, the new messages marker).
fn divider(text: String, color: gpui_kit::Rgba, line: gpui_kit::Rgba) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(16.))
        .py(px(8.))
        .child(div().flex_1().h(px(1.)).bg(line))
        .child(
            div()
                .text_size(px(theme::TEXT_TINY))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(text),
        )
        .child(div().flex_1().h(px(1.)).bg(line))
        .into_any_element()
}

/// A small square button holding an icon. The caller adds what a click does.
pub fn icon_button(
    id: impl Into<gpui_kit::ElementId>,
    name: &'static str,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id.into())
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|d| d.bg(theme::hover()))
        .child(icon(name, 18., theme::text_secondary()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(w: Option<u32>, h: Option<u32>) -> Attachment {
        Attachment {
            id: Id(1),
            filename: "a.png".into(),
            content_type: "image/png".into(),
            size: 10,
            width: w,
            height: h,
            has_thumb: true,
        }
    }

    #[test]
    fn images_fit_into_400_by_300() {
        assert_eq!(
            display_size(&attachment(Some(800), Some(600))),
            (400.0, 300.0)
        );
        assert_eq!(
            display_size(&attachment(Some(1000), Some(200))),
            (400.0, 80.0)
        );
        assert_eq!(
            display_size(&attachment(Some(100), Some(50))),
            (100.0, 50.0)
        );
        assert_eq!(display_size(&attachment(None, None)), (300.0, 200.0));
    }

    #[test]
    fn sizes_are_human() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(820 * 1024), "820 KB");
        assert_eq!(human_size(1_468_006), "1.4 MB");
    }

    #[test]
    fn tokens_are_found_before_the_cursor() {
        assert_eq!(token_before("hi @al", 6), Some((3, "@al".to_owned())));
        assert_eq!(token_before("hi @al there", 6), Some((3, "@al".to_owned())));
        assert_eq!(token_before("hi", 2), None);
        assert_eq!(token_before(":a", 2), None, "emoji need two characters");
        assert_eq!(token_before(":ab", 3), Some((0, ":ab".to_owned())));
        assert_eq!(token_before("/nick", 5), Some((0, "/nick".to_owned())));
        assert_eq!(
            token_before("a /nick", 7),
            None,
            "commands start the message"
        );
    }

    #[test]
    fn only_the_changed_rows_are_spliced() {
        let m = |n| RowKey::Message(Id(n));
        // A new message at the end.
        assert_eq!(common_ends(&[m(1), m(2)], &[m(1), m(2), m(3)]), (2, 0));
        // An older page at the start.
        assert_eq!(
            common_ends(&[m(3), m(4)], &[m(1), m(2), m(3), m(4)]),
            (0, 2)
        );
        // A deleted message in the middle.
        assert_eq!(common_ends(&[m(1), m(2), m(3)], &[m(1), m(3)]), (1, 1));
        assert_eq!(common_ends(&[], &[m(1)]), (0, 0));
    }

    #[test]
    fn typing_lines_read_well() {
        assert_eq!(typing_text(&[]), None);
        assert_eq!(typing_text(&["A".into()]).unwrap(), "A is typing…");
        assert_eq!(
            typing_text(&["A".into(), "B".into()]).unwrap(),
            "A and B are typing…"
        );
    }
}
