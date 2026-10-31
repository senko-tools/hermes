//! App state, update and view (PLAN.md section 15.8). All side effects go through `Core`.

use std::path::PathBuf;

use hermes_client_core::config::ClientConfig;
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::api::CreateInviteRequest;
use hermes_client_core::proto::id::Id;
use hermes_client_core::{Core, CoreEvent, InstanceId, view};
use iced::widget::{button, column, container, operation, row};
use iced::{Element, Fill, Point, Size, Subscription, Task, window};

use crate::buffer::settings;
use crate::font::Fonts;
use crate::message::Message;
use crate::modal::{self, add_instance, confirm, new_dm};
use crate::sidebar::collapsed::Collapsed;
use crate::sidebar::menu::{self, Action, Target};
use crate::sidebar::tree::{self, Selection};
use crate::single_instance::Endpoint;
use crate::subscription::{self, CoreEventsHandle};
use crate::theme::{self, Theme, style};
use crate::widget::context_menu;
use crate::widget::cursor::{self, Cursor};
use crate::widget::{heading, label, small};

/// What the main area shows until the pane grid exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Welcome,
    Settings,
}

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
    pub screen: Screen,
    pub status: Option<Status>,
    /// The add-instance / sign-in dialog.
    pub add_instance: Option<add_instance::AddInstance>,
    /// Where later launches forward their links (section 15.7).
    single_instance: Option<Endpoint>,
    pub collapsed: Collapsed,
    /// The channel shown in the main area.
    pub selected: Option<Selection>,
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
        let mut app = App {
            fonts: Fonts::from_config(&config.font),
            theme: Theme::dark(),
            iced_theme: style::iced_theme(&Theme::dark()),
            themes: Vec::new(),
            config,
            core,
            events,
            screen: Screen::Welcome,
            status: None,
            add_instance: None,
            single_instance,
            collapsed,
            selected: None,
            menu: None,
            cursor: Cursor::default(),
            window_size: Size::new(1200.0, 800.0),
            confirm: None,
            new_dm: None,
        };
        app.reload_theme();
        app
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
            Message::WindowFocused(focused) => self.core.set_app_focused(focused),
            Message::OpenSettings => self.screen = Screen::Settings,
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
                } else if self.confirm.is_some() {
                    self.confirm = None;
                } else if self.new_dm.is_some() {
                    self.new_dm = None;
                } else if self.add_instance.is_some() {
                    self.add_instance = None;
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

    /// Shows a channel in the main area and loads its latest messages.
    fn select(&mut self, inst: InstanceId, channel: Id) -> Task<Message> {
        self.selected = Some((inst.clone(), channel));
        self.screen = Screen::Welcome;
        self.core.set_visible_channels([(inst.clone(), channel)]);
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
                if self.selected.as_ref().is_some_and(|(i, _)| *i == inst) {
                    self.selected = None;
                }
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
                if self.selected.as_ref().is_some_and(|(i, c)| {
                    *i == inst
                        && core.store().instance(i).and_then(|s| s.space_of(*c)) == Some(space)
                }) {
                    self.selected = None;
                }
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
            (target, action) => {
                tracing::warn!("menu action {action:?} does not apply to {target:?}")
            }
        }
        Task::none()
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
            settings::Message::Close => self.screen = Screen::Welcome,
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
        Subscription::batch(subscriptions)
    }

    pub fn view(&self) -> Element<'_, Message> {
        let fonts = &self.fonts;
        let theme = &self.theme;
        let statuses: Vec<(InstanceId, ConnStatus)> = self
            .core
            .instance_ids()
            .into_iter()
            .map(|id| {
                let status = self.core.status(&id);
                (id, status)
            })
            .collect();
        let rows = tree::rows(
            &self.core.store(),
            &statuses,
            &self.collapsed,
            self.selected.as_ref(),
        );
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

        let content: Element<'_, Message> = match self.screen {
            Screen::Welcome if self.selected.is_some() => self.channel_view(),
            Screen::Welcome => {
                let mut welcome = column![heading("Welcome to Hermes", fonts)].spacing(12);
                if self.core.instance_ids().is_empty() {
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
                                .style(|_, s| style::primary(theme, s))
                                .on_press(Message::OpenAddInstance),
                        );
                } else {
                    welcome = welcome.push(
                        label("Open a channel from the sidebar.", fonts)
                            .color(theme.text.secondary.0),
                    );
                }
                container(welcome).center(Fill).into()
            }
            Screen::Settings => {
                settings::view(&self.config, &self.themes, fonts, theme).map(Message::Settings)
            }
        };

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
        main = main.push(content);

        let layout = if settings::Position::from_config(&self.config.sidebar.position)
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

    /// The selected channel's header; the message list comes with the channel buffer.
    fn channel_view(&self) -> Element<'_, Message> {
        let fonts = &self.fonts;
        let theme = &self.theme;
        let Some((inst, id)) = &self.selected else {
            return column![].into();
        };
        let store = self.core.store();
        let channel = store
            .instance(inst)
            .and_then(|s| Some((s, s.channel(*id)?)));
        let (title, topic) = match channel {
            Some((s, c)) => (
                view::channel_title(s, c),
                c.topic.clone().unwrap_or_default(),
            ),
            None => ("(not available)".to_owned(), String::new()),
        };
        let header = container(
            row![
                label(title, fonts).font(fonts.bold()),
                small(topic, fonts).color(theme.text.secondary.0),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center),
        )
        .padding([8, 12])
        .width(Fill)
        .style(|_| style::pane_header(theme));
        column![
            header,
            container(
                small("The message view is not built yet.", fonts).color(theme.text.tertiary.0)
            )
            .center(Fill),
        ]
        .into()
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
}
