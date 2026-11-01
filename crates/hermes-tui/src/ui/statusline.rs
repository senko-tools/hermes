//! The bottom line: mode, connection, voice and hints or the last message.

use std::time::Duration;

use hermes_client_core::gateway::ConnStatus;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{bold, dim, width};
use crate::app::{App, Mode};
use crate::keymap::Action;

const INFO_SECS: u64 = 8;
const ERROR_SECS: u64 = 15;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    if app.mode == Mode::Command && app.popup.is_none() {
        let text = format!(":{}", app.command.text());
        f.render_widget(Paragraph::new(text), area);
        let col = 1 + width(&app.command.text()[..app.command.cursor()]);
        f.set_cursor_position(Position::new(area.x + col as u16, area.y));
        return;
    }
    let (mode, color) = match app.mode {
        Mode::Normal => (" NORMAL ", Color::Blue),
        Mode::Insert => (" INSERT ", Color::Green),
        Mode::Command => (" COMMAND ", Color::Yellow),
    };
    let mut spans = vec![Span::styled(mode, bold().bg(color).fg(Color::Black))];
    if let Some((inst, _)) = &app.current {
        let status = match app.core.status(inst) {
            ConnStatus::Connected => Span::styled(" connected ", Style::default().fg(Color::Green)),
            ConnStatus::Connecting => {
                Span::styled(" connecting… ", Style::default().fg(Color::Yellow))
            }
            ConnStatus::Reconnecting { attempt } => Span::styled(
                format!(" reconnecting (try {attempt}) "),
                Style::default().fg(Color::Yellow),
            ),
            ConnStatus::Offline => Span::styled(" offline ", dim()),
            ConnStatus::LoggedOut => Span::styled(" signed out ", Style::default().fg(Color::Red)),
            ConnStatus::Failed { .. } => Span::styled(" failed ", Style::default().fg(Color::Red)),
        };
        spans.push(Span::raw("│"));
        spans.push(status);
    }
    if let Some(voice) = crate::voice::status_line(app) {
        spans.push(Span::raw("│"));
        spans.push(Span::styled(
            format!(" {voice} "),
            Style::default().fg(Color::Green),
        ));
    }
    spans.push(Span::raw("│"));
    let message = app.status.as_ref().filter(|(_, error, at)| {
        at.elapsed() < Duration::from_secs(if *error { ERROR_SECS } else { INFO_SECS })
    });
    match message {
        Some((text, true, _)) => spans.push(Span::styled(
            format!(" {}", text.replace('\n', " ")),
            Style::default().fg(Color::Red),
        )),
        Some((text, false, _)) => spans.push(Span::raw(format!(" {}", text.replace('\n', " ")))),
        None => {
            let k = |a| app.keymap.describe(a);
            let voice = if app.core.voice_channel().is_some() {
                format!(
                    " {}:mute {}:deafen {}:leave",
                    k(Action::ToggleMute),
                    k(Action::ToggleDeafen),
                    k(Action::LeaveVoice)
                )
            } else if crate::voice::room_here(app) {
                format!(" {}:join voice", k(Action::JoinVoice))
            } else {
                String::new()
            };
            spans.push(Span::styled(
                format!(
                    " {}:type {}:reply {}:react{voice} {}:help {}:quit",
                    k(Action::Insert),
                    k(Action::Reply),
                    k(Action::React),
                    k(Action::Help),
                    k(Action::Quit)
                ),
                dim(),
            ));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
