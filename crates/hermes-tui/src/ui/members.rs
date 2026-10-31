//! The member list: grouped by hoisted role, then online, then offline (DMs: the recipients).

use hermes_client_core::proto::models::Status;
use hermes_client_core::view::{self, MemberGroup, MemberRow};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState};

use super::{bold, border_style, dim, focused};
use crate::app::{App, Focus};

fn dot(status: Status) -> Span<'static> {
    match status {
        Status::Online => Span::styled(" ●", Style::default().fg(Color::Green)),
        Status::Idle => Span::styled(" ◐", Style::default().fg(Color::Yellow)),
        Status::Dnd => Span::styled(" ⊘", Style::default().fg(Color::Red)),
        Status::Offline => Span::raw(""),
    }
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let has_focus = focused(app, Focus::Members);
    let groups: Vec<MemberGroup> = {
        let store = app.core.store();
        match &app.current {
            Some((inst, channel)) => {
                store
                    .instance(inst)
                    .map_or_else(Vec::new, |s| match s.space_of(*channel) {
                        Some(space) => view::member_groups(s, space),
                        None => {
                            let members = s
                                .channel(*channel)
                                .map(|c| {
                                    c.recipients
                                        .iter()
                                        .map(|u| MemberRow {
                                            user_id: u.id,
                                            name: u.name().to_owned(),
                                            status: s.status(u.id),
                                            color: None,
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            vec![MemberGroup {
                                title: "Members".into(),
                                members,
                            }]
                        }
                    })
            }
            None => Vec::new(),
        }
    };
    let mut items = Vec::new();
    let mut rows = 0;
    for g in &groups {
        items.push(ListItem::new(Line::from(Span::styled(
            format!("{} — {}", g.title, g.members.len()),
            bold().fg(Color::Gray),
        ))));
        for m in &g.members {
            let mut style = match m.color {
                Some(c) => {
                    Style::default().fg(Color::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8))
                }
                None => Style::default(),
            };
            if m.status == Status::Offline {
                style = dim();
            }
            items.push(ListItem::new(Line::from(vec![
                Span::styled(format!(" {}", m.name), style),
                dot(m.status),
            ])));
            rows += 1;
        }
    }
    app.member_sel = app.member_sel.min(rows.max(1) - 1);
    // Map the member index to the list index (skipping group headers).
    let mut list_index = None;
    let mut seen = 0;
    let mut index = 0;
    for g in &groups {
        index += 1;
        for _ in &g.members {
            if seen == app.member_sel {
                list_index = Some(index);
            }
            seen += 1;
            index += 1;
        }
    }
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(" Members ")
                .border_style(border_style(has_focus)),
        )
        .highlight_style(if has_focus {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        });
    let mut state = ListState::default().with_selected(list_index);
    f.render_stateful_widget(list, area, &mut state);
}
