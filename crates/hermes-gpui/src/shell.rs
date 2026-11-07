//! The root view: the icon rail, the sidebar next to it and the main panel under its header. It
//! owns the state; each feature's module adds its methods.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, AnyWindowHandle, Context, MouseButton, Pixels, Point, Size, Subscription, Window,
    WindowControlArea, div, px,
};
use hermes_client_core::config::ClientConfig;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{ChannelKind, PresenceStatus, Status as UserStatus};
use hermes_client_core::store::Changed;
use hermes_client_core::{Core, CoreEvent, CoreEvents, InstanceId};
use tokio::runtime::Handle;

use crate::add_instance::AddInstance;
use crate::assets::{self, icon};
use crate::channel::{ChatView, Viewer};
use crate::collapsed::Collapsed;
use crate::command_bar::CommandBar;
use crate::desktop::tray::Tray;
use crate::form::Form;
use crate::images::Images;
use crate::keybinds::Keybinds;
use crate::menu::OpenMenu;
use crate::menu_entries::{Action, Target};
use crate::new_dm::NewDm;
use crate::ptt::GlobalKeys;
use crate::results::{Key, ResultsPages};
use crate::screen::{Latest, Share, capture::Capture};
use crate::settings::SettingsPage;
use crate::single_instance::{self, Endpoint};
use crate::space_settings::SpacePage;
use crate::voice::{Call, NetHistory, VolumeMenu};
use crate::{theme, watchdog};

/// What `main` hands to the shell.
pub struct Boot {
    pub core: Core,
    pub events: CoreEvents,
    pub runtime: Handle,
    pub endpoint: Endpoint,
    pub link: Option<String>,
    pub warning: Option<String>,
    pub tray: Option<Tray>,
    pub global_keys: Option<GlobalKeys>,
    pub start_hidden: bool,
}

/// A channel of an instance.
pub type ChannelKey = (InstanceId, Id);

/// What the main panel shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    Friends,
    Chat(InstanceId, Id),
    Voice(InstanceId, Id),
    /// Search, mentions or pinned messages.
    Results(Key),
}

/// The rail's selected icon, which decides what the sidebar lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selected {
    /// The app icon: Friends, with the DMs in the sidebar.
    Home,
    /// A space: its channels.
    Space(InstanceId, Id),
}

/// A line shown over the main panel until dismissed; info lines go away by themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub level: Level,
    /// Which status this is: a timer only clears the one it was started for.
    pub serial: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

/// How long an info line stays.
const INFO_FOR: Duration = Duration::from_secs(5);

pub struct Shell {
    pub core: Core,
    pub runtime: Handle,
    pub window: AnyWindowHandle,
    /// What the views show. Equal to `core.config()` except while a slider is being dragged.
    pub config: ClientConfig,
    pub page: Page,
    pub selected: Selected,
    /// The channel last open in each space, reopened when the space is selected again.
    pub last_channel: HashMap<(InstanceId, Id), Id>,
    pub collapsed: Collapsed,
    pub chats: HashMap<ChannelKey, ChatView>,
    pub images: Images,
    pub status: Option<Status>,
    status_serial: u64,
    /// Where the right button went down, until it comes up.
    pub right_pressed: Option<Target>,
    pub menu: Option<OpenMenu>,
    /// A destructive menu action waiting for confirmation.
    pub confirm: Option<(Target, Action)>,
    pub add_instance: Option<AddInstance>,
    pub form: Option<Form>,
    /// The settings page, over the whole window.
    pub settings: Option<SettingsPage>,
    /// The tab the settings page opens on.
    pub settings_tab: crate::settings::Tab,
    /// The UI scale slider's value while it is dragged: applied on release, because the slider
    /// would change size under the pointer otherwise.
    pub ui_scale_draft: Option<f64>,
    /// Names of the themes, and each loaded for the previews in settings (or what is wrong with
    /// its file).
    pub themes: Vec<String>,
    pub theme_previews: Vec<(String, Result<crate::theme::Theme, String>)>,
    /// What each opened space's settings remember, and the one showing.
    pub space_pages: HashMap<(InstanceId, Id), SpacePage>,
    pub open_space_page: Option<(InstanceId, Id)>,
    pub results: ResultsPages,
    /// What a results page goes back to.
    pub previous_page: Option<Page>,
    pub command_bar: Option<CommandBar>,
    pub keybinds: Keybinds,
    /// The member list shows next to a chat.
    pub show_members: bool,
    pub sidebar_visible: bool,
    /// The sidebar's edge is being dragged: where the drag started, and the width then.
    pub resizing: Option<(f32, f32)>,
    /// A newer release, if the update check found one.
    pub update: Option<hermes_client_core::update::Update>,
    pub new_dm: Option<NewDm>,
    /// The full-size image viewer.
    pub viewer: Option<Viewer>,
    /// Local UTC offset for timestamps, read once at startup.
    pub utc_offset: i64,
    pub next_upload: u64,
    /// A join in progress: further requests wait for it.
    pub joining: Option<ChannelKey>,
    pub headphones_hint_shown: bool,
    /// Microphone level in dBFS while the meter runs.
    pub input_level: f32,
    /// What the level meter's bar shows, and when it was last drawn.
    pub meter_shown: f32,
    pub meter_drawn: Instant,
    /// The push-to-talk key is held.
    pub ptt_down: bool,
    /// What the push-to-talk thread watches.
    pub ptt_keys: Arc<Mutex<Vec<u16>>>,
    /// A ringing incoming call (the banner).
    pub call: Option<Call>,
    pub volume_menu: Option<VolumeMenu>,
    /// The voice connection's last minutes.
    pub net: NetHistory,
    /// How many seconds the network graphs show.
    pub net_range: usize,
    /// Where the pointer is along the network graphs, 0 to 1.
    pub net_hover: Option<f32>,
    /// The window's size, as last drawn.
    pub viewport: Size<Pixels>,
    /// The network details are open, where they were asked for.
    pub network_open: Option<Point<Pixels>>,
    /// The size of the voice room's tile area, as last drawn.
    pub room_area: Size<Pixels>,
    /// The share menu is open, where it was asked for.
    pub share_menu: Option<Point<Pixels>>,
    /// The screens that can be shared, primary first.
    pub screen_names: Vec<String>,
    /// My running screen share capture.
    pub capture: Option<Capture>,
    /// The latest picture of each screen share I watch.
    pub screens: HashMap<ChannelKey, Share>,
    /// The screen share shown alone in the voice room (clicked to enlarge).
    pub enlarged_share: Option<ChannelKey>,
    /// The screen shares I chose to watch (forgotten when the share or my call ends). They are
    /// received only while their voice room shows, so a hidden room costs nothing.
    pub watching_shares: HashSet<ChannelKey>,
    /// What the core was last told to receive.
    pub watched_now: Vec<Id>,
    pub screen_latest: Option<Arc<Latest>>,
    pub tray: Option<Tray>,
    pub global_keys: Option<GlobalKeys>,
    /// The window is shown (not hidden in the tray).
    pub window_visible: bool,
    /// The window's scale factor, as last drawn.
    pub scale: f32,
    /// What was open last time: opens once its instance is ready.
    restore: Option<(InstanceId, Id)>,
    /// The title last given to the window.
    title: String,
    /// Give the keyboard to the open chat's message box at the next draw.
    pub focus_composer: bool,
    /// The window has the keyboard focus.
    pub focused: bool,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    pub fn new(boot: Boot, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let Boot {
            core,
            events,
            runtime,
            endpoint,
            link,
            warning,
            tray,
            global_keys,
            start_hidden,
        } = boot;
        let config = core.config();
        let (keybinds, key_warnings) = Keybinds::from_config(&config.keys);
        let collapsed = core
            .paths()
            .map(|p| Collapsed::load(&p.dir))
            .unwrap_or_default();
        let shell = cx.entity().downgrade();
        let subscriptions = vec![
            cx.observe_window_activation(window, |this, window, cx| {
                this.window_activation(window.is_window_active(), cx);
            }),
            cx.intercept_keystrokes(move |event, window, cx| {
                let _ = shell.update(cx, |this, cx| this.on_key(event, window, cx));
            }),
        ];
        let mut this = Self {
            core,
            runtime,
            window: window.window_handle(),
            config,
            page: Page::Friends,
            selected: Selected::Home,
            last_channel: HashMap::new(),
            collapsed,
            chats: HashMap::new(),
            images: Images::default(),
            status: None,
            status_serial: 0,
            right_pressed: None,
            menu: None,
            confirm: None,
            add_instance: None,
            form: None,
            settings: None,
            settings_tab: crate::settings::Tab::default(),
            ui_scale_draft: None,
            themes: Vec::new(),
            theme_previews: Vec::new(),
            space_pages: HashMap::new(),
            open_space_page: None,
            results: HashMap::new(),
            previous_page: None,
            command_bar: None,
            keybinds,
            show_members: true,
            sidebar_visible: true,
            resizing: None,
            update: None,
            new_dm: None,
            viewer: None,
            utc_offset: hermes_client_core::time::local_offset_secs(),
            next_upload: 0,
            joining: None,
            headphones_hint_shown: false,
            input_level: -60.0,
            meter_shown: -60.0,
            meter_drawn: Instant::now(),
            ptt_down: false,
            ptt_keys: Arc::default(),
            call: None,
            volume_menu: None,
            net: NetHistory::default(),
            net_range: crate::voice::NET_RANGES[0],
            net_hover: None,
            viewport: window.viewport_size(),
            network_open: None,
            room_area: crate::voice::no_area(),
            share_menu: None,
            screen_names: Vec::new(),
            capture: None,
            screens: HashMap::new(),
            enlarged_share: None,
            watching_shares: HashSet::new(),
            watched_now: Vec::new(),
            screen_latest: None,
            tray,
            global_keys,
            window_visible: !start_hidden,
            scale: window.scale_factor(),
            restore: None,
            title: String::new(),
            focus_composer: false,
            focused: true,
            _subscriptions: subscriptions,
        };
        this.restore_layout();
        this.reload_theme(cx);
        window.set_rem_size(gpui_kit::px(16. * crate::theme::scale()));
        this.start_theme_watcher(cx);
        this.check_for_update(cx);
        if let Some(warning) = key_warnings.into_iter().next() {
            this.warn(warning, cx);
        }
        this.start_event_pump(events, cx);
        this.start_link_listener(endpoint, cx);
        this.start_heartbeat(cx);
        this.start_net_sampler(cx);
        this.start_screen_decoder(cx);
        this.start_desktop(start_hidden, window, cx);
        if let Some(warning) = warning {
            this.show_status(format!("config: {warning}"), Level::Warning, cx);
        }
        if let Some(link) = link {
            this.open_link(link, window, cx);
        }
        this.load_avatars(cx);
        this
    }

    // ---- async --------------------------------------------------------------------------------

    /// Runs `work` on the tokio runtime and `then` with its result on the UI thread.
    pub fn run<T: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        work: impl Future<Output = T> + Send + 'static,
        then: impl FnOnce(&mut Shell, T, &mut Window, &mut Context<Shell>) + 'static,
    ) {
        let task = self.runtime.spawn(work);
        let handle = self.window;
        cx.spawn(async move |this, cx| {
            let result = match task.await {
                Ok(result) => result,
                Err(err) => {
                    tracing::error!("a background task failed: {err}");
                    return;
                }
            };
            let _ = cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| {
                    let _phase = watchdog::in_update();
                    then(this, result, window, cx);
                    cx.notify();
                })
            });
        })
        .detach();
    }

    /// Runs `work`; its text goes to the status line, as info or as an error.
    pub fn run_status(
        &self,
        cx: &mut Context<Self>,
        work: impl Future<Output = Result<String, String>> + Send + 'static,
    ) {
        self.run(cx, work, |this, result, _, cx| match result {
            Ok(text) => this.info(text, cx),
            Err(text) => this.error(text, cx),
        });
    }

    /// Hands the core's events to the UI thread, all that have queued up at once.
    fn start_event_pump(&self, mut events: CoreEvents, cx: &mut Context<Self>) {
        let handle = self.window;
        cx.spawn(async move |this, cx| {
            while let Some(first) = events.recv().await {
                let mut batch = vec![first];
                while let Some(next) = events.try_recv() {
                    batch.push(next);
                }
                let result = cx.update_window(handle, |_, window, cx| {
                    this.update(cx, |this, cx| {
                        let _phase = watchdog::in_update();
                        watchdog::beat();
                        for event in batch {
                            this.on_core(event, window, cx);
                        }
                        cx.notify();
                    })
                });
                if !matches!(result, Ok(Ok(()))) {
                    return;
                }
            }
        })
        .detach();
    }

    /// Links forwarded by later launches (section 15.7).
    fn start_link_listener(&self, endpoint: Endpoint, cx: &mut Context<Self>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        self.runtime.spawn(async move {
            if let Err(err) = single_instance::serve(endpoint, tx).await {
                tracing::warn!("not receiving links from later launches: {err}");
            }
        });
        let handle = self.window;
        cx.spawn(async move |this, cx| {
            while let Some(link) = rx.recv().await {
                let _ = cx.update_window(handle, |_, window, cx| {
                    this.update(cx, |this, cx| {
                        tracing::info!(
                            "another launch handed over {}",
                            if link.is_empty() { "nothing" } else { "a link" }
                        );
                        window.activate_window();
                        if !link.is_empty() {
                            this.open_link(link, window, cx);
                        }
                        cx.notify();
                    })
                });
            }
        })
        .detach();
    }

    /// Tells the freeze watchdog the UI thread runs, also while nothing happens.
    fn start_heartbeat(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |_, cx| {
            loop {
                cx.background_executor().timer(watchdog::BEAT_EVERY).await;
                watchdog::beat();
            }
        })
        .detach();
    }

    // ---- events -------------------------------------------------------------------------------

    fn on_core(&mut self, event: CoreEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            CoreEvent::ConfigReloaded { warnings } => {
                self.apply_config(cx);
                self.reload_theme(cx);
                if let Some(warning) = warnings.into_iter().next() {
                    self.warn(format!("config: {warning}"), cx);
                }
            }
            CoreEvent::Error(text) => self.error(text, cx),
            CoreEvent::Notification { inst, notification } => {
                self.notify(inst, notification, window, cx);
            }
            CoreEvent::IncomingCall {
                inst,
                channel,
                caller,
            } => self.incoming_call(inst, channel, caller, cx),
            CoreEvent::CallRingStopped { inst, channel } => self.call_ring_stopped(&inst, channel),
            CoreEvent::KeyframeRequested => {
                if let Some(capture) = &self.capture {
                    capture.request_keyframe();
                }
            }
            CoreEvent::VoiceStatus(_) => self.update_tray_voice(),
            CoreEvent::InputLevel(db) => self.input_level = db,
            CoreEvent::StoreChanged { inst, what } => {
                match what {
                    Changed::Messages(channel) => self.messages_changed(&inst, channel, cx),
                    Changed::All => {
                        self.everything_changed(&inst, window, cx);
                        if self.restore.as_ref().is_some_and(|(i, _)| *i == inst)
                            && let Some((inst, channel)) = self.restore.take()
                        {
                            self.select(inst, channel, window, cx);
                        }
                    }
                    Changed::Spaces | Changed::Dms | Changed::Channel(_) => {
                        self.check_page_exists(cx);
                    }
                    _ => {}
                }
                self.load_avatars(cx);
            }
            // The views read the store directly; the other events only trigger a redraw.
            CoreEvent::InstanceStatus { .. }
            | CoreEvent::Speaking { .. }
            | CoreEvent::ScreenShares => {}
        }
        self.sync_screens(window);
        self.update_badge();
    }

    fn window_activation(&mut self, active: bool, cx: &mut Context<Self>) {
        self.focused = active;
        self.core.set_app_focused(active);
        if active {
            self.mark_open_read(cx);
        }
        cx.notify();
    }

    /// A new `ready`: the open chat's window was dropped, so it loads again.
    fn everything_changed(&mut self, inst: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.check_page_exists(cx);
        if let Page::Chat(i, channel) = &self.page
            && i == inst
        {
            let (i, channel) = (i.clone(), *channel);
            self.open_chat(i, channel, window, cx);
        }
    }

    /// Leaves a channel or space that is gone (deleted, or I left it).
    fn check_page_exists(&mut self, cx: &mut Context<Self>) {
        let store = self.core.store();
        let selected_gone = match &self.selected {
            Selected::Home => false,
            Selected::Space(inst, space) => store
                .instance(inst)
                .is_some_and(|s| s.me.is_some() && !s.spaces.contains_key(space)),
        };
        let page_gone = match &self.page {
            Page::Friends | Page::Results(_) => false,
            Page::Chat(inst, channel) | Page::Voice(inst, channel) => store
                .instance(inst)
                .is_some_and(|s| s.me.is_some() && s.channel(*channel).is_none()),
        };
        drop(store);
        if selected_gone {
            self.selected = Selected::Home;
        }
        if page_gone || selected_gone {
            self.open_friends(cx);
        }
    }

    // ---- status line --------------------------------------------------------------------------

    pub fn show_status(&mut self, text: String, level: Level, cx: &mut Context<Self>) {
        self.status_serial += 1;
        let serial = self.status_serial;
        self.status = Some(Status {
            text,
            level,
            serial,
        });
        if level == Level::Info {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(INFO_FOR).await;
                let _ = this.update(cx, |this, cx| {
                    if this.status.as_ref().is_some_and(|s| s.serial == serial) {
                        this.status = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    pub fn info(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.show_status(text.into(), Level::Info, cx);
    }

    pub fn warn(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.show_status(text.into(), Level::Warning, cx);
    }

    pub fn error(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        tracing::warn!("{text}");
        self.show_status(text, Level::Error, cx);
    }

    /// Writes a setting to `config.toml` and applies it.
    pub fn persist(&mut self, key: &str, value: &str, cx: &mut Context<Self>) {
        if let Err(err) = self.core.set_setting(key, value) {
            self.error(format!("setting not saved: {err}"), cx);
        }
        self.apply_config(cx);
    }

    /// The config changed: everything that follows it.
    pub fn apply_config(&mut self, cx: &mut Context<Self>) {
        self.config = self.core.config();
        let (keybinds, warnings) = Keybinds::from_config(&self.config.keys);
        self.keybinds = keybinds;
        if let Some(warning) = warnings.into_iter().next() {
            self.warn(warning, cx);
        }
        self.update_ptt_keys();
        self.register_global_keys(cx);
        cx.notify();
    }

    /// Puts text on the clipboard; `what` names it for the status line.
    pub fn copy(&mut self, text: String, what: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        self.info(format!("{what} copied"), cx);
    }

    // ---- navigation ---------------------------------------------------------------------------

    pub fn open_friends(&mut self, cx: &mut Context<Self>) {
        self.selected = Selected::Home;
        self.page = Page::Friends;
        self.update_visible_channels();
        cx.notify();
    }

    /// Selects a space on the rail and opens the channel last open in it, else its first text
    /// channel.
    pub fn select_space(
        &mut self,
        inst: InstanceId,
        space: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = Selected::Space(inst.clone(), space);
        let channel = {
            let store = self.core.store();
            let s = store.instance(&inst);
            self.last_channel
                .get(&(inst.clone(), space))
                .copied()
                .filter(|c| s.is_some_and(|s| s.channel(*c).is_some()))
                .or_else(|| {
                    s.and_then(|s| {
                        hermes_client_core::view::channel_tree(s, space)
                            .into_iter()
                            .find(|c| c.kind == ChannelKind::Text)
                            .map(|c| c.id)
                    })
                })
        };
        if let Some(channel) = channel {
            self.select(inst, channel, window, cx);
        }
        cx.notify();
    }

    /// Opens a channel: a voice channel's voice room, otherwise its chat. The rail and sidebar
    /// follow what was opened (a notification, a call, a link).
    pub fn select(
        &mut self,
        inst: InstanceId,
        channel: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (kind, space) = {
            let store = self.core.store();
            let s = store.instance(&inst);
            (
                s.and_then(|s| s.channel(channel)).map(|c| c.kind),
                s.and_then(|s| s.space_of(channel)),
            )
        };
        self.selected = match space {
            Some(space) => {
                self.last_channel.insert((inst.clone(), space), channel);
                Selected::Space(inst.clone(), space)
            }
            None => Selected::Home,
        };
        if kind == Some(ChannelKind::Voice) {
            self.page = Page::Voice(inst, channel);
            self.update_visible_channels();
            cx.notify();
            return;
        }
        self.open_chat(inst, channel, window, cx);
    }

    /// Tells the core which channel shows, for notifications and read marks.
    pub fn update_visible_channels(&self) {
        let visible = match &self.page {
            Page::Chat(inst, channel) => vec![(inst.clone(), *channel)],
            _ => Vec::new(),
        };
        self.core.set_visible_channels(visible);
    }

    /// The channel whose chat is open.
    pub fn open_channel(&self) -> Option<ChannelKey> {
        match &self.page {
            Page::Chat(inst, channel) => Some((inst.clone(), *channel)),
            _ => None,
        }
    }

    /// The window's title, for the taskbar: the current page.
    fn page_title(&self) -> String {
        let store = self.core.store();
        match &self.page {
            Page::Friends => "Friends".into(),
            Page::Results(Key::Search(..)) => "Search".into(),
            Page::Results(Key::Mentions) => "Mentions".into(),
            Page::Results(Key::Pins(..)) => "Pinned messages".into(),
            Page::Chat(inst, channel) | Page::Voice(inst, channel) => store
                .instance(inst)
                .and_then(|s| {
                    s.channel(*channel)
                        .map(|c| hermes_client_core::view::channel_title(s, c))
                })
                .unwrap_or_else(|| "Hermes".into()),
        }
    }

    // ---- drawing ------------------------------------------------------------------------------

    /// The bar over the main panel: it drags the window and holds the page's controls and the
    /// window buttons. It has no title: the sidebar and the content say where you are.
    fn render_header(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let window_button = |id: &'static str, name: &'static str, area, close: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .w(px(46.))
                .h_full()
                .window_control_area(area)
                .when(close, |d| {
                    d.hover(|d| d.bg(theme::danger()).text_color(theme::on_accent()))
                })
                .when(!close, |d| d.hover(|d| d.bg(theme::hover())))
                .child(icon(name, 16., theme::text_secondary()))
        };
        let maximize_icon = if window.is_maximized() {
            assets::RESTORE
        } else {
            assets::MAXIMIZE
        };
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(px(theme::SIDEBAR_HEADER))
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .id("header-drag")
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .children(self.render_header_controls(cx))
            .child(
                div()
                    .flex()
                    .h_full()
                    .child(window_button(
                        "win-min",
                        assets::MINIMIZE,
                        WindowControlArea::Min,
                        false,
                    ))
                    .child(window_button(
                        "win-max",
                        maximize_icon,
                        WindowControlArea::Max,
                        false,
                    ))
                    .child(window_button(
                        "win-close",
                        assets::CLOSE,
                        WindowControlArea::Close,
                        true,
                    )),
            )
            .into_any_element()
    }

    /// The page's buttons and fields in the header. (More come with search and the member list.)
    fn render_header_controls(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.settings.is_some() || self.open_space_page.is_some() {
            return Vec::new();
        }
        let mut controls: Vec<AnyElement> = Vec::new();
        // What Ctrl+K opens, for those who do not know the key yet.
        let find = div()
            .id("header-find")
            .flex()
            .items_center()
            .gap(px(6.))
            .w(px(210.))
            .h(px(28.))
            .px(px(8.))
            .mr(px(8.))
            .rounded(px(4.))
            .cursor_pointer()
            .bg(crate::theme::raised())
            .text_size(crate::theme::caption_size())
            .text_color(theme::text_tertiary())
            .child(icon(assets::SEARCH, 14., theme::text_tertiary()))
            .child(div().flex_1().truncate().child("Find anything"))
            .child(
                crate::keybinds::Keybinds::combo_text(
                    &self.config.keys,
                    crate::keybinds::Action::CommandBar,
                )
                .unwrap_or_default(),
            )
            .on_click(cx.listener(|this, _, window, cx| this.open_command_bar(window, cx)));
        controls.push(find.into_any_element());
        if let Some(button) = self.voice_header_button(cx) {
            controls.push(button);
        }
        if let Page::Voice(inst, channel) = &self.page {
            // A voice channel's own chat.
            let (inst, channel) = (inst.clone(), *channel);
            controls.push(
                crate::channel::tip_button("header-chat", assets::CHAT, "Open the chat")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_chat(inst.clone(), channel, window, cx);
                    }))
                    .into_any_element(),
            );
        }
        if let Some((inst, channel)) = self.open_channel() {
            let (pins_inst, search_inst) = (inst.clone(), inst);
            controls.push(
                crate::channel::tip_button("header-pins", assets::PIN, "Pinned messages")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_results(Key::Pins(pins_inst.clone(), channel), window, cx);
                    }))
                    .into_any_element(),
            );
            controls.push(
                crate::channel::tip_button("header-members", assets::USERS, "Member list")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_members = !this.show_members;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            controls.push(
                crate::channel::tip_button("header-search", assets::SEARCH, "Search")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.search_from(search_inst.clone(), channel, window, cx);
                    }))
                    .into_any_element(),
            );
        }
        controls.push(
            crate::channel::tip_button("header-mentions", assets::AT, "Mentions")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_results(Key::Mentions, window, cx);
                }))
                .into_any_element(),
        );
        vec![
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .mr(px(8.))
                .children(controls)
                .into_any_element(),
        ]
    }

    fn render_status(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let status = self.status.as_ref()?;
        let color = match status.level {
            Level::Info => theme::text_secondary(),
            Level::Warning => theme::accent(),
            Level::Error => theme::danger(),
        };
        Some(
            div()
                .id("status-line")
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(8.))
                .px(px(16.))
                .py(px(6.))
                .bg(theme::sidebar())
                .border_b_1()
                .border_color(theme::border())
                .text_size(theme::secondary_size())
                .text_color(color)
                .child(div().flex_1().min_w_0().child(status.text.clone()))
                .child(
                    div()
                        .id("status-dismiss")
                        .cursor_pointer()
                        .child(icon(assets::CLOSE, 16., theme::text_tertiary()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.status = None;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }
}

impl Shell {
    /// Reads what was open last time from `[[layout]]`. The first channel counts: a layout saved
    /// by the iced client can hold several.
    fn restore_layout(&mut self) {
        let restored = self.config.layout.iter().find_map(|table| {
            match table.get("buffer")?.as_str()? {
                "channel" | "voice_room" => {}
                _ => return None,
            }
            let inst = table.get("instance")?.as_str()?.to_owned();
            let channel = Id(table.get("channel")?.as_str()?.parse::<u64>().ok()?);
            let members = table
                .get("members")
                .and_then(toml::Value::as_bool)
                .unwrap_or(true);
            Some((inst, channel, members))
        });
        if let Some((inst, channel, members)) = restored {
            self.restore = Some((inst, channel));
            self.show_members = members;
        }
    }

    /// Writes what is open to `[[layout]]` in `config.toml`.
    pub fn save_layout(&mut self) {
        let mut table = toml::Table::new();
        match &self.page {
            Page::Chat(inst, channel) => {
                table.insert("buffer".into(), "channel".into());
                table.insert("instance".into(), inst.clone().into());
                table.insert("channel".into(), channel.to_string().into());
                table.insert("members".into(), self.show_members.into());
            }
            Page::Voice(inst, channel) => {
                table.insert("buffer".into(), "voice_room".into());
                table.insert("instance".into(), inst.clone().into());
                table.insert("channel".into(), channel.to_string().into());
            }
            Page::Friends | Page::Results(_) => {}
        }
        let tables = if table.is_empty() {
            Vec::new()
        } else {
            vec![toml::Value::Table(table)]
        };
        let value = toml::Value::Array(tables);
        if let Err(err) = self.core.set_setting("layout", &value.to_string()) {
            tracing::error!("layout not saved: {err}");
        }
    }

    /// Asks GitHub for a newer release (`general.check_for_updates`).
    fn check_for_update(&mut self, cx: &mut Context<Self>) {
        if !self.config.general.check_for_updates {
            return;
        }
        self.run(
            cx,
            hermes_client_core::update::check(env!("CARGO_PKG_VERSION")),
            |this, result, _, _| match result {
                Ok(update) => this.update = update,
                Err(err) => tracing::info!("update check failed: {err}"),
            },
        );
    }

    fn render_update_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let update = self.update.as_ref()?;
        let url = update.url.clone();
        Some(
            div()
                .id("update-banner")
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(8.))
                .px(px(16.))
                .py(px(6.))
                .bg(theme::accent_soft())
                .cursor_pointer()
                .text_size(crate::theme::secondary_size())
                .child(div().flex_1().child(format!(
                    "Hermes {} is available: click to open it",
                    update.version
                )))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Err(err) = crate::open::url(&url) {
                        this.error(format!("could not open the browser: {err}"), cx);
                    }
                }))
                .into_any_element(),
        )
    }

    /// The sidebar's edge: dragging starts on press, a double-click goes back to the default.
    fn render_resize_handle(&self, on_right: bool, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("sidebar-edge")
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(6.))
            .when(on_right, |d| d.left(px(-3.)))
            .when(!on_right, |d| d.right(px(-3.)))
            .cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
                    if event.click_count >= 2 {
                        this.persist("sidebar.width", "240", cx);
                        return;
                    }
                    let width = f32::from(this.config.sidebar.width);
                    this.resizing = Some((event.position.x.as_f32(), width));
                    cx.notify();
                }),
            )
    }

    fn resize_to(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some((start, width)) = self.resizing else {
            return;
        };
        let moved = (x - start) / crate::theme::scale();
        let on_right = self.config.sidebar.position.eq_ignore_ascii_case("right");
        let new = if on_right {
            width - moved
        } else {
            width + moved
        };
        self.config.sidebar.width = new.clamp(180., 480.) as u16;
        cx.notify();
    }

    fn end_resize(&mut self, cx: &mut Context<Self>) {
        if self.resizing.take().is_some() {
            let width = self.config.sidebar.width.to_string();
            self.persist("sidebar.width", &width, cx);
        }
    }

    /// The font or scale changed: every message's height with it.
    pub fn remeasure_chats(&self) {
        for view in self.chats.values() {
            view.list.remeasure();
        }
    }
}

/// My chosen presence as the status others see.
pub fn presence_status(presence: PresenceStatus) -> UserStatus {
    match presence {
        PresenceStatus::Online => UserStatus::Online,
        PresenceStatus::Idle => UserStatus::Idle,
        PresenceStatus::Dnd => UserStatus::Dnd,
        PresenceStatus::Invisible => UserStatus::Offline,
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        watchdog::beat();
        let _phase = watchdog::in_view();
        self.scale = window.scale_factor();
        self.viewport = window.viewport_size();
        self.animate_meter(window);
        let title = self.page_title();
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }
        if std::mem::take(&mut self.focus_composer) {
            self.focus_open_composer(window, cx);
        }

        let rail = self.render_rail(cx);
        let sidebar = self.render_sidebar(cx);
        let main = match self.page.clone() {
            Page::Friends => self.render_friends(cx).into_any_element(),
            Page::Chat(inst, channel) => self.render_chat(inst, channel, cx).into_any_element(),
            Page::Voice(inst, channel) => self.render_voice(inst, channel, cx).into_any_element(),
            Page::Results(key) => self.render_results(key, cx).into_any_element(),
        };
        // In a small window the chat needs the room more than the member list does.
        let room = window.viewport_size().width.as_f32() / crate::theme::scale();
        let sidebar_room = if self.sidebar_visible {
            f32::from(self.config.sidebar.width)
        } else {
            0.
        };
        let member_room = f32::from(self.config.sidebar.member_list_width);
        let members = match &self.page {
            Page::Chat(inst, channel)
                if room - theme::RAIL_WIDTH - sidebar_room - member_room >= 480. =>
            {
                self.render_members(inst, *channel, cx)
            }
            _ => None,
        };
        let header = self.render_header(window, cx);
        let main_column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(header)
            .children(self.render_call_banner(cx))
            .children(self.render_update_banner(cx))
            .children(self.render_status(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(div().flex_1().min_w_0().h_full().child(main))
                    .children(members),
            );
        let full_page = self
            .render_space_settings(cx)
            .or_else(|| self.render_settings(cx));
        let body = match full_page {
            // Settings replace the sidebar and the chat; the header still drags the window.
            Some(page) => div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(self.render_header(window, cx))
                .children(self.render_status(cx))
                .child(div().flex_1().min_h_0().child(page))
                .into_any_element(),
            None => {
                let on_right = self.config.sidebar.position.eq_ignore_ascii_case("right");
                let width = f32::from(self.config.sidebar.width).clamp(180., 480.);
                let side = self.sidebar_visible.then(|| {
                    div()
                        .relative()
                        .flex_shrink_0()
                        .w(crate::theme::px(width))
                        .h_full()
                        .child(sidebar)
                        .child(self.render_resize_handle(on_right, cx))
                });
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .when(on_right, |d| d.flex_row_reverse())
                    .children(side)
                    .child(main_column)
                    .into_any_element()
            }
        };
        let resizing = self.resizing.is_some();
        div()
            .flex()
            .size_full()
            .font_family(crate::theme::font())
            .bg(theme::background())
            .text_color(theme::text())
            .text_size(crate::theme::body_size())
            // After any element has seen the release: it bubbles from them up to here.
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, _, _, _| this.right_pressed = None),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(|this, _, _, _| this.right_pressed = None),
            )
            .when(resizing, |d| {
                d.cursor_col_resize()
                    .on_mouse_move(
                        cx.listener(|this, event: &gpui_kit::MouseMoveEvent, _, cx| {
                            this.resize_to(event.position.x.as_f32(), cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.end_resize(cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.end_resize(cx)),
                    )
            })
            .child(rail)
            .child(body)
            .children(self.render_modals(cx))
            .children(self.render_menu())
    }
}
