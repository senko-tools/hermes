//! Drawing (PLAN.md section 16.1): sidebar | messages + input | members, and a status line.

mod input;
mod members;
mod messages;
mod popup;
mod sidebar;
mod statusline;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::app::{App, Focus};

pub const SIDEBAR_WIDTH: u16 = 28;
pub const MEMBERS_WIDTH: u16 = 22;

pub fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

pub fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// Display width of a string in terminal cells.
pub fn width(s: &str) -> usize {
    Span::raw(s).width()
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let [main, status] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(f.area());
    let sidebar_width = SIDEBAR_WIDTH.min(main.width / 3);
    let members_width = if app.show_members && main.width > 80 {
        MEMBERS_WIDTH
    } else {
        0
    };
    let [left, center, right] = Layout::horizontal([
        Constraint::Length(sidebar_width),
        Constraint::Min(20),
        Constraint::Length(members_width),
    ])
    .areas(main);

    app.areas.sidebar = left;
    app.areas.members = right;
    sidebar::draw(f, app, left);
    let input_height = input::height(app, center.width);
    let [messages_area, input_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(input_height)]).areas(center);
    app.areas.messages = messages_area;
    messages::draw(f, app, messages_area);
    input::draw(f, app, input_area);
    if members_width > 0 {
        members::draw(f, app, right);
    }
    statusline::draw(f, app, status);
    popup::draw(f, app, f.area());
}

/// A rectangle of at most `w`×`h` centered in `area`.
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

pub fn focused(app: &App, focus: Focus) -> bool {
    app.focus == focus && app.popup.is_none()
}
