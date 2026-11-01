//! Popups: autocomplete, emoji search, confirmations, the sign-in form and help.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use super::{bold, centered, dim, width};
use crate::app::{App, Popup};
use crate::commands::HELP;
use crate::keymap::ACTIONS;

fn frame(f: &mut Frame, area: Rect, title: &str) -> Rect {
    let block = Block::bordered()
        .title(Span::styled(format!(" {title} "), bold()))
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    inner
}

fn list_lines(items: &[String], selected: usize) -> Vec<Line<'static>> {
    items
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let style = if i == selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            Line::from(Span::styled(text.clone(), style))
        })
        .collect()
}

pub fn draw(f: &mut Frame, app: &App, screen: Rect) {
    let Some(popup) = &app.popup else {
        return;
    };
    match popup {
        Popup::Menu {
            title,
            items,
            selected,
        } => {
            let labels: Vec<String> = items.iter().map(|(l, _)| format!(" {l} ")).collect();
            let w = labels
                .iter()
                .map(|l| width(l))
                .max()
                .unwrap_or(10)
                .max(width(title) + 4) as u16
                + 2;
            let area = centered(screen, w, labels.len() as u16 + 3);
            let inner = frame(f, area, title);
            let mut lines = list_lines(&labels, *selected);
            lines.push(Line::from(Span::styled(
                " Enter: do it · Esc: close",
                dim(),
            )));
            f.render_widget(Paragraph::new(lines), inner);
        }
        Popup::Results {
            title,
            lines,
            selected,
            ..
        } => {
            let area = centered(
                screen,
                110,
                screen.height.saturating_sub(4).min(lines.len() as u16 + 3),
            );
            let inner = frame(f, area, &format!("{title} · Enter: go to the message"));
            let shown = inner.height as usize;
            // Keep the selection in view.
            let skip = selected.saturating_sub(shown.saturating_sub(1));
            let visible: Vec<String> = lines.iter().skip(skip).take(shown).cloned().collect();
            f.render_widget(Paragraph::new(list_lines(&visible, selected - skip)), inner);
        }
        Popup::Complete { items, selected } => {
            let labels: Vec<String> = items.iter().map(|s| s.label.clone()).collect();
            let w = labels.iter().map(|l| width(l)).max().unwrap_or(10) as u16 + 4;
            let h = labels.len() as u16 + 2;
            // Just above the input box.
            let area = Rect {
                x: screen.x + super::SIDEBAR_WIDTH.min(screen.width / 3) + 1,
                y: screen.height.saturating_sub(h + 4),
                width: w.min(screen.width),
                height: h.min(screen.height),
            };
            let inner = frame(f, area, "complete");
            f.render_widget(Paragraph::new(list_lines(&labels, *selected)), inner);
        }
        Popup::Emoji {
            query,
            results,
            selected,
            ..
        } => {
            let area = centered(screen, 40, 12);
            let inner = frame(f, area, "react");
            let mut lines = vec![Line::from(vec![
                Span::styled("search: ", dim()),
                Span::raw(query.text().to_owned()),
            ])];
            let labels: Vec<String> = results.iter().map(|(e, c)| format!("{e}  :{c}:")).collect();
            if labels.is_empty() {
                lines.push(Line::from(Span::styled(
                    "type a name, e.g. thumbsup",
                    dim(),
                )));
            }
            lines.extend(list_lines(&labels, *selected));
            f.render_widget(Paragraph::new(lines), inner);
            f.set_cursor_position(Position::new(
                inner.x + 8 + width(query.text()) as u16,
                inner.y,
            ));
        }
        Popup::Confirm { text, .. } => {
            let w = text.lines().map(width).max().unwrap_or(20) as u16 + 4;
            let area = centered(screen, w.max(30), text.lines().count() as u16 + 2);
            let inner = frame(f, area, "confirm");
            f.render_widget(
                Paragraph::new(text.clone()).wrap(Wrap { trim: false }),
                inner,
            );
        }
        Popup::Form(form) => {
            let area = centered(screen, 56, 10);
            let title = if form.register {
                format!("Create an account on {}", form.host)
            } else {
                format!("Sign in to {}", form.host)
            };
            let inner = frame(f, area, &title);
            let mut lines = Vec::new();
            for (i, field) in form.fields.iter().enumerate() {
                let value = if field.secret {
                    "•".repeat(field.input.text().chars().count())
                } else {
                    field.input.text().to_owned()
                };
                let style = if i == form.focus {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{:>9}: ", field.label), style),
                    Span::raw(value),
                ]));
            }
            lines.push(Line::from(""));
            let other = if form.register {
                "sign in instead"
            } else {
                "create an account instead"
            };
            lines.push(Line::from(Span::styled(
                format!("Enter: next/submit  Tab: field  Ctrl+R: {other}  Esc: cancel"),
                dim(),
            )));
            if form.invite.is_some() {
                lines.push(Line::from(Span::styled(
                    "The invite is used after signing in.",
                    dim(),
                )));
            }
            if form.busy {
                lines.push(Line::from(Span::styled(
                    "working…",
                    Style::default().fg(Color::Yellow),
                )));
            }
            f.render_widget(Paragraph::new(lines), inner);
            let field = &form.fields[form.focus];
            let shown = if field.secret {
                field.input.text()[..field.input.cursor()].chars().count()
            } else {
                width(&field.input.text()[..field.input.cursor()])
            };
            f.set_cursor_position(Position::new(
                inner.x + 11 + shown as u16,
                inner.y + form.focus as u16,
            ));
        }
        Popup::Notice { title, text } => {
            let w = text.lines().map(width).max().unwrap_or(20) as u16 + 4;
            let area = centered(screen, w.clamp(30, 90), text.lines().count() as u16 + 4);
            let inner = frame(f, area, title);
            let mut lines: Vec<Line> = text.lines().map(|l| Line::from(l.to_owned())).collect();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("any key closes", dim())));
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
        Popup::Call { name, .. } => {
            let area = centered(screen, 44, 5);
            let inner = frame(f, area, "incoming call");
            f.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(format!("{name} is calling you"), bold())),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("y", Style::default().fg(Color::Green)),
                        Span::raw(": join    "),
                        Span::styled("n", Style::default().fg(Color::Red)),
                        Span::raw(": dismiss"),
                    ]),
                ]),
                inner,
            );
        }
        Popup::Log => {
            let area = centered(screen, 100, screen.height.saturating_sub(4));
            let inner = frame(f, area, "messages (any key closes)");
            let shown = inner.height as usize;
            let skip = app.log.len().saturating_sub(shown);
            let lines: Vec<Line> = app
                .log
                .iter()
                .skip(skip)
                .map(|(text, error, at)| {
                    let age = at.elapsed().as_secs();
                    let age = if age < 60 {
                        format!("{age:>3}s ")
                    } else {
                        format!("{:>3}m ", age / 60)
                    };
                    Line::from(vec![
                        Span::styled(age, dim()),
                        Span::styled(
                            text.replace('\n', " "),
                            if *error {
                                Style::default().fg(Color::Red)
                            } else {
                                Style::default()
                            },
                        ),
                    ])
                })
                .collect();
            let lines = if lines.is_empty() {
                vec![Line::from(Span::styled("nothing yet", dim()))]
            } else {
                lines
            };
            f.render_widget(Paragraph::new(lines), inner);
        }
        Popup::Help => {
            let area = centered(screen, 90, 34);
            let inner = frame(f, area, "help (any key closes)");
            let mut lines = vec![Line::from(Span::styled(
                "Normal mode keys ([tui.keys] in config.toml):",
                bold(),
            ))];
            let keys: Vec<String> = ACTIONS
                .iter()
                .map(|(action, name, _)| format!("{:>7} {name}", app.keymap.describe(*action)))
                .collect();
            for pair in keys.chunks(3) {
                lines.push(Line::from(pair.join("  ")));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Commands:", bold())));
            lines.push(Line::from(HELP));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Menus and mouse:", bold())));
            lines.push(Line::from(
                "a (or a right-click) shows what you can do with the selected channel, space, \
                 message or person. Click a channel to open it; the wheel scrolls. / searches \
                 (:search! everywhere); in results, Enter jumps to the message.",
            ));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Voice:", bold())));
            lines.push(Line::from(
                "Select a voice channel or a DM and press J (from the sidebar, or inside the \
                 channel). m mutes, D deafens, L leaves. :voice answers a call. The TUI has \
                 no push-to-talk: voice activity only (:set voice.mode voice_activity).",
            ));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Insert mode: /me /shrug /nick /dm /invite /status /join /leave /mute /deafen \
                 work too.",
                dim(),
            )));
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
    }
}
