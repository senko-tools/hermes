//! App state, update and view (PLAN.md section 15.8). All side effects go through `Core`.

mod account;
mod actions;
mod forms;
mod jump;
mod results;
mod space;

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

use crate::buffer::channel::input::{self, Composer, Upload, UploadState};
use crate::buffer::channel::message::{Images, Img};
use crate::buffer::{Buffer, channel, settings, space_settings};
use crate::desktop::tray::{Tray, TrayAction};
use crate::desktop::{self, CloseAction};
use crate::font::Fonts;
use crate::keybinds::{self, Keybinds};
use crate::message::Message;
use crate::modal::image_viewer::{self, Viewer};
use crate::modal::{self, add_instance, command_bar, confirm, form, new_dm};
use crate::panes::{self, PaneAction, Panes};
use crate::sidebar::collapsed::Collapsed;
use crate::sidebar::menu::{self, Action, Target};
use crate::sidebar::tree::{self, Row};
use crate::single_instance::Endpoint;
use crate::subscription::{self, CoreEventsHandle};
use crate::theme::{self, Theme, style};
use crate::voice::{self as voice_ui, VoiceMsg};
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
    /// Thumbnails, loaded when their message is first shown.
    pub images: Images,
    /// The full-size image viewer.
    pub viewer: Option<Viewer>,
    /// The tray icon, if it could be created.
    pub tray: Option<Tray>,
    /// The window is shown (not hidden in the tray).
    pub window_visible: bool,
    /// The draft of each channel.
    pub composers: std::collections::HashMap<(InstanceId, Id), Composer>,
    next_upload: u64,
    /// The add-instance / sign-in dialog.
    pub add_instance: Option<add_instance::AddInstance>,
    /// Where later launches forward their links (section 15.7).
    single_instance: Option<Endpoint>,
    pub collapsed: Collapsed,
    /// An open context menu and where it was opened.
    pub menu: Option<(Target, Point)>,
    /// The entry chosen with the arrow keys in the open menu.
    pub menu_index: Option<usize>,
    pub(crate) cursor: Cursor,
    window_size: Size,
    /// A destructive menu action waiting for confirmation.
    pub confirm: Option<(Target, Action)>,
    pub new_dm: Option<new_dm::NewDm>,
    /// A ringing incoming call (the banner).
    pub call: Option<voice_ui::Call>,
    /// The per-user volume menu.
    pub volume_menu: Option<voice_ui::VolumeMenu>,
    /// The push-to-talk key is held.
    pub ptt_down: bool,
    /// A join in progress.
    pub joining: Option<(InstanceId, Id)>,
    pub headphones_hint_shown: bool,
    pub settings_tab: settings::Tab,
    /// Audio devices, listed when the Voice & Audio tab opens.
    pub audio_devices: Option<hermes_client_core::audio::AudioDevices>,
    /// Microphone level in dBFS while the meter runs.
    pub input_level: f32,
    pub mic_test: bool,
    /// The level meter runs (the Voice & Audio tab is shown).
    meter_on: bool,
    /// The PTT key being typed in the settings.
    pub ptt_draft: Option<String>,
    /// The global mute key, if global hotkeys work here.
    pub global_keys: Option<crate::ptt::GlobalKeys>,
    /// What each opened space settings buffer remembers.
    pub space_settings: std::collections::HashMap<(InstanceId, Id), space_settings::State>,
    default_space_settings: space_settings::State,
    /// The open small dialog (create channel, ban, …).
    pub form: Option<form::Form>,
    pub drag: crate::drag::Drag,
    pub results:
        std::collections::HashMap<crate::buffer::results::Key, crate::buffer::results::State>,
    default_results: crate::buffer::results::State,
    /// The instance shown in Settings → Account.
    pub account_inst: Option<InstanceId>,
    pub sessions: space_settings::Remote<Vec<hermes_client_core::proto::api::SessionInfo>>,
    /// A newer release, if the update check found one.
    pub update: Option<hermes_client_core::update::Update>,
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
            images: Images::new(),
            viewer: None,
            composers: std::collections::HashMap::new(),
            tray: None,
            window_visible: true,
            next_upload: 0,
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
            menu_index: None,
            cursor: Cursor::default(),
            window_size: Size::new(1200.0, 800.0),
            confirm: None,
            new_dm: None,
            call: None,
            volume_menu: None,
            ptt_down: false,
            joining: None,
            headphones_hint_shown: false,
            settings_tab: settings::Tab::default(),
            audio_devices: None,
            input_level: settings::METER_MIN,
            mic_test: false,
            meter_on: false,
            ptt_draft: None,
            global_keys: None,
            space_settings: std::collections::HashMap::new(),
            default_space_settings: space_settings::State::default(),
            form: None,
            drag: crate::drag::Drag::default(),
            results: std::collections::HashMap::new(),
            default_results: crate::buffer::results::State::default(),
            account_inst: None,
            sessions: space_settings::Remote::Idle,
            update: None,
        };
        app.reload_theme();
        if let Some(warning) = key_warnings.into_iter().next() {
            app.warn(warning);
        }
        app.core.set_visible_channels(app.panes.visible_channels());
        app
    }

    /// Loads the first page of every channel the restored layout shows.
    pub fn open_restored(&mut self) -> Task<Message> {
        for key in self.panes.visible_channels() {
            self.composers.entry(key).or_default();
        }
        Task::batch(
            self.panes
                .visible_channels()
                .into_iter()
                .map(|(inst, channel)| self.load_channel(inst, channel)),
        )
    }

    /// Asks GitHub for a newer release (`general.check_for_updates`).
    pub fn check_for_update(&self) -> Task<Message> {
        if !self.config.general.check_for_updates {
            return Task::none();
        }
        Task::perform(
            hermes_client_core::update::check(env!("CARGO_PKG_VERSION")),
            Message::UpdateChecked,
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
        let task = self.update_inner(message);
        self.sync_meter();
        task
    }

    /// Runs the level meter (and allows the mic test) only while the Voice & Audio tab shows.
    fn sync_meter(&mut self) {
        let shown = self.settings_tab == settings::Tab::Voice
            && self
                .panes
                .state
                .iter()
                .any(|(_, p)| p.buffer == Buffer::Settings);
        if shown != self.meter_on {
            self.meter_on = shown;
            self.core.set_level_meter(shown);
            if !shown {
                self.input_level = settings::METER_MIN;
                if self.mic_test {
                    self.mic_test = false;
                    self.core.set_mic_test(false);
                }
            }
        }
    }

    /// Lists the audio devices in the background (it can take a moment).
    pub fn load_devices(&self) -> Task<Message> {
        let core = self.core.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || core.list_audio_devices())
                    .await
                    .unwrap_or_default()
            },
            |devices| Message::Settings(settings::Message::Devices(devices)),
        )
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Core(event) => return self.on_core(event),
            Message::Voice(message) => return self.on_voice(message),
            Message::KeyReleased(key) => {
                if self.ptt_matches(&key) {
                    return self.on_voice(VoiceMsg::Ptt(false));
                }
            }
            Message::Tray(TrayAction::ToggleMute) => return self.on_voice(VoiceMsg::ToggleMute),
            Message::Tray(TrayAction::ToggleDeafen) => {
                return self.on_voice(VoiceMsg::ToggleDeafen);
            }
            Message::Tray(TrayAction::ToggleWindow) => {
                return if self.window_visible {
                    self.hide_window()
                } else {
                    self.show_window()
                };
            }
            Message::Tray(TrayAction::Quit) => {
                self.save_layout();
                return iced::exit();
            }
            Message::NotificationClicked(inst, channel) => {
                let show = self.show_window();
                return show.chain(self.select(inst, channel));
            }
            Message::ThemesChanged => self.reload_theme(),
            Message::WindowFocused(focused) => {
                self.focused = focused;
                self.core.set_app_focused(focused);
                if focused {
                    self.mark_visible_read();
                }
            }
            Message::OpenSettings => self.panes.open(Buffer::Settings),
            Message::Settings(message) => return self.on_settings(message),
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
                return match desktop::on_close(
                    self.config.general.close_to_tray,
                    self.tray.is_some(),
                ) {
                    CloseAction::Hide => {
                        self.window_visible = false;
                        window::set_mode(id, window::Mode::Hidden)
                    }
                    CloseAction::Quit => iced::exit(),
                };
            }
            Message::CommandBar(message) => return self.on_command_bar(message),
            Message::Channel(pane, message) => return self.on_channel(pane, message),
            Message::ImageLoaded(inst, id, result) => {
                let img = match result {
                    Ok(bytes) => Img::Ready(iced::widget::image::Handle::from_bytes(bytes)),
                    Err(err) => {
                        tracing::warn!("thumbnail {id}: {err}");
                        Img::Failed
                    }
                };
                self.images.insert((inst, id), img);
            }
            Message::ViewerLoaded(id, result) => {
                if let Some(viewer) = &mut self.viewer
                    && viewer.attachment.id == id
                {
                    viewer.image = match result {
                        Ok(bytes) => Img::Ready(iced::widget::image::Handle::from_bytes(bytes)),
                        Err(err) => {
                            tracing::warn!("image {id}: {err}");
                            Img::Failed
                        }
                    };
                }
            }
            Message::CloseViewer => self.viewer = None,
            Message::FilesPicked(inst, channel, paths) => {
                let uploads: Vec<_> = input::files_only(paths)
                    .into_iter()
                    .map(|p| self.start_upload(inst.clone(), channel, p, None))
                    .collect();
                // The dialog took the focus; give it back to the input.
                let focus = self
                    .composers
                    .get(&(inst, channel))
                    .map_or_else(Task::none, |c| operation::focus(c.editor_id.clone()));
                return Task::batch(uploads).chain(focus);
            }
            Message::FileDropped(path) => {
                if let Some((inst, channel)) = self.panes.focused_channel()
                    && path.is_file()
                {
                    return self.start_upload(inst, channel, path, None);
                }
            }
            Message::Uploaded(inst, channel, local, result) => {
                if let Some(upload) = self
                    .composers
                    .get_mut(&(inst, channel))
                    .and_then(|c| c.uploads.iter_mut().find(|u| u.local == local))
                {
                    upload.state = match result {
                        Ok(attachment) => UploadState::Ready(attachment),
                        Err(err) => UploadState::Failed(err),
                    };
                }
            }
            Message::DownloadViewer => {
                if let Some(viewer) = &self.viewer {
                    return self.download(viewer.inst.clone(), viewer.attachment.clone());
                }
            }
            Message::OpenMenu(target) => {
                self.menu = Some((target, self.cursor.get()));
                self.menu_index = None;
            }
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
                if self.viewer.is_some() {
                    self.viewer = None;
                } else if self.volume_menu.is_some() {
                    self.volume_menu = None;
                } else if self.menu.is_some() {
                    self.menu = None;
                } else if self.command_bar.is_some() {
                    self.command_bar = None;
                } else if self.confirm.is_some() {
                    self.confirm = None;
                } else if self.form.is_some() {
                    self.form = None;
                } else if self.new_dm.is_some() {
                    self.new_dm = None;
                } else if self.add_instance.is_some() {
                    self.add_instance = None;
                } else {
                    return self.key_action(keybinds::Action::MarkRead);
                }
            }
            Message::SpaceSettings(inst, space, message) => {
                return self.on_space_settings(inst, space, message);
            }
            Message::Form(message) => return self.on_form(message),
            Message::FormDone(result) => self.on_form_done(result),
            Message::MousePressed => self.drag.press(),
            Message::MouseReleased => return self.on_mouse_released(),
            Message::DragHover(item) => self.drag.hover(item),
            Message::DragLeave(item) => self.drag.leave(&item),
            Message::Nothing => {}
            Message::Shortcut(action) => return self.key_action(action),
            Message::UpdateChecked(result) => match result {
                Ok(update) => self.update = update,
                Err(err) => tracing::info!("update check failed: {err}"),
            },
            Message::OpenUpdate => {
                if let Some(update) = &self.update
                    && let Err(err) = crate::open::url(&update.url)
                {
                    self.error(format!("could not open the browser: {err}"));
                }
            }
            Message::OpenResults(key) => return self.open_results(key),
            Message::Results(key, message) => return self.on_results(key, message),
            Message::SearchFrom(inst, channel) => return self.search_from(inst, channel),
            Message::InviteCreated(inst, space, link) => {
                self.form = None;
                let reload = self.update(Message::SpaceSettings(
                    inst,
                    space,
                    space_settings::Message::Refresh,
                ));
                return Task::batch([self.update(Message::Copy(link, "Invite link")), reload]);
            }
            Message::IconPicked(inst, space, path) => {
                let Some(path) = path else {
                    return Task::none();
                };
                let core = self.core.clone();
                return Task::perform(
                    async move {
                        let icon = core.upload(&inst, &path).await?;
                        core.rest(&inst)?
                            .update_space(
                                space,
                                &hermes_client_core::proto::api::UpdateSpaceRequest {
                                    icon_id: Some(Some(icon.id)),
                                    ..Default::default()
                                },
                            )
                            .await?;
                        Ok::<_, hermes_client_core::CoreError>(())
                    },
                    |r| {
                        Message::Done(
                            r.map(|()| "Icon changed".to_owned())
                                .map_err(|e| format!("could not change the icon: {e}")),
                        )
                    },
                );
            }
            Message::Forwarded(link) => {
                tracing::info!(
                    "another launch handed over {}",
                    if link.is_empty() { "nothing" } else { "a link" }
                );
                let show = self.show_window();
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
        let composer = self.composers.entry((inst.clone(), channel)).or_default();
        let focus = operation::focus(composer.editor_id.clone());
        self.load_channel(inst, channel).chain(focus)
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
                let newer = view.at_bottom() && !view.loading_newer && view.jump.is_none();
                if view.at_bottom() {
                    self.mark_read_if_visible(pane);
                }
                if older {
                    return self.load_older(pane, inst, channel);
                }
                if newer {
                    return self.load_newer(pane, inst, channel);
                }
            }
            channel::Message::Measured(key, height) => {
                view.heights.set(key, height);
                let jumping = view.jump.is_some();
                let thumbs = if let channel::rows::RowKey::Message(id) = key {
                    self.load_thumbnails(&inst, channel, id)
                } else {
                    Task::none()
                };
                if jumping {
                    return Task::batch([thumbs, self.scroll_to_jump(pane)]);
                }
                return thumbs;
            }
            channel::Message::JumpTo(id) => {
                view.jump = Some(id);
                view.highlight = Some(id);
                return self.load_jump(pane, inst, channel, id);
            }
            channel::Message::JumpLoaded => return self.scroll_to_jump(pane),
            channel::Message::NewerLoaded => view.loading_newer = false,
            channel::Message::Hover(id, entered) => {
                if entered {
                    view.hovered = Some(id);
                } else if view.hovered == Some(id) {
                    view.hovered = None;
                    if view.palette == Some(id) {
                        view.palette = None;
                    }
                }
            }
            channel::Message::Palette(id) => view.palette = id,
            channel::Message::More(id) => {
                self.menu = Some((Target::Message(inst, channel, id), self.cursor.get()));
            }
            channel::Message::Author(user) => {
                let space = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.space_of(channel));
                self.menu = Some((Target::Member(inst, space, user), self.cursor.get()));
            }
            channel::Message::React(id, emoji, add) => {
                view.palette = None;
                let core = self.core.clone();
                return Task::perform(
                    async move {
                        if add {
                            core.react(&inst, channel, id, &emoji).await
                        } else {
                            core.unreact(&inst, channel, id, &emoji).await
                        }
                    },
                    |result| match result {
                        Ok(()) => Message::ChannelOpened,
                        Err(err) => Message::Done(Err(format!("reaction failed: {err}"))),
                    },
                );
            }
            channel::Message::Link(id, link) => match link {
                channel::markup::Link::Url(url) => {
                    if let Err(err) = crate::open::url(&url) {
                        self.error(format!("could not open the link: {err}"));
                    }
                }
                channel::markup::Link::Channel(target) => return self.select(inst, target),
                channel::markup::Link::Spoiler => {
                    view.revealed.insert(id);
                }
            },
            channel::Message::OpenImage(attachment) => {
                self.viewer = Some(Viewer {
                    inst: inst.clone(),
                    attachment: attachment.clone(),
                    image: Img::Loading,
                });
                let core = self.core.clone();
                let id = attachment.id;
                return Task::perform(
                    async move {
                        core.rest(&inst)
                            .map_err(|e| e.to_string())?
                            .download(attachment.id, &attachment.filename)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    move |result| Message::ViewerLoaded(id, result),
                );
            }
            channel::Message::Download(attachment) => return self.download(inst, attachment),
            channel::Message::Input(message) => return self.on_input(inst, channel, message),
            channel::Message::Reply(id) => {
                let composer = self.composers.entry((inst, channel)).or_default();
                if composer.editing.is_some() {
                    composer.clear();
                }
                composer.reply_to = Some(id);
                return operation::focus(composer.editor_id.clone());
            }
            channel::Message::StartEdit(id) => return self.start_edit(inst, channel, id),
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

    fn on_input(
        &mut self,
        inst: InstanceId,
        channel: Id,
        message: input::Message,
    ) -> Task<Message> {
        let key = (inst.clone(), channel);
        let composer = self.composers.entry(key.clone()).or_default();
        match message {
            input::Message::Edit(action) => {
                let edit = action.is_edit();
                composer.content.perform(action);
                let text = composer.text();
                if edit && !text.is_empty() && !text.starts_with('/') && composer.editing.is_none()
                {
                    self.core.typing(&inst, channel);
                }
                self.refresh_suggestions(&inst, channel);
            }
            input::Message::Suggest(delta) => {
                let len = composer.suggestions.len() as i32;
                if len > 0 {
                    composer.selected = (composer.selected as i32 + delta).rem_euclid(len) as usize;
                }
            }
            input::Message::Accept(n) => {
                let n = if n == usize::MAX {
                    composer.selected
                } else {
                    n
                };
                if let (Some(token), Some(s)) = (composer.token(), composer.suggestions.get(n)) {
                    let insert = s.insert.clone();
                    composer.replace_token(&token, &insert);
                }
                composer.suggestions.clear();
                return operation::focus(composer.editor_id.clone());
            }
            input::Message::Cancel => {
                if !composer.suggestions.is_empty() {
                    composer.suggestions.clear();
                } else if composer.editing.is_some() {
                    let uploads = std::mem::take(&mut composer.uploads);
                    composer.clear();
                    composer.uploads = uploads;
                } else if composer.reply_to.is_some() {
                    composer.reply_to = None;
                } else {
                    return self.key_action(keybinds::Action::MarkRead);
                }
            }
            input::Message::EditLast => {
                let last = {
                    let store = self.core.store();
                    let s = store.instance(&inst);
                    let me = s.and_then(|s| s.me_id());
                    s.and_then(|s| s.window(channel)).and_then(|w| {
                        w.messages
                            .iter()
                            .rev()
                            .find(|m| Some(m.author.id) == me && !m.deleted)
                            .map(|m| m.id)
                    })
                };
                if let Some(id) = last {
                    return self.start_edit(inst, channel, id);
                }
            }
            input::Message::Paste => return self.paste(inst, channel),
            input::Message::PickFiles => {
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title("Attach files")
                            .pick_files()
                            .await
                            .unwrap_or_default()
                            .into_iter()
                            .map(|f| f.path().to_path_buf())
                            .collect::<Vec<_>>()
                    },
                    move |paths| Message::FilesPicked(inst.clone(), channel, paths),
                );
            }
            input::Message::RemoveUpload(local) => composer.uploads.retain(|u| u.local != local),
            input::Message::Submit => return self.submit(inst, channel),
        }
        Task::none()
    }

    /// Autocomplete for the token at the cursor.
    fn refresh_suggestions(&mut self, inst: &InstanceId, channel: Id) {
        let key = (inst.clone(), channel);
        let Some(composer) = self.composers.get_mut(&key) else {
            return;
        };
        let suggestions = match composer.token() {
            Some(token) => self
                .core
                .store()
                .instance(inst)
                .map(|s| {
                    hermes_client_core::view::complete(s, channel, &token, input::MAX_SUGGESTIONS)
                })
                .unwrap_or_default(),
            None => Vec::new(),
        };
        if suggestions != composer.suggestions {
            composer.selected = 0;
        }
        composer.suggestions = suggestions;
    }

    fn start_edit(&mut self, inst: InstanceId, channel: Id, id: Id) -> Task<Message> {
        let content = self
            .core
            .store()
            .instance(&inst)
            .and_then(|s| s.window(channel))
            .and_then(|w| w.get(id))
            .map(|m| m.content.clone());
        let Some(content) = content else {
            return Task::none();
        };
        let composer = self.composers.entry((inst, channel)).or_default();
        composer.reply_to = None;
        composer.suggestions.clear();
        composer.editing = Some(id);
        composer.set_text(&content);
        operation::focus(composer.editor_id.clone())
    }

    /// Sends the draft (or saves the edit, or runs a slash command).
    fn submit(&mut self, inst: InstanceId, channel: Id) -> Task<Message> {
        use hermes_client_core::commands::{self, SlashCommand};

        let key = (inst.clone(), channel);
        let composer = self.composers.entry(key).or_default();
        let text = composer.text();
        if let Some(message) = composer.editing {
            if text.trim().is_empty() {
                self.error("A message cannot be empty; delete it instead.".into());
                return Task::none();
            }
            composer.clear();
            let core = self.core.clone();
            return Task::perform(
                async move { core.edit_message(&inst, channel, message, &text).await },
                |result| match result {
                    Ok(()) => Message::ChannelOpened,
                    Err(err) => Message::Done(Err(format!("could not edit: {err}"))),
                },
            );
        }
        let attachments = match composer.ready_attachments() {
            Ok(a) => a,
            Err(err) => {
                self.info(err.into());
                return Task::none();
            }
        };
        if text.trim().is_empty() && attachments.is_empty() {
            return Task::none();
        }
        let command = if text.trim().is_empty() {
            SlashCommand::Send(String::new())
        } else {
            match commands::parse(&text) {
                Ok(c) => c,
                Err(err) => {
                    self.error(err.to_string());
                    return Task::none();
                }
            }
        };
        if let Some(content) = command.message_text() {
            let draft = hermes_client_core::Draft {
                content,
                reply_to: composer.reply_to,
                attachments,
            };
            composer.clear();
            if let Err(err) = self.core.send_message(&inst, channel, draft) {
                self.error(err.to_string());
            }
            // Sending scrolls to the newest message.
            let scroll = self
                .panes
                .state
                .iter()
                .find(|(_, p)| {
                    p.buffer
                        == Buffer::Channel {
                            inst: inst.clone(),
                            channel,
                        }
                })
                .map(|(_, p)| p.channel.scroll_id.clone());
            return match scroll {
                Some(id) => operation::snap_to(
                    id,
                    iced::widget::scrollable::RelativeOffset { x: 0.0, y: 0.0 },
                ),
                None => Task::none(),
            };
        }
        composer.clear();
        match command {
            SlashCommand::Send(_) | SlashCommand::Me(_) => Task::none(),
            SlashCommand::Join(name) => {
                let found = self.core.store().instance(&inst).and_then(|s| {
                    hermes_client_core::view::find_voice_channel(s, Some(channel), &name)
                });
                match found {
                    Some(room) => self.on_voice(VoiceMsg::Join(inst, room)),
                    None => {
                        self.error(format!("No voice channel {name:?} here"));
                        Task::none()
                    }
                }
            }
            SlashCommand::Leave => self.on_voice(VoiceMsg::Leave),
            SlashCommand::Mute => self.on_voice(VoiceMsg::ToggleMute),
            SlashCommand::Deafen => self.on_voice(VoiceMsg::ToggleDeafen),
            SlashCommand::Status(status) => {
                self.core.set_presence(status);
                self.info(format!("Status: {status:?}"));
                Task::none()
            }
            SlashCommand::Nick(nick) => {
                let space = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.space_of(channel));
                let Some(space) = space else {
                    self.error("Nicknames are per space; open a space channel.".into());
                    return Task::none();
                };
                let core = self.core.clone();
                Task::perform(
                    async move {
                        let req = hermes_client_core::proto::api::UpdateMemberRequest {
                            nickname: Some(nick),
                            role_ids: None,
                        };
                        core.rest(&inst)?.update_member(space, None, &req).await?;
                        Ok::<_, hermes_client_core::CoreError>(())
                    },
                    |result| match result {
                        Ok(()) => Message::Done(Ok("Nickname changed".into())),
                        Err(err) => Message::Done(Err(err.to_string())),
                    },
                )
            }
            SlashCommand::Dm(name) => {
                let wanted = name.trim().trim_start_matches('@').to_lowercase();
                let user = self.core.store().instance(&inst).and_then(|s| {
                    let me = s.me_id();
                    s.users
                        .values()
                        .filter(|u| Some(u.id) != me)
                        .find(|u| u.username == wanted)
                        .or_else(|| {
                            s.users
                                .values()
                                .filter(|u| Some(u.id) != me)
                                .find(|u| u.name().to_lowercase() == wanted)
                        })
                        .map(|u| u.id)
                });
                match user {
                    Some(user) => self.message_user(inst, user),
                    None => {
                        self.error(format!("No user {name:?} known here"));
                        Task::none()
                    }
                }
            }
            SlashCommand::Invite => {
                let space = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.space_of(channel));
                match space {
                    Some(space) => {
                        self.run_action(Target::Space(inst, space), Action::InvitePeople)
                    }
                    None => {
                        self.error("Invites are for spaces; open a space channel.".into());
                        Task::none()
                    }
                }
            }
        }
    }

    /// Ctrl+V: an image on the clipboard is attached, text is pasted.
    fn paste(&mut self, inst: InstanceId, channel: Id) -> Task<Message> {
        let mut clipboard = match arboard::Clipboard::new() {
            Ok(c) => c,
            Err(err) => {
                self.error(format!("clipboard unavailable: {err}"));
                return Task::none();
            }
        };
        if let Ok(image) = clipboard.get_image() {
            let png = match encode_png(image.width, image.height, &image.bytes) {
                Ok(png) => png,
                Err(err) => {
                    self.error(format!("could not read the pasted image: {err}"));
                    return Task::none();
                }
            };
            let dir = std::env::temp_dir().join("hermes-gui");
            self.next_upload += 1;
            let path = dir.join(format!(
                "paste-{}-{}.png",
                std::process::id(),
                self.next_upload
            ));
            if let Err(err) =
                std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, png))
            {
                self.error(format!("could not keep the pasted image: {err}"));
                return Task::none();
            }
            return self.start_upload(inst, channel, path, Some("pasted image.png".into()));
        }
        if let Ok(text) = clipboard.get_text()
            && let Some(composer) = self.composers.get_mut(&(inst.clone(), channel))
        {
            composer
                .content
                .perform(iced::widget::text_editor::Action::Edit(
                    iced::widget::text_editor::Edit::Paste(std::sync::Arc::new(text)),
                ));
            self.refresh_suggestions(&inst, channel);
        }
        Task::none()
    }

    /// Adds an upload chip and starts uploading `path`.
    fn start_upload(
        &mut self,
        inst: InstanceId,
        channel: Id,
        path: std::path::PathBuf,
        name: Option<String>,
    ) -> Task<Message> {
        self.next_upload += 1;
        let local = self.next_upload;
        let name = name.unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into())
        });
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        self.composers
            .entry((inst.clone(), channel))
            .or_default()
            .uploads
            .push(Upload {
                local,
                name,
                size,
                state: UploadState::Uploading,
            });
        let core = self.core.clone();
        Task::perform(
            async move {
                let result = core.upload(&inst, &path).await.map_err(|e| e.to_string());
                (inst, result)
            },
            move |(inst, result)| Message::Uploaded(inst, channel, local, result),
        )
    }

    /// Starts loading the thumbnails of a message's images that are not loaded yet.
    fn load_thumbnails(&mut self, inst: &InstanceId, channel: Id, message: Id) -> Task<Message> {
        let avatar = if self.config.buffer.show_avatars {
            self.core
                .store()
                .instance(inst)
                .and_then(|s| {
                    let author = s.window(channel)?.get(message)?.author.id;
                    s.user(author)?.avatar.clone()
                })
                .map(|a| a.id)
                .filter(|id| !self.images.contains_key(&(inst.clone(), *id)))
        } else {
            None
        };
        if !self.config.buffer.image_previews {
            return match avatar {
                Some(id) => self.load_images(inst, vec![id]),
                None => Task::none(),
            };
        }
        let wanted: Vec<Id> = {
            let store = self.core.store();
            let Some(msg) = store
                .instance(inst)
                .and_then(|s| s.window(channel))
                .and_then(|w| w.get(message))
            else {
                return Task::none();
            };
            msg.attachments
                .iter()
                .filter(|a| channel::message::is_image(a))
                .map(|a| a.id)
                .filter(|id| !self.images.contains_key(&(inst.clone(), *id)))
                .chain(avatar)
                .collect()
        };
        self.load_images(inst, wanted)
    }

    /// Loads the thumbnails of these attachments into `images`.
    fn load_images(&mut self, inst: &InstanceId, wanted: Vec<Id>) -> Task<Message> {
        let mut tasks = Vec::new();
        for id in wanted {
            self.images.insert((inst.clone(), id), Img::Loading);
            let core = self.core.clone();
            let inst = inst.clone();
            tasks.push(Task::perform(
                async move {
                    let result = match core.rest(&inst) {
                        Ok(rest) => rest.thumbnail(id).await.map_err(|e| e.to_string()),
                        Err(err) => Err(err.to_string()),
                    };
                    (inst, result)
                },
                move |(inst, result)| Message::ImageLoaded(inst, id, result),
            ));
        }
        Task::batch(tasks)
    }

    /// Downloads an attachment into the Downloads folder (never overwriting a file).
    fn download(
        &self,
        inst: InstanceId,
        attachment: hermes_client_core::proto::models::Attachment,
    ) -> Task<Message> {
        let core = self.core.clone();
        Task::perform(
            async move {
                let bytes = core
                    .rest(&inst)
                    .map_err(|e| e.to_string())?
                    .download(attachment.id, &attachment.filename)
                    .await
                    .map_err(|e| e.to_string())?;
                let dir = crate::open::downloads_dir();
                let path = tokio::task::spawn_blocking(move || {
                    crate::open::save_new(&dir, &attachment.filename, &bytes)
                })
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
                Ok(format!("Saved to {}", path.display()))
            },
            |result: Result<String, String>| {
                Message::Done(result.map_err(|e| format!("download failed: {e}")))
            },
        )
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
            (Target::Message(inst, channel, message), Action::CopyText) => {
                let text = core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.window(channel))
                    .and_then(|w| w.get(message))
                    .map(|m| m.content.clone());
                if let Some(text) = text {
                    return self.update(Message::Copy(text, "Message text"));
                }
            }
            (Target::Message(_, _, message), Action::CopyMessageId) => {
                return self.update(Message::Copy(message.to_string(), "Message ID"));
            }
            (Target::Message(inst, channel, message), Action::Pin | Action::Unpin) => {
                let pin = action == Action::Pin;
                return Task::perform(
                    async move {
                        let rest = core.rest(&inst)?;
                        if pin {
                            rest.pin(channel, message).await?;
                        } else {
                            rest.unpin(channel, message).await?;
                        }
                        Ok::<_, hermes_client_core::CoreError>(())
                    },
                    move |result| match result {
                        Ok(()) => Message::Done(Ok(if pin { "Pinned" } else { "Unpinned" }.into())),
                        Err(err) => Message::Done(Err(err.to_string())),
                    },
                );
            }
            (Target::Message(inst, channel, message), Action::DeleteMessage) => {
                return Task::perform(
                    async move { core.delete_message(&inst, channel, message).await },
                    |result| match result {
                        Ok(()) => Message::ChannelOpened,
                        Err(err) => Message::Done(Err(format!("could not delete: {err}"))),
                    },
                );
            }
            (target, action) => return self.run_more_action(target, action),
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
        if !captured && !self.ptt_down && self.ptt_matches(key) {
            return self.on_voice(VoiceMsg::Ptt(true));
        }
        // Context menu: arrows choose, Enter runs (keyboard users, section 15 accessibility).
        if self.menu.is_some() {
            match key {
                keyboard::Key::Named(Named::ArrowDown) => return self.step_menu(1),
                keyboard::Key::Named(Named::ArrowUp) => return self.step_menu(-1),
                keyboard::Key::Named(Named::Enter | Named::Space) => {
                    if let Some(message) = self.menu_choice() {
                        return self.update(message);
                    }
                    return Task::none();
                }
                _ => {}
            }
        }
        // Tab moves between the fields of a dialog or settings page.
        if *key == keyboard::Key::Named(Named::Tab)
            && !captured
            && !modifiers.control()
            && !modifiers.alt()
        {
            return if modifiers.shift() {
                operation::focus_previous()
            } else {
                operation::focus_next()
            };
        }
        // Shift+F10 or the Menu key: the menu of the focused channel, as a right-click would.
        let menu_key = *key == keyboard::Key::Named(Named::ContextMenu)
            || (*key == keyboard::Key::Named(Named::F10) && modifiers.shift());
        if menu_key && self.menu.is_none() {
            if let Some((inst, channel)) = self.panes.focused_channel() {
                let at = Point::new(self.window_size.width / 3.0, self.window_size.height / 4.0);
                self.menu = Some((Target::Channel(inst, channel), at));
                self.menu_index = Some(0);
            }
            return Task::none();
        }
        if let Some(bar) = &mut self.command_bar {
            match key {
                keyboard::Key::Named(Named::ArrowDown) => bar.move_selection(1),
                keyboard::Key::Named(Named::ArrowUp) => bar.move_selection(-1),
                _ if escape => self.command_bar = None,
                _ => {}
            }
            return Task::none();
        }
        let overlay = self.viewer.is_some()
            || self.volume_menu.is_some()
            || self.menu.is_some()
            || self.confirm.is_some()
            || self.new_dm.is_some()
            || self.form.is_some()
            || self.add_instance.is_some();
        if escape && (overlay || !captured) {
            return self.update(Message::Escape);
        }
        if overlay || (captured && !(modifiers.control() || modifiers.alt() || modifiers.logo())) {
            return Task::none();
        }
        match self.keybinds.action(key, physical, modifiers) {
            Some(action) => self.key_action(action),
            None => self.type_into_composer(key, modifiers, captured),
        }
    }

    /// Typing while nothing has the keyboard goes into the focused channel's message box, as in
    /// other chat apps.
    fn type_into_composer(
        &mut self,
        key: &keyboard::Key,
        modifiers: Modifiers,
        captured: bool,
    ) -> Task<Message> {
        let keyboard::Key::Character(text) = key else {
            return Task::none();
        };
        if captured || modifiers.control() || modifiers.alt() || modifiers.logo() {
            return Task::none();
        }
        let Some((inst, channel)) = self.panes.focused_channel() else {
            return Task::none();
        };
        let can_send = self.core.store().instance(&inst).is_some_and(|s| {
            hermes_client_core::view::can(
                s,
                channel,
                hermes_client_core::proto::permissions::Permissions::SEND_MESSAGES,
            )
        });
        if !can_send {
            return Task::none();
        }
        let composer = self.composers.entry((inst.clone(), channel)).or_default();
        composer
            .content
            .perform(iced::widget::text_editor::Action::Edit(
                iced::widget::text_editor::Edit::Paste(std::sync::Arc::new(text.to_string())),
            ));
        let focus = operation::focus(composer.editor_id.clone());
        self.refresh_suggestions(&inst, channel);
        focus
    }

    /// The open menu's entries (separators left out).
    fn menu_entries(&self) -> Vec<(Target, Action)> {
        let Some((target, _)) = &self.menu else {
            return Vec::new();
        };
        let status = target
            .inst()
            .map_or(ConnStatus::Connected, |inst| self.core.status(inst));
        let store = self.core.store();
        menu::entries_with(
            target,
            &status,
            target.inst().and_then(|i| store.instance(i)),
            Some(self.core.presence()),
        )
        .into_iter()
        .flatten()
        .map(|e| (target.clone(), e.action))
        .collect()
    }

    fn step_menu(&mut self, delta: i32) -> Task<Message> {
        let len = self.menu_entries().len() as i32;
        if len > 0 {
            let next = match self.menu_index {
                Some(i) => (i as i32 + delta).rem_euclid(len),
                None if delta > 0 => 0,
                None => len - 1,
            };
            self.menu_index = Some(next as usize);
        }
        Task::none()
    }

    fn menu_choice(&self) -> Option<Message> {
        let i = self.menu_index?;
        self.menu_entries()
            .into_iter()
            .nth(i)
            .map(|(target, action)| Message::Menu(target, action))
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
            K::Search => {
                return match self.panes.focused_channel() {
                    Some((inst, channel)) => self.search_from(inst, channel),
                    None => match self.core.instance_ids().first() {
                        Some(inst) => self
                            .open_results(crate::buffer::results::Key::Search(inst.clone(), None)),
                        None => Task::none(),
                    },
                };
            }
            K::ToggleMute => return self.on_voice(VoiceMsg::ToggleMute),
            K::ToggleDeafen => return self.on_voice(VoiceMsg::ToggleDeafen),
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
                self.on_settings(settings::Message::Theme(name))
            }
            Some(command_bar::Command::AddInstance) => self.open_add_instance(String::new()),
            Some(command_bar::Command::JoinVoice(inst, channel)) => {
                self.on_voice(VoiceMsg::Join(inst, channel))
            }
            Some(command_bar::Command::LeaveVoice) => self.on_voice(VoiceMsg::Leave),
            Some(command_bar::Command::SpaceSettings(inst, space)) => {
                self.open_space_settings(inst, space, crate::buffer::space_settings::Tab::Overview)
            }
            Some(command_bar::Command::Results(key)) => self.open_results(key),
            Some(command_bar::Command::Menu(target, action)) => {
                self.update(Message::Menu(target, action))
            }
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
        let core = &self.core;
        tree::rows(
            &core.store(),
            &statuses,
            &self.collapsed,
            self.panes.focused_channel().as_ref(),
            &|inst, user| core.is_speaking(inst, user),
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
        // Focus moves only when the dialog changes step: typing the first letter of a username
        // must not jump to the password field.
        let before = std::mem::discriminant(&dialog.step);
        match dialog.update(message, &self.core) {
            add_instance::Action::None => {
                if std::mem::discriminant(&dialog.step) != before {
                    return focus_input(dialog.focus());
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

    fn on_core(&mut self, event: CoreEvent) -> Task<Message> {
        let mut task = Task::none();
        match event {
            CoreEvent::ConfigReloaded { warnings } => {
                self.apply_config(self.core.config());
                if let Some(warning) = warnings.into_iter().next() {
                    self.warn(format!("config: {warning}"));
                }
            }
            CoreEvent::Notification { inst, notification } => {
                task = self.notify(inst, notification);
            }
            CoreEvent::Error(text) => self.error(text),
            CoreEvent::IncomingCall {
                inst,
                channel,
                caller,
            } => self.incoming_call(inst, channel, caller),
            CoreEvent::CallRingStopped { inst, channel } => self.call_ring_stopped(&inst, channel),
            CoreEvent::InputLevel(db) => self.input_level = db,
            CoreEvent::VoiceStatus(_) => self.update_tray_voice(),
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
        self.update_badge();
        task
    }

    /// A toast (if enabled) and a taskbar flash for DMs and mentions while unfocused.
    fn notify(
        &mut self,
        inst: InstanceId,
        notification: hermes_client_core::notify::Notification,
    ) -> Task<Message> {
        let settings = self.config.notifications.clone();
        if !settings.enabled {
            return Task::none();
        }
        let flash = if !self.focused && settings.flash_taskbar && desktop::flashes(&notification) {
            window::oldest().then(|id| match id {
                Some(id) => {
                    window::request_user_attention(id, Some(window::UserAttention::Informational))
                }
                None => Task::none(),
            })
        } else {
            Task::none()
        };
        let toast = Task::perform(
            desktop::show_toast(inst, notification, settings),
            |clicked| match clicked {
                Some((inst, channel)) => Message::NotificationClicked(inst, channel),
                None => Message::ChannelOpened,
            },
        );
        Task::batch([flash, toast])
    }

    /// The tray's badge dot follows unread mentions and DMs.
    fn update_badge(&mut self) {
        if let Some(tray) = &mut self.tray {
            let badge = desktop::has_badge(&self.core.store(), &self.core.instance_ids());
            tray.set_badge(badge);
        }
    }

    fn hide_window(&mut self) -> Task<Message> {
        self.window_visible = false;
        self.save_layout();
        window::oldest().then(|id| match id {
            Some(id) => window::set_mode(id, window::Mode::Hidden),
            None => Task::none(),
        })
    }

    fn show_window(&mut self) -> Task<Message> {
        self.window_visible = true;
        window::oldest().then(|id| match id {
            Some(id) => window::set_mode(id, window::Mode::Windowed)
                .chain(window::minimize(id, false))
                .chain(window::gain_focus(id)),
            None => Task::none(),
        })
    }

    fn on_settings(&mut self, message: settings::Message) -> Task<Message> {
        use settings::Message as S;

        let message = match self.on_account(message) {
            Ok(task) => return task,
            Err(message) => message,
        };
        let mut voice = self.config.voice.clone();
        match message {
            S::Tab(tab) => {
                self.settings_tab = tab;
                if tab == settings::Tab::Voice && self.audio_devices.is_none() {
                    return self.load_devices();
                }
                return Task::none();
            }
            S::RefreshDevices => return self.load_devices(),
            S::Devices(devices) => {
                self.audio_devices = Some(devices);
                return Task::none();
            }
            S::MicTest(on) => {
                self.mic_test = on;
                self.core.set_mic_test(on);
                return Task::none();
            }
            S::PttKey(text) => {
                self.ptt_draft = Some(text);
                return Task::none();
            }
            S::SavePttKey => {
                if let Some(text) = self.ptt_draft.take() {
                    let text = text.trim().to_owned();
                    if !text.is_empty() && crate::ptt::virtual_keys(&text).is_none() {
                        self.error(format!("{text:?} is not a key Hermes knows"));
                        self.ptt_draft = Some(text);
                    } else {
                        let value = toml::Value::String(text).to_string();
                        self.persist("voice.ptt_key", &value);
                        self.info("Push-to-talk key saved".into());
                    }
                }
                return Task::none();
            }
            S::JoinOn(join_on) => {
                let value = toml::Value::String(join_on.as_config().to_owned()).to_string();
                self.persist("voice.join_on", &value);
                return Task::none();
            }
            // Sliders: shown live, applied on release.
            S::InputVolume(v) => {
                self.config.voice.input_volume = (v * 100.0).round() / 100.0;
                return Task::none();
            }
            S::OutputVolume(v) => {
                self.config.voice.output_volume = (v * 100.0).round() / 100.0;
                return Task::none();
            }
            S::SoundsVolume(v) => {
                self.config.voice.sounds_volume = (v * 100.0).round() / 100.0;
                return Task::none();
            }
            S::VadSensitivity(db) => {
                self.config.voice.vad_sensitivity = db;
                return Task::none();
            }
            S::ReleaseDelay(ms) => {
                self.config.voice.ptt_release_delay_ms = ms;
                return Task::none();
            }
            S::SaveVoice => {}
            S::InputDevice(device) => voice.input_device = device.as_config(),
            S::OutputDevice(device) => voice.output_device = device.as_config(),
            S::Mode(mode) => voice.mode = mode.as_config().to_owned(),
            S::NoiseSuppression(on) => voice.noise_suppression = on,
            S::AutoGain(on) => voice.auto_gain = on,
            S::Sounds(on) => voice.sounds = on,
            other => {
                self.on_appearance(other);
                return Task::none();
            }
        }
        self.save_voice(&voice);
        Task::none()
    }

    /// Applies `[voice]` to the audio engine and writes it to `config.toml`.
    fn save_voice(&mut self, voice: &hermes_client_core::config::Voice) {
        let settings = hermes_client_core::voice::audio_settings(voice);
        if let Err(err) = self.core.set_audio_settings(settings) {
            self.error(format!("setting not saved: {err}"));
        }
        self.apply_config(self.core.config());
    }

    fn on_appearance(&mut self, message: settings::Message) {
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
            _ => {}
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
        self.register_global_keys();
        if theme_changed {
            self.reload_theme();
        }
    }

    /// Registers `[keys] toggle_mute_global` (when it changed).
    pub fn register_global_keys(&mut self) {
        let combo = self
            .config
            .keys
            .get(keybinds::GLOBAL_MUTE)
            .cloned()
            .unwrap_or_default();
        let Some(keys) = &mut self.global_keys else {
            return;
        };
        if keys.combo == combo {
            return;
        }
        if let Err(err) = keys.set_mute(&combo) {
            self.warn(format!("[keys] {}: {err}", keybinds::GLOBAL_MUTE));
        }
    }

    /// Whether `key` is the push-to-talk key, for systems without global key polling (the
    /// window has to be focused there). On Windows the polling thread does it.
    fn ptt_matches(&self, key: &keyboard::Key) -> bool {
        if cfg!(windows) {
            return false;
        }
        let Some(combo) = keybinds::Combo::parse(&self.config.voice.ptt_key) else {
            return false;
        };
        combo.matches(
            key,
            keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified),
            Modifiers::default(),
        ) || matches!((&combo.key, key), (keybinds::KeyName::Char(c), keyboard::Key::Character(k)) if k.to_lowercase() == *c)
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

    pub(crate) fn info(&mut self, text: String) {
        tracing::info!("{text}");
        self.status = Some(Status {
            text,
            level: Level::Info,
        });
    }

    pub(crate) fn warn(&mut self, text: String) {
        tracing::warn!("{text}");
        self.status = Some(Status {
            text,
            level: Level::Warning,
        });
    }

    pub(crate) fn error(&mut self, text: String) {
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
        if self.tray.is_some() {
            subscriptions.push(Subscription::run(crate::desktop::tray::events).map(Message::Tray));
        }
        if self.call.is_some() {
            subscriptions.push(
                iced::time::every(voice_ui::RING_EVERY).map(|_| Message::Voice(VoiceMsg::RingTick)),
            );
        }
        if let Some(keys) = crate::ptt::virtual_keys(&self.config.voice.ptt_key) {
            subscriptions
                .push(crate::ptt::poll(keys).map(|down| Message::Voice(VoiceMsg::Ptt(down))));
        }
        if self.global_keys.is_some() {
            subscriptions
                .push(crate::ptt::global_presses().map(|()| Message::Voice(VoiceMsg::GlobalMute)));
        }
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
            self.ptt_down,
            self.drag.target(),
            self.update.as_ref(),
            &self.images,
        );

        let account_inst = self.account_instance();
        let mut main = column![].width(Fill).height(Fill);
        if let Some(call) = &self.call {
            main = main.push(voice_ui::banner(&self.core, call, fonts, theme));
        }
        if let Some(status) = &self.status {
            main = main.push(
                container(
                    row![
                        small(status.text.as_str(), fonts).width(Fill),
                        button(small("Dismiss", fonts))
                            .padding(style::BUTTON_PAD)
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
        main =
            main.push(self.panes.view(&panes::Context {
                core: &self.core,
                fonts,
                theme,
                config: &self.config,
                themes: &self.themes,
                utc_offset: self.utc_offset,
                images: &self.images,
                composers: &self.composers,
                settings_tab: self.settings_tab,
                voice: settings::VoiceView {
                    devices: self.audio_devices.as_ref(),
                    level: self.input_level,
                    mic_test: self.mic_test,
                    ptt_draft: self.ptt_draft.as_deref(),
                },
                space_settings: &self.space_settings,
                default_space_settings: &self.default_space_settings,
                drag: &self.drag,
                results: &self.results,
                default_results: &self.default_results,
                account:
                    crate::buffer::settings_tabs::AccountView {
                        instances: self.core.instance_ids(),
                        selected: account_inst.clone(),
                        me:
                            account_inst.as_ref().and_then(|i| {
                                self.core.store().instance(i).and_then(|s| s.me.clone())
                            }),
                        sessions: &self.sessions,
                        utc_offset: self.utc_offset,
                    },
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
            let status = target
                .inst()
                .map_or(ConnStatus::Connected, |inst| self.core.status(inst));
            let entries = {
                let store = self.core.store();
                menu::entries_with(
                    target,
                    &status,
                    target.inst().and_then(|i| store.instance(i)),
                    Some(self.core.presence()),
                )
            };
            let mut index = 0;
            let items = entries
                .into_iter()
                .map(|e| {
                    e.map(|e| {
                        let selected = self.menu_index == Some(index);
                        index += 1;
                        context_menu::Item {
                            label: e.action.label().to_owned(),
                            message: Message::Menu(target.clone(), e.action),
                            danger: e.action.is_red(),
                            checked: e.checked,
                            selected,
                        }
                    })
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
        if let Some(menu) = &self.volume_menu {
            root = voice_ui::volume_overlay(root, menu, self.window_size, fonts, theme);
        }
        if let Some(viewer) = &self.viewer {
            root = image_viewer::view(
                root,
                viewer,
                fonts,
                theme,
                Message::CloseViewer,
                Message::DownloadViewer,
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
        if let Some(form) = &self.form {
            root = modal::overlay(
                root,
                form.view(fonts, theme).map(Message::Form),
                theme,
                Some(Message::Form(form::Message::Cancel)),
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
        Action::DeleteMessage => (
            "Delete this message?",
            "It is removed for everyone. This cannot be undone.",
        ),
        Action::LeaveSpace => (
            "Leave this space?",
            "You will need a new invite to join again.",
        ),
        Action::DeleteSpace => (
            "Delete this space?",
            "Every channel, message, role and invite of the space is deleted for everyone. This              cannot be undone.",
        ),
        Action::DeleteChannel => (
            "Delete this channel?",
            "Its messages are deleted for everyone. Channels inside a deleted category move to              the top level. This cannot be undone.",
        ),
        Action::LeaveGroup => (
            "Leave this group?",
            "You stop getting its messages. Someone in it can add you again.",
        ),
        Action::Kick => (
            "Kick this member?",
            "They are removed from the space but can rejoin with an invite.",
        ),
        Action::DeleteRole => (
            "Delete this role?",
            "Members who have it lose its permissions. This cannot be undone.",
        ),
        Action::RevokeInvite => (
            "Revoke this invite?",
            "The link stops working at once. People who already joined stay.",
        ),
        Action::TransferOwnership => (
            "Transfer ownership?",
            "They become the owner and can delete the space. Only they can give it back.",
        ),
        _ => ("Are you sure?", ""),
    }
}

/// RGBA pixels from the clipboard as a PNG file.
fn encode_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let image = image::RgbaImage::from_raw(width as u32, height as u32, rgba.to_vec())
        .ok_or("the image data has the wrong size")?;
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
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
    async fn context_menus_work_with_the_keyboard() {
        use iced::keyboard::Key;
        use iced::keyboard::key::Named;

        const ANY: keyboard::key::Physical =
            keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
        let dir = temp_dir("menu-keys");
        let mut app = app_in(&dir);
        let pane = app.panes.focus;
        let _ = app.update(Message::OpenMenu(Target::Pane(pane)));
        assert!(app.menu_choice().is_none(), "nothing chosen yet");
        let key = |name| Message::Key(Key::Named(name), ANY, Modifiers::empty(), false);
        let _ = app.update(key(Named::ArrowDown));
        let _ = app.update(key(Named::ArrowDown));
        assert!(matches!(
            app.menu_choice(),
            Some(Message::Menu(Target::Pane(p), Action::SplitDown)) if p == pane
        ));
        let _ = app.update(key(Named::ArrowUp));
        let _ = app.update(key(Named::ArrowUp));
        assert!(
            matches!(
                app.menu_choice(),
                Some(Message::Menu(Target::Pane(p), Action::ClosePane)) if p == pane
            ),
            "wraps around"
        );
        let _ = app.update(key(Named::Enter));
        assert!(
            app.menu.is_none(),
            "Enter runs the entry and closes the menu"
        );
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
