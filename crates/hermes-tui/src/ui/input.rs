//! The message input (insert mode) with reply/edit bars, and the `:` line.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{border_style, dim, width};
use crate::app::{App, Mode};

/// Rows the input box needs (2 for the border, up to 6 lines of text).
pub fn height(app: &App, area_width: u16) -> u16 {
    let inner = area_width.saturating_sub(2).max(1);
    let text = app.input.text();
    let lines: usize = text
        .split('\n')
        .map(|l| wrap_line(l, inner as usize).len())
        .sum();
    // After a line that fills its last row exactly, the cursor sits on a row of its own.
    let (cursor_row, _) = cursor_position(text, app.input.cursor(), inner);
    lines.max(cursor_row + 1).clamp(1, 6) as u16 + 2
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let insert = app.mode == Mode::Insert && app.popup.is_none();
    let title = {
        let store = app.core.store();
        let context = app.current.as_ref().and_then(|(inst, channel)| {
            let s = store.instance(inst)?;
            let space = s.space_of(*channel);
            let w = s.window(*channel);
            if let Some(id) = app.editing {
                let preview: String = w
                    .and_then(|w| w.get(id))
                    .and_then(|m| m.content.lines().next())
                    .unwrap_or_default()
                    .chars()
                    .take(30)
                    .collect();
                return Some(format!(" Editing \"{preview}\" — Enter save, Esc cancel "));
            }
            let reply = app.reply_to?;
            let author = w?.get(reply).map(|m| s.display_name(space, m.author.id))?;
            Some(format!(" Replying to {author} — Esc cancel "))
        });
        context.unwrap_or_else(|| {
            if insert {
                " Enter send, Alt+Enter newline, Tab complete, Esc normal ".into()
            } else {
                String::new()
            }
        })
    };
    let block = Block::bordered()
        .title(Span::styled(title, Style::default().fg(Color::Yellow)))
        .border_style(border_style(insert));
    let inner = block.inner(area);
    let text = app.input.text();
    let (cursor_row, cursor_col) = cursor_position(text, app.input.cursor(), inner.width);
    // Once the text is taller than the box, scroll so the cursor's row stays visible.
    let scroll = cursor_row.saturating_sub(inner.height.saturating_sub(1) as usize) as u16;
    let paragraph = if text.is_empty() && !insert {
        let channel = app
            .current
            .as_ref()
            .and_then(|(inst, channel)| {
                let store = app.core.store();
                let s = store.instance(inst)?;
                let c = s.channel(*channel)?;
                Some(hermes_client_core::view::channel_title(s, c))
            })
            .unwrap_or_default();
        Paragraph::new(Line::from(Span::styled(
            format!("> message {channel} (i to type)"),
            dim(),
        )))
    } else {
        // Wrapped here by character, the same way `cursor_position` counts (ratatui's word wrap
        // would put the cursor in the wrong place on long lines).
        let w = inner.width.max(1) as usize;
        Paragraph::new(
            text.split('\n')
                .flat_map(|l| wrap_line(l, w))
                .map(|row| Line::from(row.to_owned()))
                .collect::<Vec<_>>(),
        )
        .scroll((scroll, 0))
    };
    f.render_widget(paragraph.block(block), area);
    if insert {
        let y = inner.y + (cursor_row as u16 - scroll).min(inner.height.saturating_sub(1));
        f.set_cursor_position(Position::new(inner.x + cursor_col as u16, y));
    }
}

/// Splits one line into rows of at most `w` cells, by character (a character wider than the box
/// gets a row of its own).
fn wrap_line(line: &str, w: usize) -> Vec<&str> {
    let mut rows = Vec::new();
    let mut start = 0;
    let mut used = 0;
    for (i, c) in line.char_indices() {
        let cw = width(c.encode_utf8(&mut [0; 4]));
        if used + cw > w && used > 0 {
            rows.push(&line[start..i]);
            start = i;
            used = 0;
        }
        used += cw;
    }
    rows.push(&line[start..]);
    rows
}

/// The cursor's wrapped (row, column) inside a box `box_width` cells wide.
fn cursor_position(text: &str, cursor: usize, box_width: u16) -> (usize, usize) {
    let w = box_width.max(1) as usize;
    let before = &text[..cursor];
    let (row, line) = match before.rsplit_once('\n') {
        Some((earlier, line)) => (
            earlier.split('\n').map(|l| wrap_line(l, w).len()).sum(),
            line,
        ),
        None => (0, before),
    };
    // On its own line, the cursor sits after the last row's text, or at the start of the next
    // row when that one is full.
    let rows = wrap_line(line, w);
    let row = row + rows.len() - 1;
    let col = rows.last().map_or(0, |r| width(r));
    if col >= w { (row + 1, 0) } else { (row, col) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_follows_wraps_and_newlines() {
        assert_eq!(cursor_position("", 0, 10), (0, 0));
        assert_eq!(cursor_position("abc", 3, 10), (0, 3));
        // 12 cells in a 10-wide box wrap onto a second row.
        assert_eq!(cursor_position("abcdefghijkl", 12, 10), (1, 2));
        let text = "a\nb\nc\nd\ne\nf\ng\nh";
        assert_eq!(cursor_position(text, text.len(), 10), (7, 1));
        // The cursor at the start stays on the first row.
        assert_eq!(cursor_position(text, 0, 10), (0, 0));
    }

    #[test]
    fn rows_wrap_by_character_like_the_cursor() {
        // Word wrapping would move "world" to the next row whole; the cursor counts characters.
        assert_eq!(wrap_line("hello world", 8), ["hello wo", "rld"]);
        assert_eq!(cursor_position("hello world", 11, 8), (1, 3));
        assert_eq!(wrap_line("", 8), [""]);
        assert_eq!(wrap_line("abcdefgh", 8), ["abcdefgh"]);
        assert_eq!(cursor_position("abcdefgh", 8, 8), (1, 0));
        // A wide character that does not fit moves to the next row.
        assert_eq!(wrap_line("abc😀", 4), ["abc", "😀"]);
        assert_eq!(cursor_position("abc😀", "abc😀".len(), 4), (1, 2));
    }
}
