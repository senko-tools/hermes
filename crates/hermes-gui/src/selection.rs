//! Selecting chat text with the mouse. gpui has no selectable text, so each piece of text in a
//! message (a [`Part`]) notes its layout while it paints, a press on one starts a selection, and
//! window-level listeners follow the drag. A selection can span messages; what it covers is drawn
//! as a highlight behind the text.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, Context, DispatchPhase, HighlightStyle, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, StyledText, TextLayout, Window,
    canvas,
};
use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Message as ChatMessage;
use hermes_client_core::store::InstanceStore;
use hermes_client_core::time;

use crate::markup::{self, Block};
use crate::rows::{MessageRow, Row};
use crate::shell::Shell;
use crate::theme;

/// The text parts of a message, in the order they read.
pub const REPLY: u16 = 0;
pub const AUTHOR: u16 = 1;
pub const TIME: u16 = 2;
/// Text blocks follow, one each.
pub const BLOCKS: u16 = 3;
/// File names follow, one each.
pub const FILES: u16 = 2000;

/// How far the mouse has to move before a press counts as a drag.
const DRAG_START: f32 = 4.;
/// How close to the list's edge a drag starts scrolling it.
const EDGE: f32 = 32.;
/// How many pixels per frame a drag at the very edge scrolls.
const EDGE_SCROLL: f32 = 14.;

/// One piece of text in a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Part {
    pub message: Id,
    pub index: u16,
}

/// A place in the text. Spots sort in reading order, since message IDs grow with time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Spot {
    pub part: Part,
    pub byte: usize,
}

/// Where a file card is in the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilePlace {
    pub channel: Id,
    pub message: Id,
    pub index: usize,
}

impl FilePlace {
    fn part(self) -> Part {
        Part {
            message: self.message,
            index: FILES.saturating_add(self.index as u16),
        }
    }
}

/// What was painted in the last frame: where each part is, how its text is laid out, and the text.
type Laid = Rc<RefCell<BTreeMap<Part, (Bounds<Pixels>, TextLayout, SharedString)>>>;

/// What a press selects, by how many clicks it took: like a browser, one click selects by
/// letters, a double click by words, a triple click a message's whole text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Unit {
    #[default]
    Letter,
    Word,
    Message,
}

#[derive(Default)]
pub struct TextSelection {
    /// The chat the selection is in.
    chat: Option<(InstanceId, Id)>,
    anchor: Option<Spot>,
    head: Option<Spot>,
    /// The left button is down on text: where, and on which spot.
    pressed: Option<(Point<Pixels>, Spot)>,
    unit: Unit,
    /// What a double or triple click selected; a drag after it keeps it and grows from it.
    origin: Option<(Spot, Spot)>,
    dragging: bool,
    /// The last press became a drag, so its release is no click.
    pub dragged: bool,
    /// Where the mouse is during a drag.
    at: Point<Pixels>,
    laid: Laid,
    /// Where the message list was drawn, for scrolling at its edges.
    pub(crate) list: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl TextSelection {
    /// Where `part` was painted last.
    #[cfg(test)]
    pub fn bounds_of(&self, part: Part) -> Option<Bounds<Pixels>> {
        self.laid.borrow().get(&part).map(|(bounds, _, _)| *bounds)
    }

    /// The ends of the selection, first one first.
    fn ends(&self) -> Option<(Spot, Spot)> {
        let (a, b) = (self.anchor?, self.head?);
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    fn in_chat(&self, inst: &str, channel: Id) -> bool {
        self.chat
            .as_ref()
            .is_some_and(|(i, c)| i == inst && *c == channel)
    }

    pub fn dragging(&self) -> bool {
        self.dragging
    }

    /// Forgets the selection.
    fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
    }

    /// The bytes of `part`, `len` bytes long, that are selected.
    fn range_in(&self, inst: &str, channel: Id, part: Part, len: usize) -> Option<Range<usize>> {
        if !self.in_chat(inst, channel) {
            return None;
        }
        let (from, to) = self.ends()?;
        covered(from, to, part, len)
    }

    /// Whether `at` is over a letter of selectable text (not the empty space beside or under
    /// it): where the text cursor shows (#2). Uses the last frame's layout.
    pub fn over_text(&self, at: Point<Pixels>) -> bool {
        self.laid.borrow().values().any(|(bounds, layout, _)| {
            bounds.contains(&at) && layout.index_for_position(at).is_ok()
        })
    }

    /// The spot of the text under `at`, if there is any.
    fn spot_under(&self, at: Point<Pixels>) -> Option<Spot> {
        let laid = self.laid.borrow();
        let (part, (_, layout, _)) = laid.iter().find(|(_, (b, _, _))| b.contains(&at))?;
        Some(Spot {
            part: *part,
            byte: layout.index_for_position(at).unwrap_or_else(|i| i),
        })
    }

    /// The spot of the text nearest to `at`: first by line, then along it. Only in `message`
    /// when one is given.
    fn spot_nearest(&self, at: Point<Pixels>, message: Option<Id>) -> Option<Spot> {
        let laid = self.laid.borrow();
        let (part, (_, layout, _)) = laid
            .iter()
            .filter(|(part, _)| message.is_none_or(|m| part.message == m))
            .min_by(|(_, (a, _, _)), (_, (b, _, _))| {
                let (a, b) = (distance(*a, at), distance(*b, at));
                a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
            })?;
        Some(Spot {
            part: *part,
            byte: layout.index_for_position(at).unwrap_or_else(|i| i),
        })
    }

    /// The `unit` around `spot`: its word, or its message's whole text.
    fn unit_around(&self, spot: Spot, unit: Unit) -> (Spot, Spot) {
        let laid = self.laid.borrow();
        let at = |part: Part, byte: usize| Spot { part, byte };
        match unit {
            Unit::Letter => (spot, spot),
            Unit::Word => match laid.get(&spot.part) {
                Some((_, _, text)) => {
                    let word = word_at(text, spot.byte);
                    (at(spot.part, word.start), at(spot.part, word.end))
                }
                None => (spot, spot),
            },
            Unit::Message => {
                let is_block = |p: &Part| (BLOCKS..FILES).contains(&p.index);
                let whole = |part: Part| {
                    let len = laid.get(&part).map_or(0, |(_, _, text)| text.len());
                    (at(part, 0), at(part, len))
                };
                if !is_block(&spot.part) {
                    // A name, a time or a file name: just that.
                    return whole(spot.part);
                }
                let mut blocks = laid
                    .keys()
                    .filter(|p| p.message == spot.part.message && is_block(p));
                let first = blocks.next().copied().unwrap_or(spot.part);
                let last = blocks.next_back().copied().unwrap_or(first);
                (whole(first).0, whole(last).1)
            }
        }
    }

    /// Moves the head to the text nearest `at`, by the unit the press chose.
    fn extend_to(&mut self, at: Point<Pixels>) {
        let Some(nearest) = self.spot_nearest(at, None) else {
            return;
        };
        match self.origin {
            Some((start, end)) if self.unit != Unit::Letter => {
                let (from, to) = self.unit_around(nearest, self.unit);
                if nearest < start {
                    self.anchor = Some(end);
                    self.head = Some(from);
                } else {
                    self.anchor = Some(start);
                    self.head = Some(to.max(end));
                }
            }
            _ => self.head = Some(nearest),
        }
    }
}

/// How a character counts when a double click looks for a word.
#[derive(PartialEq)]
enum Kind {
    Word,
    Space,
    Other,
}

fn kind(c: char) -> Kind {
    if c.is_alphanumeric() || c == '_' {
        Kind::Word
    } else if c.is_whitespace() {
        Kind::Space
    } else {
        Kind::Other
    }
}

/// What a double click at `byte` selects in `text`: the word there, with apostrophes and dots
/// inside it (`don't`, `3.14`); a run of spaces; or a single other character, such as a comma.
fn word_at(text: &str, byte: usize) -> Range<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    if chars.is_empty() {
        return 0..0;
    }
    let byte = byte.min(text.len());
    // The character starting at `byte`, or the one before it at the end of the text.
    let mut i = chars
        .iter()
        .position(|(at, _)| *at >= byte)
        .unwrap_or(chars.len() - 1);
    // A press just after a word (on its right half, or before the space or comma after it) means
    // that word, as in a browser.
    if i > 0 && kind(chars[i].1) != Kind::Word && kind(chars[i - 1].1) == Kind::Word {
        i -= 1;
    }
    let end_of = |i: usize| chars.get(i + 1).map_or(text.len(), |(at, _)| *at);
    match kind(chars[i].1) {
        Kind::Other => chars[i].0..end_of(i),
        Kind::Space => {
            let (mut from, mut to) = (i, i);
            while from > 0 && kind(chars[from - 1].1) == Kind::Space {
                from -= 1;
            }
            while to + 1 < chars.len() && kind(chars[to + 1].1) == Kind::Space {
                to += 1;
            }
            chars[from].0..end_of(to)
        }
        Kind::Word => {
            let joins = |c: char| matches!(c, '\'' | '\u{2019}' | '.');
            let word = |j: usize| chars.get(j).is_some_and(|(_, c)| kind(*c) == Kind::Word);
            let (mut from, mut to) = (i, i);
            loop {
                if from > 0 && word(from - 1) {
                    from -= 1;
                } else if from > 1 && joins(chars[from - 1].1) && word(from - 2) {
                    from -= 2;
                } else {
                    break;
                }
            }
            loop {
                if word(to + 1) {
                    to += 1;
                } else if to + 2 < chars.len() && joins(chars[to + 1].1) && word(to + 2) {
                    to += 2;
                } else {
                    break;
                }
            }
            chars[from].0..end_of(to)
        }
    }
}

/// How far `at` is from `bounds`, vertically and horizontally.
fn distance(bounds: Bounds<Pixels>, at: Point<Pixels>) -> (f32, f32) {
    let away = |low: f32, high: f32, x: f32| (low - x).max(x - high).max(0.);
    (
        away(
            bounds.top().as_f32(),
            bounds.bottom().as_f32(),
            at.y.as_f32(),
        ),
        away(
            bounds.left().as_f32(),
            bounds.right().as_f32(),
            at.x.as_f32(),
        ),
    )
}

/// What a selection from `from` to `to` covers of `part`, which is `len` bytes long.
fn covered(from: Spot, to: Spot, part: Part, len: usize) -> Option<Range<usize>> {
    if part < from.part || part > to.part {
        return None;
    }
    let start = if part == from.part { from.byte } else { 0 };
    let end = if part == to.part { to.byte } else { len };
    (start.min(len) < end.min(len)).then(|| start.min(len)..end.min(len))
}

/// Splits sorted, non-overlapping `highlights` around `selected`, since gpui cannot overlap them.
/// `mark` makes the style of selected bytes from the style they had, if any.
fn split_highlights<S: Clone>(
    highlights: Vec<(Range<usize>, S)>,
    selected: Range<usize>,
    mark: impl Fn(Option<&S>) -> S,
) -> Vec<(Range<usize>, S)> {
    let mut out = Vec::with_capacity(highlights.len() + 3);
    let mut done = 0;
    let unstyled = |out: &mut Vec<(Range<usize>, S)>, from: usize, to: usize| {
        let (start, end) = (from.max(selected.start), to.min(selected.end));
        if start < end {
            out.push((start..end, mark(None)));
        }
    };
    for (range, style) in highlights {
        unstyled(&mut out, done, range.start);
        done = range.end;
        let (start, end) = (range.start.max(selected.start), range.end.min(selected.end));
        if start >= end {
            out.push((range, style));
            continue;
        }
        if range.start < start {
            out.push((range.start..start, style.clone()));
        }
        out.push((start..end, mark(Some(&style))));
        if end < range.end {
            out.push((end..range.end, style));
        }
    }
    unstyled(&mut out, done, selected.end);
    out
}

/// A block's text as it is drawn, and which bytes of it are not to be copied.
fn block_text(block: &Block, edited: bool, revealed: bool) -> (String, Vec<Range<usize>>) {
    let mut text = String::new();
    let mut hidden = Vec::new();
    match block {
        Block::Code { code, .. } => text.push_str(code),
        Block::Text(pieces) | Block::Quote(pieces) => {
            for piece in pieces {
                let start = text.len();
                text.push_str(&piece.text);
                if piece.spoiler && !revealed {
                    hidden.push(start..text.len());
                }
            }
            if edited && matches!(block, Block::Text(_)) {
                let start = text.len();
                text.push_str(" (edited)");
                hidden.push(start..text.len());
            }
        }
    }
    (text, hidden)
}

/// `text[range]` without the `hidden` bytes.
fn visible(text: &str, range: Range<usize>, hidden: &[Range<usize>]) -> String {
    let mut out = String::new();
    let mut at = range.start;
    for skip in hidden {
        let (start, end) = (skip.start.max(at), skip.end.min(range.end));
        if start >= end {
            continue;
        }
        out.push_str(&text[at..start]);
        at = end;
    }
    out.push_str(&text[at..range.end]);
    out
}

impl Shell {
    /// A piece of selectable text and what has to be drawn next to it (in a relative parent) so
    /// that the mouse can find it.
    pub fn selectable(
        &self,
        inst: &str,
        channel: Id,
        part: Part,
        text: impl Into<SharedString>,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
    ) -> (StyledText, AnyElement) {
        let text: SharedString = text.into();
        let highlights = match self
            .selection
            .range_in(inst, channel, part, text.len())
            .map(|r| r.start..r.end)
        {
            Some(selected) => {
                let color = theme::alpha(theme::accent(), 0.4);
                split_highlights(highlights, selected, |style| HighlightStyle {
                    background_color: Some(color.into()),
                    ..style.copied().unwrap_or_default()
                })
            }
            None => highlights,
        };
        let styled = StyledText::new(text.clone()).with_highlights(highlights);
        let layout = styled.layout().clone();
        let laid = self.selection.laid.clone();
        // While painting: only what is really on screen has a finished layout.
        let record = canvas(
            |_, _, _| (),
            move |bounds, (), _, _| {
                laid.borrow_mut()
                    .insert(part, (bounds, layout.clone(), text.clone()));
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element();
        (styled, record)
    }

    /// A file's name as selectable text, and its recorder. Without `place` (the full view) it is
    /// plain text.
    pub fn selectable_file_name(
        &self,
        inst: &str,
        place: Option<FilePlace>,
        name: &str,
    ) -> (AnyElement, Option<AnyElement>) {
        match place {
            Some(place) => {
                let (styled, record) =
                    self.selectable(inst, place.channel, place.part(), name.to_owned(), vec![]);
                (styled.into_any_element(), Some(record))
            }
            None => (SharedString::from(name.to_owned()).into_any_element(), None),
        }
    }

    /// Notes where the message list is drawn, in a relative parent.
    pub fn selection_list_marker(&self) -> AnyElement {
        let list = self.selection.list.clone();
        canvas(move |bounds, _, _| list.set(Some(bounds)), |_, (), _, _| {})
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .into_any_element()
    }

    /// A press in `message`: it may become a selection, from the text under it or, in the row's
    /// empty space, from the message's nearest text. A double click selects the word there and a
    /// triple click the message's whole text, as in a browser.
    pub fn text_press(
        &mut self,
        inst: &InstanceId,
        channel: Id,
        message: Id,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let at = event.position;
        // A press on a file (its play button, its seek bar) is the file's, unless on its name.
        let on_file = self.hovered_file == Some(message);
        let selection = &mut self.selection;
        let spot = selection.spot_under(at).or_else(|| {
            (!on_file)
                .then(|| selection.spot_nearest(at, Some(message)))
                .flatten()
        });
        selection.pressed = spot.map(|spot| (at, spot));
        selection.origin = None;
        selection.unit = match event.click_count {
            0 | 1 => Unit::Letter,
            2 => Unit::Word,
            _ => Unit::Message,
        };
        let Some(spot) = spot else {
            return;
        };
        selection.chat = Some((inst.clone(), channel));
        if selection.unit != Unit::Letter {
            let (from, to) = selection.unit_around(spot, selection.unit);
            selection.anchor = Some(from);
            selection.head = Some(to);
            selection.origin = Some((from, to));
            selection.at = at;
            // Selected already, so this press is no click, and moving grows it at once.
            selection.dragging = true;
            selection.dragged = true;
        }
        cx.notify();
    }

    /// While there is a selection or a press: listens to the mouse in the whole window.
    pub fn selection_catcher(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let selection = &self.selection;
        if selection.anchor.is_none() && selection.pressed.is_none() && !selection.dragged {
            return None;
        }
        let this = cx.entity().downgrade();
        Some(
            canvas(
                |_, _, _| (),
                move |_, (), window, _| {
                    let pressed = this.clone();
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                            let _ = pressed.update(cx, |this, cx| {
                                this.selection.clear();
                                this.selection.dragged = false;
                                cx.notify();
                            });
                        }
                    });
                    let moved = this.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            let _ = moved.update(cx, |this, cx| {
                                this.selection_move(event, cx);
                            });
                        }
                    });
                    let released = this.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                            let _ = released.update(cx, |this, cx| this.selection_end(cx));
                        }
                    });
                },
            )
            .absolute()
            .size_0()
            .into_any_element(),
        )
    }

    fn selection_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let selection = &mut self.selection;
        let Some((start, spot)) = selection.pressed else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.selection_end(cx);
            return;
        }
        selection.at = event.position;
        if !selection.dragging {
            let moved = (event.position.x - start.x)
                .as_f32()
                .hypot((event.position.y - start.y).as_f32());
            if moved < DRAG_START * theme::scale() {
                return;
            }
            selection.dragging = true;
            selection.dragged = true;
            selection.anchor = Some(spot);
        }
        selection.extend_to(event.position);
        cx.notify();
    }

    fn selection_end(&mut self, cx: &mut Context<Self>) {
        self.selection.pressed = None;
        self.selection.dragging = false;
        cx.notify();
    }

    /// Before each draw: a drag at the list's edge scrolls it. Then forgets the last frame's text.
    pub fn advance_selection(&mut self, window: &mut Window) {
        if self.selection.dragging
            && let Some(bounds) = self.selection.list.get()
            && let Some(view) = self
                .selection
                .chat
                .as_ref()
                .and_then(|key| self.chats.get(key))
        {
            let y = self.selection.at.y.as_f32();
            let edge = EDGE * theme::scale();
            let above = bounds.top().as_f32() + edge - y;
            let below = y - (bounds.bottom().as_f32() - edge);
            let scroll = if above > 0. {
                -(above / edge).min(1.)
            } else if below > 0. {
                (below / edge).min(1.)
            } else {
                0.
            };
            if scroll != 0. {
                view.list.scroll_by(gpui_kit::px(scroll * EDGE_SCROLL));
                let at = self.selection.at;
                self.selection.extend_to(at);
                window.request_animation_frame();
            }
        }
        self.selection.laid.borrow_mut().clear();
    }

    /// The selected text, ready for the clipboard.
    pub fn selected_text(&self) -> Option<String> {
        let selection = &self.selection;
        let (from, to) = selection.ends()?;
        let (inst, channel) = selection.chat.clone()?;
        let view = self.chats.get(&(inst.clone(), channel))?;
        let store = self.core.store();
        let s = store.instance(&inst)?;
        let window = s.window(channel)?;
        let mut out = String::new();
        let mut previous: Option<Part> = None;
        for row in &view.rows {
            let Row::Message(m) = row else { continue };
            if m.id < from.part.message || m.id > to.part.message {
                continue;
            }
            let Some(msg) = window.get(m.id) else {
                continue;
            };
            let revealed = view.revealed.contains(&m.id);
            for (index, text, hidden) in self.message_parts(s, channel, m, msg, revealed) {
                let part = Part {
                    message: m.id,
                    index,
                };
                let Some(range) = covered(from, to, part, text.len()) else {
                    continue;
                };
                let snippet = visible(&text, range, &hidden);
                if snippet.is_empty() {
                    continue;
                }
                if let Some(previous) = previous {
                    let same_line = previous.message == part.message
                        && previous.index == AUTHOR
                        && part.index == TIME;
                    out.push(if same_line { ' ' } else { '\n' });
                }
                out.push_str(&snippet);
                previous = Some(part);
            }
        }
        (!out.is_empty()).then_some(out)
    }

    /// Every selectable text of a message, drawn the same way as in the chat.
    fn message_parts(
        &self,
        s: &InstanceStore,
        channel: Id,
        m: &MessageRow,
        msg: &ChatMessage,
        revealed: bool,
    ) -> Vec<(u16, String, Vec<Range<usize>>)> {
        let space = s.space_of(channel);
        let mut parts = Vec::new();
        if let Some(reply) = &m.reply {
            let loaded = s.window(channel).and_then(|w| w.get(reply.id));
            let excerpt = crate::channel::reply_text(loaded, &reply.excerpt);
            parts.push((REPLY, excerpt, Vec::new()));
        }
        if !m.grouped {
            parts.push((AUTHOR, s.display_name(space, msg.author.id), Vec::new()));
            let time_text = time::format(
                msg.created_at() as i64,
                &self.config.buffer.timestamp_format,
                self.utc_offset,
            );
            parts.push((TIME, time_text, Vec::new()));
        }
        let blocks = markup::blocks(&msg.content, s, space);
        let last = blocks.len().saturating_sub(1);
        for (i, block) in blocks.iter().enumerate() {
            let edited = i == last && msg.edited_at.is_some();
            let (text, hidden) = block_text(block, edited, revealed);
            parts.push((BLOCKS + i as u16, text, hidden));
        }
        for (i, attachment) in msg.attachments.iter().enumerate() {
            parts.push((
                FILES.saturating_add(i as u16),
                attachment.filename.clone(),
                Vec::new(),
            ));
        }
        parts
    }

    /// Ctrl+C with text selected. `false` if nothing is selected.
    pub fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        match self.selected_text() {
            Some(text) => {
                self.copy(text, "Text", cx);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::Piece;

    fn part(message: u64, index: u16) -> Part {
        Part {
            message: Id(message),
            index,
        }
    }

    fn spot(message: u64, index: u16, byte: usize) -> Spot {
        Spot {
            part: part(message, index),
            byte,
        }
    }

    #[test]
    fn a_selection_covers_whole_parts_between_its_ends() {
        let (from, to) = (spot(1, BLOCKS, 3), spot(3, BLOCKS, 2));
        assert_eq!(covered(from, to, part(1, BLOCKS), 10), Some(3..10));
        assert_eq!(covered(from, to, part(2, AUTHOR), 5), Some(0..5));
        assert_eq!(covered(from, to, part(3, BLOCKS), 10), Some(0..2));
        assert_eq!(covered(from, to, part(1, AUTHOR), 5), None);
        assert_eq!(covered(from, to, part(4, BLOCKS), 5), None);
    }

    #[test]
    fn a_selection_within_one_part_and_past_its_end() {
        let (from, to) = (spot(1, BLOCKS, 2), spot(1, BLOCKS, 20));
        assert_eq!(covered(from, to, part(1, BLOCKS), 5), Some(2..5));
        let empty = spot(1, BLOCKS, 2);
        assert_eq!(covered(empty, empty, part(1, BLOCKS), 5), None);
    }

    /// Marked bytes are 10 plus their old style, or 99 where they had none.
    fn split(
        highlights: &[(Range<usize>, u32)],
        selected: Range<usize>,
    ) -> Vec<(Range<usize>, u32)> {
        split_highlights(highlights.to_vec(), selected, |s| s.map_or(99, |s| s + 10))
    }

    #[test]
    fn highlights_split_around_the_selection() {
        assert_eq!(
            split(&[(0..4, 1), (4..8, 2)], 2..6),
            vec![(0..2, 1), (2..4, 11), (4..6, 12), (6..8, 2)]
        );
    }

    #[test]
    fn unstyled_text_gets_a_highlight_of_its_own() {
        assert_eq!(split(&[], 2..5), vec![(2..5, 99)]);
        assert_eq!(
            split(&[(3..4, 1)], 0..6),
            vec![(0..3, 99), (3..4, 11), (4..6, 99)]
        );
    }

    #[test]
    fn highlights_outside_the_selection_stay() {
        assert_eq!(split(&[(0..2, 1)], 5..7), vec![(0..2, 1), (5..7, 99)]);
    }

    #[test]
    fn hidden_spoilers_and_the_edited_mark_are_not_copied() {
        let pieces = vec![
            Piece {
                text: "a ".into(),
                ..Piece::default()
            },
            Piece {
                text: "secret".into(),
                spoiler: true,
                ..Piece::default()
            },
            Piece {
                text: " b".into(),
                ..Piece::default()
            },
        ];
        let block = Block::Text(pieces);
        let (text, hidden) = block_text(&block, true, false);
        assert_eq!(text, "a secret b (edited)");
        assert_eq!(visible(&text, 0..text.len(), &hidden), "a  b");
        let (text, hidden) = block_text(&block, false, true);
        assert_eq!(visible(&text, 0..text.len(), &hidden), "a secret b");
        assert_eq!(visible(&text, 3..5, &hidden), "ec");
    }

    /// The word a double click at the first `|` in `marked` selects.
    fn word(marked: &str) -> String {
        let at = marked.find('|').unwrap();
        let text = marked.replacen('|', "", 1);
        text[word_at(&text, at)].to_owned()
    }

    #[test]
    fn a_double_click_selects_a_word() {
        assert_eq!(word("|hello there, friend"), "hello");
        assert_eq!(word("hello th|ere, friend"), "there");
        assert_eq!(word("hello there|, friend"), "there");
        assert_eq!(word("hello there,| friend"), " ");
        assert_eq!(word("hello there, frien|d"), "friend");
        assert_eq!(word("hello there, friend|"), "friend");
        assert_eq!(word("oisko visio jos teikäl|äine latais"), "teikäläine");
        assert_eq!(word("don|'t stop"), "don't");
        assert_eq!(word("pi is 3|.14."), "3.14");
        assert_eq!(word("a  |   b"), "     ");
        assert_eq!(word("wow |(nice)"), "(");
        assert_eq!(word("springing-it-spring|trap"), "springtrap");
        assert_eq!(word("snake_ca|se"), "snake_case");
        assert_eq!(word("|"), "");
    }

    #[test]
    fn distance_is_zero_inside() {
        let bounds = Bounds::new(
            gpui_kit::point(gpui_kit::px(10.), gpui_kit::px(10.)),
            gpui_kit::size(gpui_kit::px(20.), gpui_kit::px(20.)),
        );
        let at = |x, y| gpui_kit::point(gpui_kit::px(x), gpui_kit::px(y));
        assert_eq!(distance(bounds, at(15., 15.)), (0., 0.));
        assert_eq!(distance(bounds, at(0., 15.)), (0., 10.));
        assert_eq!(distance(bounds, at(15., 40.)), (10., 0.));
    }
}
