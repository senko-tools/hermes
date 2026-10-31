//! The pane grid (PLAN.md section 15.1): any number of splits, each pane showing one buffer.
//! The layout is saved in `[[layout]]` (see [`crate::pane`]).

use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Status as Presence;
use hermes_client_core::{Core, InstanceId, view};
use iced::widget::pane_grid::{self, Axis, PaneGrid};
use iced::widget::{Space, button, column, container, mouse_area, row, scrollable};
use iced::{Alignment, Border, Element, Fill, Length};

use crate::buffer::channel::{self, ChannelView};
use crate::buffer::{Buffer, settings};
use crate::font::Fonts;
use crate::message::Message;
use crate::pane::{self, Pane};
use crate::sidebar::menu::Target;
use crate::theme::{Theme, style};
use crate::widget::{heading, label, small};

const MEMBER_LIST_WIDTH: f32 = 200.0;

pub struct Panes {
    pub state: pane_grid::State<Pane>,
    pub focus: pane_grid::Pane,
}

impl Panes {
    /// The saved layout, or one empty pane.
    pub fn restore(layout: &[toml::Table]) -> Panes {
        match pane::restore(layout) {
            Some(config) => {
                let state = pane_grid::State::with_configuration(config);
                let focus = *state.iter().next().expect("a layout has panes").0;
                Panes { state, focus }
            }
            None => {
                let (state, focus) = pane_grid::State::new(Pane::new(Buffer::Empty));
                Panes { state, focus }
            }
        }
    }

    pub fn save(&self) -> Vec<toml::Table> {
        pane::save(&self.state)
    }

    pub fn focused(&self) -> Option<&Pane> {
        self.state.get(self.focus)
    }

    /// The channel in the focused pane.
    pub fn focused_channel(&self) -> Option<(InstanceId, Id)> {
        match &self.focused()?.buffer {
            Buffer::Channel { inst, channel } => Some((inst.clone(), *channel)),
            _ => None,
        }
    }

    /// Shows `buffer` in the focused pane. A channel starts at the bottom, with the divider
    /// before the messages newer than `read_marker`.
    pub fn open(&mut self, buffer: Buffer) {
        self.open_with_marker(buffer, None);
    }

    pub fn open_with_marker(&mut self, buffer: Buffer, read_marker: Option<Id>) {
        if let Some(pane) = self.state.get_mut(self.focus)
            && pane.buffer != buffer
        {
            pane.buffer = buffer;
            pane.channel = ChannelView::new(read_marker);
        }
    }

    /// Splits `pane`; the new pane shows the same buffer and gets the focus.
    pub fn split(&mut self, pane: pane_grid::Pane, axis: Axis) {
        let Some(mut current) = self.state.get(pane).cloned() else {
            return;
        };
        current.channel = ChannelView::new(current.channel.read_marker);
        if let Some((new, _)) = self.state.split(axis, pane, current) {
            self.focus = new;
        }
    }

    /// Closes `pane`; the last pane is emptied instead.
    pub fn close(&mut self, pane: pane_grid::Pane) {
        if self.state.len() == 1 {
            self.open(Buffer::Empty);
            return;
        }
        if let Some((_, sibling)) = self.state.close(pane)
            && (self.focus == pane || self.state.get(self.focus).is_none())
        {
            self.focus = sibling;
        }
    }

    pub fn toggle_maximize(&mut self, pane: pane_grid::Pane) {
        if self.state.maximized().is_some() {
            self.state.restore();
        } else {
            self.state.maximize(pane);
        }
    }

    pub fn toggle_members(&mut self, pane: pane_grid::Pane) {
        if let Some(pane) = self.state.get_mut(pane) {
            pane.members = !pane.members;
        }
    }

    /// Moves the focus to the next (`1`) or previous (`-1`) pane, in reading order.
    pub fn cycle_focus(&mut self, delta: i32) {
        let order = self.order();
        let Some(i) = order.iter().position(|p| *p == self.focus) else {
            return;
        };
        let len = order.len() as i32;
        self.focus = order[(i as i32 + delta).rem_euclid(len) as usize];
        if self.state.maximized().is_some() {
            self.state.maximize(self.focus);
        }
    }

    /// Panes from the layout tree, left/top first.
    fn order(&self) -> Vec<pane_grid::Pane> {
        fn walk(node: &pane_grid::Node, out: &mut Vec<pane_grid::Pane>) {
            match node {
                pane_grid::Node::Split { a, b, .. } => {
                    walk(a, out);
                    walk(b, out);
                }
                pane_grid::Node::Pane(p) => out.push(*p),
            }
        }
        let mut out = Vec::new();
        walk(self.state.layout(), &mut out);
        out
    }

    /// Every channel shown in some pane (for notifications and window trimming).
    pub fn visible_channels(&self) -> Vec<(InstanceId, Id)> {
        self.state
            .iter()
            .filter_map(|(_, p)| match &p.buffer {
                Buffer::Channel { inst, channel } => Some((inst.clone(), *channel)),
                _ => None,
            })
            .collect()
    }

    /// Empties panes whose buffer matches (a removed instance, a channel that is gone).
    pub fn clear_where(&mut self, f: impl Fn(&Buffer) -> bool) {
        for (_, pane) in self.state.iter_mut() {
            if f(&pane.buffer) {
                pane.buffer = Buffer::Empty;
            }
        }
    }

    pub fn on_drag(&mut self, event: pane_grid::DragEvent) {
        match event {
            pane_grid::DragEvent::Picked { pane } => self.focus = pane,
            pane_grid::DragEvent::Dropped { pane, target } => self.state.drop(pane, target),
            pane_grid::DragEvent::Canceled { .. } => {}
        }
    }

    pub fn view<'a>(&'a self, ctx: &Context<'a>) -> Element<'a, Message> {
        let total = self.state.len();
        PaneGrid::new(&self.state, |id, pane, maximized| {
            let focused = id == self.focus;
            let theme = ctx.theme;
            let (title, topic) = title(ctx.core, &pane.buffer);
            let mut controls = row![].spacing(4).align_y(Alignment::Center);
            if matches!(pane.buffer, Buffer::Channel { .. }) {
                controls = controls.push(
                    header_button(
                        if pane.members {
                            "Hide members"
                        } else {
                            "Members"
                        },
                        ctx,
                    )
                    .on_press(Message::PaneMenu(id, PaneAction::ToggleMembers)),
                );
            }
            if total > 1 {
                controls = controls.push(
                    header_button(if maximized { "Restore" } else { "Maximize" }, ctx)
                        .on_press(Message::PaneMenu(id, PaneAction::Maximize)),
                );
            }
            controls = controls.push(
                mouse_area(header_button("⋮", ctx).on_press(Message::OpenMenu(Target::Pane(id))))
                    .on_right_press(Message::OpenMenu(Target::Pane(id))),
            );
            let title_bar = pane_grid::TitleBar::new(
                row![
                    label(title, ctx.fonts).font(ctx.fonts.bold()),
                    small(topic, ctx.fonts).color(theme.text.secondary.0),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
            .controls(pane_grid::Controls::new(controls))
            .always_show_controls()
            .padding([6, 10])
            .style(move |_| style::pane_header(theme));

            pane_grid::Content::new(body(id, pane, ctx))
                .title_bar(title_bar)
                .style(move |_| container::Style {
                    background: Some(theme.general.background.0.into()),
                    border: Border {
                        color: if focused && total > 1 {
                            theme.pane.focused_border.0
                        } else {
                            theme.general.border.0
                        },
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..container::Style::default()
                })
        })
        .spacing(4)
        .on_click(Message::PaneClicked)
        .on_drag(Message::PaneDragged)
        .on_resize(6, Message::PaneResized)
        .into()
    }
}

/// What a pane's view needs from the app.
pub struct Context<'a> {
    pub core: &'a Core,
    pub fonts: &'a Fonts,
    pub theme: &'a Theme,
    pub config: &'a hermes_client_core::config::ClientConfig,
    pub themes: &'a [String],
    pub utc_offset: i64,
}

/// Actions from a pane header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneAction {
    ToggleMembers,
    Maximize,
}

fn header_button<'a>(text: &'a str, ctx: &Context<'a>) -> button::Button<'a, Message> {
    let theme = ctx.theme;
    button(small(text, ctx.fonts))
        .padding([2, 8])
        .style(move |_, s| style::sidebar_item(theme, s, false))
}

/// The header title and topic of a buffer.
pub fn title(core: &Core, buffer: &Buffer) -> (String, String) {
    match buffer {
        Buffer::Empty => ("Hermes".into(), String::new()),
        Buffer::Settings => ("Settings".into(), String::new()),
        Buffer::Channel { inst, channel } => {
            let store = core.store();
            match store
                .instance(inst)
                .and_then(|s| Some((s, s.channel(*channel)?)))
            {
                Some((s, c)) => (
                    view::channel_title(s, c),
                    c.topic.clone().unwrap_or_default(),
                ),
                None => ("(not available)".into(), String::new()),
            }
        }
    }
}

fn body<'a>(id: pane_grid::Pane, pane: &'a Pane, ctx: &Context<'a>) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    match &pane.buffer {
        Buffer::Empty => {
            let mut welcome = column![heading("Welcome to Hermes", fonts)].spacing(12);
            if ctx.core.instance_ids().is_empty() {
                welcome = welcome
                    .push(
                        label(
                            "Paste an invite link from a friend, or the address of a server.",
                            fonts,
                        )
                        .color(theme.text.secondary.0),
                    )
                    .push(
                        button(label("Add an instance", fonts))
                            .style(move |_, s| style::primary(theme, s))
                            .on_press(Message::OpenAddInstance),
                    );
            } else {
                welcome = welcome.push(
                    label("Open a channel from the sidebar, or press Ctrl+K.", fonts)
                        .color(theme.text.secondary.0),
                );
            }
            container(welcome).center(Fill).into()
        }
        Buffer::Settings => {
            settings::view(ctx.config, ctx.themes, fonts, theme).map(Message::Settings)
        }
        Buffer::Channel { inst, channel } => {
            let messages = channel::view(
                &channel::Context {
                    core: ctx.core,
                    config: ctx.config,
                    fonts,
                    theme,
                    utc_offset: ctx.utc_offset,
                },
                inst,
                *channel,
                &pane.channel,
            )
            .map(move |m| Message::Channel(id, m));
            if pane.members {
                row![messages, member_list(ctx, inst, *channel)].into()
            } else {
                messages
            }
        }
    }
}

/// The members of the channel's space by hoisted role, or a DM's recipients.
fn member_list<'a>(ctx: &Context<'a>, inst: &InstanceId, channel: Id) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let store = ctx.core.store();
    let mut list = column![].spacing(2).padding(8);
    if let Some(s) = store.instance(inst) {
        let groups = match s.space_of(channel) {
            Some(space) => view::member_groups(s, space),
            None => s
                .channel(channel)
                .map(|dm| {
                    vec![view::MemberGroup {
                        title: "Members".into(),
                        members: dm
                            .recipients
                            .iter()
                            .map(|u| view::MemberRow {
                                user_id: u.id,
                                name: u.name().to_owned(),
                                status: s.status(u.id),
                                color: None,
                            })
                            .collect(),
                    }]
                })
                .unwrap_or_default(),
        };
        for group in groups {
            list = list.push(
                small(
                    format!("{} — {}", group.title.to_uppercase(), group.members.len()),
                    fonts,
                )
                .color(theme.text.tertiary.0),
            );
            for member in group.members {
                let dot = match member.status {
                    Presence::Online => theme.text.success.0,
                    Presence::Idle => theme.text.warning.0,
                    Presence::Dnd => theme.text.error.0,
                    _ => theme.text.tertiary.0,
                };
                let name_color = member.color.map_or(theme.text.primary.0, |c| {
                    iced::Color::from_rgb8((c >> 16) as u8, (c >> 8) as u8, c as u8)
                });
                list = list.push(
                    row![
                        small("●", fonts).color(dot),
                        label(member.name, fonts).color(name_color),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                );
            }
            list = list.push(Space::new().height(6));
        }
    }
    container(scrollable(list).height(Fill))
        .width(Length::Fixed(MEMBER_LIST_WIDTH))
        .height(Fill)
        .style(move |_| style::sidebar(theme))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(id: u64) -> Buffer {
        Buffer::Channel {
            inst: "h".into(),
            channel: Id(id),
        }
    }

    #[test]
    fn split_open_focus_and_close() {
        let mut panes = Panes::restore(&[]);
        panes.open(channel(11));
        let first = panes.focus;
        panes.split(first, Axis::Vertical);
        let second = panes.focus;
        assert_ne!(first, second);
        assert_eq!(
            panes.focused().unwrap().buffer,
            channel(11),
            "the split copies the buffer"
        );
        panes.open(channel(12));
        assert_eq!(panes.visible_channels().len(), 2);

        panes.cycle_focus(1);
        assert_eq!(panes.focus, first);
        panes.cycle_focus(-1);
        assert_eq!(panes.focus, second);

        panes.clear_where(|b| *b == channel(12));
        assert_eq!(panes.focused().unwrap().buffer, Buffer::Empty);

        panes.close(second);
        assert_eq!(panes.focus, first);
        assert_eq!(panes.state.len(), 1);
        panes.close(first);
        assert_eq!(panes.state.len(), 1, "the last pane stays, emptied");
        assert_eq!(panes.focused().unwrap().buffer, Buffer::Empty);
    }

    #[test]
    fn restore_uses_the_saved_layout() {
        let mut panes = Panes::restore(&[]);
        panes.open(Buffer::Settings);
        panes.split(panes.focus, Axis::Horizontal);
        panes.toggle_members(panes.focus);
        let saved = panes.save();
        let restored = Panes::restore(&saved);
        assert_eq!(restored.state.len(), 2);
        assert_eq!(restored.save(), saved);
    }
}
