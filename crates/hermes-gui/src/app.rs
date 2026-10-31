//! App state, update and view (PLAN.md section 15.8). All side effects go through `Core`.

use std::path::PathBuf;

use hermes_client_core::config::ClientConfig;
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::api::CreateInviteRequest;
use hermes_client_core::proto::id::Id;
use hermes_client_core::store::{Changed, HistoryAnchor};
use hermes_client_core::{Core, CoreEvent, InstanceId, time};
use iced::keyboard::{self, Modifiers};
use iced::widget::pane_grid::Axis;
use iced::widget::{button, column, container, operation, row};
use iced::{Element, Fill, Point, Size, Subscription, Task, window};

use crate::buffer::{Buffer, channel, settings};
use crate::font::Fonts;
use crate::keybinds::{self, Keybinds};
use crate::message::Message;
use crate::modal::{self, add_instance, command_bar, confirm, new_dm};
use crate::panes::{self, PaneAction, Panes};
use crate::sidebar::collapsed::Collapsed;
use crate::sidebar::menu::{self, Action, Target};
use crate::sidebar::tree::{self, Row};
use crate::single_instance::Endpoint;
use crate::subscription::{self, CoreEventsHandle};
use crate::theme::{self, Theme, style};
use crate::widget::context_menu;
use crate::widget::cursor::{self, Cursor};
use crate::widget::small;

/// A line shown above the main area until dismissed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub level: Level,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

pub struct App {
    pub core: Core,
    events: Option<CoreEventsHandle>,
    /// What the views show. Equal to `core.config()` except while a slider is being dragged.
    pub config: ClientConfig,
    pub theme: Theme,
    pub iced_theme: iced::Theme,
    pub themes: Vec<String>,
    pub fonts: Fonts,
    pub status: Option<Status>,
    pub panes: Panes,
    pub keybinds: Keybinds,
    pub sidebar_visible: bool,
    pub command_bar: Option<command_bar::CommandBar>,
    /// Local UTC offset for timestamps, read once at startup.
    pub utc_offset: i64,
    /// The window has the keyboard focus.
    pub focused: bool,
    /// The add-instance / sign-in dialog.
    pub add_instance: Option<add_instance::AddInstance>,
    /// Where later launches forward their links (section 15.7).
    single_instance: Option<Endpoint>,
    pub collapsed: Collapsed,
    /// An open context menu and where it was opened.
    pub menu: Option<(Target, Point)>,
    cursor: Cursor,
    window_size: Size,
    /// A destructive menu action waiting for confirmation.
    pub confirm: Option<(Target, Action)>,
    pub new_dm: Option<new_dm::NewDm>,
}

impl App {
    pub fn new(
        core: Core,
        events: Option<CoreEventsHandle>,
        single_instance: Option<Endpoint>,
    ) -> App {
        let config = core.config();
        let collapsed = core
            .paths()
            .map(|p| Collapsed::load(&p.dir))
            .unwrap_or_default();
        let (keybinds, key_warnings) = Keybinds::from_config(&config.keys);
        let mut app = App {
            panes: Panes::restore(&config.layout),
            keybinds,
            sidebar_visible: true,
            command_bar: None,
            utc_offset: time::local_offset_secs(),
            focused: true,
            fonts: Fonts::from_config(&config.font),
            theme: Theme::dark(),
            iced_theme: style::iced_theme(&Theme::dark()),
            themes: Vec::new(),
            config,
            core,
            events,
            status: None,
            add_instance: None,
            single_instance,
            collapsed,
            menu: None,
            cursor: Cursor::default(),
            window_size: Size::new(1200.0, 800.0),
            confirm: None,
            new_dm: None,
        };
        app.reload_theme();
        if let Some(warning) = key_warnings.into_iter().next() {
            app.warn(warning);
        }
        app.core.set_visible_channels(app.panes.visible_channels());
        app
    }

    /// Loads the first page of every channel the restored layout shows.
    pub fn open_restored(&self) -> Task<Message> {
        Task::batch(
            self.panes
                .visible_channels()
                .into_iter()
                .map(|(inst, channel)| self.load_channel(inst, channel)),
        )
    }

    pub fn themes_dir(&self) -> Option<PathBuf> {
        self.core.paths().map(|p| p.dir.join(theme::THEMES_DIR))
    }

    pub fn title(&self) -> String {
        "Hermes".to_owned()
    }

    pub fn theme(&self) -> iced::Theme {
        self.iced_theme.clone()
    }

    pub fn scale_factor(&self) -> f32 {
        self.fonts.scale
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Core(event) => self.on_core(event),
            Message::ThemesChanged => self.reload_theme(),
            Message::WindowFocused(focused) => {
                self.focused = focused;
                self.core.set_app_focused(focused);
                if focused {
                    self.mark_visible_read();
                }
            }
            Message::OpenSettings => self.panes.open(Buffer::Settings),
            Message::Settings(message) => self.on_settings(message),
            Message::DismissStatus => self.status = None,
            Message::FontLoaded | Message::ChannelOpened => {}
            Message::OpenAddInstance => return self.open_add_instance(String::new()),
            Message::SignIn(inst) => return self.sign_in(&inst),
            Message::AddInstance(message) => return self.on_add_instance(message),
            Message::Toggle(node) => {
                if let Err(err) = self.collapsed.toggle(&node) {
                    self.error(format!("could not save the sidebar state: {err}"));
                }
            }
            Message::Select(inst, channel) => return self.select(inst, channel),
            Message::PaneClicked(pane) => self.panes.focus = pane,
            Message::PaneDragged(event) => self.panes.on_drag(event),
            Message::PaneResized(event) => self.panes.state.resize(event.split, event.ratio),
            Message::PaneMenu(pane, action) => match action {
                PaneAction::ToggleMembers => self.panes.toggle_members(pane),
                PaneAction::Maximize => self.panes.toggle_maximize(pane),
            },
            Message::Key(key, physical, modifiers, captured) => {
                return self.on_key(&key, physical, modifiers, captured);
            }
            Message::CloseRequested(id) => {
                self.save_layout();
                return window::close(id);
            }
            Message::CommandBar(message) => return self.on_command_bar(message),
            Message::Channel(pane, message) => return self.on_channel(pane, message),
            Message::OpenMenu(target) => self.menu = Some((target, self.cursor.get())),
            Message::CloseMenu => self.menu = None,
            Message::Menu(target, action) => {
                self.menu = None;
                if action.is_destructive() {
                    self.confirm = Some((target, action));
                } else {
                    return self.run_action(target, action);
                }
            }
            Message::Confirmed => {
                if let Some((target, action)) = self.confirm.take() {
                    return self.run_action(target, action);
                }
            }
            Message::CancelConfirm => self.confirm = None,
            Message::NewDm(inst) => {
                self.new_dm = Some(new_dm::NewDm::new(inst));
                return operation::focus(iced::widget::Id::new(new_dm::INPUT));
            }
            Message::NewDmMsg(message) => return self.on_new_dm(message),
            Message::DmCreated(inst, result) => match result {
                Ok(channel) => {
                    self.new_dm = None;
                    return self.select(inst, channel);
                }
                Err(err) => {
                    if let Some(dialog) = &mut self.new_dm {
                        dialog.busy = false;
                        dialog.error = Some(err);
                    }
                }
            },
            Message::Copy(text, what) => {
                self.info(format!("{what} copied"));
                return iced::clipboard::write(text);
            }
            Message::Done(result) => match result {
                Ok(text) => self.info(text),
                Err(text) => self.error(text),
            },
            Message::WindowResized(size) => self.window_size = size,
            Message::Escape => {
                if self.menu.is_some() {
                    self.menu = None;
                } else if self.command_bar.is_some() {
                    self.command_bar = None;
                } else if self.confirm.is_some() {
                    self.confirm = None;
                } else if self.new_dm.is_some() {
                    self.new_dm = None;
                } else if self.add_instance.is_some() {
                    self.add_instance = None;
                } else {
                    return self.key_action(keybinds::Action::MarkRead);
                }
            }
            Message::Forwarded(link) => {
                tracing::info!(
                    "another launch handed over {}",
                    if link.is_empty() { "nothing" } else { "a link" }
                );
                let show = window::oldest().then(|id| match id {
                    Some(id) => window::minimize(id, false).chain(window::gain_focus(id)),
                    None => Task::none(),
                });
                let open = if link.is_empty() {
                    Task::none()
                } else {
                    self.open_link(link)
                };
                return show.chain(open);
            }
        }
        Task::none()
    }

    /// Shows a channel in the focused pane and loads its latest messages.
    fn select(&mut self, inst: InstanceId, channel: Id) -> Task<Message> {
        let marker = self
            .core
            .store()
            .instance(&inst)
            .and_then(|s| s.read_states.get(&channel).map(|r| r.last_read_id));
        self.panes.open_with_marker(
            Buffer::Channel {
                inst: inst.clone(),
                channel,
            },
            marker,
        );
        self.load_channel(inst, channel)
    }

    fn on_channel(
        &mut self,
        pane: iced::widget::pane_grid::Pane,
        message: channel::Message,
    ) -> Task<Message> {
        let Some(Buffer::Channel { inst, channel }) =
            self.panes.state.get(pane).map(|p| p.buffer.clone())
        else {
            return Task::none();
        };
        let Some(view) = self.panes.state.get_mut(pane).map(|p| &mut p.channel) else {
            return Task::none();
        };
        match message {
            channel::Message::Scrolled(viewport) => {
                view.scrolled(&viewport);
                let older = view.near_top() && !view.loading_older;
                if view.at_bottom() {
                    self.mark_read_if_visible(pane);
                }
                if older {
                    return self.load_older(pane, inst, channel);
                }
            }
            channel::Message::Measured(key, height) => {
                view.heights.set(key, height);
            }
            channel::Message::OlderLoaded => view.loading_older = false,
            channel::Message::JumpToPresent => {
                let id = view.scroll_id.clone();
                let live = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.window(channel).map(|w| w.reached_end))
                    .unwrap_or(false);
                // Anchored to the bottom: offset 0 is the newest message.
                let snap = operation::snap_to(
                    id,
                    iced::widget::scrollable::RelativeOffset { x: 0.0, y: 0.0 },
                );
                if live {
                    return snap;
                }
                let core = self.core.clone();
                return Task::perform(
                    async move {
                        core.load_history(&inst, channel, HistoryAnchor::Latest)
                            .await
                    },
                    |result| match result {
                        Ok(()) => Message::ChannelOpened,
                        Err(err) => Message::Done(Err(format!("could not load messages: {err}"))),
                    },
                )
                .chain(snap);
            }
            channel::Message::Retry(local_id) => {
                if let Err(err) = self.core.retry_message(&inst, channel, local_id) {
                    self.error(err.to_string());
                }
            }
            channel::Message::Discard(local_id) => {
                self.core.discard_message(&inst, channel, local_id);
            }
        }
        Task::none()
    }

    /// Loads the page before the oldest loaded message of a pane's channel.
    fn load_older(
        &mut self,
        pane: iced::widget::pane_grid::Pane,
        inst: InstanceId,
        channel: Id,
    ) -> Task<Message> {
        let oldest = {
            let store = self.core.store();
            let Some(window) = store.instance(&inst).and_then(|s| s.window(channel)) else {
                return Task::none();
            };
            if !window.loaded || window.reached_start {
                return Task::none();
            }
            match window.messages.first() {
                Some(m) => m.id,
                None => return Task::none(),
            }
        };
        if let Some(p) = self.panes.state.get_mut(pane) {
            p.channel.loading_older = true;
        }
        let core = self.core.clone();
        Task::perform(
            async move {
                core.load_history(&inst, channel, HistoryAnchor::Before(oldest))
                    .await
            },
            move |result| {
                if let Err(err) = result {
                    tracing::warn!("older messages: {err}");
                }
                Message::Channel(pane, channel::Message::OlderLoaded)
            },
        )
    }

    /// Marks a pane's channel read when its newest message is on screen and the window has the
    /// focus (section 15.2).
    fn mark_read_if_visible(&self, pane: iced::widget::pane_grid::Pane) {
        if !self.focused {
            return;
        }
        let Some(p) = self.panes.state.get(pane) else {
            return;
        };
        let Buffer::Channel { inst, channel } = &p.buffer else {
            return;
        };
        if !p.channel.at_bottom() {
            return;
        }
        let last = {
            let store = self.core.store();
            let Some(s) = store.instance(inst) else {
                return;
            };
            let Some(window) = s.window(*channel) else {
                return;
            };
            if !window.reached_end {
                return;
            }
            let Some(last) = window
                .messages
                .iter()
                .rev()
                .find(|m| !m.deleted)
                .map(|m| m.id)
            else {
                return;
            };
            if s.read_states
                .get(channel)
                .is_some_and(|r| r.last_read_id >= last)
            {
                return;
            }
            last
        };
        self.core.mark_read(inst, *channel, last);
    }

    fn mark_visible_read(&self) {
        let panes: Vec<_> = self.panes.state.iter().map(|(id, _)| *id).collect();
        for pane in panes {
            self.mark_read_if_visible(pane);
        }
    }

    fn load_channel(&self, inst: InstanceId, channel: Id) -> Task<Message> {
        self.core
            .set_visible_channels(self.panes.visible_channels());
        let core = self.core.clone();
        Task::perform(
            async move { core.open_channel(&inst, channel).await },
            |result| match result {
                Ok(()) => Message::ChannelOpened,
                Err(err) => Message::Done(Err(format!("could not open the channel: {err}"))),
            },
        )
    }

    fn run_action(&mut self, target: Target, action: Action) -> Task<Message> {
        let core = self.core.clone();
        match (target, action) {
            (Target::Instance(inst), Action::Reconnect) => {
                let result = core
                    .set_online(&inst, false)
                    .and_then(|()| core.set_online(&inst, true));
                if let Err(err) = result {
                    self.error(err.to_string());
                }
            }
            (Target::Instance(inst), Action::GoOffline | Action::GoOnline) => {
                if let Err(err) = core.set_online(&inst, action == Action::GoOnline) {
                    self.error(err.to_string());
                }
            }
            (Target::Instance(inst), Action::LogOut) => {
                return Task::perform(
                    async move {
                        core.logout(&inst)
                            .await
                            .map(|()| format!("Logged out of {inst}"))
                            .map_err(|e| e.to_string())
                    },
                    Message::Done,
                );
            }
            (Target::Instance(inst), Action::RemoveInstance) => {
                self.panes
                    .clear_where(|b| matches!(b, Buffer::Channel { inst: i, .. } if *i == inst));
                return Task::perform(
                    async move {
                        core.remove_instance(&inst)
                            .await
                            .map(|()| format!("Removed {inst}"))
                            .map_err(|e| e.to_string())
                    },
                    Message::Done,
                );
            }
            (Target::Space(inst, space), Action::InvitePeople) => {
                return Task::perform(
                    async move {
                        let invite = core
                            .rest(&inst)?
                            .create_invite(space, &CreateInviteRequest::default())
                            .await?;
                        Ok::<_, hermes_client_core::CoreError>(
                            core.invite_link(&inst, &invite.code),
                        )
                    },
                    |result| match result {
                        Ok(Some(link)) => Message::Copy(link, "Invite link"),
                        Ok(None) => Message::Done(Err("no such instance".into())),
                        Err(err) => {
                            Message::Done(Err(format!("could not create an invite: {err}")))
                        }
                    },
                );
            }
            (Target::Space(_, space), Action::CopySpaceId) => {
                return self.update(Message::Copy(space.to_string(), "Space ID"));
            }
            (Target::Space(inst, space), Action::LeaveSpace) => {
                self.panes.clear_where(|b| match b {
                    Buffer::Channel { inst: i, channel } => {
                        *i == inst
                            && core.store().instance(i).and_then(|s| s.space_of(*channel))
                                == Some(space)
                    }
                    _ => false,
                });
                return Task::perform(
                    async move {
                        core.rest(&inst)?.leave_space(space).await?;
                        Ok::<_, hermes_client_core::CoreError>(())
                    },
                    |result| match result {
                        Ok(()) => Message::Done(Ok("Left the space".into())),
                        Err(err) => Message::Done(Err(format!("could not leave: {err}"))),
                    },
                );
            }
            (Target::Channel(inst, channel), Action::MarkRead) => {
                let last = core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.channel(channel))
                    .and_then(|c| c.last_message_id);
                if let Some(last) = last {
                    core.mark_read(&inst, channel, last);
                }
            }
            (Target::Channel(_, channel), Action::CopyChannelId) => {
                return self.update(Message::Copy(channel.to_string(), "Channel ID"));
            }
            (Target::Pane(pane), Action::SplitRight) => self.panes.split(pane, Axis::Vertical),
            (Target::Pane(pane), Action::SplitDown) => self.panes.split(pane, Axis::Horizontal),
            (Target::Pane(pane), Action::MaximizePane) => self.panes.toggle_maximize(pane),
            (Target::Pane(pane), Action::ToggleMembers) => self.panes.toggle_members(pane),
            (Target::Pane(pane), Action::ClosePane) => {
                self.panes.close(pane);
                self.core
                    .set_visible_channels(self.panes.visible_channels());
            }
            (target, action) => {
                tracing::warn!("menu action {action:?} does not apply to {target:?}")
            }
        }
        Task::none()
    }

    /// A key press. While a dialog or menu is open only Escape and the command bar keys count;
    /// keys a text input used only count with Ctrl, Alt or Super.
    fn on_key(
        &mut self,
        key: &keyboard::Key,
        physical: keyboard::key::Physical,
        modifiers: Modifiers,
        captured: bool,
    ) -> Task<Message> {
        use keyboard::key::Named;

        let escape = *key == keyboard::Key::Named(Named::Escape);
        if let Some(bar) = &mut self.command_bar {
            match key {
                keyboard::Key::Named(Named::ArrowDown) => bar.move_selection(1),
                keyboard::Key::Named(Named::ArrowUp) => bar.move_selection(-1),
                _ if escape => self.command_bar = None,
                _ => {}
            }
            return Task::none();
        }
        let overlay = self.menu.is_some()
            || self.confirm.is_some()
            || self.new_dm.is_some()
            || self.add_instance.is_some();
        if escape && (overlay || !captured) {
            return self.update(Message::Escape);
        }
        if overlay || (captured && !(modifiers.control() || modifiers.alt() || modifiers.logo())) {
            return Task::none();
        }
        match self.keybinds.action(key, physical, modifiers) {
            Some(action) => self.key_action(action),
            None => Task::none(),
        }
    }

    fn key_action(&mut self, action: keybinds::Action) -> Task<Message> {
        use keybinds::Action as K;

        let focus = self.panes.focus;
        match action {
            K::CommandBar => {
                let store = self.core.store();
                let entries = command_bar::entries(
                    &store,
                    &self.core.instance_ids(),
                    &self.themes,
                    &self.config.keys,
                );
                drop(store);
                self.command_bar = Some(command_bar::CommandBar::new(entries));
                return operation::focus(iced::widget::Id::new(command_bar::INPUT));
            }
            K::NextChannel => return self.step_channel(1, false),
            K::PrevChannel => return self.step_channel(-1, false),
            K::NextUnread => return self.step_channel(1, true),
            K::PrevUnread => return self.step_channel(-1, true),
            K::FocusNextPane => self.panes.cycle_focus(1),
            K::FocusPrevPane => self.panes.cycle_focus(-1),
            K::SplitRight => self.panes.split(focus, Axis::Vertical),
            K::SplitDown => self.panes.split(focus, Axis::Horizontal),
            K::ClosePane => {
                self.panes.close(focus);
                self.core
                    .set_visible_channels(self.panes.visible_channels());
            }
            K::MaximizePane => self.panes.toggle_maximize(focus),
            K::ToggleMemberList => self.panes.toggle_members(focus),
            K::ToggleSidebar => self.sidebar_visible = !self.sidebar_visible,
            K::Settings => self.panes.open(Buffer::Settings),
            K::MarkRead => {
                if let Some((inst, channel)) = self.panes.focused_channel() {
                    return self.run_action(Target::Channel(inst, channel), Action::MarkRead);
                }
            }
            K::Search => self.info("Search comes in a later version".into()),
            K::ToggleMute | K::ToggleDeafen => {
                self.info("Voice comes in a later version".into());
            }
        }
        Task::none()
    }

    /// Opens the next or previous channel of the sidebar (only unread ones if `unread`),
    /// wrapping around.
    fn step_channel(&mut self, delta: i32, unread: bool) -> Task<Message> {
        let channels: Vec<(InstanceId, Id, bool)> = self
            .sidebar_rows()
            .into_iter()
            .filter_map(|row| match row {
                Row::Channel {
                    inst, id, badge, ..
                }
                | Row::Dm {
                    inst, id, badge, ..
                } => Some((inst, id, badge.unread || badge.mentions > 0)),
                _ => None,
            })
            .collect();
        if channels.is_empty() {
            return Task::none();
        }
        let current = self.panes.focused_channel();
        let start =
            current.and_then(|(i, c)| channels.iter().position(|(ci, cc, _)| *ci == i && *cc == c));
        let len = channels.len() as i32;
        for step in 1..=len {
            let index = match start {
                Some(s) => (s as i32 + delta * step).rem_euclid(len),
                None if delta > 0 => step - 1,
                None => len - step,
            } as usize;
            let (inst, id, is_unread) = &channels[index];
            if !unread || *is_unread {
                return self.select(inst.clone(), *id);
            }
        }
        Task::none()
    }

    fn on_command_bar(&mut self, message: command_bar::Message) -> Task<Message> {
        let Some(bar) = &mut self.command_bar else {
            return Task::none();
        };
        let command = match message {
            command_bar::Message::Input(text) => {
                bar.set_input(text);
                return Task::none();
            }
            command_bar::Message::Close => {
                self.command_bar = None;
                return Task::none();
            }
            command_bar::Message::Submit => bar.command(bar.selected),
            command_bar::Message::Run(n) => bar.command(n),
        };
        self.command_bar = None;
        match command {
            Some(command_bar::Command::Open(inst, channel)) => self.select(inst, channel),
            Some(command_bar::Command::MessageUser(inst, user)) => self.message_user(inst, user),
            Some(command_bar::Command::Action(action)) => self.key_action(action),
            Some(command_bar::Command::Theme(name)) => {
                self.on_settings(settings::Message::Theme(name));
                Task::none()
            }
            Some(command_bar::Command::AddInstance) => self.open_add_instance(String::new()),
            None => Task::none(),
        }
    }

    /// Opens the 1:1 DM with `user`, creating it if needed.
    fn message_user(&mut self, inst: InstanceId, user: Id) -> Task<Message> {
        let existing = self.core.store().instance(&inst).and_then(|s| {
            s.dms
                .values()
                .find(|dm| {
                    dm.kind == hermes_client_core::proto::models::ChannelKind::Dm
                        && s.dm_partner(dm).is_some_and(|u| u.id == user)
                })
                .map(|dm| dm.id)
        });
        if let Some(dm) = existing {
            return self.select(inst, dm);
        }
        let core = self.core.clone();
        Task::perform(
            async move {
                let result = match core.rest(&inst) {
                    Ok(rest) => rest
                        .create_dm(vec![user])
                        .await
                        .map(|c| c.id)
                        .map_err(|e| e.to_string()),
                    Err(err) => Err(err.to_string()),
                };
                (inst, result)
            },
            |(inst, result)| Message::DmCreated(inst, result),
        )
    }

    /// Writes the pane layout to `[[layout]]` in `config.toml`.
    pub fn save_layout(&mut self) {
        let value = toml::Value::Array(
            self.panes
                .save()
                .into_iter()
                .map(toml::Value::Table)
                .collect(),
        );
        if let Err(err) = self.core.set_setting("layout", &value.to_string()) {
            tracing::error!("layout not saved: {err}");
        }
    }

    fn sidebar_rows(&self) -> Vec<Row> {
        let statuses: Vec<(InstanceId, ConnStatus)> = self
            .core
            .instance_ids()
            .into_iter()
            .map(|id| {
                let status = self.core.status(&id);
                (id, status)
            })
            .collect();
        tree::rows(
            &self.core.store(),
            &statuses,
            &self.collapsed,
            self.panes.focused_channel().as_ref(),
        )
    }

    fn on_new_dm(&mut self, message: new_dm::Message) -> Task<Message> {
        let Some(dialog) = &mut self.new_dm else {
            return Task::none();
        };
        match message {
            new_dm::Message::Input(text) => dialog.input = text,
            new_dm::Message::Cancel => self.new_dm = None,
            new_dm::Message::Submit => {
                if dialog.busy {
                    return Task::none();
                }
                let inst = dialog.inst.clone();
                let ids = match self.core.store().instance(&inst) {
                    Some(store) => new_dm::resolve(store, &dialog.input),
                    None => Err("Not connected to this instance.".into()),
                };
                let ids = match ids {
                    Ok(ids) => ids,
                    Err(err) => {
                        dialog.error = Some(err);
                        return Task::none();
                    }
                };
                dialog.busy = true;
                dialog.error = None;
                let core = self.core.clone();
                return Task::perform(
                    async move {
                        let result = match core.rest(&inst) {
                            Ok(rest) => rest
                                .create_dm(ids)
                                .await
                                .map(|c| c.id)
                                .map_err(|e| e.to_string()),
                            Err(err) => Err(err.to_string()),
                        };
                        (inst, result)
                    },
                    |(inst, result)| Message::DmCreated(inst, result),
                );
            }
        }
        Task::none()
    }

    /// Starts the add-instance flow with an invite link or address (command line, a forwarded
    /// `hermes://` link).
    pub fn open_link(&mut self, link: String) -> Task<Message> {
        let focus = self.open_add_instance(link);
        focus.chain(self.on_add_instance(add_instance::Message::SubmitAddress))
    }

    fn open_add_instance(&mut self, input: String) -> Task<Message> {
        let dialog = add_instance::AddInstance::new(input);
        let focus = dialog.focus();
        self.add_instance = Some(dialog);
        focus_input(focus)
    }

    /// Signs in again to an instance that is logged out.
    fn sign_in(&mut self, inst: &InstanceId) -> Task<Message> {
        let Some(entry) = self.core.config().instance(inst).cloned() else {
            return Task::none();
        };
        self.add_instance = Some(add_instance::AddInstance::sign_in(
            &entry.url,
            &entry.username,
        ));
        self.on_add_instance(add_instance::Message::SubmitAddress)
    }

    fn on_add_instance(&mut self, message: add_instance::Message) -> Task<Message> {
        let Some(dialog) = &mut self.add_instance else {
            return Task::none();
        };
        let before = dialog.focus();
        match dialog.update(message, &self.core) {
            add_instance::Action::None => {
                let after = dialog.focus();
                if after != before {
                    return focus_input(after);
                }
                Task::none()
            }
            add_instance::Action::Run(task) => task.map(Message::AddInstance),
            add_instance::Action::Close => {
                self.add_instance = None;
                Task::none()
            }
            add_instance::Action::SignedIn(inst) => {
                self.add_instance = None;
                self.info(format!("Signed in to {inst}"));
                Task::none()
            }
        }
    }

    fn on_core(&mut self, event: CoreEvent) {
        match event {
            CoreEvent::ConfigReloaded { warnings } => {
                self.apply_config(self.core.config());
                if let Some(warning) = warnings.into_iter().next() {
                    self.warn(format!("config: {warning}"));
                }
            }
            CoreEvent::Error(text) => self.error(text),
            CoreEvent::StoreChanged {
                inst,
                what: Changed::Messages(channel),
            } => {
                let panes: Vec<_> = self
                    .panes
                    .state
                    .iter()
                    .filter(|(_, p)| {
                        p.buffer
                            == Buffer::Channel {
                                inst: inst.clone(),
                                channel,
                            }
                    })
                    .map(|(id, _)| *id)
                    .collect();
                for pane in panes {
                    self.mark_read_if_visible(pane);
                }
            }
            // The views read the store directly; the other events only trigger a redraw.
            _ => {}
        }
    }

    fn on_settings(&mut self, message: settings::Message) {
        match message {
            settings::Message::Theme(name) => {
                let value = toml::Value::String(name).to_string();
                self.persist("general.theme", &value);
            }
            settings::Message::FontSize(size) => {
                self.config.font.size = size;
                self.fonts = Fonts::from_config(&self.config.font);
            }
            settings::Message::UiScale(scale) => {
                self.config.font.ui_scale = (scale * 100.0).round() / 100.0;
                self.fonts = Fonts::from_config(&self.config.font);
            }
            settings::Message::SidebarWidth(width) => self.config.sidebar.width = width,
            settings::Message::SidebarPosition(position) => {
                let value = toml::Value::String(position.as_config().to_owned()).to_string();
                self.persist("sidebar.position", &value);
            }
            settings::Message::Save(setting) => {
                let (key, value) = settings::saved_value(&self.config, setting);
                self.persist(key, &value);
            }
            settings::Message::Close => self.panes.clear_where(|b| *b == Buffer::Settings),
        }
    }

    /// Writes one setting to `config.toml` and applies the result.
    fn persist(&mut self, key: &str, value: &str) {
        match self.core.set_setting(key, value) {
            Ok(()) => self.apply_config(self.core.config()),
            Err(err) => {
                self.error(format!("setting not saved: {err}"));
                self.apply_config(self.core.config());
            }
        }
    }

    fn apply_config(&mut self, config: ClientConfig) {
        let theme_changed = config.general.theme != self.config.general.theme;
        if config.keys != self.config.keys {
            let (keybinds, warnings) = Keybinds::from_config(&config.keys);
            self.keybinds = keybinds;
            if let Some(warning) = warnings.into_iter().next() {
                self.warn(warning);
            }
        }
        self.fonts = Fonts::from_config(&config.font);
        self.config = config;
        if theme_changed {
            self.reload_theme();
        }
    }

    /// Loads the configured theme again (from disk if it is a user theme).
    fn reload_theme(&mut self) {
        let dir = self.themes_dir();
        let loaded = Theme::load(&self.config.general.theme, dir.as_deref());
        self.themes = theme::available(dir.as_deref());
        self.iced_theme = style::iced_theme(&loaded.theme);
        self.theme = loaded.theme;
        if let Some(warning) = loaded.warnings.into_iter().next() {
            self.warn(warning);
        }
    }

    fn info(&mut self, text: String) {
        tracing::info!("{text}");
        self.status = Some(Status {
            text,
            level: Level::Info,
        });
    }

    fn warn(&mut self, text: String) {
        tracing::warn!("{text}");
        self.status = Some(Status {
            text,
            level: Level::Warning,
        });
    }

    fn error(&mut self, text: String) {
        tracing::error!("{text}");
        self.status = Some(Status {
            text,
            level: Level::Error,
        });
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![subscription::window_focus()];
        if let Some(events) = &self.events {
            subscriptions.push(subscription::core_events(events.clone()));
        }
        if let Some(dir) = self.themes_dir() {
            subscriptions.push(subscription::themes(dir));
        }
        if let Some(endpoint) = &self.single_instance {
            subscriptions.push(subscription::single_instance(endpoint.clone()));
        }
        subscriptions.push(subscription::window_and_keys());
        subscriptions.push(window::close_requests().map(Message::CloseRequested));
        Subscription::batch(subscriptions)
    }

    pub fn view(&self) -> Element<'_, Message> {
        let fonts = &self.fonts;
        let theme = &self.theme;
        let rows = self.sidebar_rows();
        let sidebar = crate::sidebar::view(
            &self.core,
            &rows,
            fonts,
            theme,
            self.config
                .sidebar
                .width
                .clamp(settings::MIN_SIDEBAR_WIDTH, settings::MAX_SIDEBAR_WIDTH),
        );

        let mut main = column![].width(Fill).height(Fill);
        if let Some(status) = &self.status {
            main = main.push(
                container(
                    row![
                        small(status.text.as_str(), fonts).width(Fill),
                        button(small("Dismiss", fonts))
                            .style(|_, s| style::secondary(theme, s))
                            .on_press(Message::DismissStatus),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                )
                .padding([4, 8])
                .style(|_| style::status_bar(theme, status.level)),
            );
        }
        main = main.push(self.panes.view(&panes::Context {
            core: &self.core,
            fonts,
            theme,
            config: &self.config,
            themes: &self.themes,
            utc_offset: self.utc_offset,
        }));

        let layout = if !self.sidebar_visible {
            row![main]
        } else if settings::Position::from_config(&self.config.sidebar.position)
            == settings::Position::Right
        {
            row![main, sidebar]
        } else {
            row![sidebar, main]
        };
        let base = container(layout)
            .width(Fill)
            .height(Fill)
            .style(|_| style::app(theme));
        let mut root: Element<'_, Message> = base.into();
        if let Some((target, at)) = &self.menu {
            let status = match target {
                Target::Instance(inst) | Target::Space(inst, _) | Target::Channel(inst, _) => {
                    self.core.status(inst)
                }
                Target::Pane(_) => ConnStatus::Connected,
            };
            let items = menu::actions(target, &status)
                .into_iter()
                .map(|action| context_menu::Item {
                    label: action.label(),
                    message: Message::Menu(target.clone(), action),
                    danger: action.is_destructive(),
                })
                .collect();
            root = context_menu::overlay(
                root,
                items,
                *at,
                self.window_size,
                fonts,
                theme,
                Message::CloseMenu,
            );
        }
        if let Some(bar) = &self.command_bar {
            root = modal::overlay(
                root,
                bar.view(fonts, theme).map(Message::CommandBar),
                theme,
                Some(Message::CommandBar(command_bar::Message::Close)),
            );
        }
        if let Some(dialog) = &self.add_instance {
            root = modal::overlay(
                root,
                dialog.view(fonts, theme).map(Message::AddInstance),
                theme,
                None,
            );
        }
        if let Some(dialog) = &self.new_dm {
            root = modal::overlay(
                root,
                dialog.view(fonts, theme).map(Message::NewDmMsg),
                theme,
                Some(Message::NewDmMsg(new_dm::Message::Cancel)),
            );
        }
        if let Some((_, action)) = &self.confirm {
            let (title, text) = confirm_text(*action);
            root = modal::overlay(
                root,
                confirm::view(
                    title,
                    text,
                    action.label(),
                    fonts,
                    theme,
                    Message::Confirmed,
                    Message::CancelConfirm,
                ),
                theme,
                Some(Message::CancelConfirm),
            );
        }
        cursor::tracker(&self.cursor, root).into()
    }
}

/// Title and text of the confirmation for a destructive action.
fn confirm_text(action: Action) -> (&'static str, &'static str) {
    match action {
        Action::RemoveInstance => (
            "Remove this instance?",
            "You are signed out and it disappears from the sidebar. Your account on the server stays.",
        ),
        Action::LeaveSpace => (
            "Leave this space?",
            "You will need a new invite to join again.",
        ),
        _ => ("Are you sure?", ""),
    }
}

fn focus_input(id: Option<&'static str>) -> Task<Message> {
    match id {
        Some(id) => operation::focus(iced::widget::Id::new(id)),
        None => Task::none(),
    }
}

/// Loads the bundled fonts.
pub fn load_fonts() -> Task<Message> {
    Task::batch(
        crate::font::BUNDLED
            .into_iter()
            .map(|bytes| iced::font::load(bytes).map(|_| Message::FontLoaded)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::settings::Setting;
    use hermes_client_core::CoreOptions;
    use hermes_client_core::config::ConfigPaths;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hermes-gui-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn app_in(dir: &std::path::Path) -> App {
        let paths = ConfigPaths::in_dir(dir);
        let config = ClientConfig::load(&paths.config()).unwrap().config;
        let options = CoreOptions::with_paths("hermes-gui-test", paths).unwrap();
        let (core, _events) = Core::new(config, options);
        App::new(core, None, None)
    }

    #[tokio::test]
    async fn settings_are_written_to_config_toml() {
        let dir = temp_dir("settings");
        std::fs::write(
            dir.join("config.toml"),
            "# my settings\n[general]\ntheme = \"dark\" # keep me\n",
        )
        .unwrap();
        let mut app = app_in(&dir);
        assert_eq!(app.theme.name, "dark");

        let _ = app.update(Message::Settings(settings::Message::Theme("light".into())));
        assert_eq!(app.theme.name, "light");

        // Sliders change the view right away and save on release.
        let _ = app.update(Message::Settings(settings::Message::FontSize(16)));
        assert_eq!(app.fonts.size, 16.0);
        assert_eq!(app.core.config().font.size, 13);
        let _ = app.update(Message::Settings(settings::Message::Save(
            Setting::FontSize,
        )));
        let _ = app.update(Message::Settings(settings::Message::UiScale(1.2500001)));
        let _ = app.update(Message::Settings(settings::Message::Save(Setting::UiScale)));
        let _ = app.update(Message::Settings(settings::Message::SidebarPosition(
            settings::Position::Right,
        )));
        let _ = app.update(Message::Settings(settings::Message::SidebarWidth(300)));
        let _ = app.update(Message::Settings(settings::Message::Save(
            Setting::SidebarWidth,
        )));

        let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(text.contains("# my settings"), "{text}");
        assert!(text.contains("theme = \"light\" # keep me"), "{text}");
        let saved = ClientConfig::parse(&text).unwrap().config;
        assert_eq!(saved.font.size, 16);
        assert_eq!(saved.font.ui_scale, 1.25);
        assert_eq!(saved.sidebar.position, "right");
        assert_eq!(saved.sidebar.width, 300);
        assert!(app.status.is_none(), "{:?}", app.status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn config_reload_applies_theme_and_fonts() {
        let dir = temp_dir("reload");
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        std::fs::write(
            dir.join("themes").join("mine.toml"),
            "[general]\nbackground = \"#010203\"\n",
        )
        .unwrap();
        let mut app = app_in(&dir);
        assert!(app.themes.contains(&"mine".to_owned()));

        // The core's config changes like this when its watcher sees an edited file.
        app.core.set_setting("general.theme", "\"mine\"").unwrap();
        app.core.set_setting("font.size", "18").unwrap();
        let _ = app.update(Message::Core(CoreEvent::ConfigReloaded {
            warnings: vec!["unknown key `x` (ignored)".into()],
        }));
        assert_eq!(app.theme.name, "mine");
        assert_eq!(
            app.theme.general.background.0,
            iced::Color::from_rgb8(1, 2, 3)
        );
        assert_eq!(app.fonts.size, 18.0);
        // The partial theme warns too; one warning is shown at a time.
        assert!(app.status.is_some());

        // Editing the theme file is picked up by ThemesChanged.
        std::fs::write(
            dir.join("themes").join("mine.toml"),
            "[general]\nbackground = \"#040506\"\n",
        )
        .unwrap();
        let _ = app.update(Message::ThemesChanged);
        assert_eq!(
            app.theme.general.background.0,
            iced::Color::from_rgb8(4, 5, 6)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn keys_split_panes_and_the_layout_is_saved() {
        use hermes_client_core::proto::id::Id;
        use iced::keyboard::Key;

        const ANY: keyboard::key::Physical =
            keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
        let dir = temp_dir("layout");
        let mut app = app_in(&dir);
        assert_eq!(app.panes.state.len(), 1);
        let ctrl = Modifiers::CTRL;

        let _ = app.update(Message::Select("h".into(), Id(11)));
        let _ = app.update(Message::Key(Key::Character("\\".into()), ANY, ctrl, false));
        assert_eq!(app.panes.state.len(), 2);
        let _ = app.update(Message::Select("h".into(), Id(12)));
        assert_eq!(app.panes.focused_channel(), Some(("h".into(), Id(12))));
        // Typing plain letters into an input does not trigger keybinds; Ctrl combos do.
        let _ = app.update(Message::Key(
            Key::Character("w".into()),
            ANY,
            Modifiers::empty(),
            true,
        ));
        assert_eq!(app.panes.state.len(), 2);
        let _ = app.update(Message::Key(Key::Character("u".into()), ANY, ctrl, true));
        assert!(app.panes.focused().unwrap().members);
        let _ = app.update(Message::Key(Key::Character("b".into()), ANY, ctrl, false));
        assert!(!app.sidebar_visible);

        // The command bar opens, filters and runs.
        let _ = app.update(Message::Key(Key::Character("k".into()), ANY, ctrl, false));
        assert!(app.command_bar.is_some());
        let _ = app.update(Message::CommandBar(command_bar::Message::Input(
            "open settings".into(),
        )));
        let _ = app.update(Message::CommandBar(command_bar::Message::Submit));
        assert!(app.command_bar.is_none());
        assert_eq!(app.panes.focused().unwrap().buffer, Buffer::Settings);

        app.save_layout();
        let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        let saved = ClientConfig::parse(&text).unwrap().config;
        assert_eq!(saved.layout, app.panes.save(), "{text}");
        let restored = app_in(&dir);
        assert_eq!(restored.panes.save(), app.panes.save());
        assert!(
            restored
                .panes
                .visible_channels()
                .contains(&("h".into(), Id(11)))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
