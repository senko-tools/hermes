//! The sidebar tree: instances, spaces, channels with unread/mention markers, DMs.

use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::models::ChannelKind;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState};

use super::{bold, border_style, dim, focused, width};
use crate::app::{App, Focus, SidebarItem};

fn status_text(status: &ConnStatus) -> (&'static str, Color) {
    match status {
        ConnStatus::Connected => ("", Color::Green),
        ConnStatus::Connecting => (" connecting…", Color::Yellow),
        ConnStatus::Reconnecting { .. } => (" reconnecting…", Color::Yellow),
        ConnStatus::Offline => (" offline", Color::DarkGray),
        ConnStatus::LoggedOut => (" signed out", Color::Red),
        ConnStatus::Failed { .. } => (" failed", Color::Red),
    }
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let inner_width = area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = app
        .sidebar
        .iter()
        .map(|item| match item {
            SidebarItem::Instance { inst, status } => {
                let (text, color) = status_text(status);
                ListItem::new(Line::from(vec![
                    Span::styled(inst.clone(), bold().fg(Color::Cyan)),
                    Span::styled(text, Style::default().fg(color)),
                ]))
            }
            SidebarItem::Space { name, .. } => {
                ListItem::new(Line::from(Span::styled(format!(" {name}"), bold())))
            }
            SidebarItem::DmHeader => ListItem::new(Line::from(Span::styled(" DMs", bold()))),
            SidebarItem::Channel {
                inst,
                channel,
                kind,
                name,
                depth,
                unread,
                mentions,
            } => {
                let indent = " ".repeat(*depth as usize + 1);
                let (icon, label) = match kind {
                    ChannelKind::Text => ("# ", name.clone()),
                    ChannelKind::Voice => ("◂) ", name.clone()),
                    ChannelKind::Category => ("", name.to_uppercase()),
                    ChannelKind::Dm | ChannelKind::GroupDm => ("", name.clone()),
                };
                let is_current = app
                    .current
                    .as_ref()
                    .is_some_and(|(i, c)| i == inst && c == channel);
                let mut style = if *kind == ChannelKind::Category {
                    dim()
                } else if *unread {
                    bold().fg(Color::White)
                } else {
                    Style::default().fg(Color::Gray)
                };
                if is_current {
                    style = style.fg(Color::Cyan);
                }
                let badge = if *mentions > 0 {
                    format!("@{mentions}")
                } else {
                    String::new()
                };
                let left = format!("{indent}{icon}{label}");
                let pad = inner_width
                    .saturating_sub(width(&left) + width(&badge))
                    .max(1);
                ListItem::new(Line::from(vec![
                    Span::styled(left, style),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(badge, bold().fg(Color::Red)),
                ]))
            }
        })
        .collect();
    let has_focus = focused(app, Focus::Sidebar);
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(" Hermes ")
                .border_style(border_style(has_focus)),
        )
        .highlight_style(if has_focus {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        });
    let mut state = ListState::default().with_selected(Some(app.sidebar_sel));
    f.render_stateful_widget(list, area, &mut state);
}
