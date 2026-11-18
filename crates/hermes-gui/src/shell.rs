//! The root view: the icon rail, the sidebar next to it and the main panel under its header. It
//! owns the state; each feature's module adds its methods.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::AnimationExt as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, AnyWindowHandle, Context, MouseButton, Pixels, Point, Size, Subscription, Window,
    div,
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
use crate::theme::px;
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

/// How long a copy button says "Copied".
const COPIED_FOR: Duration = Duration::from_millis(1200);

/// How many recently open channels each server remembers.
const RECENT_CHANNELS: usize = 10;

/// How long an info line stays: `general.status_seconds`.
static INFO_SECONDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(5);

type MemberGroupsCache = std::cell::RefCell<
    HashMap<(InstanceId, Id), std::rc::Rc<Vec<hermes_client_core::view::MemberGroup>>>,
>;

pub struct Shell {
    pub core: Core,
    pub runtime: Handle,
    pub window: AnyWindowHandle,
    /// What the views show. Equal to `core.config()` except while a slider is being dragged.
    pub config: ClientConfig,
    pub page: Page,
    pub selected: Selected,
    /// The channels last open in each space, the latest last: the latest is reopened when the
    /// space is selected again, and the one before when it is deleted.
    pub recent_channels: HashMap<(InstanceId, Id), Vec<Id>>,
    pub collapsed: Collapsed,
    /// How many times each category was opened by a click: its rows fade in under a new key.
    pub category_opened: HashMap<(InstanceId, Id), u32>,
    pub chats: HashMap<ChannelKey, ChatView>,
    pub images: Images,
    pub status: Option<Status>,
    status_serial: u64,
    /// Where the right button went down, until it comes up.
    pub right_pressed: Option<Target>,
    /// The page drawn in the last frame, and the one fading out after a change (with a serial
    /// for its animation).
    shown_page: Option<Page>,
    leaving: Option<(Page, u64)>,
    page_serial: u64,
    /// The message whose file the mouse is over: its highlight stays off.
    pub hovered_file: Option<hermes_client_core::proto::id::Id>,
    /// What the pointer is over, of the things that change on a spring under it (`hover.rs`).
    pub hovered: HashSet<gpui_kit::SharedString>,
    /// The copy button used a moment ago: it says "Copied".
    pub copied: Option<String>,
    /// What is playing its way out, with the serial of its close (`modal.rs`).
    pub closing_serials: HashMap<&'static str, u64>,
    pub closing_serial: u64,
    /// Messages and reaction chips drawn before (`channel.rs`).
    pub reactions_seen: std::cell::RefCell<crate::channel::ReactionsSeen>,
    /// Instances whose connection banner showed: `None` while it does, then when the
    /// connection came back (the banner says so for a moment).
    pub reconnect_banner: std::cell::RefCell<HashMap<InstanceId, Option<Instant>>>,
    /// Each sidebar row's place in the list as last drawn, and whether it is a category.
    pub sidebar_order: std::rc::Rc<std::cell::RefCell<HashMap<Id, (usize, bool)>>>,
    /// The voice panel's height as last painted, in window pixels.
    pub voice_panel_height: std::rc::Rc<std::cell::Cell<f32>>,
    /// Chat text selected with the mouse.
    pub selection: crate::selection::TextSelection,
    pub menu: Option<OpenMenu>,
    /// Where the last menu opened: what its entries open goes there too.
    pub menu_at: Point<Pixels>,
    /// The same, with what was clicked: popups its entries open hug it.
    pub menu_from: crate::modal::PopupFrom,
    /// What popups can hug, as last painted: every element with a menu, and the buttons that
    /// open popups. Filled while painting, emptied at the start of each frame.
    pub popup_triggers: std::rc::Rc<std::cell::RefCell<Vec<gpui_kit::Bounds<Pixels>>>>,
    /// The profile card that is open.
    pub profile: Option<crate::user_menu::ProfileCard>,
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
    pub emoji_picker: Option<crate::emoji_picker::EmojiPicker>,
    pub keybinds: Keybinds,
    /// What the Friends page lists.
    pub friends_tab: crate::friends::FriendsTab,
    /// The member list shows next to a chat.
    pub show_members: bool,
    pub sidebar_visible: bool,
    /// The sidebar's edge is being dragged: where the drag started, and the width then.
    pub resizing: Option<(f32, f32)>,
    /// What is being resized is the member list, not the sidebar.
    pub resizing_members: bool,
    /// The status went idle by itself, so it comes back by itself.
    pub auto_idle: bool,
    /// What the system asked for, under the setting.
    pub system_reduce_motion: bool,
    /// A newer release, if the update check found one.
    pub update: Option<hermes_client_core::update::Update>,
    /// The updater's window, while it is open.
    pub updater: Option<crate::updater_window::Updater>,
    /// "Not now" on the newer-server banner, for this run.
    pub newer_server_dismissed: bool,
    pub new_dm: Option<NewDm>,
    /// The full-size image viewer.
    pub viewer: Option<Viewer>,
    /// The safety number of a conversation.
    pub keys_dialog: Option<crate::encryption::KeysDialog>,
    /// The popup over an unencrypted chat that was just opened.
    pub unencrypted_popup: Option<(InstanceId, Id)>,
    /// How far the rail's servers are scrolled.
    pub rail_scroll: gpui_kit::ScrollHandle,
    /// The rail slot last selected: the pill springs on from there.
    pub rail_pill_slot: std::cell::Cell<f32>,
    /// Per highlighted sidebar row: the state it changed from and the one it shows, so a change
    /// between two highlighted states crossfades instead of growing the bar again.
    /// How tall each row growing into the chat turned out, by its animation key (see
    /// `channel::grow_in`).
    pub grow_heights: crate::channel::GrowHeights,
    /// Colours of code blocks already worked out.
    pub syntax_cache: std::cell::RefCell<crate::syntax::Cache>,
    pub row_tints: std::cell::RefCell<
        std::collections::HashMap<String, (crate::sidebar::RowState, crate::sidebar::RowState)>,
    >,
    /// Message text parsed already, by what it depends on. Parsing and resolving mentions per
    /// visible row per frame was the biggest cost of drawing a chat.
    pub markup_cache: std::cell::RefCell<HashMap<u64, std::rc::Rc<Vec<crate::markup::Block>>>>,
    /// The member list, grouped and sorted, per server.
    pub member_groups_cache: MemberGroupsCache,
    /// Plays mouse-wheel notches back smoothly.
    pub smooth_scroll: crate::smooth_scroll::SmoothScroll,
    /// Sound and video files that were clicked: loading, playing or paused.
    pub media: crate::media::cards::Media,
    /// A picked picture being cropped before it is uploaded.
    pub crop: Option<crate::crop::Crop>,
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
    /// Which measure the network popup graphs (its tiles' order).
    pub net_graph: usize,
    /// Where the pointer is along the network graphs, 0 to 1.
    pub net_hover: Option<f32>,
    /// The window's size, as last drawn.
    pub viewport: Size<Pixels>,
    /// The network details are open, where they were asked for.
    pub network_open: Option<crate::modal::PopupFrom>,
    /// The size of the voice room's tile area, as last drawn.
    pub room_area: Size<Pixels>,
    /// The share picker, while it is open.
    pub share_menu: Option<crate::share_picker::SharePicker>,
    /// Counts the picker's openings, so an old refresh loop stops.
    pub share_generation: u64,
    /// The screens that can be shared, primary first.
    pub screen_names: Vec<String>,
    /// My running screen share capture.
    pub capture: Option<Capture>,
    /// My share's sound, while it is sent.
    pub share_sound: Option<crate::screen::sound::ShareSound>,
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
    /// A share shown in a window of its own.
    pub popped_out: crate::screen::popout::PoppedOut,
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
    /// The colour last given to the title bar.
    title_bar: Option<gpui_kit::Rgba>,
    /// The unread count shown on the taskbar button.
    taskbar_count: Option<u32>,
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
            recent_channels: HashMap::new(),
            collapsed,
            category_opened: HashMap::new(),
            chats: HashMap::new(),
            images: Images::default(),
            status: None,
            status_serial: 0,
            right_pressed: None,
            hovered_file: None,
            hovered: HashSet::new(),
            copied: None,
            closing_serials: HashMap::new(),
            closing_serial: 0,
            reactions_seen: Default::default(),
            reconnect_banner: Default::default(),
            sidebar_order: Default::default(),
            voice_panel_height: Default::default(),
            shown_page: None,
            leaving: None,
            page_serial: 0,
            selection: Default::default(),
            menu: None,
            menu_at: Point::default(),
            menu_from: crate::modal::PopupFrom::point(Point::default()),
            popup_triggers: Default::default(),
            profile: None,
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
            emoji_picker: None,
            keybinds,
            friends_tab: crate::friends::FriendsTab::default(),
            show_members: true,
            sidebar_visible: true,
            resizing: None,
            resizing_members: false,
            auto_idle: false,
            system_reduce_motion: false,
            update: None,
            newer_server_dismissed: false,
            updater: None,
            new_dm: None,
            viewer: None,
            keys_dialog: None,
            unencrypted_popup: None,
            rail_scroll: gpui_kit::ScrollHandle::new(),
            rail_pill_slot: std::cell::Cell::new(0.),
            row_tints: Default::default(),
            grow_heights: Default::default(),
            syntax_cache: Default::default(),
            markup_cache: Default::default(),
            member_groups_cache: Default::default(),
            smooth_scroll: crate::smooth_scroll::SmoothScroll::default(),
            crop: None,
            media: Default::default(),
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
            net_graph: 0,
            net_hover: None,
            viewport: window.viewport_size(),
            network_open: None,
            room_area: crate::voice::no_area(),
            share_menu: None,
            share_generation: 0,
            screen_names: Vec::new(),
            capture: None,
            share_sound: None,
            screens: HashMap::new(),
            enlarged_share: None,
            watching_shares: HashSet::new(),
            watched_now: Vec::new(),
            screen_latest: None,
            popped_out: None,
            tray,
            global_keys,
            window_visible: !start_hidden,
            scale: window.scale_factor(),
            restore: None,
            title: String::new(),
            title_bar: None,
            taskbar_count: None,
            focus_composer: false,
            focused: true,
            _subscriptions: subscriptions,
        };
        this.restore_layout();
        crate::open::clear_pasted();
        this.reload_theme(cx);
        this.system_reduce_motion = cx.reduce_motion();
        this.show_members = this.config.sidebar.show_members;
        this.sidebar_visible = this.config.sidebar.visible;
        this.apply_motion(cx);
        window.set_rem_size(gpui_kit::px(16. * crate::theme::scale()));
        this.start_theme_watcher(cx);
        this.start_idle_watch(cx);
        this.check_for_update(cx);
        if let Some(warning) = key_warnings.into_iter().next() {
            this.warn(warning, cx);
        }
        this.start_event_pump(events, cx);
        this.start_link_listener(endpoint, cx);
        this.start_heartbeat(cx);
        this.start_meter_timer(cx);
        this.start_call_clock(cx);
        this.start_media_sweep(cx);
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
                        // The microphone level comes 50 times a second: the meter's own timer
                        // decides when it is worth a redraw.
                        let redraw = batch.iter().any(|e| !matches!(e, CoreEvent::InputLevel(_)));
                        for event in batch {
                            this.on_core(event, window, cx);
                        }
                        if redraw {
                            cx.notify();
                        }
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
                        // Also out of the tray: a second launch means "show me".
                        this.show_window(window, cx);
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

    /// Redraws once a second during a call, for the time on the voice channel's row, and while an
    /// instance has just lost its connection, so the connection icon and the chat's banner change
    /// once the outage has lasted long enough to show.
    fn start_call_clock(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = this.update(cx, |this, cx| {
                    let just_lost = this.core.instance_ids().iter().any(|inst| {
                        this.core.down_for(inst).is_some_and(|d| {
                            d <= crate::sidebar::BANNER_AFTER + Duration::from_secs(1)
                        })
                    });
                    if this.core.voice_channel().is_some() || just_lost {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    return;
                }
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
            CoreEvent::Info(text) => self.info(text, cx),
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
            CoreEvent::InputLevel(db) => {
                self.input_level = db;
                return;
            }
            CoreEvent::StoreChanged { inst, what } => {
                let people_changed = matches!(
                    what,
                    Changed::All
                        | Changed::Me
                        | Changed::Spaces
                        | Changed::Space(_)
                        | Changed::Members(_)
                        | Changed::Users
                );
                if people_changed || matches!(what, Changed::Channel(_) | Changed::Dms) {
                    self.markup_cache.borrow_mut().clear();
                }
                if people_changed || matches!(what, Changed::Presence(_)) {
                    self.member_groups_cache.borrow_mut().clear();
                }
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
                        self.check_page_exists(window, cx);
                    }
                    _ => {}
                }
                if people_changed {
                    self.load_avatars(cx);
                }
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
        self.check_page_exists(window, cx);
        if let Page::Chat(i, channel) = &self.page
            && i == inst
        {
            let (i, channel) = (i.clone(), *channel);
            self.open_chat(i, channel, window, cx);
        }
    }

    /// Leaves a channel or space that is gone (deleted, or I left it).
    fn check_page_exists(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        // A deleted channel: back to the one open before it in its server (a voice channel's
        // room, without joining it).
        if page_gone
            && !selected_gone
            && let Selected::Space(inst, space) = self.selected.clone()
            && let Some(channel) = self.fallback_channel(&inst, space)
        {
            self.select(inst, channel, window, cx);
            return;
        }
        if page_gone || selected_gone {
            self.open_friends(cx);
        }
    }

    /// What a server opens: its latest channel that still exists, else its first text channel.
    fn fallback_channel(&self, inst: &InstanceId, space: Id) -> Option<Id> {
        let store = self.core.store();
        let s = store.instance(inst)?;
        self.recent_channels
            .get(&(inst.clone(), space))
            .and_then(|recent| {
                recent
                    .iter()
                    .rev()
                    .copied()
                    .find(|c| s.channel(*c).is_some())
            })
            .or_else(|| {
                hermes_client_core::view::channel_tree(s, space)
                    .into_iter()
                    .find(|c| c.kind == ChannelKind::Text)
                    .map(|c| c.id)
            })
    }

    // ---- status line --------------------------------------------------------------------------

    pub fn show_status(&mut self, text: String, level: Level, cx: &mut Context<Self>) {
        self.status_serial += 1;
        let serial = self.status_serial;
        self.cancel_closing("status");
        self.status = Some(Status {
            text,
            level,
            serial,
        });
        if level == Level::Info {
            cx.spawn(async move |this, cx| {
                let seconds = INFO_SECONDS.load(std::sync::atomic::Ordering::Relaxed);
                cx.background_executor()
                    .timer(Duration::from_secs(seconds))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.status.as_ref().is_some_and(|s| s.serial == serial) {
                        this.close_later("status", |s| s.status = None, cx);
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
        let old_buffer = self.config.buffer.clone();
        self.config = self.core.config();
        if self.config.buffer != old_buffer {
            self.refresh_chats();
        }
        let (keybinds, warnings) = Keybinds::from_config(&self.config.keys);
        self.keybinds = keybinds;
        if let Some(warning) = warnings.into_iter().next() {
            self.warn(warning, cx);
        }
        self.update_ptt_keys();
        self.register_global_keys(cx);
        self.apply_motion(cx);
        cx.notify();
    }

    fn apply_motion(&self, cx: &mut Context<Self>) {
        let general = &self.config.general;
        theme::set_motion_speed(&general.motion_speed);
        cx.set_reduce_motion(general.reduce_motion || self.system_reduce_motion);
        INFO_SECONDS.store(
            u64::from(general.status_seconds.max(1)),
            std::sync::atomic::Ordering::Relaxed,
        );
        theme::HEADING_EXTRA.store(
            self.config.font.heading_extra.clamp(0, 16),
            std::sync::atomic::Ordering::Relaxed,
        );
        crate::channel::MEDIA_SCALE.store(
            self.config.buffer.media_scale.clamp(25, 300),
            std::sync::atomic::Ordering::Relaxed,
        );
        self.smooth_scroll
            .configure(general.smooth_scroll, general.scroll_speed as f32);
    }

    /// Whether animated pictures move right now.
    pub fn animating(&self) -> bool {
        self.focused && self.config.general.animate_pictures
    }

    pub fn toggle_members(&mut self, cx: &mut Context<Self>) {
        let shown = !self.show_members;
        self.show_members = shown;
        self.persist("sidebar.show_members", &shown.to_string(), cx);
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        let shown = !self.sidebar_visible;
        self.sidebar_visible = shown;
        self.persist("sidebar.visible", &shown.to_string(), cx);
    }

    /// Puts text on the clipboard; `what` names it for the status line.
    pub fn copy(&mut self, text: String, what: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        self.info(format!("{what} copied"), cx);
    }

    /// The same from a copy button named `button`: it says "Copied" for a moment.
    pub fn copy_from(
        &mut self,
        button: impl Into<String>,
        text: String,
        what: &str,
        cx: &mut Context<Self>,
    ) {
        self.copy(text, what, cx);
        let button = button.into();
        self.copied = Some(button.clone());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            let _ = this.update(cx, |this, cx| {
                if this.copied.as_deref() == Some(button.as_str()) {
                    this.copied = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// A copy button's label: `label`, or "Copied" just after it was used.
    pub fn copy_label(&self, button: &str, label: &'static str) -> &'static str {
        if self.copied.as_deref() == Some(button) {
            "Copied"
        } else {
            label
        }
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
        if let Some(channel) = self.fallback_channel(&inst, space) {
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
                let recent = self
                    .recent_channels
                    .entry((inst.clone(), space))
                    .or_default();
                recent.retain(|c| *c != channel);
                recent.push(channel);
                if recent.len() > RECENT_CHANNELS {
                    recent.remove(0);
                }
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

    /// The window's title: the page, then its space, like `general | Maidenless - Hermes`.
    pub fn page_title(&self) -> String {
        let store = self.core.store();
        let place = if let Some((inst, space)) = &self.open_space_page {
            let name = store
                .instance(inst)
                .and_then(|s| s.spaces.get(space))
                .map_or("", |s| s.space.name.as_str());
            title_parts("Server settings", Some(name))
        } else if self.settings.is_some() {
            "Settings".to_owned()
        } else {
            match &self.page {
                Page::Friends => "Friends".to_owned(),
                Page::Results(Key::Search(..)) => "Search".to_owned(),
                Page::Results(Key::Mentions) => "Mentions".to_owned(),
                Page::Results(Key::Pins(..)) => "Pinned messages".to_owned(),
                Page::Chat(inst, channel) | Page::Voice(inst, channel) => store
                    .instance(inst)
                    .and_then(|s| {
                        let c = s.channel(*channel)?;
                        let space = s
                            .space_of(*channel)
                            .and_then(|id| s.spaces.get(&id))
                            .map(|st| st.space.name.as_str());
                        Some(match space {
                            Some(space) => {
                                title_parts(c.name.as_deref().unwrap_or(""), Some(space))
                            }
                            None => hermes_client_core::view::channel_title(s, c),
                        })
                    })
                    .unwrap_or_default(),
            }
        };
        if place.is_empty() {
            "Hermes".to_owned()
        } else {
            format!("{place} - Hermes")
        }
    }

    // ---- drawing ------------------------------------------------------------------------------

    /// The bar over the main panel, holding the page's buttons and fields.
    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(px(theme::SIDEBAR_HEADER))
            .border_b_1()
            .border_color(theme::border())
            .child(div().flex_1())
            .children(self.render_header_controls(cx))
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
            .rounded(theme::radius_small())
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
            if self.core.channel_is_private(&inst, channel) {
                let keys_inst = inst.clone();
                controls.push(
                    crate::channel::tip_button(
                        "header-keys",
                        assets::LOCK,
                        "End-to-end encrypted: compare safety numbers",
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_keys_dialog(keys_inst.clone(), channel, cx);
                    }))
                    .into_any_element(),
                );
            }
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
                        this.toggle_members(cx);
                    }))
                    // Pressed in while the list shows; the press flows in and out.
                    .with_spring(
                        "header-members-on",
                        gpui_kit::SpringAnimation::new(crate::theme::paced(
                            crate::hover::HOVER_SPRING,
                        ))
                        .to(self.show_members),
                        |d, phase| d.bg(theme::alpha(theme::selected(), phase.0.clamp(0., 1.))),
                    )
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
        use gpui_kit::AnimationExt as _;
        let status = self.status.as_ref()?;
        let (color, glyph) = match status.level {
            Level::Info => (theme::text_secondary(), assets::INFO),
            Level::Warning => (theme::accent(), assets::INFO),
            Level::Error => (theme::danger(), assets::BAN),
        };
        let serial = status.serial;
        let leaving = crate::modal::closing("status");
        // An info line goes by itself: a thin bar runs out until it does.
        let countdown = (status.level == Level::Info).then(|| {
            let seconds = INFO_SECONDS
                .load(std::sync::atomic::Ordering::Relaxed)
                .max(1);
            div()
                .absolute()
                .left_0()
                .bottom_0()
                .h(px(2.))
                .bg(theme::alpha(theme::accent(), 0.5))
                .with_animation(
                    gpui_kit::SharedString::from(format!("status-countdown-{serial}")),
                    gpui_kit::Animation::new(Duration::from_secs(seconds)),
                    |d, t| d.w(gpui_kit::relative(1. - t)),
                )
        });
        Some(
            div()
                .id("status-line")
                .relative()
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
                .child(icon(glyph, 16., color))
                .child(div().flex_1().min_w_0().child(status.text.clone()))
                .child(
                    crate::channel::tip_button("status-dismiss", assets::CLOSE, "Dismiss")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_later("status", |s| s.status = None, cx);
                        })),
                )
                .children(countdown)
                // Each new line fades in, and out when it goes.
                .with_animation(
                    gpui_kit::SharedString::from(format!(
                        "status-{}-{serial}",
                        if leaving { "out" } else { "in" }
                    )),
                    gpui_kit::Animation::new(if leaving {
                        crate::modal::pop()
                    } else {
                        theme::fade() * 2
                    })
                    .with_easing(gpui_kit::ease_out_quint()),
                    move |d, t| d.opacity(if leaving { 1. - t } else { t }),
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
        if let Some((inst, channel, _)) = restored {
            self.restore = Some((inst, channel));
        }
    }

    /// Writes what is open to `[[layout]]` in `config.toml`.
    pub fn save_layout(&mut self, cx: &Context<Self>) {
        if let Some(open) = self.open_channel() {
            self.remember_draft(&open, cx);
        }
        self.core.save_outbox();
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
    /// Asks now, and says what came of it.
    pub fn check_for_update_now(&mut self, cx: &mut Context<Self>) {
        self.run(
            cx,
            hermes_client_core::update::check(env!("CARGO_PKG_VERSION")),
            |this, result, _, cx| match result {
                Ok(Some(update)) => {
                    this.update = Some(update);
                    cx.notify();
                }
                Ok(None) => this.info("Hermes is up to date", cx),
                Err(err) => this.error(format!("could not check for updates: {err}"), cx),
            },
        );
    }

    fn check_for_update(&mut self, cx: &mut Context<Self>) {
        if !self.config.general.check_for_updates {
            return;
        }
        self.run(
            cx,
            hermes_client_core::update::check(env!("CARGO_PKG_VERSION")),
            |this, result, _, cx| match result {
                Ok(update) => {
                    // Installed right away when that is wanted and this build can.
                    if let Some(found) = &update
                        && this.config.general.auto_update
                        && hermes_client_core::updater::available()
                    {
                        this.start_update(found.version.clone(), found.url.clone(), cx);
                    }
                    this.update = update;
                }
                Err(err) => tracing::info!("update check failed: {err}"),
            },
        );
    }

    fn render_update_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let update = self.update.as_ref()?;
        let url = update.url.clone();
        let version = update.version.clone();
        // This build updates itself: the banner starts it, else it opens the release page.
        let self_update = hermes_client_core::updater::available();
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
                .child(
                    crate::channel::tip_button("update-dismiss", assets::CLOSE, "Not now")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.update = None;
                            cx.notify();
                        })),
                )
                .cursor_pointer()
                .text_size(crate::theme::secondary_size())
                .child(div().flex_1().child(if self_update {
                    format!("Hermes {} is available: click to update", update.version)
                } else {
                    format!("Hermes {} is available: click to open it", update.version)
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if self_update {
                        this.start_update(version.clone(), url.clone(), cx);
                        return;
                    }
                    if let Err(err) = crate::open::url(&url) {
                        this.error(format!("could not open the browser: {err}"), cx);
                    }
                }))
                .into_any_element(),
        )
    }

    /// A server can do things this build doesn't know: some of it doesn't show until Hermes is
    /// updated.
    fn render_newer_server_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.newer_server_dismissed {
            return None;
        }
        let newer = {
            let store = self.core.store();
            store.instances.values().any(|s| s.server_is_newer())
        };
        if !newer {
            return None;
        }
        Some(
            div()
                .id("newer-server-banner")
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(8.))
                .px(px(16.))
                .py(px(6.))
                .bg(theme::accent_soft())
                .child(
                    crate::channel::tip_button("newer-server-dismiss", assets::CLOSE, "Not now")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.newer_server_dismissed = true;
                            cx.notify();
                        })),
                )
                .cursor_pointer()
                .text_size(crate::theme::secondary_size())
                .child(div().flex_1().child(
                    "A server you use is newer than this Hermes: some things won't show until                      you update. Click to check for an update",
                ))
                .on_click(cx.listener(|this, _, _, cx| this.check_for_update_now(cx)))
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
        if self.resizing_members {
            self.config.sidebar.member_list_width = (width - moved).clamp(180., 420.) as u16;
            cx.notify();
            return;
        }
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
            if std::mem::take(&mut self.resizing_members) {
                let width = self.config.sidebar.member_list_width.to_string();
                self.persist("sidebar.member_list_width", &width, cx);
            } else {
                let width = self.config.sidebar.width.to_string();
                self.persist("sidebar.width", &width, cx);
            }
        }
    }

    /// The font or scale changed: every message's height with it.
    pub fn remeasure_chats(&self) {
        for view in self.chats.values() {
            view.list.remeasure();
        }
    }
}

/// `page | space`, or the page alone.
fn title_parts(page: &str, space: Option<&str>) -> String {
    match space {
        Some(space) if !space.is_empty() => format!("{page} | {space}"),
        _ => page.to_owned(),
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

impl Shell {
    /// The main panel for `page`.
    fn render_page(&mut self, page: Page, cx: &mut Context<Self>) -> AnyElement {
        match page {
            Page::Friends if self.core.instance_ids().is_empty() => {
                self.render_welcome(cx).into_any_element()
            }
            Page::Friends => self.render_friends(cx).into_any_element(),
            Page::Chat(inst, channel) => self.render_chat(inst, channel, cx).into_any_element(),
            Page::Voice(inst, channel) => self.render_voice(inst, channel, cx).into_any_element(),
            Page::Results(key) => self.render_results(key, cx).into_any_element(),
        }
    }

    /// When the page changed since the last frame, the old one fades out under the new one
    /// fading in, for `theme::fade()`. A voice page is never drawn twice (it holds share video), and
    /// result pages share one search field, so those only fade in.
    fn note_page_change(&mut self, cx: &mut Context<Self>) {
        if self.shown_page.as_ref() == Some(&self.page) {
            return;
        }
        let Some(previous) = self.shown_page.replace(self.page.clone()) else {
            return;
        };
        let voice = |p: &Page| matches!(p, Page::Voice(..));
        let both_results = matches!(
            (&previous, &self.page),
            (Page::Results(_), Page::Results(_))
        );
        if voice(&previous) || voice(&self.page) || both_results {
            self.leaving = None;
            return;
        }
        self.page_serial += 1;
        let serial = self.page_serial;
        self.leaving = Some((previous, serial));
        // Gone once faded, so nothing invisible is left under the mouse.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(theme::fade() + std::time::Duration::from_millis(20))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.leaving.as_ref().is_some_and(|(_, s)| *s == serial) {
                    this.leaving = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        watchdog::beat();
        let _phase = watchdog::in_view();
        for image in self.images.retired.drain(..) {
            let _ = window.drop_image(image);
        }
        self.scale = window.scale_factor();
        self.viewport = window.viewport_size();
        // This frame paints them again.
        self.popup_triggers.borrow_mut().clear();
        let title = self.page_title();
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }
        let bar = theme::sidebar();
        if self.title_bar != Some(bar) {
            crate::desktop::window::set_title_bar(window, bar);
            self.title_bar = Some(bar);
        }
        let count = crate::desktop::badge_count(&self.core.store(), &self.core.instance_ids());
        if self.taskbar_count != Some(count) {
            crate::desktop::taskbar::set_count(window, count);
            self.taskbar_count = Some(count);
        }
        self.advance_media(window);
        self.advance_selection(window);
        if std::mem::take(&mut self.focus_composer) {
            self.focus_open_composer(window, cx);
        }

        let rail = self.render_rail(cx).into_any_element();
        let sidebar = self.render_sidebar(cx);
        self.note_page_change(cx);
        // The page that is leaving goes first, so the new one is drawn over it.
        let leaving = self
            .leaving
            .as_ref()
            .map(|(page, serial)| (page.clone(), *serial));
        let leaving = leaving.map(|(page, serial)| {
            use gpui_kit::AnimationExt as _;
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(self.render_page(page, cx))
                .with_animation(
                    gpui_kit::SharedString::from(format!("page-leave-{serial}")),
                    // It holds on while the new page comes in over it, and goes at the end,
                    // so the two never both show faint.
                    gpui_kit::Animation::new(theme::fade()),
                    |d, t| d.opacity(1. - t * t * t),
                )
        });
        let main = self.render_page(self.page.clone(), cx);
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
        let header = self.render_header(cx);
        let main_column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(header)
            .children(self.render_call_banner(cx))
            .children(
                self.render_update_banner(cx)
                    .or_else(|| self.render_newer_server_banner(cx)),
            )
            .children(self.render_status(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .children(leaving)
                            .child({
                                let id =
                                    gpui_kit::SharedString::from(format!("page-{:?}", self.page));
                                if matches!(self.page, Page::Chat(..)) {
                                    theme::enter_rising(id, main)
                                } else {
                                    theme::enter(id, main)
                                }
                            }),
                    )
                    .children(members),
            );
        let full_page = self
            .render_space_settings(cx)
            .or_else(|| self.render_settings(cx));
        let showing_page = full_page.is_some();
        let body = match full_page {
            // Settings replace the sidebar and the chat.
            Some(page) => div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .h_full()
                .children(self.render_status(cx))
                .child(div().flex_1().min_h_0().child(theme::enter(
                    if self.open_space_page.is_some() {
                        "space-settings"
                    } else {
                        "settings"
                    },
                    page,
                )))
                .into_any_element(),
            None => main_column.into_any_element(),
        };
        let on_right = self.config.sidebar.position.eq_ignore_ascii_case("right");
        let width = f32::from(self.config.sidebar.width).clamp(180., 480.);
        // The rail and the sidebar side by side, with the voice and user panels across both.
        let left = if showing_page || !self.sidebar_visible {
            rail
        } else {
            div()
                .relative()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .w(crate::theme::px(theme::RAIL_WIDTH + width))
                .h_full()
                .bg(theme::sidebar())
                .when(on_right, |d| d.border_l_1())
                .when(!on_right, |d| d.border_r_1())
                .border_color(theme::border())
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .when(on_right, |d| d.flex_row_reverse())
                        .child(rail)
                        .child(div().flex_1().min_w_0().h_full().child(sidebar)),
                )
                .children(self.render_bottom_panels(cx))
                .child(self.render_resize_handle(on_right, cx))
                .into_any_element()
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
            .child(self.smooth_scroll.catcher())
            .children(self.media_scrub_catcher(cx))
            .children(self.selection_catcher(cx))
            .when(self.selection.dragging(), |d| d.cursor_text())
            .when(on_right, |d| d.flex_row_reverse())
            .child(left)
            .child(body)
            .children(self.render_modals(cx))
            .children(self.render_menu())
    }
}
