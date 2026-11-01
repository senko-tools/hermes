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
    let inner = area_width.saturating_sub(2).max(1) as usize;
    let lines: usize = app
        .input
        .text()
        .split('\n')
        .map(|l| width(l).div_ceil(inner).max(1))
        .sum();
    lines.clamp(1, 6) as u16 + 2
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let insert = app.mode == Mode::Insert && app.popup.is_none();
    let title = {
        let store = app.core.store();
        let context = app.current.as_ref().and_then(|(inst, channel)| {
            let s = store.instance(inst)?;
            let space = s.space_of(*channel);
            if let Some(id) = app.editing {
                let _ = id;
                return Some(" Editing — Enter save, Esc cancel ".to_owned());
            }
            let reply = app.reply_to?;
            let w = s.window(*channel)?;
            let author = w.get(reply).map(|m| s.display_name(space, m.author.id))?;
            Some(format!(" Replying to {author} — Ctrl+C cancel "))
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
        Paragraph::new(
            text.split('\n')
                .map(|l| Line::from(l.to_owned()))
                .collect::<Vec<_>>(),
        )
        .wrap(ratatui::widgets::Wrap { trim: false })
    };
    f.render_widget(paragraph.block(block), area);
    if insert {
        // Cursor: count wrapped rows before the cursor.
        let w = inner.width.max(1) as usize;
        let before = &text[..app.input.cursor()];
        let mut row = 0;
        let mut col = 0;
        let lines: Vec<&str> = before.split('\n').collect();
        for (i, line) in lines.iter().enumerate() {
            let lw = width(line);
            if i + 1 < lines.len() {
                row += lw.div_ceil(w).max(1);
            } else {
                row += lw / w;
                col = lw % w;
            }
        }
        let y = inner.y + (row as u16).min(inner.height.saturating_sub(1));
        f.set_cursor_position(Position::new(inner.x + col as u16, y));
    }
}
