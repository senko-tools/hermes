//! App state, update and view (PLAN.md section 15.8). All side effects go through `Core`.

use std::path::PathBuf;

use hermes_client_core::config::ClientConfig;
use hermes_client_core::{Core, CoreEvent, InstanceId};
use iced::widget::{button, column, container, operation, row};
use iced::{Element, Fill, Subscription, Task, window};

use crate::buffer::settings;
use crate::font::Fonts;
use crate::message::Message;
use crate::modal::{self, add_instance};
use crate::single_instance::Endpoint;
use crate::subscription::{self, CoreEventsHandle};
use crate::theme::{self, Theme, style};
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
}

impl App {
    pub fn new(
        core: Core,
        events: Option<CoreEventsHandle>,
        single_instance: Option<Endpoint>,
    ) -> App {
        let config = core.config();
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
            Message::FontLoaded => {}
            Message::OpenAddInstance => return self.open_add_instance(String::new()),
            Message::SignIn(inst) => return self.sign_in(&inst),
            Message::AddInstance(message) => return self.on_add_instance(message),
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
        Subscription::batch(subscriptions)
    }

    pub fn view(&self) -> Element<'_, Message> {
        let fonts = &self.fonts;
        let theme = &self.theme;
        let sidebar = crate::sidebar::view(
            &self.core,
            fonts,
            theme,
            self.config
                .sidebar
                .width
                .clamp(settings::MIN_SIDEBAR_WIDTH, settings::MAX_SIDEBAR_WIDTH),
        );

        let content: Element<'_, Message> = match self.screen {
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
        match &self.add_instance {
            Some(dialog) => modal::overlay(
                base,
                dialog.view(fonts, theme).map(Message::AddInstance),
                theme,
                None,
            ),
            None => base.into(),
        }
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
