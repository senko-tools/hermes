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
            let name = ctx
                .space
                .and_then(|s| ctx.store.spaces.get(&s))
                .and_then(|s| s.role(*id))
                .map_or_else(|| "role".to_owned(), |r| r.name.clone());
            push(lines, &format!("@{name}"), style.fg(Color::Magenta));
        }
        M::ChannelMention(id) => {
            let name = ctx
                .store
                .channel(*id)
                .and_then(|c| c.name.clone())
                .unwrap_or_else(|| "channel".into());
            push(lines, &format!("#{name}"), style.fg(Color::Cyan));
        }
        M::Everyone => push(lines, "@everyone", style.fg(Color::Magenta)),
        M::Here => push(lines, "@here", style.fg(Color::Magenta)),
    }
}

fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn attachment_line(i: usize, a: &Attachment) -> Pieces {
    let text = match (a.width, a.height) {
        (Some(w), Some(h)) => format!("[{}] [image: {} {w}×{h}]", i + 1, a.filename),
        _ => format!("[{}] [file: {} {}]", i + 1, a.filename, size_text(a.size)),
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

/// One rendered entry (message or pending message).
struct Rendered {
    lines: Vec<Line<'static>>,
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
            .and_then(|s| self.ctx.store.spaces.get(&s))
            .and_then(|state| {
                let member = state.members.get(&author)?;
                state
                    .roles
                    .iter()
                    .find(|r| r.color.is_some() && member.role_ids.contains(&r.id))
                    .and_then(|r| r.color)
            })
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
    if let Some(footer) = crate::voice::room_footer(app, &inst, channel) {
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

    let mut rendered: Vec<Rendered> = Vec::new();
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
    let mut prev: Option<&Message> = None;
    for (i, m) in messages.iter().enumerate() {
        let mut lines = Vec::new();
        let day = time::day(m.created_at() as i64, app.utc_offset);
        if prev.is_none_or(|p| time::day(p.created_at() as i64, app.utc_offset) != day) {
            let label = time::format(m.created_at() as i64, "%Y-%m-%d", app.utc_offset);
            lines.push(Line::from(Span::styled(
                format!("──── {label} ────"),
                dim(),
            )));
        }
        let grouped = prev.is_some_and(|p| {
            !m.deleted && !p.deleted && view::is_grouped(p, m, layout.group_minutes)
        }) && lines.is_empty();
        let hl = (selected == Some(i) && has_focus).then_some(highlight);
        lines.extend(layout.message(m, grouped, hl));
        rendered.push(Rendered { lines });
        prev = Some(m);
    }
    for (j, p) in pending.iter().enumerate() {
        let hl = (selected == Some(messages.len() + j) && has_focus).then_some(highlight);
        rendered.push(Rendered {
            lines: layout.pending(p, hl),
        });
    }

    // Typing line at the bottom of the pane.
    let typing = s.typing(channel, now_ms());
    let typing_text = match typing.as_slice() {
        [] => String::new(),
        [a] => format!("{} is typing…", s.display_name(space, *a)),
        [a, b] => format!(
            "{} and {} are typing…",
            s.display_name(space, *a),
            s.display_name(space, *b)
        ),
        _ => "several people are typing…".into(),
    };
    let show_typing = config.buffer.show_typing && !typing_text.is_empty();
    let height = inner.height as usize - usize::from(show_typing);
    app.page = (height / 2).max(1);

    // All lines, and where the selected entry ends.
    let mut all: Vec<Line> = lines_before;
    let mut sel_range = None;
    for (i, r) in rendered.into_iter().enumerate() {
        let start = all.len();
        all.extend(r.lines);
        if selected == Some(i) {
            sel_range = Some((start, all.len()));
        }
    }
    let top = match sel_range {
        // Keep the selected entry fully visible, as low as possible.
        Some((start, end)) => {
            let bottom = end.max(height.min(all.len()));
            let top = bottom.saturating_sub(height);
            top.min(start)
        }
        None => all.len().saturating_sub(height),
    };
    let visible: Vec<Line> = all.into_iter().skip(top).take(height).collect();
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

    #[test]
    fn sizes() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(2 * 1024 * 1024 + 100_000), "2.1 MB");
    }
}
