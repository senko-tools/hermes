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
    SharedString, StrikethroughStyle, Subscription, UnderlineStyle, Window, canvas, div, img, list,
    rgba,
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

mod attachment_render;
mod chat_render;
mod composer;
mod composer_render;
mod files;
mod list;
mod markup_render;
mod message_actions;
mod message_render;
mod open;
mod viewer;

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
    /// My read marker when the channel was opened (the divider position). While I watch the
    /// newest messages it follows them, so what I see arrive gets no divider.
    pub read_marker: Option<Id>,
    /// The "new messages" divider has been on screen this visit: no button leads to it then.
    pub divider_seen: std::rc::Rc<std::cell::Cell<bool>>,
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
/// A video whose sender sent a picture of it: shown before it loads (#8).
pub fn has_poster(a: &Attachment) -> bool {
    crate::media::is_video(&a.content_type, &a.filename)
        && (a.has_thumb || a.sealed.as_ref().is_some_and(|s| s.thumb.is_some()))
}

pub fn is_image(a: &Attachment) -> bool {
    // An SVG has no thumbnail: its original is drawn (#5).
    crate::images::is_svg(&a.content_type, &a.filename)
        || ((a.has_thumb || a.sealed.is_some()) && a.content_type.starts_with("image/"))
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

/// A day divider's text: the date in `[buffer] date_format`. `day` counts days since
/// 1970-01-01 (local).
pub fn day_label(day: i64, format: &str) -> String {
    time::format(day * 86_400_000, format, 0)
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
pub fn grow_in(heights: &GrowHeights, key: SharedString, element: AnyElement) -> AnyElement {
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
            // The padding above and below a message depends on its group.
            (m.grouped, m.ends_group).hash(&mut hash);
            m.next_starts_with_file.hash(&mut hash);
            if let Some(message) = window.and_then(|w| w.get(m.id)) {
                message.content.hash(&mut hash);
                message.edited_at.hash(&mut hash);
                message.attachments.len().hash(&mut hash);
                for r in &message.reactions {
                    (&r.emoji, r.count).hash(&mut hash);
                }
            }
        }
        Row::Pending(p) => {
            (p.grouped, p.ends_group).hash(&mut hash);
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

/// A day divider: the date centred between two hairlines. The lines start where message text
/// starts (past the avatar column) and stop as far from the right edge, so they sit inside the
/// chat's content and the date stays centred in the chat whatever the messages' widths.
fn divider(text: String, color: gpui_kit::Rgba, line: gpui_kit::Rgba) -> AnyElement {
    // A list row is as wide as its content unless it says otherwise: without the full width
    // the two lines (which only fill what is left) would have none.
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(16. + GUTTER))
        .py(px(8.))
        .child(div().flex_1().h(px(1.)).bg(line))
        .child(
            div()
                .flex_shrink_0()
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
        .w_full()
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

/// The composer's "X/Y" counter, shown only near the limit (the last tenth of it), and whether
/// the text is over the limit.
fn length_counter(len: usize, max: usize) -> Option<(String, bool)> {
    (max > 0 && len >= max - max / 10).then(|| (format!("{len}/{max}"), len > max))
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
            author: hermes_client_core::testing::fixtures::user(2, "a", None),
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
    fn the_length_counter_shows_near_the_limit() {
        assert_eq!(length_counter(0, 4000), None);
        assert_eq!(length_counter(3599, 4000), None);
        assert_eq!(
            length_counter(3600, 4000),
            Some(("3600/4000".into(), false))
        );
        assert_eq!(
            length_counter(4000, 4000),
            Some(("4000/4000".into(), false))
        );
        assert_eq!(length_counter(4001, 4000), Some(("4001/4000".into(), true)));
        assert_eq!(length_counter(5, 0), None);
    }
}
