//! The pane grid (PLAN.md section 15.1): any number of splits, each pane showing one buffer.
//! The layout is saved in `[[layout]]` (see [`crate::pane`]).

use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Status as Presence;
use hermes_client_core::{Core, InstanceId, view};
use iced::widget::pane_grid::{self, Axis, PaneGrid};
use iced::widget::{Space, button, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Border, Element, Fill};

use crate::buffer::channel::{self, ChannelView};
use crate::buffer::{Buffer, settings};
use crate::font::Fonts;
use crate::message::Message;
use crate::pane::{self, Pane};
use crate::resize::{Edge, Side};
use crate::sidebar::menu::Target;
use crate::theme::{Theme, style};
use crate::voice::VoiceMsg;
use crate::widget::icon::{self, icon};
use crate::widget::{ButtonKind, text_button};
use crate::widget::{caption, icon_button, label, tip};

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

    /// The channel view of the first pane showing `channel`.
    pub fn channel_view_mut(&mut self, inst: &InstanceId, channel: Id) -> Option<&mut ChannelView> {
        self.state
            .iter_mut()
            .find(|(_, p)| shows_channel(&p.buffer, inst, channel))
            .map(|(_, p)| &mut p.channel)
    }

    /// The channel view of the focused pane, if it shows a channel.
    pub fn focused_channel_view_mut(&mut self) -> Option<&mut ChannelView> {
        let pane = self.state.get_mut(self.focus)?;
        matches!(pane.buffer, Buffer::Channel { .. }).then_some(&mut pane.channel)
    }

    /// Ends message selection in every pane showing `channel`.
    pub fn clear_selection(&mut self, inst: &InstanceId, channel: Id) {
        for (_, p) in self.state.iter_mut() {
            if shows_channel(&p.buffer, inst, channel) {
                p.channel.clear_selection();
            }
        }
    }

    /// The name of what the focused pane shows, for "Return to …".
    pub fn focused_title(&self, core: &Core) -> String {
        match self.focused().map(|p| &p.buffer) {
            Some(buffer @ Buffer::Channel { .. }) => {
                let (name, _) = title(core, buffer);
                match title_icon(core, buffer) {
                    icon::HASH if !name.starts_with('#') => format!("#{name}"),
                    _ => name,
                }
            }
            Some(buffer) => title(core, buffer).0,
            None => "Home".into(),
        }
    }

    pub fn focused_channel(&self) -> Option<(InstanceId, Id)> {
        match &self.focused()?.buffer {
            Buffer::Channel { inst, channel } => Some((inst.clone(), *channel)),
            _ => None,
        }
    }

    /// The channel the focused pane shows, as its chat or its voice room (what the sidebar
    /// marks as selected).
    pub fn focused_target(&self) -> Option<(InstanceId, Id)> {
        match &self.focused()?.buffer {
            Buffer::Channel { inst, channel } | Buffer::VoiceRoom { inst, channel } => {
                Some((inst.clone(), *channel))
            }
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
        let grid = PaneGrid::new(&self.state, |id, pane, maximized| {
            let focused = id == self.focus;
            let theme = ctx.theme;
            let fonts = ctx.fonts;
            let (title, topic) = title(ctx.core, &pane.buffer);
            let glyph = title_icon(ctx.core, &pane.buffer);
            let title = match glyph {
                icon::HASH => title.trim_start_matches('#').to_owned(),
                _ => title,
            };
            let mut controls = row![].spacing(2).align_y(Alignment::Center);
            for (text, message) in voice_controls(ctx.core, &pane.buffer) {
                controls = controls.push(voice_button(text, ctx).on_press(message));
            }
            if let Buffer::Channel { inst, channel } = &pane.buffer {
                controls = controls.push(Space::new().width(style::GAP_S));
                controls = controls.push(tip(
                    icon_button(icon::PIN, false, theme).on_press(Message::OpenResults(
                        crate::buffer::results::Key::Pins(inst.clone(), *channel),
                    )),
                    "Pinned messages",
                    fonts,
                    theme,
                ));
                controls = controls.push(tip(
                    icon_button(icon::SEARCH, false, theme)
                        .on_press(Message::SearchFrom(inst.clone(), *channel)),
                    "Search (Ctrl+F)",
                    fonts,
                    theme,
                ));
                controls = controls.push(tip(
                    icon_button(icon::USERS, pane.members, theme)
                        .on_press(Message::PaneMenu(id, PaneAction::ToggleMembers)),
                    if pane.members {
                        "Hide members (Ctrl+U)"
                    } else {
                        "Show members (Ctrl+U)"
                    },
                    fonts,
                    theme,
                ));
            }
            if total > 1 {
                controls = controls.push(tip(
                    icon_button(
                        if maximized {
                            icon::RESTORE
                        } else {
                            icon::MAXIMIZE
                        },
                        false,
                        theme,
                    )
                    .on_press(Message::PaneMenu(id, PaneAction::Maximize)),
                    if maximized { "Restore" } else { "Maximize" },
                    fonts,
                    theme,
                ));
            }
            // No tooltip: it would cover the menu this opens.
            controls = controls.push(
                mouse_area(
                    icon_button(icon::MORE_VERTICAL, false, theme)
                        .on_press(Message::OpenMenu(Target::Pane(id))),
                )
                .on_right_press(Message::OpenMenu(Target::Pane(id))),
            );
            let mut heading = row![
                icon(glyph, style::ICON).color(theme.icons.default.0),
                crate::widget::title(title, fonts).wrapping(text::Wrapping::None),
            ]
            .spacing(style::GAP_S)
            .align_y(Alignment::Center);
            if !topic.is_empty() {
                heading = heading
                    .push(container(Space::new().width(1).height(18)).style(move |_| {
                        container::Style {
                            background: Some(theme.general.border.0.into()),
                            ..container::Style::default()
                        }
                    }))
                    .push(
                        caption(topic, fonts)
                            .color(theme.text.secondary.0)
                            .wrapping(text::Wrapping::None),
                    );
            }
            let split = total > 1;
            // Calm splits: the focused pane is marked by a gold line under its header; the
            // others show their header buttons only while hovered.
            let marked = split && focused;
            let mut title_bar =
                pane_grid::TitleBar::new(heading.height(style::ICON_BUTTON).clip(true))
                    .controls(pane_grid::Controls::new(
                        container(controls).height(style::ICON_BUTTON),
                    ))
                    .padding([style::GAP_S, style::GAP])
                    .style(move |_| {
                        let mut st = style::pane_header(theme);
                        if marked {
                            st.shadow = iced::Shadow {
                                color: theme.pane.focused_border.0,
                                offset: iced::Vector::new(0.0, 2.0),
                                blur_radius: 0.0,
                            };
                        }
                        st
                    });
            if !split || focused {
                title_bar = title_bar.always_show_controls();
            }

            pane_grid::Content::new(body(id, pane, ctx))
                .title_bar(title_bar)
                .style(move |_| container::Style {
                    background: Some(theme.surface.main.0.into()),
                    ..container::Style::default()
                })
        })
        // A 1 px gap over the border-colored backing reads as a divider line.
        .spacing(1)
        .on_click(Message::PaneClicked)
        .on_drag(Message::PaneDragged)
        .on_resize(6, Message::PaneResized);
        let divider = ctx.theme.general.border.0;
        container(grid)
            .style(move |_| container::Style {
                background: Some(divider.into()),
                ..container::Style::default()
            })
            .into()
    }
}

/// What a pane's view needs from the app.
pub struct Context<'a> {
    pub core: &'a Core,
    pub fonts: &'a Fonts,
    pub theme: &'a Theme,
    pub config: &'a hermes_client_core::config::ClientConfig,
    pub themes: &'a [(String, Result<Theme, String>)],
    pub utc_offset: i64,
    pub images: &'a channel::message::Images,
    pub composers: &'a std::collections::HashMap<(InstanceId, Id), channel::input::Composer>,
    pub settings_tab: settings::Tab,
    pub voice: settings::VoiceView<'a>,
    pub space_settings:
        &'a std::collections::HashMap<(InstanceId, Id), crate::buffer::space_settings::State>,
    /// Shown for a space whose settings were never opened (restored layouts).
    pub default_space_settings: &'a crate::buffer::space_settings::State,
    pub drag: &'a crate::drag::Drag,
    pub results:
        &'a std::collections::HashMap<crate::buffer::results::Key, crate::buffer::results::State>,
    pub default_results: &'a crate::buffer::results::State,
    pub account: crate::buffer::settings_tabs::AccountView<'a>,
    /// What the open right-click menu is about, to keep it highlighted.
    pub menu: Option<&'a Target>,
    /// The panel being resized, to mark its edge.
    pub resizing: Option<crate::resize::Edge>,
    /// The UI scale slider's value while dragged.
    pub ui_scale_draft: Option<f64>,
    /// What closing a settings page goes back to ("#general", "Home").
    pub return_to: String,
    /// The latest picture of each screen share I watch.
    pub screens: &'a std::collections::HashMap<(InstanceId, Id), crate::screen::view::Share>,
    /// The screen shares I chose to watch.
    pub watching_shares: &'a std::collections::HashSet<(InstanceId, Id)>,
    /// The screen share shown alone in its voice room.
    pub enlarged_share: Option<&'a (InstanceId, Id)>,
}

/// Actions from a pane header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneAction {
    ToggleMembers,
    Maximize,
}

/// A voice action in a pane header: joining stands out in the accent color, leaving in red.
fn voice_button<'a>(text: &'static str, ctx: &Context<'a>) -> button::Button<'a, Message> {
    let theme = ctx.theme;
    let glyph = match text {
        "Leave voice" => icon::HANG_UP,
        "Chat" => icon::CHAT,
        "Voice room" => icon::WAVEFORM,
        "Share screen" => icon::MONITOR_SHARE,
        "Stop sharing" => icon::MONITOR,
        _ => icon::PHONE,
    };
    let kind = match text {
        "Join voice" | "Start call" => 1,
        "Leave voice" | "Stop sharing" => 2,
        _ => 0,
    };
    // Exactly as tall as the icon buttons beside it at any text size, so every pane header is
    // as tall as the sidebar's.
    button(
        container(
            row![icon(glyph, 14.0), label(text, ctx.fonts)]
                .spacing(style::GAP_S)
                .align_y(Alignment::Center),
        )
        .center_y(Fill),
    )
    .height(style::ICON_BUTTON)
    .padding([0.0, style::BUTTON_PAD.left])
    .style(move |_, s| match kind {
        1 => style::primary(theme, s),
        2 => style::danger(theme, s),
        _ => style::ghost(theme, s, false),
    })
}

/// The icon in front of a pane's title.
fn title_icon(core: &Core, buffer: &Buffer) -> crate::widget::icon::Glyph {
    use crate::buffer::results::Key;
    use hermes_client_core::proto::models::ChannelKind;

    match buffer {
        Buffer::Empty => icon::HOME,
        Buffer::Settings => icon::SETTINGS,
        Buffer::SpaceSettings { .. } => icon::SHIELD,
        Buffer::VoiceRoom { .. } => icon::WAVEFORM,
        Buffer::Results(Key::Mentions) => icon::AT,
        Buffer::Results(Key::Pins(..)) => icon::PIN,
        Buffer::Results(Key::Search(..)) => icon::SEARCH,
        Buffer::Channel { inst, channel } => {
            let kind = core
                .store()
                .instance(inst)
                .and_then(|s| s.channel(*channel).map(|c| c.kind));
            match kind {
                Some(ChannelKind::Voice) => icon::VOLUME,
                Some(ChannelKind::Dm) => icon::AT,
                Some(ChannelKind::GroupDm) => icon::USERS,
                _ => icon::HASH,
            }
        }
    }
}

/// The voice buttons of a pane header: join / start call / leave and the voice room for voice
/// channels and DMs, the chat for a voice room.
pub fn voice_controls(core: &Core, buffer: &Buffer) -> Vec<(&'static str, Message)> {
    use hermes_client_core::proto::models::ChannelKind;

    let (inst, channel, room) = match buffer {
        Buffer::Channel { inst, channel } => (inst, *channel, false),
        Buffer::VoiceRoom { inst, channel } => (inst, *channel, true),
        _ => return Vec::new(),
    };
    let kind = core
        .store()
        .instance(inst)
        .and_then(|s| s.channel(channel).map(|c| c.kind));
    let Some(kind) = kind.filter(|k| {
        matches!(
            k,
            ChannelKind::Voice | ChannelKind::Dm | ChannelKind::GroupDm
        )
    }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if core.voice_channel() == Some((inst.clone(), channel)) {
        // Shares are seen in the voice room only by people in the call.
        if core.sharing_screen() {
            out.push(("Stop sharing", Message::Voice(VoiceMsg::ToggleShare)));
        } else {
            out.push(("Share screen", Message::Voice(VoiceMsg::OpenShareMenu)));
        }
        out.push(("Leave voice", Message::Voice(VoiceMsg::Leave)));
    } else {
        let others = {
            let store = core.store();
            store.instance(inst).is_some_and(|s| {
                let me = s.me_id();
                s.voice_states
                    .values()
                    .any(|v| v.channel_id == channel && Some(v.user_id) != me)
            })
        };
        let text = if kind != ChannelKind::Voice && !others {
            "Start call"
        } else {
            "Join voice"
        };
        out.push((text, Message::Voice(VoiceMsg::Join(inst.clone(), channel))));
    }
    if room {
        out.push(("Chat", Message::OpenChat(inst.clone(), channel)));
    } else {
        out.push((
            "Voice room",
            Message::Voice(VoiceMsg::OpenRoom(inst.clone(), channel)),
        ));
    }
    out
}

fn shows_channel(buffer: &Buffer, inst: &InstanceId, channel: Id) -> bool {
    matches!(buffer, Buffer::Channel { inst: i, channel: c } if i == inst && *c == channel)
}

/// The header title and topic of a buffer.
pub fn title(core: &Core, buffer: &Buffer) -> (String, String) {
    match buffer {
        Buffer::Empty => ("Home".into(), String::new()),
        Buffer::Settings => ("Settings".into(), String::new()),
        Buffer::Results(key) => crate::buffer::results::title(core, key),
        Buffer::SpaceSettings { inst, space } => {
            let store = core.store();
            let name = store
                .instance(inst)
                .and_then(|s| s.spaces.get(space))
                .map_or_else(|| "(not available)".to_owned(), |sp| sp.space.name.clone());
            (name, "Space settings".into())
        }
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
        Buffer::VoiceRoom { inst, channel } => {
            let store = core.store();
            match store
                .instance(inst)
                .and_then(|s| Some((s, s.channel(*channel)?)))
            {
                Some((s, c)) => (view::channel_title(s, c), "Voice room".into()),
                None => ("(not available)".into(), String::new()),
            }
        }
    }
}

/// `active`: the focused pane, or the only one; only it shows its member list, so a split
/// window has at most one.
fn body<'a>(id: pane_grid::Pane, pane: &'a Pane, ctx: &Context<'a>) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    match &pane.buffer {
        Buffer::Empty => {
            let accent = theme.accent.base.0;
            let badge = container(icon(icon::FEATHER, 30.0).color(accent))
                .center_x(64)
                .center_y(64)
                .style(move |_| container::Style {
                    background: Some(accent.scale_alpha(0.14).into()),
                    border: Border {
                        radius: 32.0.into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                });
            let mut welcome = column![badge, crate::widget::display("Welcome to Hermes", fonts),]
                .spacing(style::GAP_L)
                .align_x(Alignment::Center);
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
                        button(
                            row![icon(icon::PLUS, 14.0), label("Add an instance", fonts)]
                                .spacing(style::GAP_S)
                                .align_y(Alignment::Center),
                        )
                        .padding(style::BUTTON_PAD)
                        .style(move |_, s| style::primary(theme, s))
                        .on_press(Message::OpenAddInstance),
                    );
            } else {
                let first = ctx.core.instance_ids().first().cloned();
                let action = |glyph: crate::widget::icon::Glyph,
                              title: &'static str,
                              hint: &'static str,
                              message| {
                    welcome_action(glyph, title, hint, message, fonts, theme)
                };
                let mut grid = column![
                    row![
                        action(
                            icon::SEARCH,
                            "Find anything",
                            "Channels, people · Ctrl+K",
                            Message::Shortcut(crate::keybinds::Action::CommandBar),
                        ),
                        action(
                            icon::CHAT,
                            "Search messages",
                            "All your chats · Ctrl+F",
                            Message::Shortcut(crate::keybinds::Action::Search),
                        ),
                    ]
                    .spacing(style::GAP),
                    row![
                        action(
                            icon::LINK,
                            "Join with an invite",
                            "Paste a friend's link",
                            Message::OpenAddInstance,
                        ),
                        action(
                            icon::SETTINGS,
                            "Settings",
                            "Look and sound · Ctrl+,",
                            Message::OpenSettings,
                        ),
                    ]
                    .spacing(style::GAP),
                ]
                .spacing(style::GAP);
                if let Some(inst) = first {
                    grid = grid.push(
                        row![
                            action(
                                icon::PLUS,
                                "Create a space",
                                "Channels and roles",
                                Message::Menu(
                                    Target::Instance(inst),
                                    crate::sidebar::menu::Action::CreateSpace,
                                ),
                            ),
                            action(
                                icon::AT,
                                "Mentions",
                                "Messages about you",
                                Message::OpenResults(crate::buffer::results::Key::Mentions),
                            ),
                        ]
                        .spacing(style::GAP),
                    );
                }
                welcome = welcome
                    .push(
                        label("Pick a channel in the sidebar, or start here.", fonts)
                            .color(theme.text.secondary.0),
                    )
                    .push(Space::new().height(style::GAP_S))
                    .push(grid)
                    .push(Space::new().height(style::GAP_S))
                    .push(
                        caption(
                            "Right-click for actions · double-click voice to join",
                            fonts,
                        )
                        .color(theme.text.tertiary.0)
                        .align_x(Alignment::Center),
                    );
            }
            container(welcome.max_width(600)).center(Fill).into()
        }
        Buffer::Settings | Buffer::SpaceSettings { .. } => page_view(&pane.buffer, ctx),
        Buffer::VoiceRoom { inst, channel } => {
            let enlarged = ctx
                .enlarged_share
                .filter(|(i, _)| i == inst)
                .map(|(_, user)| *user);
            crate::buffer::voice_room::view(
                ctx.core,
                inst,
                *channel,
                &crate::buffer::voice_room::Shares {
                    pictures: ctx.screens,
                    watching: ctx.watching_shares,
                    enlarged,
                    preference: crate::screen::capture::Preference::from_config(
                        &ctx.config.voice.share_preference,
                    ),
                },
                ctx.images,
                fonts,
                theme,
            )
        }
        Buffer::Results(key) => {
            let state = ctx.results.get(key).unwrap_or(ctx.default_results);
            let key = key.clone();
            crate::buffer::results::view(ctx.core, &key, state, ctx.utc_offset, fonts, theme)
                .map(move |m| Message::Results(key.clone(), m))
        }
        Buffer::Channel { inst, channel } => {
            let messages = channel::view(
                &channel::Context {
                    core: ctx.core,
                    config: ctx.config,
                    fonts,
                    theme,
                    utc_offset: ctx.utc_offset,
                    images: ctx.images,
                    composer: ctx.composers.get(&(inst.clone(), *channel)),
                    menu_message: match ctx.menu {
                        Some(Target::Message(i, c, m)) if i == inst && c == channel => Some(*m),
                        _ => None,
                    },
                },
                inst,
                *channel,
                &pane.channel,
            )
            .map(move |m| Message::Channel(id, m));
            // The same shape whatever shows, so toggling the member list or a banner keeps the
            // messages' state (their scroll position).
            let banner: Element<'a, Message> =
                connection_banner(ctx, inst).unwrap_or_else(|| Space::new().into());
            let members: Element<'a, Message> = if pane.members {
                member_list(ctx, inst, *channel)
            } else {
                Space::new().into()
            };
            row![column![banner, messages], members].into()
        }
    }
}

/// A line above the messages while the instance is not connected, saying what that means.
fn connection_banner<'a>(ctx: &Context<'a>, inst: &InstanceId) -> Option<Element<'a, Message>> {
    use hermes_client_core::gateway::ConnStatus;

    let theme = ctx.theme;
    let fonts = ctx.fonts;
    let (text, action): (String, Option<(&'static str, Message)>) = match ctx.core.status(inst) {
        ConnStatus::Connected => return None,
        ConnStatus::Connecting => ("Connecting…".into(), None),
        ConnStatus::Reconnecting { attempt } => (
            format!(
                "Connection lost, reconnecting (attempt {attempt})… Messages you send now are                  sent when it is back."
            ),
            None,
        ),
        ConnStatus::Failed { reason } => (
            format!("Cannot connect: {reason}"),
            Some((
                "Try again",
                Message::Menu(
                    Target::Instance(inst.clone()),
                    crate::sidebar::menu::Action::Reconnect,
                ),
            )),
        ),
        ConnStatus::LoggedOut => (
            "You are signed out of this server.".into(),
            Some(("Sign in", Message::SignIn(inst.clone()))),
        ),
        ConnStatus::Offline => (
            "You went offline for this server.".into(),
            Some((
                "Go online",
                Message::Menu(
                    Target::Instance(inst.clone()),
                    crate::sidebar::menu::Action::GoOnline,
                ),
            )),
        ),
    };
    let mut line = row![caption(text, fonts).width(Fill)]
        .spacing(style::GAP_S)
        .align_y(Alignment::Center);
    if let Some((label_text, message)) = action {
        line = line
            .push(text_button(label_text, ButtonKind::Secondary, fonts, theme).on_press(message));
    }
    Some(
        container(line)
            .padding([style::GAP_XS, style::GAP_S])
            .width(Fill)
            .style(move |_| style::status_bar(theme, crate::app::Level::Warning))
            .into(),
    )
}

/// Settings or a space's settings: shown over the whole window (and, for layouts saved before
/// they moved there, inside a pane).
pub fn page_view<'a>(buffer: &Buffer, ctx: &Context<'a>) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    match buffer {
        Buffer::Settings => settings::view(
            ctx.config,
            ctx.themes,
            ctx.settings_tab,
            ctx.ui_scale_draft,
            &ctx.return_to,
            &ctx.voice,
            &ctx.account,
            fonts,
            theme,
        )
        .map(Message::Settings),
        Buffer::SpaceSettings { inst, space } => {
            let store = ctx.core.store();
            let Some(s) = store.instance(inst) else {
                return container(label("This instance is not available.", fonts))
                    .center(Fill)
                    .into();
            };
            let state = ctx
                .space_settings
                .get(&(inst.clone(), *space))
                .unwrap_or(ctx.default_space_settings);
            let drop_target = ctx
                .drag
                .target()
                .filter(|t| t.inst == *inst && t.space == *space)
                .map(|t| (t.kind, t.id));
            let (inst, space) = (inst.clone(), *space);
            crate::buffer::space_settings::view(&crate::buffer::space_settings::Ctx {
                s,
                space,
                state,
                fonts,
                theme,
                utc_offset: ctx.utc_offset,
                drop_target,
                return_to: &ctx.return_to,
                inst: &inst,
                images: ctx.images,
            })
            .map(move |m| Message::SpaceSettings(inst.clone(), space, m))
        }
        _ => Space::new().into(),
    }
}

/// A card on the welcome page: an icon tile, a title and a hint.
fn welcome_action<'a>(
    glyph: crate::widget::icon::Glyph,
    title: &'static str,
    hint: &'static str,
    message: Message,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let accent = theme.accent.base.0;
    button(
        row![
            container(icon(glyph, 18.0).color(accent))
                .center_x(38)
                .center_y(38)
                .style(move |_| container::Style {
                    background: Some(accent.scale_alpha(0.12).into()),
                    border: Border {
                        radius: style::radius(theme).into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                }),
            column![
                label(title, fonts).font(fonts.bold()),
                caption(hint, fonts).color(theme.text.tertiary.0),
            ]
            .spacing(3),
        ]
        .spacing(style::GAP)
        .align_y(Alignment::Center),
    )
    .padding(style::GAP)
    .width(286)
    .height(68)
    .style(move |_, s| {
        let hovered = matches!(s, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(
                if hovered {
                    theme.surface.hover.0
                } else {
                    theme.text.code_bg.0
                }
                .into(),
            ),
            text_color: theme.text.primary.0,
            border: Border {
                color: if hovered {
                    accent.scale_alpha(0.5)
                } else {
                    theme.general.border.0
                },
                width: 1.0,
                radius: style::radius_large(theme).into(),
            },
            ..button::Style::default()
        }
    })
    .on_press(message)
    .into()
}

/// The members of the channel's space by hoisted role, or a DM's recipients.
/// Avatar size in the member list.
const MEMBER_AVATAR: f32 = 28.0;

fn member_list<'a>(ctx: &Context<'a>, inst: &InstanceId, channel: Id) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let store = ctx.core.store();
    let mut list = column![].spacing(2).padding([style::GAP, style::GAP_S]);
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
                container(
                    text(format!(
                        "{} — {}",
                        group.title.to_uppercase(),
                        group.members.len()
                    ))
                    .font(fonts.bold())
                    .size(fonts.caption())
                    .color(theme.text.tertiary.0),
                )
                .padding([style::GAP_S, style::GAP_S]),
            );
            for member in group.members {
                let offline = member.status == Presence::Offline;
                let dot = style::presence(theme, member.status);
                let name_color = member
                    .color
                    .map_or(theme.text.primary.0, style::rgb)
                    .scale_alpha(if offline { 0.5 } else { 1.0 });
                let picture =
                    crate::widget::avatar::picture(ctx.images, inst, s.user(member.user_id));
                let face = crate::widget::avatar::view(
                    &member.name,
                    picture.as_ref(),
                    MEMBER_AVATAR,
                    fonts,
                );
                let face = if offline {
                    face
                } else {
                    crate::widget::avatar::with_dot(
                        face,
                        MEMBER_AVATAR,
                        dot,
                        theme.surface.sidebar.0,
                    )
                };
                let target = Target::Member(inst.clone(), s.space_of(channel), member.user_id);
                let open = ctx.menu == Some(&target);
                list = list.push(
                    mouse_area(
                        button(
                            container(
                                row![
                                    face,
                                    label(member.name, fonts)
                                        .color(name_color)
                                        .wrapping(text::Wrapping::None),
                                ]
                                .spacing(style::GAP)
                                .align_y(Alignment::Center),
                            )
                            .center_y(Fill),
                        )
                        .height(style::ROW_H + style::GAP_XS)
                        .padding([0.0, style::GAP_S])
                        .width(Fill)
                        .style(move |_, st| {
                            if open {
                                style::menu_open(theme)
                            } else {
                                style::sidebar_item(theme, st, false)
                            }
                        })
                        .on_press(Message::OpenMenu(target.clone())),
                    )
                    .on_right_press(Message::OpenMenu(target)),
                );
            }
            list = list.push(Space::new().height(style::GAP));
        }
    }
    let width = Edge::Members.clamp(ctx.config.sidebar.member_list_width);
    crate::resize::with_handle(
        container(
            scrollable(list)
                .height(Fill)
                .style(move |_, st| style::scrollbar(theme, st)),
        )
        .width(f32::from(width))
        .height(Fill)
        .style(move |_| style::sidebar(theme)),
        Edge::Members,
        Side::Left,
        ctx.resizing == Some(Edge::Members),
        theme,
    )
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
