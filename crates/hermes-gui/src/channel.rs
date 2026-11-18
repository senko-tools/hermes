//! The chat of a channel or DM (PLAN.md section 15.2): a virtualized message list held at the
//! bottom, with grouping, day separators, the "New messages" divider, reply previews, pending
//! messages, attachments, reactions and the typing line.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{RopeExt as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnimationExt as _, AnyElement, App, Context, Entity, ExternalPaths, FontStyle, FontWeight,
    HighlightStyle, InteractiveText, ListAlignment, ListScrollEvent, ListState, MouseButton,
    SharedString, StrikethroughStyle, Subscription, UnderlineStyle, Window, div, img, list, rgba,
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
use crate::selection::{self, FilePlace, Part};
use crate::shell::{ChannelKey, Page, Shell};
use crate::theme::{self, px};

const MAX_IMAGE_WIDTH: f32 = 400.0;
const MAX_IMAGE_HEIGHT: f32 = 300.0;

/// `buffer.media_scale`, for the functions that only have a picture's size to go on.
pub static MEDIA_SCALE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(100);

/// How much larger than normal pictures and videos are drawn.
pub fn media_scale() -> f32 {
    MEDIA_SCALE.load(std::sync::atomic::Ordering::Relaxed) as f32 / 100.
}
/// Suggestions shown at most.
const MAX_SUGGESTIONS: usize = 8;
/// Older history loads when this many rows from the top are showing.
const LOAD_OLDER_WITHIN: usize = 15;
/// The avatar column, which grouped messages leave empty.
const GUTTER: f32 = theme::AVATAR + 16.0;
/// The hover group of a message row, for the time of a grouped message.
const ROW_GROUP: &str = "message-row";
/// Compact messages: the picture before the name.
const COMPACT_AVATAR: f32 = 18.;

/// A forwarded message's text: who wrote it where, then their words quoted, then a link to the
/// original when it had files.
fn forwarded_text(content: &str, author: &str, place: &str, files: usize, link: &str) -> String {
    let mut out = format!("*Forwarded from {author} in {place}*");
    for line in content.lines() {
        out.push_str("\n> ");
        out.push_str(line);
    }
    if files > 0 {
        let s = if files == 1 { "" } else { "s" };
        out.push_str(&format!("\n{files} file{s}: {link}"));
    }
    out
}

/// What a message's time says under the pointer: the whole date, and when it was edited.
fn time_tip(msg: &ChatMessage, utc_offset: i64) -> SharedString {
    let sent = time::long(msg.created_at() as i64, utc_offset);
    SharedString::from(match msg.edited_at {
        Some(at) => format!("{sent} · edited {}", time::long(at, utc_offset)),
        None => sent,
    })
}

/// The height of one line of body text (gpui's default line height).
fn body_line() -> gpui_kit::Pixels {
    theme::body_size() * 1.618
}

/// Compact messages: the time column, as wide as `time_text` in every row so the text lines up,
/// holding `time` on a group's first line and the time on hover for the rest.
fn compact_time_slot(time_text: &str, time: Option<AnyElement>) -> AnyElement {
    let chars = time_text.chars().count().max(5) as f32;
    let width = gpui_kit::px((chars * 0.62 * theme::caption_size().as_f32()).ceil()) + px(10.);
    let time = time.unwrap_or_else(|| {
        // Always laid out and only made visible on hover, so hovering never resizes the row.
        div()
            .text_size(theme::caption_size())
            .text_color(rgba(0))
            .group_hover(ROW_GROUP, |d| d.text_color(theme::text_tertiary()))
            .child(time_text.to_owned())
            .into_any_element()
    });
    div()
        .flex()
        .items_center()
        .w(width)
        .h(body_line())
        .child(time)
        .into_any_element()
}

/// Compact messages: the picture, name and lock (`head`) and then the text, on one line. Each
/// piece of the head is centred on the text's first line.
fn compact_line(head: Vec<AnyElement>, texts: Vec<AnyElement>) -> AnyElement {
    let head = head.into_iter().map(|piece| {
        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .h(body_line())
            .child(piece)
    });
    div()
        .flex()
        .items_start()
        .gap(px(6.))
        .children(head)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w_0()
                .children(texts),
        )
        .into_any_element()
}

/// A file being attached.
#[derive(Debug, Clone, PartialEq)]
pub struct Upload {
    pub local: u64,
    pub name: String,
    pub size: u64,
    pub state: UploadState,
}

// A few at a time, and short-lived: not worth a box.
#[allow(clippy::large_enum_variant)]
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
    /// How much larger than fitted it is shown: 1 = fitted.
    pub zoom: f32,
    /// How far its middle is from the area's middle, in pixels.
    pub pan: (f32, f32),
    /// While dragging: where the pointer and the pan were when it started.
    pub drag: Option<((f32, f32), (f32, f32))>,
    /// The picture's fitted size and the area's bounds, as last laid out.
    pub laid_out: std::rc::Rc<std::cell::Cell<ViewerLayout>>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ViewerLayout {
    /// The picture's size when fitted (zoom 1).
    pub fitted: (f32, f32),
    /// The area's middle and size, in window pixels.
    pub middle: (f32, f32),
    pub area: (f32, f32),
}

/// The furthest the viewer zooms in.
const MAX_ZOOM: f32 = 8.;

impl Viewer {
    /// Zooms to `zoom`, keeping the picture's point under `at` (window pixels) where it is.
    fn zoom_to(&mut self, zoom: f32, at: (f32, f32)) {
        let zoom = zoom.clamp(1., MAX_ZOOM);
        let layout = self.laid_out.get();
        let p = (at.0 - layout.middle.0, at.1 - layout.middle.1);
        let k = zoom / self.zoom;
        self.pan = (p.0 - (p.0 - self.pan.0) * k, p.1 - (p.1 - self.pan.1) * k);
        self.zoom = zoom;
        self.clamp_pan();
    }

    /// Keeps the picture over the area: it can't be pushed further than its edges.
    fn clamp_pan(&mut self) {
        let layout = self.laid_out.get();
        let limit = |fitted: f32, area: f32| ((fitted * self.zoom - area) / 2.).max(0.);
        let (lx, ly) = (
            limit(layout.fitted.0, layout.area.0),
            limit(layout.fitted.1, layout.area.1),
        );
        self.pan = (self.pan.0.clamp(-lx, lx), self.pan.1.clamp(-ly, ly));
        if self.zoom <= 1. {
            self.pan = (0., 0.);
        }
    }

    /// Double-click: from fitted to the picture's own size (or twice fitted, when it fits
    /// already), and back.
    fn toggle_zoom(&mut self, at: (f32, f32), natural_width: f32) {
        if self.zoom > 1. {
            self.zoom = 1.;
            self.pan = (0., 0.);
            return;
        }
        let fitted = self.laid_out.get().fitted.0;
        let full = if fitted > 0. && natural_width > fitted * 1.05 {
            natural_width / fitted
        } else {
            2.
        };
        self.zoom_to(full, at);
    }
}

const BULK_DELETE_AT_ONCE: usize = 100;
/// Parsed messages kept; it is emptied when full.
const MARKUP_CACHE_SIZE: usize = 4096;

/// Messages and reaction chips already drawn: a chip that is new on a message already shown
/// pops in. Emptied when large; the worst that does is skip a pop.
#[derive(Default)]
pub struct ReactionsSeen {
    pub messages: HashSet<Id>,
    pub chips: HashSet<(Id, String)>,
}

const REACTIONS_SEEN_MAX: usize = 20_000;

/// How long the banner says "Reconnected" once the connection is back.
const RECONNECTED_FOR: std::time::Duration = std::time::Duration::from_secs(2);

/// How long the message jumped to stays highlighted.
const HIGHLIGHT_FOR: std::time::Duration = std::time::Duration::from_secs(4);

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
    /// Opening it failed: it offers to try again instead of loading for ever.
    pub load_failed: bool,
    /// The jump to the first new message was used this visit.
    pub unread_jump_done: bool,
    /// The row after the last one shown: the messages from there on are below the view.
    pub shown_end: usize,
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
    /// Messages just deleted: the height their row had, and when its gap started to close.
    gone: HashMap<Id, (gpui_kit::Pixels, std::time::Instant)>,
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
    (a.has_thumb || a.sealed.is_some()) && a.content_type.starts_with("image/")
}

/// The size an inline image is shown at: scaled into 400×300, never enlarged.
pub fn display_size(a: &Attachment) -> (f32, f32) {
    match (a.width, a.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => {
            let (w, h) = (w as f32, h as f32);
            let scale = (MAX_IMAGE_WIDTH * media_scale() / w)
                .min(MAX_IMAGE_HEIGHT * media_scale() / h)
                .min(1.0);
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

/// A reaction's count. When it changes the new number drops in from above; a count that is
/// only shown again (the row scrolled back) stays still.
fn reaction_count(key: &str, count: u32) -> impl IntoElement {
    div().relative().child(count.to_string()).with_spring(
        SharedString::from(format!("react-count-{key}")),
        gpui_kit::SpringAnimation::new(crate::theme::paced(gpui_kit::SpringConfig::new(
            420., 30., 1.,
        )))
        .to(count as f32),
        move |d, shown| {
            let behind = (count as f32 - shown).abs().min(1.);
            d.top(px(-5. * behind)).opacity(1. - 0.6 * behind)
        },
    )
}

/// Three dots that rise and brighten one after another, over and over: someone is typing.
pub fn typing_dots(id: impl Into<SharedString>, color: gpui_kit::Rgba) -> AnyElement {
    use gpui_kit::{Animation, AnimationExt as _};
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(3.))
        .h(px(10.))
        .with_animation(
            id.into(),
            Animation::new(std::time::Duration::from_millis(1200)).repeat(),
            move |d, t| {
                d.children((0..3).map(|n| {
                    // Each dot's bump comes a little after the one before.
                    let at = (t - n as f32 * 0.16).rem_euclid(1.);
                    let bump = if at < 0.4 {
                        (at / 0.4 * std::f32::consts::PI).sin()
                    } else {
                        0.
                    };
                    div()
                        .size(px(4.))
                        .rounded(px(2.))
                        .relative()
                        .top(px(-2. * bump))
                        .bg(theme::alpha(color, 0.35 + 0.65 * bump))
                }))
            },
        )
        .into_any_element()
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

/// How tall rows growing into the chat turned out, by animation key.
pub type GrowHeights = std::rc::Rc<std::cell::RefCell<HashMap<SharedString, gpui_kit::Pixels>>>;

/// Grows `element` from nothing to its own height, so the rows around it glide aside instead of
/// jumping: a message that just arrived. Its height is measured on the first frame, while it is
/// still closed; once open, it is left to size itself again (reactions, pictures loading).
fn grow_in(heights: &GrowHeights, key: SharedString, element: AnyElement) -> AnyElement {
    use gpui_kit::{Animation, AnimationExt as _};
    let height = heights.borrow().get(&key).copied();
    let record = heights.clone();
    let record_key = key.clone();
    div()
        .w_full()
        .overflow_hidden()
        .child(div().w_full().flex_shrink_0().child(element))
        .on_children_prepainted(move |bounds, _, _| {
            if let Some(b) = bounds.first() {
                let mut heights = record.borrow_mut();
                if heights.len() > 256 {
                    heights.clear();
                }
                heights.insert(record_key.clone(), b.size.height);
            }
        })
        .with_animation(
            key,
            Animation::new(theme::fade() * 2).with_easing(gpui_kit::ease_out_quint()),
            move |d, t| {
                if t >= 1. {
                    d
                } else {
                    d.max_h(height.map_or(gpui_kit::px(0.), |h| h * t))
                }
            },
        )
        .into_any_element()
}

/// A just-deleted message's row: an empty gap closing from `height`, with a faint wash that
/// fades as it goes.
fn closing_gap(id: Id, height: gpui_kit::Pixels) -> AnyElement {
    use gpui_kit::{Animation, AnimationExt as _};
    div()
        .w_full()
        .px(px(8.))
        .child(
            div()
                .size_full()
                .rounded(theme::row_radius())
                .bg(theme::alpha(theme::danger(), 0.08)),
        )
        .with_animation(
            SharedString::from(format!("gone-{id}")),
            Animation::new(theme::fade() * 2).with_easing(gpui_kit::ease_in_out),
            move |d, t| d.h(height * (1. - t)).opacity(1. - t),
        )
        .into_any_element()
}

/// What a row's height depends on, besides the width.
fn signature(row: &Row, s: &InstanceStore, channel: Id) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let window = s.window(channel);
    match row {
        Row::Message(m) => {
            m.grouped.hash(&mut hash);
            m.ends_group.hash(&mut hash);
            m.next_starts_with_file.hash(&mut hash);
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
        list.set_scroll_handler(cx.listener(move |_, event: &ListScrollEvent, window, cx| {
            // gpui keeps the list's state mutably borrowed while it calls this handler, so reading
            // the list here panics. Deferred work runs once the event has been dispatched.
            let near_top = event.visible_range.start < LOAD_OLDER_WITHIN;
            let at_end = event.visible_range.end >= event.count;
            let shown_end = event.visible_range.end;
            let key = scroll_key.clone();
            cx.defer_in(window, move |this, _, cx| {
                if let Some(view) = this.chats.get_mut(&key) {
                    view.shown_end = shown_end;
                }
                this.chat_scrolled(&key, near_top, at_end, cx);
            });
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
                load_failed: false,
                unread_jump_done: false,
                shown_end: 0,
                selected: BTreeSet::new(),
                anchor: None,
                input,
                reply_to: None,
                editing: None,
                uploads: Vec::new(),
                suggestions: Vec::new(),
                suggestion: 0,
                gone: HashMap::new(),
                _input_events: events,
            },
        );
        if let Some(text) = self.core.draft(&key.0, key.1)
            && let Some(view) = self.chats.get(key)
        {
            view.input
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
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
        if let Some(left) = self.open_channel().filter(|open| *open != key) {
            self.remember_draft(&left, cx);
        }
        // Pictures that failed get another chance.
        self.images
            .thumbs
            .retain(|_, picture| !matches!(picture, Img::Failed));
        self.chat_view(&key, window, cx);
        // Coming back moves the "new messages" line to what arrived since the last visit.
        if self.open_channel().as_ref() != Some(&key) {
            let marker = self
                .core
                .store()
                .instance(&inst)
                .and_then(|s| s.read_states.get(&channel).map(|r| r.last_read_id));
            if let Some(view) = self.chats.get_mut(&key) {
                view.read_marker = marker;
                view.unread_jump_done = false;
            }
        }
        self.page = Page::Chat(inst.clone(), channel);
        self.on_chat_opened(&inst, channel, cx);
        self.update_visible_channels();
        self.sync_rows(&key);
        self.focus_composer = true;
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.open_channel(&inst, channel).await },
            move |this, result, _, cx| {
                let failed = result.is_err();
                if let Some(view) = this.chats.get_mut(&key) {
                    view.load_failed = failed;
                }
                if let Err(err) = result {
                    this.error(format!("could not open the channel: {err}"), cx);
                }
                this.sync_rows(&key);
                this.mark_open_read(cx);
                cx.notify();
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

    /// What `content` looks like parsed, from the cache when it was parsed before.
    fn cached_blocks(
        &self,
        content: &str,
        s: &hermes_client_core::store::InstanceStore,
        space: Option<Id>,
    ) -> std::rc::Rc<Vec<markup::Block>> {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let formatted = self.config.buffer.markdown;
        (content, space, s.me_id(), formatted).hash(&mut hasher);
        let key = hasher.finish();
        let mut cache = self.markup_cache.borrow_mut();
        if let Some(found) = cache.get(&key) {
            return found.clone();
        }
        if cache.len() >= MARKUP_CACHE_SIZE {
            cache.clear();
        }
        let blocks = std::rc::Rc::new(if formatted {
            markup::blocks(content, s, space)
        } else {
            markup::plain(content)
        });
        cache.insert(key, blocks.clone());
        blocks
    }

    /// Keeps what is written in the composer of `key` for when Hermes comes back.
    pub fn remember_draft(&self, key: &ChannelKey, cx: &Context<Self>) {
        if let Some(view) = self.chats.get(key) {
            let text = view.input.read(cx).value().to_string();
            self.core.set_draft(&key.0, key.1, &text);
        }
    }

    /// Says so above the chat when messages are not getting through: at once when the
    /// connection gave up, and after [`crate::sidebar::BANNER_AFTER`] while it is reconnecting
    /// (a short blip only shows in the connection icon).
    fn render_connection_banner(
        &self,
        inst: &InstanceId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use hermes_client_core::gateway::ConnStatus;
        let text = match self.core.status(inst) {
            ConnStatus::Reconnecting { attempt }
                if self
                    .core
                    .down_for(inst)
                    .is_some_and(|d| d >= crate::sidebar::BANNER_AFTER) =>
            {
                format!("Connection lost. Reconnecting (try {attempt})…")
            }
            ConnStatus::Failed { reason } => format!("Not connected: {reason}"),
            // Back after the banner showed: it says so in green for a moment, then goes.
            status => {
                let mut banners = self.reconnect_banner.borrow_mut();
                match banners.get(inst) {
                    Some(None) if status == ConnStatus::Connected => {
                        let now = std::time::Instant::now();
                        banners.insert(inst.clone(), Some(now));
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(RECONNECTED_FOR).await;
                            let _ = this.update(cx, |_, cx| cx.notify());
                        })
                        .detach();
                    }
                    Some(Some(at)) if at.elapsed() < RECONNECTED_FOR => {}
                    _ => {
                        banners.remove(inst);
                        return None;
                    }
                }
                return Some(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(8.))
                        .mx(px(16.))
                        .mb(px(6.))
                        .p(px(8.))
                        .rounded(theme::row_radius())
                        .bg(theme::raised())
                        .border_1()
                        .border_color(theme::success())
                        .text_size(theme::secondary_size())
                        .text_color(theme::success())
                        .child(icon(assets::LAN_CONNECT, 16., theme::success()))
                        .child(div().flex_1().child("Reconnected"))
                        .into_any_element(),
                );
            }
        };
        self.reconnect_banner
            .borrow_mut()
            .insert(inst.clone(), None);
        let inst = inst.clone();
        Some(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(8.))
                .mx(px(16.))
                .mb(px(6.))
                .p(px(8.))
                .rounded(theme::row_radius())
                .bg(theme::raised())
                .border_1()
                .border_color(theme::warning())
                .text_size(theme::secondary_size())
                .child(icon(assets::LAN_DISCONNECT, 16., theme::warning()))
                .child(div().flex_1().min_w_0().truncate().child(text))
                .child(
                    Button::new("connection-retry")
                        .cursor_pointer()
                        .label("Reconnect")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Err(err) = this.core.set_online(&inst, true) {
                                this.error(format!("could not reconnect: {err}"), cx);
                            }
                        })),
                )
                // It rises over the message box as it fades in.
                .relative()
                .with_animation(
                    "connection-banner-in",
                    gpui_kit::Animation::new(theme::fade() * 2)
                        .with_easing(gpui_kit::ease_out_quint()),
                    |d, t| d.opacity(t).top(px(6. * (1. - t))),
                )
                .into_any_element(),
        )
    }

    /// A setting that changes how messages look moved: every open chat follows it at once.
    pub fn refresh_chats(&mut self) {
        let keys: Vec<ChannelKey> = self.chats.keys().cloned().collect();
        for key in &keys {
            self.sync_rows(key);
        }
        self.remeasure_chats();
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
        // A message deleted since: its gap closes from the height it had. Gaps closed by now go.
        let window = s.window(key.1);
        let closing = theme::fade() * 2 + std::time::Duration::from_millis(40);
        view.gone.retain(|_, (_, since)| since.elapsed() < closing);
        for (ix, row) in view.rows.iter().enumerate() {
            if let Row::Message(m) = row
                && window.and_then(|w| w.get(m.id)).is_some_and(|m| m.deleted)
                && let Some(bounds) = view.list.bounds_for_item(ix)
                && bounds.size.height > px(0.)
            {
                view.gone
                    .insert(m.id, (bounds.size.height, std::time::Instant::now()));
            }
        }
        let new = match window {
            Some(window) => rows::build(
                s,
                window,
                &rows::Options {
                    group_minutes: self.config.buffer.group_minutes,
                    utc_offset: self.utc_offset,
                    read_marker: view.read_marker,
                    gone: view.gone.keys().copied().collect(),
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

    fn chat_scrolled(
        &mut self,
        key: &ChannelKey,
        near_top: bool,
        at_end: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.chats.get(key) else {
            return;
        };
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
            move |this, result, _, cx| {
                if let Err(err) = result {
                    tracing::warn!("older messages: {err}");
                    this.error(format!("could not load older messages: {err}"), cx);
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
            move |this, result, _, cx| {
                if let Err(err) = result {
                    tracing::warn!("newer messages: {err}");
                    this.error(format!("could not load newer messages: {err}"), cx);
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
        let clear = key.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HIGHLIGHT_FOR).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(view) = this.chats.get_mut(&clear)
                    && view.highlight == Some(message)
                {
                    view.highlight = None;
                    cx.notify();
                }
            });
        })
        .detach();
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
    fn jump_to_present(&mut self, key: &ChannelKey, window: &mut Window, cx: &mut Context<Self>) {
        let live = self
            .core
            .store()
            .instance(&key.0)
            .and_then(|s| s.window(key.1).map(|w| w.reached_end))
            .unwrap_or(false);
        if live {
            if let Some(view) = self.chats.get(key) {
                crate::smooth_scroll::glide_to_end(view.list.clone(), window, cx);
            }
            cx.notify();
            return;
        }
        if let Some(view) = self.chats.get(key) {
            view.list.scroll_to_end();
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
        // Mentions are edited as typed: `@bob`, not `<@3>`.
        let content = {
            let store = self.core.store();
            store.instance(&key.0).and_then(|s| {
                let content = &s.window(key.1)?.get(message)?.content;
                Some(view::readable_mentions(s, key.1, content))
            })
        };
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
        // `@bob` and `#general` as typed become real mentions.
        let resolve = |this: &Self, text: String| {
            let store = this.core.store();
            match store.instance(&inst) {
                Some(s) => view::resolve_mentions(s, channel, &text),
                None => text,
            }
        };
        if let Some(message) = view.editing {
            let text = resolve(self, text);
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
            let content = resolve(self, content);
            let draft = Draft {
                content,
                reply_to,
                attachments,
            };
            if let Err(err) = self.core.send_message(&inst, channel, draft) {
                self.error(err.to_string(), cx);
            }
            self.sync_rows(&key);
            // Sending glides to the newest message.
            if let Some(view) = self.chats.get(&key) {
                crate::smooth_scroll::glide_to_end(view.list.clone(), window, cx);
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
                    self.error("Nicknames are per server; open a server channel.", cx);
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
                None => self.error("Invites are for servers; open a server channel.", cx),
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
        // Too large for the server: said at once, nothing is read or sent.
        let refusal = self
            .core
            .store()
            .instance(&key.0)
            .and_then(|s| s.upload_refusal(size));
        let Some(view) = self.chats.get_mut(key) else {
            return;
        };
        let refused = refusal.is_some();
        view.uploads.push(Upload {
            local,
            name,
            size,
            state: refusal.map_or(UploadState::Uploading, UploadState::Failed),
        });
        if refused {
            cx.notify();
            return;
        }
        let core = self.core.clone();
        let inst = key.0.clone();
        let channel = key.1;
        let key = key.clone();
        self.run(
            cx,
            async move {
                // A picture's size, a sound's waveform and a video's length go with it.
                let file = path.clone();
                let info = tokio::task::spawn_blocking(move || {
                    let (waveform, duration_ms) = crate::media::measure_file(&file);
                    hermes_client_core::secure::FileInfo {
                        size: image::image_dimensions(&file).ok(),
                        waveform,
                        duration_ms,
                    }
                })
                .await
                .unwrap_or_default();
                core.upload_to(&inst, channel, &path, info)
                    .await
                    .map_err(|e| e.to_string())
            },
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
        let dir = crate::open::paste_dir();
        self.next_upload += 1;
        let path = dir.join(format!(
            "paste-{}-{}.png",
            std::process::id(),
            self.next_upload
        ));
        // Encoding a large screenshot takes long enough to be felt on the UI thread.
        let key = key.clone();
        let written = path.clone();
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(move || {
                    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                    buffer.save(&written).map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |this, saved, _, cx| match saved {
                Ok(()) => this.start_upload(&key, path, Some("pasted image.png".into()), cx),
                Err(err) => this.error(format!("could not keep the pasted image: {err}"), cx),
            },
        );
        true
    }

    /// Downloads an attachment into the Downloads folder (never overwriting a file).
    pub fn download(&self, inst: InstanceId, attachment: Attachment, cx: &mut Context<Self>) {
        let core = self.core.clone();
        let (folder, ask) = (
            self.config.general.download_dir.clone(),
            self.config.general.ask_where_to_save,
        );
        self.run_status(cx, async move {
            let result: Result<String, String> = async {
                let chosen = if ask {
                    let name = crate::open::safe_filename(&attachment.filename);
                    let Some(file) = rfd::AsyncFileDialog::new()
                        .set_file_name(name)
                        .save_file()
                        .await
                    else {
                        return Ok("Download cancelled".to_owned());
                    };
                    Some(file.path().to_path_buf())
                } else {
                    None
                };
                let bytes = core
                    .download(&inst, &attachment)
                    .await
                    .map_err(|e| e.to_string())?;
                let dir = if folder.is_empty() {
                    crate::open::downloads_dir()
                } else {
                    std::path::PathBuf::from(folder)
                };
                let path = tokio::task::spawn_blocking(move || match chosen {
                    Some(path) => std::fs::write(&path, &bytes).map(|()| path),
                    None => crate::open::save_new(&dir, &attachment.filename, &bytes),
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

    /// A file card's download button.
    pub fn download_button(
        &self,
        inst: &InstanceId,
        a: &Attachment,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let (inst, attachment) = (inst.clone(), a.clone());
        tip_button(
            SharedString::from(format!("file-download-{}", a.id)),
            assets::DOWNLOAD,
            "Download",
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.download(inst.clone(), attachment.clone(), cx);
        }))
    }

    pub(crate) fn open_image(
        &mut self,
        inst: InstanceId,
        attachment: Attachment,
        cx: &mut Context<Self>,
    ) {
        self.viewer = Some(Viewer {
            inst: inst.clone(),
            attachment: attachment.clone(),
            image: Img::Loading,
            zoom: 1.,
            pan: (0., 0.),
            drag: None,
            laid_out: Default::default(),
        });
        let core = self.core.clone();
        let id = attachment.id;
        self.run(
            cx,
            async move {
                let bytes = core
                    .download(&inst, &attachment)
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
        let layout = viewer.laid_out.clone();
        let zoom = viewer.zoom;
        let zoomed = zoom > 1. && layout.get().fitted.0 > 0.;
        let content = match &viewer.image {
            Img::Ready(picture) => {
                let shown = picture.shown(self.animating());
                // Its own width in window pixels, for the double-click's full size.
                let natural = shown.size(0).width.0 as f32 / self.scale.max(0.5);
                let fitted = layout.get().fitted;
                // The id lets gpui step through an animation's frames; without one a GIF
                // stays on its first frame.
                let picture =
                    img(shown)
                        .object_fit(gpui_kit::ObjectFit::Contain)
                        .id(SharedString::from(format!(
                            "viewer-pic-{}",
                            viewer.attachment.id
                        )));
                let picture = if zoomed {
                    picture
                        .flex_shrink_0()
                        .w(px(fitted.0 * zoom))
                        .h(px(fitted.1 * zoom))
                } else {
                    picture.max_w_full().max_h_full()
                };
                let record = layout.clone();
                div()
                    .id("viewer-picture")
                    .relative()
                    .flex()
                    .flex_shrink_0()
                    .left(px(viewer.pan.0))
                    .top(px(viewer.pan.1))
                    .when(zoomed, |d| d.cursor_grab())
                    .when(!zoomed, |d| d.max_w_full().max_h_full())
                    .child(picture)
                    // Fitted, it notes the size it fits at: zooming starts from there.
                    .when(!zoomed, |d| {
                        gpui_kit::base::ElementExt::on_prepaint(d, move |b, _, _| {
                            let mut l = record.get();
                            l.fitted = (b.size.width.as_f32(), b.size.height.as_f32());
                            record.set(l);
                        })
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if let Some(v) = &mut this.viewer
                                && v.zoom > 1.
                            {
                                let at = event.position;
                                v.drag = Some(((at.x.as_f32(), at.y.as_f32()), v.pan));
                            }
                        }),
                    )
                    // A click on the picture leaves the viewer open; a double-click zooms.
                    .on_click(
                        cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
                            cx.stop_propagation();
                            if event.click_count() >= 2
                                && let Some(v) = &mut this.viewer
                            {
                                let at = event.position();
                                v.toggle_zoom((at.x.as_f32(), at.y.as_f32()), natural);
                                cx.notify();
                            }
                        }),
                    )
                    .into_any_element()
            }
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
                    .font_family(theme::font())
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
                            .child(
                                tip_button("viewer-download", assets::DOWNLOAD, "Download")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        if let Some(viewer) = &this.viewer {
                                            this.download(
                                                viewer.inst.clone(),
                                                viewer.attachment.clone(),
                                                cx,
                                            );
                                        }
                                    })),
                            )
                            .child(tip_button("viewer-close", assets::CLOSE, "Close").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.viewer = None;
                                    cx.notify();
                                }),
                            )),
                    )
                    .child({
                        let record = layout.clone();
                        div()
                            .id("viewer-area")
                            .relative()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .p(px(16.))
                            .child(
                                gpui_kit::canvas(
                                    move |bounds, _, _| {
                                        let mut l = record.get();
                                        let c = bounds.center();
                                        l.middle = (c.x.as_f32(), c.y.as_f32());
                                        l.area = (
                                            bounds.size.width.as_f32() - 32.,
                                            bounds.size.height.as_f32() - 32.,
                                        );
                                        record.set(l);
                                    },
                                    |_, (), _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .child(content)
                            // The wheel zooms toward the pointer.
                            .on_scroll(cx.listener(|this, event: &gpui_kit::ScrollEvent, _, cx| {
                                let Some(v) = &mut this.viewer else { return };
                                if !matches!(v.image, Img::Ready(_)) {
                                    return;
                                }
                                cx.stop_propagation();
                                let dy = event.delta.pixel_delta(px(20.)).y.as_f32();
                                let at = event.position;
                                v.zoom_to(
                                    v.zoom * (dy * 0.004).exp(),
                                    (at.x.as_f32(), at.y.as_f32()),
                                );
                                cx.notify();
                            }))
                            // Dragging moves a zoomed picture.
                            .on_mouse_move(cx.listener(
                                |this, event: &gpui_kit::MouseMoveEvent, _, cx| {
                                    let Some(v) = &mut this.viewer else { return };
                                    let Some((start, pan)) = v.drag else { return };
                                    if event.pressed_button != Some(MouseButton::Left) {
                                        v.drag = None;
                                        return;
                                    }
                                    let at = event.position;
                                    v.pan = (
                                        pan.0 + at.x.as_f32() - start.0,
                                        pan.1 + at.y.as_f32() - start.1,
                                    );
                                    v.clamp_pan();
                                    cx.notify();
                                },
                            ))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, _| {
                                    if let Some(v) = &mut this.viewer {
                                        v.drag = None;
                                    }
                                }),
                            )
                    }),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }

    /// Sends message `message` of `from` again in `to`, quoted under a "Forwarded from" line.
    /// Files stay where they are (an upload belongs to one message); a link leads to them.
    pub fn forward_message(
        &mut self,
        inst: InstanceId,
        from: Id,
        message: Id,
        to: Id,
        cx: &mut Context<Self>,
    ) {
        let content = {
            let store = self.core.store();
            let Some(s) = store.instance(&inst) else {
                return;
            };
            let Some(m) = s.window(from).and_then(|w| w.get(message)) else {
                return;
            };
            let place = s
                .channel(from)
                .map(|c| view::channel_title(s, c))
                .unwrap_or_default();
            let author = s.display_name(s.space_of(from), m.author.id);
            forwarded_text(
                &m.content,
                &author,
                &place,
                m.attachments.len(),
                &format!("hermes://{inst}/channels/{from}/{message}"),
            )
        };
        let draft = Draft {
            content,
            reply_to: None,
            attachments: Vec::new(),
        };
        match self.core.send_message(&inst, to, draft) {
            Ok(_) => self.info("Forwarded", cx),
            Err(err) => self.error(format!("could not forward: {err}"), cx),
        }
        if self.chats.contains_key(&(inst.clone(), to)) {
            self.sync_rows(&(inst, to));
        }
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
            (Target::Message(inst, channel, message), Action::MarkUnreadHere) => {
                self.core.mark_unread(&inst, channel, message);
                cx.notify();
            }
            (Target::Message(inst, channel, message), Action::Forward) => {
                self.open_forward(inst, channel, message, window, cx);
            }
            (Target::Message(inst, channel, message), Action::CopyLink) => {
                self.copy(
                    format!("hermes://{inst}/channels/{channel}/{message}"),
                    "Message link",
                    cx,
                );
            }
            (Target::Message(inst, channel, message), Action::Report) => {
                let excerpt = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.window(channel))
                    .and_then(|w| w.get(message).map(|m| m.content.clone()))
                    .unwrap_or_default();
                self.open_form_report(inst, channel, message, excerpt, window, cx);
            }
            (Target::Messages(inst, channel, messages), Action::DeleteMessage) => {
                if let Some(view) = self.chats.get_mut(&(inst.clone(), channel)) {
                    view.clear_selection();
                }
                self.run_status(cx, async move {
                    let total = messages.len();
                    let mut failed = 0;
                    let mut last_error = None;
                    let rest = core.rest(&inst).map_err(|e| e.to_string())?;
                    for ids in messages.chunks(BULK_DELETE_AT_ONCE) {
                        if let Err(err) = rest.bulk_delete(channel, ids.to_vec()).await {
                            failed += ids.len();
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
        // How many messages are below what shows, when all of them are loaded.
        let below = view
            .rows
            .iter()
            .skip(view.shown_end)
            .filter(|r| matches!(r, Row::Message(_)))
            .count();
        let jump_label = if live && below > 0 {
            SharedString::from(format!(
                "{below} newer message{} · Jump to present",
                if below == 1 { "" } else { "s" }
            ))
        } else {
            SharedString::from("Jump to present")
        };
        let newest = window_state.and_then(|w| w.messages.iter().last().map(|m| m.id));
        let first_unread = view
            .read_marker
            .filter(|marker| !view.unread_jump_done && newest.is_some_and(|n| n > *marker));
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

        let content: AnyElement = if !loaded && view.load_failed {
            let (retry_inst, retry_channel) = key.clone();
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .text_color(theme::text_secondary())
                .child("This channel could not be loaded.")
                .child(
                    Button::new("chat-retry")
                        .cursor_pointer()
                        .label("Try again")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_chat(retry_inst.clone(), retry_channel, window, cx);
                        })),
                )
                .into_any_element()
        } else if !loaded {
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
                        .text_size(theme::heading_size())
                        .child(format!("Welcome to {title}")),
                )
                .child(
                    div()
                        .text_size(theme::secondary_size())
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
                .child(self.selection_list_marker())
                .child(messages)
                .children(first_unread.map(|marker| {
                    let (jump_inst, jump_channel) = key.clone();
                    div()
                        .absolute()
                        .top(px(8.))
                        .right(px(16.))
                        .child(
                            Button::new("jump-unread")
                                .cursor_pointer()
                                .label("Jump to new messages")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(view) =
                                        this.chats.get_mut(&(jump_inst.clone(), jump_channel))
                                    {
                                        view.unread_jump_done = true;
                                    }
                                    this.jump_to(
                                        jump_inst.clone(),
                                        jump_channel,
                                        marker,
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        // It drops in from the top edge.
                        .with_animation(
                            "jump-unread-in",
                            gpui_kit::Animation::new(theme::fade() * 2)
                                .with_easing(gpui_kit::ease_out_quint()),
                            |d, t| d.opacity(t).top(px(8. * t)),
                        )
                }))
                .when(!at_end || !live, |d| {
                    d.child(
                        div()
                            .absolute()
                            .bottom(px(12.))
                            .right(px(16.))
                            .child(
                                Button::new("jump-present")
                                    .cursor_pointer()
                                    .primary()
                                    .label(jump_label)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.jump_to_present(&jump_key, window, cx);
                                    })),
                            )
                            // It rises from the bottom edge.
                            .with_animation(
                                "jump-present-in",
                                gpui_kit::Animation::new(theme::fade() * 2)
                                    .with_easing(gpui_kit::ease_out_quint()),
                                |d, t| d.opacity(t).bottom(px(12. * t)),
                            ),
                    )
                })
                .children(typing.map(|text| {
                    div()
                        .absolute()
                        .bottom(px(2.))
                        .left(px(16.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(theme::radius_small())
                        .bg(theme::background())
                        .text_size(theme::caption_size())
                        .text_color(theme::text_secondary())
                        .child(typing_dots("typing-dots", theme::text_secondary()))
                        .child(text)
                        // It fades in when someone starts.
                        .with_animation(
                            "typing-in",
                            gpui_kit::Animation::new(theme::fade())
                                .with_easing(gpui_kit::ease_out_quint()),
                            |d, t| d.opacity(t),
                        )
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
                .text_size(theme::secondary_size())
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
            .children(self.render_connection_banner(&key.0, cx))
            .children(self.render_new_devices(&key.0, key.1, cx))
            .children(self.render_unencrypted_bar(&key.0, key.1))
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
            .rounded(theme::row_radius())
            .bg(theme::selected())
            .child(div().text_size(theme::body_size()).child(what))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme::caption_size())
                    .text_color(theme::text_tertiary())
                    .child("Ctrl+click to add · Shift+click for a range"),
            )
            .child(
                Button::new("selection-cancel")
                    .cursor_pointer()
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
                    .cursor_pointer()
                    .danger()
                    .label("Delete")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Only what still exists and I may still delete.
                        let selectable = selectable_messages(this, &delete_key.0, delete_key.1);
                        if let Some(view) = this.chats.get_mut(&delete_key) {
                            view.selected.retain(|id| selectable.contains(id));
                            let ids: Vec<Id> = view.selected.iter().copied().collect();
                            if !ids.is_empty() {
                                this.cancel_closing("confirm");
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
                    .rounded(theme::radius_small())
                    .cursor_pointer()
                    .text_size(theme::secondary_size())
                    // The chosen row keeps its own highlight.
                    .when(n != view.suggestion, |d| d.hover(|d| d.bg(theme::hover())))
                    .child(suggestion.label.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.accept_suggestion(&accept, n, window, cx);
                    }))
                    // The highlight flows from row to row as the arrows move it.
                    .with_spring(
                        ("suggestion-chosen", n),
                        gpui_kit::SpringAnimation::new(crate::theme::paced(
                            crate::hover::HOVER_SPRING,
                        ))
                        .to(n == view.suggestion),
                        |d, phase| d.bg(theme::alpha(theme::selected(), phase.0.clamp(0., 1.))),
                    )
            });
            div()
                .absolute()
                .bottom_full()
                .left(px(16.))
                .right(px(16.))
                .mb(px(6.))
                .p(px(6.))
                .rounded(theme::radius_large())
                .bg(theme::sidebar())
                .border_1()
                .border_color(theme::border())
                .shadow_md()
                .children(rows)
                // It rises out of the message box as it fades in.
                .with_animation(
                    "suggestions-in",
                    gpui_kit::Animation::new(theme::fade()).with_easing(gpui_kit::ease_out_quint()),
                    |d, t| d.opacity(t).mb(px(6. - 4. * (1. - t))),
                )
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
                        .min_w_0()
                        .truncate()
                        .text_size(theme::secondary_size())
                        .text_color(theme::text_secondary())
                        .child(text),
                )
                .child(
                    tip_button("composer-cancel", assets::CLOSE, "Cancel").on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.composer_cancel(&cancel_key, window, cx);
                        },
                    )),
                )
                // It opens out of the message box instead of pushing it at once.
                .overflow_hidden()
                .with_animation(
                    SharedString::from(format!("composer-mode-{glyph}")),
                    gpui_kit::Animation::new(theme::fade() * 2)
                        .with_easing(gpui_kit::ease_out_quint()),
                    |d, t| d.max_h(px(44. * t)).opacity(t),
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
            let uploading = matches!(upload.state, UploadState::Uploading);
            let failed = matches!(upload.state, UploadState::Failed(_));
            let state = div().text_color(color).child(state);
            // Still going up: the state breathes.
            let state = if uploading {
                state
                    .with_animation(
                        ("upload-busy", local as usize),
                        gpui_kit::Animation::new(std::time::Duration::from_millis(1200))
                            .repeat()
                            .with_easing(gpui_kit::pulsating_between(0.4, 1.)),
                        |d, t| d.opacity(t),
                    )
                    .into_any_element()
            } else {
                state.into_any_element()
            };
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(8.))
                .py(px(4.))
                .rounded(theme::radius_small())
                .bg(theme::selected())
                .border_1()
                .border_color(if failed {
                    theme::danger()
                } else {
                    theme::alpha(theme::border(), 0.)
                })
                .text_size(theme::caption_size())
                .child(icon(assets::FILE, 14., theme::text_secondary()))
                .child(div().max_w(px(240.)).truncate().child(upload.name.clone()))
                .child(state)
                .child(
                    tip_button(("upload-remove", local as usize), assets::CLOSE, "Remove")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(view) = this.chats.get_mut(&remove) {
                                view.uploads.retain(|u| u.local != local);
                            }
                            cx.notify();
                        })),
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
        let emoji_key = key.clone();
        let paste_key = key.clone();
        let max_mb = self
            .core
            .store()
            .instance(&key.0)
            .and_then(|s| s.max_upload_mb());
        let attach_tip = SharedString::from(match max_mb {
            Some(mb) => format!("Attach files up to {mb} MB (or drop them here)"),
            None => "Attach files (or drop them here)".to_owned(),
        });
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
            .children(popup)
            .child(
                // Flush with the panel's sides and bottom, as tall as the user panel next to it.
                div()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .min_h(px(theme::FOOTER_HEIGHT))
                    .px(px(12.))
                    .py(px(4.))
                    .bg(theme::raised())
                    .border_t_1()
                    .border_color(theme::border())
                    .children(mode_bar)
                    .children(uploads)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                icon_button("composer-attach", assets::ATTACH)
                                    .tooltip(move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            attach_tip.clone(),
                                        )
                                        .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.pick_files(pick_key.clone(), cx);
                                    })),
                            )
                            .child(div().flex_1().min_w_0().child(input))
                            .child({
                                let key = emoji_key.clone();
                                icon_button("composer-emoji", assets::EMOJI)
                                    .tooltip(|window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new("Emoji")
                                            .build(window, cx)
                                    })
                                    .on_click(cx.listener(
                                        move |this, event: &gpui_kit::ClickEvent, window, cx| {
                                            this.open_emoji_picker(
                                                crate::emoji_picker::PickFor::Compose(key.clone()),
                                                event.position(),
                                                window,
                                                cx,
                                            );
                                        },
                                    ))
                            }),
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
            let wanted: Vec<Attachment> = {
                let store = self.core.store();
                store
                    .instance(inst)
                    .and_then(|s| s.window(channel)?.get(m.id).cloned())
                    .map(|msg| {
                        msg.attachments
                            .iter()
                            .filter(|a| is_image(a))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default()
            };
            for attachment in &wanted {
                self.load_thumbnail(inst, attachment, cx);
            }
        }
        let store = self.core.store();
        let Some(s) = store.instance(inst) else {
            return div().into_any_element();
        };
        // Rows that just came grow in, so the list glides instead of jumping: someone else's
        // fresh message, and mine while it sends (it then stays as it is).
        let grow = (!cx.reduce_motion())
            .then(|| match &row {
                Row::Message(m) => s.window(channel).and_then(|w| w.get(m.id)).and_then(|msg| {
                    let age = now_ms() - msg.created_at() as i64;
                    ((-500..1500).contains(&age) && s.me_id() != Some(msg.author.id))
                        .then(|| SharedString::from(format!("grow-{}", m.id)))
                }),
                Row::Pending(p) => s
                    .window(channel)
                    .and_then(|w| w.pending(p.local_id))
                    .filter(|p| matches!(p.state, PendingState::Sending | PendingState::Queued))
                    .filter(|p| now_ms() - p.created_at < 1500)
                    .map(|_| SharedString::from(format!("grow-pending-{:?}", p.local_id))),
                _ => None,
            })
            .flatten();
        let element = match &row {
            Row::Older => div()
                .flex()
                .justify_center()
                .py(px(8.))
                .text_size(theme::caption_size())
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
            Row::NewMessages => new_messages_divider(),
            Row::Message(m) => match s.window(channel).and_then(|w| w.get(m.id)) {
                Some(message) => self.render_message(s, inst, channel, m, message, cx),
                None => div().into_any_element(),
            },
            Row::Pending(p) => self.render_pending(s, inst, channel, p, cx),
            Row::Gone(id) => {
                let height = self
                    .chats
                    .get(&key)
                    .and_then(|v| v.gone.get(id))
                    .map_or(px(0.), |(h, _)| *h);
                closing_gap(*id, height)
            }
        };
        match grow {
            Some(grow_key) => grow_in(&self.grow_heights, grow_key, element),
            None => element,
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
        let compact = self.config.buffer.compact;
        let mut body: Vec<AnyElement> = Vec::new();
        // Compact: what goes before the text on its first line, and the time in front of it.
        let mut head_line: Vec<AnyElement> = Vec::new();
        let mut compact_time: Option<AnyElement> = None;
        if let Some(reply) = &m.reply {
            let loaded = s.window(channel).and_then(|w| w.get(reply.id));
            let excerpt = reply_text(loaded, &reply.excerpt);
            let (jump_inst, reply_id) = (inst.clone(), reply.id);
            body.push(
                div()
                    .id(SharedString::from(format!("reply-{id}")))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .text_size(theme::caption_size())
                    .text_color(theme::text_tertiary())
                    // A click jumps to it: its text brightens under the pointer.
                    .hover(|d| d.text_color(theme::text_secondary()))
                    .child(icon(assets::REPLY, 12., theme::text_tertiary()))
                    .child(
                        div()
                            .text_color(theme::text_secondary())
                            .font_weight(FontWeight::MEDIUM)
                            .child(reply.author.clone()),
                    )
                    .child({
                        let part = Part {
                            message: id,
                            index: selection::REPLY,
                        };
                        let (text, record) = self.selectable(inst, channel, part, excerpt, vec![]);
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(text)
                            .child(record)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        if !this.selection.dragged {
                            this.jump_to(jump_inst.clone(), channel, reply_id, window, cx);
                        }
                    }))
                    .into_any_element(),
            );
        }
        if !m.grouped {
            let name = s.display_name(space, msg.author.id);
            let color = space
                .and_then(|sp| author_color(s, sp, msg.author.id))
                .map_or(theme::role_default(), theme::rgb_u32);
            let part = |index| Part { message: id, index };
            let (name, name_record) =
                self.selectable(inst, channel, part(selection::AUTHOR), name, vec![]);
            let (time, time_record) = self.selectable(
                inst,
                channel,
                part(selection::TIME),
                time_text.clone(),
                vec![],
            );
            let author = self.author_menu(
                div()
                    .id(SharedString::from(format!("author-{id}")))
                    .relative()
                    .cursor_pointer()
                    .text_size(theme::body_size())
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color)
                    .hover(|d| d.underline())
                    .child(name)
                    .child(name_record),
                inst,
                space,
                msg.author.id,
                cx,
            );
            // Under the pointer its time says the whole date, and when it was edited.
            let time_tip = time_tip(msg, self.utc_offset);
            let time = div()
                .id(SharedString::from(format!("time-{id}")))
                .relative()
                .cursor_text()
                .text_size(theme::caption_size())
                .text_color(theme::text_tertiary())
                .child(time)
                .child(time_record)
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(time_tip.clone()).build(window, cx)
                });
            let lock = crate::encryption::lock_mark(
                msg.lock,
                s.channel_encrypted(channel)
                    && msg.kind == hermes_client_core::proto::models::MessageKind::Default,
            );
            if compact {
                // Time, small picture, name and text on one line.
                if self.config.buffer.show_avatars {
                    let name = s.display_name(space, msg.author.id);
                    let user = s.user(msg.author.id).unwrap_or(&msg.author);
                    let avatar = self.avatar(inst, Some(user), &name, COMPACT_AVATAR);
                    head_line.push(
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
                        .into_any_element(),
                    );
                }
                head_line.push(author.into_any_element());
                head_line.extend(lock.map(IntoElement::into_any_element));
                compact_time = Some(time.into_any_element());
            } else {
                body.push(
                    div()
                        .flex()
                        .gap(px(8.))
                        .items_baseline()
                        .child(author)
                        .child(time)
                        .children(lock)
                        .into_any_element(),
                );
            }
        }
        let revealed = view.is_some_and(|v| v.revealed.contains(&id));
        let blocks = if s.blocked.contains(&msg.author.id) && !revealed {
            std::rc::Rc::new(vec![markup::Block::Text(vec![markup::Piece {
                text: "Message from someone you blocked. Click to show it.".to_owned(),
                link: Some(Link::Reveal),
                ..markup::Piece::default()
            }])])
        } else {
            self.cached_blocks(&msg.content, s, space)
        };
        let last = blocks.len().saturating_sub(1);
        let system = m.system;
        let mut texts: Vec<AnyElement> = Vec::new();
        for (i, block) in blocks.iter().cloned().enumerate() {
            let edited = i == last && msg.edited_at.is_some();
            texts
                .push(self.render_block(block, inst, channel, id, i, system, revealed, edited, cx));
        }
        if msg.content.is_empty() && msg.edited_at.is_some() {
            texts.push(
                div()
                    .text_size(theme::caption_size())
                    .text_color(theme::text_tertiary())
                    .child("(edited)")
                    .into_any_element(),
            );
        }
        if compact && !(head_line.is_empty() && texts.is_empty()) {
            body.push(compact_line(head_line, texts));
        } else {
            body.extend(texts);
        }
        for (index, attachment) in msg.attachments.iter().enumerate() {
            let place = FilePlace {
                channel,
                message: id,
                index,
            };
            // A file keeps a little more room from what is above it. When nothing is, the row's
            // own space holds it (`rows::padding`), so the highlight stays even.
            let after_something = !body.is_empty();
            body.push(
                div()
                    .when(after_something, |d| d.mt(px(crate::rows::FILE_SPACE)))
                    .child(self.render_attachment(inst, place, attachment, cx))
                    .into_any_element(),
            );
        }
        // Messages and chips drawn before: a chip new on a message already shown pops in, one
        // only shown again (scrolled back into view) does not.
        let shown_before = {
            let mut seen = self.reactions_seen.borrow_mut();
            if seen.messages.len() > REACTIONS_SEEN_MAX {
                seen.messages.clear();
                seen.chips.clear();
            }
            !seen.messages.insert(id)
        };
        if !msg.reactions.is_empty() {
            let chips = msg.reactions.iter().enumerate().map(|(n, r)| {
                let fresh_chip = self
                    .reactions_seen
                    .borrow_mut()
                    .chips
                    .insert((id, r.emoji.clone()))
                    && shown_before;
                let (mine, emoji) = (r.me, r.emoji.clone());
                let (inst, emoji_id) =
                    (inst.clone(), SharedString::from(format!("react-{id}-{n}")));
                let chip = div()
                    .id(emoji_id)
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(10.))
                    .border_1()
                    .cursor_pointer()
                    .text_size(theme::secondary_size())
                    .when(!mine, |d| d.hover(|d| d.bg(theme::hover())))
                    .child(r.emoji.clone())
                    .child(reaction_count(&format!("{id}-{}", r.emoji), r.count))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.react(inst.clone(), channel, id, emoji.clone(), !mine, cx);
                    }))
                    // Reacting or taking it back: the accent flows in or out.
                    .with_spring(
                        SharedString::from(format!("react-mine-{id}-{}", r.emoji)),
                        gpui_kit::SpringAnimation::new(crate::theme::paced(
                            crate::hover::HOVER_SPRING,
                        ))
                        .to(mine),
                        |d, phase| {
                            let t = phase.0.clamp(0., 1.);
                            d.bg(theme::mix(theme::sidebar(), theme::accent_soft(), t))
                                .border_color(theme::mix(theme::border(), theme::accent(), t))
                        },
                    );
                if fresh_chip {
                    // New on a message already shown: it pops up into its place.
                    div()
                        .relative()
                        .child(chip)
                        .with_spring(
                            SharedString::from(format!("react-new-{id}-{}", r.emoji)),
                            gpui_kit::SpringAnimation::new(crate::theme::paced(
                                gpui_kit::SpringConfig::new(520., 22., 1.),
                            ))
                            .to(true)
                            .from(false),
                            |d, phase| {
                                d.opacity(phase.0.clamp(0., 1.))
                                    .top(px(6. * (1. - phase.0)))
                            },
                        )
                        .into_any_element()
                } else {
                    chip.into_any_element()
                }
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

        let gutter: AnyElement = if compact {
            compact_time_slot(&time_text, compact_time)
        } else if !m.grouped && self.config.buffer.show_avatars {
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
            let tip = time_tip(msg, self.utc_offset);
            div()
                .id(SharedString::from(format!("gutter-time-{id}")))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .pt(px(3.))
                .text_size(theme::caption_size())
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
        let (top, bottom) = crate::rows::padding(m, self.config.buffer.compact);
        let (click_inst, press_inst) = (inst.clone(), inst.clone());
        let target = Target::Message(inst.clone(), channel, id);
        let element = div()
            .id(SharedString::from(format!("message-{id}")))
            .group(ROW_GROUP)
            .relative()
            .flex()
            .w_full()
            .pt(px(top))
            .pb(px(bottom))
            .pl(px(8.))
            .pr(px(8.))
            .rounded(theme::radius_small())
            .when(selected, |d| d.bg(theme::accent_soft()))
            // Just edited while it shows: a soft flash says it changed.
            .when(
                msg.edited_at
                    .is_some_and(|at| (-500..1500).contains(&(hermes_client_core::now_ms() - at))),
                |d| {
                    d.child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded(theme::radius_small())
                            .with_animation(
                                SharedString::from(format!(
                                    "edited-flash-{id}-{}",
                                    msg.edited_at.unwrap_or_default()
                                )),
                                gpui_kit::Animation::new(std::time::Duration::from_millis(900))
                                    .with_easing(gpui_kit::ease_out_quint()),
                                |d, t| d.bg(theme::alpha(theme::accent_soft(), 1. - t)),
                            ),
                    )
                },
            )
            // The message jumped to glows, then fades back over the time it stays marked.
            .when(jumped && !selected, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(theme::radius_small())
                        .with_animation(
                            SharedString::from(format!("jumped-{id}")),
                            gpui_kit::Animation::new(HIGHLIGHT_FOR),
                            |d, t| {
                                // Full for the first half, then easing away.
                                let fade = ((t - 0.5) * 2.).clamp(0., 1.);
                                d.bg(theme::alpha(theme::selected(), 1. - fade * fade))
                            },
                        ),
                )
            })
            .when(highlight && !selected && !jumped, |d| {
                d.bg(theme::mention_soft())
                    .border_l_2()
                    .border_color(theme::accent())
            })
            // Under the pointer its background eases in and out instead of switching.
            .when(!selected && !jumped && !highlight, |d| {
                let key = format!("message-hover-{id}");
                let hovered = self.is_hovered(&key) && self.hovered_file != Some(id);
                d.on_hover(self.track_hover(key.clone(), cx)).child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(theme::radius_small())
                        .with_spring(
                            SharedString::from(key),
                            gpui_kit::SpringAnimation::new(crate::theme::paced(
                                crate::hover::HOVER_SPRING,
                            ))
                            .to(hovered),
                            |d, phase| {
                                d.bg(theme::alpha(theme::hover_soft(), phase.0.clamp(0., 1.)))
                            },
                        ),
                )
            })
            // Text can be selected from anywhere in the row; names, links and files keep their own.
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui_kit::MouseDownEvent, _, cx| {
                    this.text_press(&press_inst, channel, id, event, cx);
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.selection.dragged {
                    return;
                }
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
            .child(
                div()
                    .flex_shrink_0()
                    .when(!compact, |d| d.w(px(GUTTER)))
                    .child(gutter),
            )
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
        let row = div()
            .w_full()
            .px(px(8.))
            .child(self.with_menu(element, target, cx));
        // Someone else's message that just arrived fades in, rising into place. Only fresh ones:
        // the list drops rows scrolled away, and an old one coming back must not replay it.
        let age = hermes_client_core::now_ms() - msg.created_at() as i64;
        let fresh = (-500..1500).contains(&age);
        let mine = s.me_id() == Some(msg.author.id);
        if fresh && !mine {
            row.relative()
                .with_animation(
                    SharedString::from(format!("arrived-{id}")),
                    gpui_kit::Animation::new(theme::fade() * 2)
                        .with_easing(gpui_kit::ease_out_quint()),
                    |d, t| d.opacity(t).top(px(10. * (1. - t))),
                )
                .into_any_element()
        } else if fresh {
            // Mine, just confirmed: it brightens from the dimmed row it was while sending.
            row.with_animation(
                SharedString::from(format!("confirmed-{id}")),
                gpui_kit::Animation::new(theme::fade() * 2).with_easing(gpui_kit::ease_out_quint()),
                |d, t| d.opacity(0.6 + 0.4 * t),
            )
            .into_any_element()
        } else {
            row.into_any_element()
        }
    }

    /// `element` opens the person's profile on a left click and their menu on a right click.
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
        self.with_own_menu(element, target, cx)
            .on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
                    cx.stop_propagation();
                    if !this.selection.dragged {
                        this.open_profile(&left, event.position(), cx);
                    }
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
        let quick: Vec<String> = self
            .config
            .buffer
            .quick_reactions
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let more_inst = inst.clone();
        let more = div()
            .id(("quick-reaction", usize::MAX))
            .flex()
            .items_center()
            .justify_center()
            .size(px(30.))
            .rounded(theme::radius_small())
            .cursor_pointer()
            .hover(|d| d.bg(theme::hover()))
            .child("…")
            .on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_emoji_picker(
                        crate::emoji_picker::PickFor::React(more_inst.clone(), channel, message),
                        event.position(),
                        window,
                        cx,
                    );
                }),
            );
        let buttons = quick.into_iter().enumerate().map(|(n, emoji)| {
            let inst = inst.clone();
            div()
                .id(("quick-reaction", n))
                .flex()
                .items_center()
                .justify_center()
                .size(px(30.))
                .rounded(theme::radius_small())
                .cursor_pointer()
                .hover(|d| d.bg(theme::hover()))
                .child(emoji.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.react(inst.clone(), channel, message, emoji.clone(), true, cx);
                }))
        });
        let buttons = buttons.chain(std::iter::once(more));
        let close_inst = inst.clone();
        div()
            .id("palette")
            .absolute()
            .right(px(16.))
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(3.))
            .rounded(theme::row_radius())
            .bg(theme::sidebar())
            .border_1()
            .border_color(theme::border())
            .occlude()
            .children(buttons)
            .child(
                tip_button("palette-close", assets::CLOSE, "Close")
                    // Like the reactions beside it.
                    .size(px(30.))
                    .hover(|d| d.bg(theme::hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(view) = this.chats.get_mut(&(close_inst.clone(), channel)) {
                            view.palette = None;
                        }
                        cx.notify();
                    })),
            )
            // It pops up over the message's corner as it fades in.
            .with_animation(
                SharedString::from(format!("palette-in-{message}")),
                gpui_kit::Animation::new(theme::fade()).with_easing(gpui_kit::ease_out_quint()),
                |d, t| d.opacity(t).top(px(-16. + 6. * (1. - t))),
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
                .text_size(theme::caption_size())
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
            Block::Code { code, language } => {
                let part = Part {
                    message,
                    index: selection::BLOCKS + index as u16,
                };
                let colors = self
                    .syntax_cache
                    .borrow_mut()
                    .highlights(&code, language.as_deref());
                let (text, record) = self.selectable(inst, channel, part, code, colors);
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .relative()
                            .cursor_text()
                            .p(px(8.))
                            .rounded(theme::radius_small())
                            .bg(theme::code_background())
                            .border_1()
                            .border_color(theme::border())
                            .font_family(theme::mono_font())
                            .text_size(theme::secondary_size())
                            .whitespace_normal()
                            .child(text)
                            .child(record),
                    )
                    .when(edited, |d| d.child(edited_mark()))
                    .into_any_element()
            }
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
        let part = Part {
            message,
            index: selection::BLOCKS + index as u16,
        };
        let (styled, record) = self.selectable(inst, channel, part, text, highlights);
        let body = div()
            .relative()
            .cursor_text()
            .text_size(theme::body_size())
            .when(system, |d| d.text_color(theme::text_secondary()));
        if links.is_empty() {
            return body.child(styled).child(record).into_any_element();
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
                    if !this.selection.dragged {
                        this.follow_link(inst, channel, message, link, window, cx);
                    }
                });
            },
        ))
        .child(record)
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
            Link::User(user) => {
                let space = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.space_of(channel));
                self.run_action(
                    Target::Member(inst, space, user),
                    crate::menu_entries::Action::Profile,
                    window,
                    cx,
                );
            }
            Link::Spoiler | Link::Reveal => {
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
        place: FilePlace,
        a: &Attachment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if is_image(a) && self.config.buffer.image_previews {
            let (w, h) = display_size(a);
            let picture = self.images.thumbs.get(&(inst.clone(), a.id));
            if let Some(Img::Ready(image)) = picture {
                let (open_inst, attachment) = (inst.clone(), a.clone());
                let picture = div()
                    .id(SharedString::from(format!("image-{}", a.id)))
                    .relative()
                    .w(px(w))
                    .h(px(h))
                    .rounded(theme::row_radius())
                    .overflow_hidden()
                    .cursor_pointer()
                    .group(IMAGE_GROUP)
                    .child({
                        let picture = img(image.shown(self.animating()))
                            .w(px(w))
                            .h(px(h))
                            .rounded(theme::row_radius())
                            .id(SharedString::from(format!("image-pic-{}", a.id)));
                        // It just arrived: it fades in over its placeholder.
                        let just_arrived = self
                            .images
                            .thumb_arrived
                            .get(&(inst.clone(), a.id))
                            .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(400));
                        if just_arrived {
                            div()
                                .child(picture)
                                .with_animation(
                                    SharedString::from(format!("image-in-{}", a.id)),
                                    gpui_kit::Animation::new(theme::fade() * 2)
                                        .with_easing(gpui_kit::ease_out_quint()),
                                    |d, t| d.opacity(t),
                                )
                                .into_any_element()
                        } else {
                            picture.into_any_element()
                        }
                    })
                    // What opening or saving it downloads, under the mouse.
                    .child(
                        div()
                            .absolute()
                            .bottom(px(6.))
                            .right(px(6.))
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(8.))
                            .bg(gpui_kit::hsla(0., 0., 0., 0.65))
                            .text_size(theme::caption_size())
                            .text_color(gpui_kit::rgb(0xffffff))
                            .opacity(0.)
                            .group_hover(IMAGE_GROUP, |s| s.opacity(1.))
                            .child(human_size(a.size)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_image(open_inst.clone(), attachment.clone(), cx);
                    }));
                return self
                    .with_file_menu(picture, inst, place, a.id, cx)
                    .into_any_element();
            }
            let failed = matches!(picture, Some(Img::Failed));
            let text = if failed {
                "Image not available"
            } else {
                "Loading…"
            };
            // While it loads, the space it will fill breathes softly.
            let shimmer = (!failed).then(|| {
                div()
                    .absolute()
                    .inset_0()
                    .rounded(theme::row_radius())
                    .with_animation(
                        SharedString::from(format!("image-wait-{}", a.id)),
                        gpui_kit::Animation::new(std::time::Duration::from_millis(1400))
                            .repeat()
                            .with_easing(gpui_kit::pulsating_between(0., 1.)),
                        |d, t| d.bg(theme::alpha(theme::hover(), 0.6 * t)),
                    )
            });
            let waiting = div()
                .id(SharedString::from(format!("image-{}", a.id)))
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .cursor_default()
                .w(px(w))
                .h(px(h))
                .rounded(theme::row_radius())
                .bg(theme::code_background())
                .text_size(theme::caption_size())
                .text_color(theme::text_tertiary())
                .children(shimmer)
                .child(text);
            return self
                .with_file_menu(waiting, inst, place, a.id, cx)
                .into_any_element();
        }
        if crate::media::is_video(&a.content_type, &a.filename) {
            return self.render_video_card(inst, place, a, cx);
        }
        if crate::media::is_sound(&a.content_type, &a.filename) {
            return self.render_audio_card(inst, place, a, cx);
        }
        let (name, name_record) = self.selectable_file_name(inst, Some(place), &a.filename);
        let card = div()
            .id(SharedString::from(format!("file-{}", a.id)))
            .flex()
            .items_center()
            .gap(px(12.))
            .cursor_default()
            .max_w(px(420.))
            .px(px(12.))
            .py(px(8.))
            .rounded(theme::row_radius())
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
                            .relative()
                            .cursor_text()
                            .truncate()
                            .text_size(theme::secondary_size())
                            .text_color(theme::link())
                            .child(name)
                            .children(name_record),
                    )
                    .child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(human_size(a.size)),
                    ),
            )
            .child(self.download_button(inst, a, cx));
        self.with_file_menu(card, inst, place, a.id, cx)
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
            // The row itself is dimmed while on its way: the text keeps its colour.
            PendingState::Sending => (theme::text(), None),
            PendingState::Queued => (theme::text(), Some("waiting for connection")),
            PendingState::Failed(_) => (theme::danger(), Some("not sent")),
        };
        let me = s.me_id();
        let name = me.map(|id| s.display_name(None, id)).unwrap_or_default();
        let local_id: LocalId = p.local_id;
        let compact = self.config.buffer.compact;
        let time_text = time::format(
            pending.created_at,
            &self.config.buffer.timestamp_format,
            self.utc_offset,
        );
        let mut body = div().flex().flex_col().gap(px(2.)).flex_1().min_w_0();
        let content = div()
            .text_size(theme::body_size())
            .text_color(color)
            .child(pending.content.clone())
            .into_any_element();
        if compact {
            let mut head = Vec::new();
            if !p.grouped {
                if self.config.buffer.show_avatars {
                    let user = me.and_then(|id| s.user(id));
                    head.push(self.avatar(inst, user, &name, COMPACT_AVATAR));
                }
                head.push(
                    div()
                        .text_size(theme::body_size())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::role_default())
                        .child(name.clone())
                        .into_any_element(),
                );
            }
            body = body.child(compact_line(head, vec![content]));
        } else if !p.grouped {
            body = body.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_baseline()
                    .child(
                        div()
                            .text_size(theme::body_size())
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::role_default())
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(time_text.clone()),
                    ),
            );
            body = body.child(content);
        } else {
            body = body.child(content);
        }
        if let Some(note) = note {
            let mut line = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(theme::caption_size())
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
                            .cursor_pointer()
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
                            .cursor_pointer()
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
        let gutter = if compact {
            let shown = (!p.grouped).then(|| {
                div()
                    .text_size(theme::caption_size())
                    .text_color(theme::text_tertiary())
                    .child(time_text.clone())
                    .into_any_element()
            });
            compact_time_slot(&time_text, shown)
        } else if !p.grouped && self.config.buffer.show_avatars {
            let user = me.and_then(|id| s.user(id));
            self.avatar(inst, user, &name, theme::AVATAR)
        } else {
            div().into_any_element()
        };
        let (top, bottom) = crate::rows::pending_padding(p, compact);
        // On its way it is dimmed as a whole; one that failed is not, so its buttons read.
        let on_its_way = !matches!(pending.state, PendingState::Failed(_));
        div()
            .flex()
            .w_full()
            .pt(px(top))
            .pb(px(bottom))
            .px(px(16.))
            .when(on_its_way, |d| d.opacity(0.6))
            .child(
                div()
                    .flex_shrink_0()
                    .when(!compact, |d| d.w(px(GUTTER)))
                    .child(gutter),
            )
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
        .text_size(theme::secondary_size())
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
                .text_size(theme::caption_size())
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(text),
        )
        .child(div().flex_1().h(px(1.)).bg(line))
        .into_any_element()
}

/// Where the unread messages start: a red line ending in a NEW tag, apart from the day lines.
fn new_messages_divider() -> AnyElement {
    div()
        .flex()
        .items_center()
        .px(px(16.))
        .py(px(8.))
        .child(div().flex_1().h(px(1.)).bg(theme::danger()))
        .child(
            div()
                .flex_shrink_0()
                .px(px(6.))
                .rounded_r(px(4.))
                .bg(theme::danger())
                .text_size(theme::caption_size())
                .font_weight(FontWeight::BOLD)
                .text_color(theme::on_accent())
                .child("NEW"),
        )
        .into_any_element()
}

/// [`icon_button`] that says what it does when hovered.
pub fn tip_button(
    id: impl Into<gpui_kit::ElementId>,
    name: &'static str,
    tip: &'static str,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    icon_button(id, name).tooltip(move |window, cx| {
        gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
    })
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
        .rounded(theme::radius_small())
        .cursor_pointer()
        .group(ICON_BUTTON)
        .child(
            icon(name, 18., theme::text_secondary())
                .group_hover(ICON_BUTTON, |s| s.text_color(theme::accent())),
        )
}

/// The hover group of [`icon_button`]: the innermost one wins, so one name serves them all.
const ICON_BUTTON: &str = "icon-button";
/// The hover group of a picture in the chat: its size shows under the mouse.
const IMAGE_GROUP: &str = "chat-image";

/// What a reply shows of the message it answers: the message itself when it is loaded (it may
/// have been edited since), else the excerpt sent with the reply. An empty excerpt also means a
/// message with only files, so "deleted" is said only when the loaded message says so.
pub(crate) fn reply_text(loaded: Option<&ChatMessage>, excerpt: &str) -> String {
    let files = |m: &ChatMessage| {
        m.attachments
            .iter()
            .map(|a| a.filename.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let text = match loaded {
        Some(m) if m.deleted => return "original message deleted".to_owned(),
        Some(m) if m.content.trim().is_empty() && !m.attachments.is_empty() => files(m),
        Some(m) => m.content.chars().take(100).collect(),
        None if excerpt.is_empty() => return "original message".to_owned(),
        None => excerpt.to_owned(),
    };
    text.replace('\n', " ")
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
            waveform: None,
            duration_ms: None,
            sealed: None,
        }
    }

    fn message(content: &str, files: Vec<Attachment>, deleted: bool) -> ChatMessage {
        ChatMessage {
            id: Id(9 << 22),
            channel_id: Id(1),
            author: crate::fixtures::user(2, "a", None),
            content: content.into(),
            reply_to: None,
            edited_at: None,
            deleted,
            pinned: false,
            kind: hermes_client_core::proto::models::MessageKind::Default,
            attachments: files,
            reactions: vec![],
            mentions: Default::default(),
            nonce: None,
            e2ee: None,
            lock: Default::default(),
            message_key: None,
        }
    }

    #[test]
    fn a_reply_shows_the_loaded_message() {
        let edited = message("new\ntext", vec![], false);
        assert_eq!(reply_text(Some(&edited), "old"), "new text");
    }

    #[test]
    fn a_reply_to_a_picture_names_the_file_instead_of_saying_deleted() {
        let picture = message("", vec![attachment(None, None)], false);
        assert_eq!(reply_text(Some(&picture), ""), "a.png");
        assert_eq!(reply_text(None, ""), "original message");
    }

    #[test]
    fn only_a_deleted_message_is_called_deleted() {
        let gone = message("", vec![], true);
        assert_eq!(reply_text(Some(&gone), "kept"), "original message deleted");
        assert_eq!(reply_text(None, "kept"), "kept");
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
    fn forwarded_text_quotes_and_links_files() {
        assert_eq!(
            forwarded_text(
                "hi
there",
                "Bob",
                "#general",
                0,
                "hermes://x"
            ),
            "*Forwarded from Bob in #general*
> hi
> there"
        );
        assert_eq!(
            forwarded_text("", "Bob", "#general", 2, "hermes://x"),
            "*Forwarded from Bob in #general*
2 files: hermes://x"
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
