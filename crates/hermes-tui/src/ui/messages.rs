//! The message pane: header, day separators, grouped messages with markup, replies,
//! attachments, reactions, pending messages, and the typing line.

use hermes_client_core::now_ms;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::markup::{self, Span as M};
use hermes_client_core::proto::models::{Attachment, Message, MessageKind};
use hermes_client_core::store::{InstanceStore, PendingMessage, PendingState};
use hermes_client_core::view;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{bold, border_style, dim, focused, width};
use crate::app::{App, Focus};
use hermes_client_core::time;

/// Styled text pieces of one logical line.
type Pieces = Vec<(String, Style)>;

struct Ctx<'a> {
    store: &'a InstanceStore,
    space: Option<Id>,
}

/// Converts markup into logical lines of styled pieces.
fn render_markup(ctx: &Ctx, content: &str, base: Style) -> Vec<Pieces> {
    let mut lines: Vec<Pieces> = vec![Vec::new()];
    for span in markup::parse(content) {
        render_span(ctx, &span, base, &mut lines);
    }
    lines
}

fn push(lines: &mut Vec<Pieces>, text: &str, style: Style) {
    let mut first = true;
    for part in text.split('\n') {
        if !first {
            lines.push(Vec::new());
        }
        first = false;
        if !part.is_empty() {
            lines
                .last_mut()
                .expect("never empty")
                .push((part.to_owned(), style));
        }
    }
}

/// Starts a new line unless the current one is empty.
fn block_break(lines: &mut Vec<Pieces>) {
    if lines.last().is_some_and(|l| !l.is_empty()) {
        lines.push(Vec::new());
    }
}

fn render_span(ctx: &Ctx, span: &M, style: Style, lines: &mut Vec<Pieces>) {
    let children = |spans: &[M], style: Style, lines: &mut Vec<Pieces>| {
        for s in spans {
            render_span(ctx, s, style, lines);
        }
    };
    match span {
        M::Text(t) => push(lines, t, style),
        M::LineBreak => lines.push(Vec::new()),
        M::Bold(s) => children(s, style.add_modifier(Modifier::BOLD), lines),
        M::Italic(s) => children(s, style.add_modifier(Modifier::ITALIC), lines),
        M::Underline(s) => children(s, style.add_modifier(Modifier::UNDERLINED), lines),
        M::Strike(s) => children(s, style.add_modifier(Modifier::CROSSED_OUT), lines),
        M::Spoiler(s) => {
            // Hidden: show blocks of the same length.
            let mut inner = vec![Vec::new()];
            children(s, style, &mut inner);
            let len: usize = inner.iter().flatten().map(|(t, _)| t.chars().count()).sum();
            push(lines, &"▒".repeat(len.max(3)), style.fg(Color::DarkGray));
        }
        M::Code(code) => push(lines, code, style.fg(Color::Yellow)),
        M::CodeBlock { code, .. } => {
            block_break(lines);
            for line in code.trim_end_matches('\n').split('\n') {
                lines
                    .last_mut()
                    .expect("never empty")
                    .push((format!("│ {line}"), style.fg(Color::Yellow)));
                lines.push(Vec::new());
            }
        }
        M::Quote(s) => {
            block_break(lines);
            let mut inner = vec![Vec::new()];
            children(s, style.fg(Color::Gray), &mut inner);
            for line in inner {
                let mut l = vec![("▎ ".to_owned(), dim())];
                l.extend(line);
                lines.last_mut().expect("never empty").extend(l);
                lines.push(Vec::new());
            }
        }
        M::Link { url, children: c } => {
            let mut inner = vec![Vec::new()];
            children(c, style, &mut inner);
            let text: String = inner.iter().flatten().map(|(t, _)| t.as_str()).collect();
            let link = style.fg(Color::Blue).add_modifier(Modifier::UNDERLINED);
            if text == *url {
                push(lines, url, link);
            } else {
                push(lines, &text, link);
                push(lines, &format!(" ({url})"), dim());
            }
        }
        M::UserMention(id) => {
            let name = ctx.store.display_name(ctx.space, *id);
            push(lines, &format!("@{name}"), style.fg(Color::Magenta));
        }
        M::RoleMention(id) => {
            let name = view::role_mention_name(ctx.store, ctx.space, *id);
            push(lines, &format!("@{name}"), style.fg(Color::Magenta));
        }
        M::ChannelMention(id) => {
            let name = view::channel_mention_name(ctx.store, *id);
            push(lines, &format!("#{name}"), style.fg(Color::Cyan));
        }
        M::Everyone => push(lines, "@everyone", style.fg(Color::Magenta)),
        M::Here => push(lines, "@here", style.fg(Color::Magenta)),
    }
}

fn attachment_line(i: usize, a: &Attachment) -> Pieces {
    let text = match (a.width, a.height) {
        (Some(w), Some(h)) => format!("[{}] [image: {} {w}×{h}]", i + 1, a.filename),
        _ => format!(
            "[{}] [file: {} {}]",
            i + 1,
            a.filename,
            view::human_size(a.size)
        ),
    };
    vec![(text, Style::default().fg(Color::Blue))]
}

/// Wraps styled pieces to `width` cells (by character).
fn wrap(pieces: &Pieces, width: usize) -> Vec<Pieces> {
    let width = width.max(1);
    let mut out: Vec<Pieces> = vec![Vec::new()];
    let mut used = 0;
    for (text, style) in pieces {
        let mut current = String::new();
        for c in text.chars() {
            let w = super::width(c.encode_utf8(&mut [0; 4]));
            if used + w > width && used > 0 {
                if !current.is_empty() {
                    out.last_mut()
                        .expect("never empty")
                        .push((std::mem::take(&mut current), *style));
                }
                out.push(Vec::new());
                used = 0;
            }
            current.push(c);
            used += w;
        }
        if !current.is_empty() {
            out.last_mut().expect("never empty").push((current, *style));
        }
    }
    out
}

struct Layout<'a> {
    ctx: Ctx<'a>,
    width: usize,
    name_width: usize,
    time_format: String,
    offset: i64,
    group_minutes: u32,
}

impl Layout<'_> {
    fn prefix_width(&self) -> usize {
        5 + 1 + self.name_width + 2
    }

    /// Lays out `body` lines behind the `time name` prefix.
    fn compose(
        &self,
        time: &str,
        name: Option<(&str, Style)>,
        body: Vec<Pieces>,
        highlight: Option<Style>,
    ) -> Vec<Line<'static>> {
        let prefix_w = self.prefix_width();
        let text_w = self.width.saturating_sub(prefix_w).max(10);
        let mut lines = Vec::new();
        let mut first = true;
        for logical in body {
            for wrapped in wrap(&logical, text_w) {
                let mut spans: Vec<Span<'static>> = Vec::new();
                if first {
                    let t = format!("{time:<5} ");
                    spans.push(Span::styled(t, dim()));
                    let (n, style) = name.unwrap_or(("", Style::default()));
                    let pad = self.name_width.saturating_sub(width(n));
                    spans.push(Span::styled(n.to_owned(), style));
                    spans.push(Span::raw(" ".repeat(pad + 2)));
                    first = false;
                } else {
                    spans.push(Span::raw(" ".repeat(prefix_w)));
                }
                spans.extend(wrapped.into_iter().map(|(t, s)| Span::styled(t, s)));
                let mut line = Line::from(spans);
                if let Some(h) = highlight {
                    line = line.style(h);
                }
                lines.push(line);
            }
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(format!("{time:<5}"), dim())));
        }
        lines
    }

    fn author_style(&self, author: Id) -> Style {
        let color = self
            .ctx
            .space
            .and_then(|space| view::role_color(self.ctx.store, space, author))
            .map(|c| Color::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8));
        bold().fg(color.unwrap_or(Color::Green))
    }

    fn message(&self, m: &Message, grouped: bool, highlight: Option<Style>) -> Vec<Line<'static>> {
        let time = time::format(m.created_at() as i64, &self.time_format, self.offset);
        let name = self.ctx.store.display_name(self.ctx.space, m.author.id);
        let mut body: Vec<Pieces> = Vec::new();
        if let Some(reply) = &m.reply_to {
            let who = self.ctx.store.display_name(self.ctx.space, reply.author_id);
            let excerpt: String = reply.excerpt.chars().take(60).collect();
            body.push(vec![(format!("↪ {who}: {excerpt}"), dim())]);
        }
        if m.deleted {
            body.push(vec![(
                "(message deleted)".into(),
                dim().add_modifier(Modifier::ITALIC),
            )]);
        } else {
            let base = if m.kind == MessageKind::System {
                dim().add_modifier(Modifier::ITALIC)
            } else {
                Style::default()
            };
            if !m.content.is_empty() {
                let mut content = render_markup(&self.ctx, &m.content, base);
                while content.last().is_some_and(Vec::is_empty) && content.len() > 1 {
                    content.pop();
                }
                body.extend(content);
            }
            for (i, a) in m.attachments.iter().enumerate() {
                body.push(attachment_line(i, a));
            }
            let mut tail: Pieces = Vec::new();
            if m.edited_at.is_some() {
                tail.push((" (edited)".into(), dim()));
            }
            for r in &m.reactions {
                let style = if r.me {
                    bold().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::Gray)
                };
                tail.push((format!(" [{} {}]", r.emoji, r.count), style));
            }
            if m.pinned {
                tail.push((" 📌".into(), Style::default()));
            }
            if !tail.is_empty() {
                match body.last_mut() {
                    Some(last) => last.extend(tail),
                    None => body.push(tail),
                }
            }
        }
        let name = (!grouped).then(|| (name.as_str(), self.author_style(m.author.id)));
        self.compose(&time, name, body, highlight)
    }

    fn pending(&self, p: &PendingMessage, highlight: Option<Style>) -> Vec<Line<'static>> {
        let time = time::format(p.created_at, &self.time_format, self.offset);
        let me = self.ctx.store.me_id();
        let name = me.map_or_else(String::new, |id| {
            self.ctx.store.display_name(self.ctx.space, id)
        });
        let style = match &p.state {
            PendingState::Failed(_) => Style::default().fg(Color::Red),
            _ => dim(),
        };
        let mut body = render_markup(&self.ctx, &p.content, style);
        for (i, a) in p.attachments.iter().enumerate() {
            body.push(attachment_line(i, a));
        }
        let note = match &p.state {
            PendingState::Queued => " (waiting for connection)".to_owned(),
            PendingState::Sending => " (sending…)".to_owned(),
            PendingState::Failed(reason) => format!(" ✗ {reason} — R retry, x delete"),
        };
        body.last_mut()
            .expect("render_markup returns a line")
            .push((note, style.add_modifier(Modifier::ITALIC)));
        self.compose(
            &time,
            Some((&name, style.add_modifier(Modifier::BOLD))),
            body,
            highlight,
        )
    }
}

/// The `height` lines on screen: `before` (the line above the oldest entry), then entries
/// `0..count` as laid out by `render`. With an `anchor` the view keeps that entry fully visible,
/// as low as possible; without one it shows the newest lines.
///
/// Only what can be on screen is laid out (a window holds hundreds of messages, and this runs
/// every frame): from the anchor, or the newest entry, upwards until the pane is full, then below
/// the anchor while it is still not full.
fn visible_lines<T>(
    count: usize,
    anchor: Option<usize>,
    height: usize,
    before: Vec<T>,
    mut render: impl FnMut(usize) -> Vec<T>,
) -> Vec<T> {
    let end = anchor.map_or(count, |a| a + 1);
    let mut chunks = Vec::new();
    let mut above = 0;
    let mut first = end;
    while first > 0 && above < height {
        first -= 1;
        let lines = render(first);
        above += lines.len();
        chunks.push(lines);
    }
    // `all` is a contiguous part of the full list of lines; when it does not start at the top,
    // it holds at least `height` lines up to the anchor's end, so the view comes out the same.
    let mut all = Vec::with_capacity(above + height + 1);
    if first == 0 {
        all.extend(before);
    }
    let mut anchor_range = None;
    for (k, lines) in chunks.into_iter().rev().enumerate() {
        let start = all.len();
        all.extend(lines);
        if anchor == Some(first + k) {
            anchor_range = Some((start, all.len()));
        }
    }
    let mut next = end;
    while next < count && all.len() < height {
        all.extend(render(next));
        next += 1;
    }
    let top = match anchor_range {
        Some((start, end)) => {
            let bottom = end.max(height.min(all.len()));
            bottom.saturating_sub(height).min(start)
        }
        None => all.len().saturating_sub(height),
    };
    all.into_iter().skip(top).take(height).collect()
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let has_focus = focused(app, Focus::Messages);
    let store = app.core.store();
    let Some((inst, channel)) = app.current.clone() else {
        let block = Block::bordered()
            .title(" Hermes ")
            .border_style(border_style(has_focus));
        let hint = Paragraph::new(vec![
            Line::from(""),
            Line::from("  Select a channel in the sidebar (j/k, Enter)."),
            Line::from("  :instance add <invite link or address> adds a server."),
            Line::from("  :help lists all commands."),
        ])
        .style(dim())
        .block(block);
        f.render_widget(hint, area);
        return;
    };
    let Some(s) = store.instance(&inst) else {
        return;
    };
    let channel_info = s.channel(channel);
    let space = s.space_of(channel);
    let title = match (channel_info, space) {
        (Some(c), Some(space)) => format!(
            " {} — {} @ {inst} ",
            view::channel_title(s, c),
            s.spaces.get(&space).map_or("", |sp| sp.space.name.as_str())
        ),
        (Some(c), None) => format!(" {} @ {inst} ", view::channel_title(s, c)),
        _ => format!(" {inst} "),
    };
    let topic = channel_info
        .and_then(|c| c.topic.clone())
        .unwrap_or_default();
    let mut block = Block::bordered()
        .title(Line::from(vec![
            Span::styled(title, bold()),
            Span::styled(topic, dim()),
        ]))
        .border_style(border_style(has_focus));
    if let Some(footer) = crate::voice::room_footer(app, s, &inst, channel) {
        block = block.title_bottom(Line::from(Span::styled(
            footer,
            Style::default().fg(Color::Green),
        )));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 2 {
        return;
    }
    let config = app.core.config();
    let window = s.window(channel);
    let empty_messages = Vec::new();
    let empty_pending = Vec::new();
    let (messages, pending) = window.map_or((&empty_messages, &empty_pending), |w| {
        (&w.messages, &w.pending)
    });
    let name_width = messages
        .iter()
        .map(|m| width(&s.display_name(space, m.author.id)))
        .chain(s.me_id().map(|me| width(&s.display_name(space, me))))
        .max()
        .unwrap_or(4)
        .min(14);
    let layout = Layout {
        ctx: Ctx { store: s, space },
        width: inner.width as usize,
        name_width,
        time_format: config.buffer.timestamp_format.clone(),
        offset: app.utc_offset,
        group_minutes: config.buffer.group_minutes,
    };
    let highlight = Style::default().bg(Color::Rgb(40, 44, 52));
    let selected = app.msg_sel;

    let mut lines_before: Vec<Line> = Vec::new();
    match window {
        Some(w) if w.reached_start => lines_before.push(Line::from(Span::styled(
            "— this is the start of the channel —",
            dim(),
        ))),
        Some(w) if w.loaded => lines_before.push(Line::from(Span::styled(
            "↑ older messages: scroll up (k) to load",
            dim(),
        ))),
        _ => lines_before.push(Line::from(Span::styled("loading…", dim()))),
    }
    let offset = app.utc_offset;
    // One entry (message or pending message) with its day separator. A message's look depends
    // only on itself and the message before it, so entries can be laid out in any order.
    let render = |i: usize| -> Vec<Line<'static>> {
        let hl = (selected == Some(i) && has_focus).then_some(highlight);
        let Some(m) = messages.get(i) else {
            return pending
                .get(i - messages.len())
                .map_or_else(Vec::new, |p| layout.pending(p, hl));
        };
        let prev = i.checked_sub(1).and_then(|j| messages.get(j));
        let mut lines = Vec::new();
        let day = time::day(m.created_at() as i64, offset);
        if prev.is_none_or(|p| time::day(p.created_at() as i64, offset) != day) {
            let label = time::format(m.created_at() as i64, "%Y-%m-%d", offset);
            lines.push(Line::from(Span::styled(
                format!("──── {label} ────"),
                dim(),
            )));
        }
        let grouped = prev.is_some_and(|p| {
            !m.deleted && !p.deleted && view::is_grouped(p, m, layout.group_minutes)
        }) && lines.is_empty();
        lines.extend(layout.message(m, grouped, hl));
        lines
    };

    // Typing line at the bottom of the pane.
    let typing = s.typing(channel, now_ms());
    let names: Vec<String> = typing.iter().map(|u| s.display_name(space, *u)).collect();
    let typing_text = view::typing_text(&names).unwrap_or_default();
    let show_typing = config.buffer.show_typing && !typing_text.is_empty();
    let height = inner.height as usize - usize::from(show_typing);
    app.page = (height / 2).max(1);

    let count = messages.len() + pending.len();
    let anchor = selected.filter(|&i| i < count);
    let visible = visible_lines(count, anchor, height, lines_before, render);
    f.render_widget(
        Paragraph::new(visible),
        Rect {
            height: height as u16,
            ..inner
        },
    );
    if show_typing {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                typing_text,
                dim().add_modifier(Modifier::ITALIC),
            ))),
            Rect {
                y: inner.y + inner.height - 1,
                height: 1,
                ..inner
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_by_width() {
        let pieces: Pieces = vec![
            ("hello ".into(), Style::default()),
            ("wörld😀x".into(), bold()),
        ];
        let lines = wrap(&pieces, 6);
        let texts: Vec<String> = lines
            .iter()
            .map(|l| l.iter().map(|(t, _)| t.as_str()).collect())
            .collect();
        assert_eq!(texts, ["hello ", "wörld", "😀x"]);
    }

    /// What `visible_lines` must match: lay out everything, then pick the view.
    fn visible_naive(lens: &[usize], anchor: Option<usize>, height: usize) -> Vec<(usize, usize)> {
        let mut all = vec![(usize::MAX, 0)];
        let mut range = None;
        for (i, &len) in lens.iter().enumerate() {
            let start = all.len();
            all.extend((0..len).map(|l| (i, l)));
            if anchor == Some(i) {
                range = Some((start, all.len()));
            }
        }
        let top = match range {
            Some((start, end)) => {
                let bottom = end.max(height.min(all.len()));
                bottom.saturating_sub(height).min(start)
            }
            None => all.len().saturating_sub(height),
        };
        all.into_iter().skip(top).take(height).collect()
    }

    #[test]
    fn only_visible_entries_are_laid_out_and_the_view_is_unchanged() {
        let patterns: [&[usize]; 7] = [
            &[],
            &[1],
            &[3],
            &[1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
            &[2, 5, 1, 9, 1, 3],
            &[12, 1, 1],
            &[1, 1, 7, 1, 1, 2, 2, 4],
        ];
        for lens in patterns {
            for height in 1..15 {
                for anchor in std::iter::once(None).chain((0..lens.len()).map(Some)) {
                    let mut rendered = Vec::new();
                    let got =
                        visible_lines(lens.len(), anchor, height, vec![(usize::MAX, 0)], |i| {
                            rendered.push(i);
                            (0..lens[i]).map(|l| (i, l)).collect()
                        });
                    assert_eq!(
                        got,
                        visible_naive(lens, anchor, height),
                        "{lens:?} height {height} anchor {anchor:?}"
                    );
                    // Each entry at most once, and only entries that are (partly) on screen.
                    let mut once = rendered.clone();
                    once.sort_unstable();
                    once.dedup();
                    assert_eq!(once.len(), rendered.len());
                    let shown: Vec<usize> = got.iter().map(|&(i, _)| i).collect();
                    assert!(
                        rendered.iter().all(|i| shown.contains(i)),
                        "{lens:?} height {height} anchor {anchor:?}: rendered {rendered:?}"
                    );
                }
            }
        }
        // A long history costs only what fits.
        let mut calls = 0;
        let got = visible_lines(500, None, 10, vec![0], |i| {
            calls += 1;
            vec![i]
        });
        assert_eq!(got, (490..500).collect::<Vec<_>>());
        assert_eq!(calls, 10);
    }
}
