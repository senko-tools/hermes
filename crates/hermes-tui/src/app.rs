//! TUI state: mode, focus, selection, input, popups, and what every key does (PLAN.md section
//! 16.2). Network actions run as tasks and report back through [`TaskMsg`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use hermes_client_core::address::{AddTarget, HostPort, InviteLink};
use hermes_client_core::commands::{self as slash, SlashCommand};
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::api::{CreateInviteRequest, UpdateMemberRequest};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::proto::permissions::Permissions;
use hermes_client_core::store::{HistoryAnchor, LocalId, PendingState};
use hermes_client_core::view::{self, Suggestion};
use hermes_client_core::{CertStatus, Core, CoreError, CoreEvent, Draft, InstanceId, Probe};
use tokio::sync::mpsc;

use crate::commands::{self, Command};
use crate::input::Input;
use crate::keymap::{Action, Key, Keymap, Lookup};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Messages,
    Members,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarItem {
    Instance {
        inst: InstanceId,
        status: ConnStatus,
    },
    Space {
        inst: InstanceId,
        name: String,
    },
    Channel {
        inst: InstanceId,
        channel: Id,
        kind: ChannelKind,
        name: String,
        depth: u8,
        unread: bool,
        mentions: u32,
    },
    DmHeader,
}

impl SidebarItem {
    fn channel(&self) -> Option<(&InstanceId, Id)> {
        match self {
            SidebarItem::Channel { inst, channel, .. } => Some((inst, *channel)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub label: &'static str,
    pub input: Input,
    pub secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub host: HostPort,
    /// Invite code from an invite link: joined after signing in.
    pub invite: Option<String>,
    /// Create an account (`true`) or sign in.
    pub register: bool,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub busy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    DeleteMessage {
        inst: InstanceId,
        channel: Id,
        message: Id,
    },
    Discard {
        inst: InstanceId,
        channel: Id,
        local_id: LocalId,
    },
    Trust {
        probe: Box<Probe>,
        invite: Option<String>,
    },
    RemoveInstance(InstanceId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Popup {
    Complete {
        items: Vec<Suggestion>,
        selected: usize,
    },
    Emoji {
        query: Input,
        results: Vec<(String, String)>,
        selected: usize,
        target: (InstanceId, Id, Id),
    },
    Confirm {
        text: String,
        action: ConfirmAction,
    },
    Form(Form),
    Help,
}

/// Results of background tasks.
#[derive(Debug)]
pub enum TaskMsg {
    Info(String),
    Error(String),
    Probed {
        probe: Box<Probe>,
        invite: Option<String>,
    },
    SignedIn {
        inst: InstanceId,
    },
    FormFailed(String),
    Open {
        inst: InstanceId,
        channel: Id,
    },
}

pub struct App {
    pub core: Core,
    pub keymap: Keymap,
    pub mode: Mode,
    pub focus: Focus,
    pub popup: Option<Popup>,
    pub sidebar: Vec<SidebarItem>,
    pub sidebar_sel: usize,
    pub current: Option<(InstanceId, Id)>,
    /// Index into the current channel's entries; `None` follows the newest.
    pub msg_sel: Option<usize>,
    pub member_sel: usize,
    pub input: Input,
    pub command: Input,
    pub drafts: HashMap<(InstanceId, Id), String>,
    pub reply_to: Option<Id>,
    pub editing: Option<Id>,
    pub show_members: bool,
    pub status: Option<(String, bool, Instant)>,
    pub pending_key: Option<Key>,
    pub tasks: mpsc::UnboundedSender<TaskMsg>,
    pub utc_offset: i64,
    pub quit: bool,
    /// Messages per page for Ctrl+u/d (set by the UI from the pane height).
    pub page: usize,
}

impl App {
    pub fn new(core: Core, tasks: mpsc::UnboundedSender<TaskMsg>, utc_offset: i64) -> Self {
        let (keymap, warnings) = Keymap::new(&core.config().tui.keys);
        let mut app = App {
            core,
            keymap,
            mode: Mode::Normal,
            focus: Focus::Sidebar,
            popup: None,
            sidebar: Vec::new(),
            sidebar_sel: 0,
            current: None,
            msg_sel: None,
            member_sel: 0,
            input: Input::default(),
            command: Input::default(),
            drafts: HashMap::new(),
            reply_to: None,
            editing: None,
            show_members: true,
            status: None,
            pending_key: None,
            tasks,
            utc_offset,
            quit: false,
            page: 10,
        };
        if let Some(w) = warnings.first() {
            app.error(w.clone());
        }
        app.rebuild_sidebar();
        if app.sidebar.is_empty() {
            app.info(":instance add <invite link or address> to get started".into());
        }
        app
    }

    pub fn info(&mut self, text: String) {
        self.status = Some((text, false, Instant::now()));
    }

    pub fn error(&mut self, text: String) {
        self.status = Some((text, true, Instant::now()));
    }

    // ---- sidebar --------------------------------------------------------------------------

    pub fn rebuild_sidebar(&mut self) {
        let selected = self.sidebar.get(self.sidebar_sel).cloned();
        let mut items = Vec::new();
        let config = self.core.config();
        {
            let store = self.core.store();
            for entry in &config.instances {
                let inst = entry.id.clone();
                items.push(SidebarItem::Instance {
                    inst: inst.clone(),
                    status: self.core.status(&inst),
                });
                let Some(s) = store.instance(&inst) else {
                    continue;
                };
                let mut spaces: Vec<_> = s.spaces.values().collect();
                spaces.sort_by_key(|sp| (sp.space.name.to_lowercase(), sp.space.id));
                for space in spaces {
                    items.push(SidebarItem::Space {
                        inst: inst.clone(),
                        name: space.space.name.clone(),
                    });
                    for c in view::channel_tree(s, space.space.id) {
                        items.push(SidebarItem::Channel {
                            inst: inst.clone(),
                            channel: c.id,
                            kind: c.kind,
                            name: c.name,
                            depth: c.depth + 1,
                            unread: c.unread,
                            mentions: c.mentions,
                        });
                    }
                }
                let dms = view::dm_list(s);
                if !dms.is_empty() {
                    items.push(SidebarItem::DmHeader);
                    for c in dms {
                        items.push(SidebarItem::Channel {
                            inst: inst.clone(),
                            channel: c.id,
                            kind: c.kind,
                            name: c.name,
                            depth: 1,
                            unread: c.unread,
                            mentions: c.mentions,
                        });
                    }
                }
            }
        }
        self.sidebar = items;
        // Keep the selection on the same item (by identity, ignoring the changing flags).
        let same = |a: &SidebarItem, b: &SidebarItem| match (a, b) {
            (SidebarItem::Instance { inst: x, .. }, SidebarItem::Instance { inst: y, .. }) => {
                x == y
            }
            (SidebarItem::Channel { channel: x, .. }, SidebarItem::Channel { channel: y, .. }) => {
                x == y
            }
            _ => a == b,
        };
        if let Some(sel) = selected
            && let Some(i) = self.sidebar.iter().position(|item| same(item, &sel))
        {
            self.sidebar_sel = i;
        }
        self.sidebar_sel = self.sidebar_sel.min(self.sidebar.len().saturating_sub(1));
    }

    fn selected_instance(&self) -> Option<InstanceId> {
        if let Some((inst, _)) = &self.current {
            return Some(inst.clone());
        }
        self.sidebar[..=self.sidebar_sel.min(self.sidebar.len().saturating_sub(1))]
            .iter()
            .rev()
            .find_map(|item| match item {
                SidebarItem::Instance { inst, .. } | SidebarItem::Channel { inst, .. } => {
                    Some(inst.clone())
                }
                _ => None,
            })
            .or_else(|| self.core.instance_ids().into_iter().next())
    }

    /// Opens a channel: saves the old draft, restores this one, loads history.
    pub fn open(&mut self, inst: InstanceId, channel: Id) {
        if let Some(old) = self.current.take() {
            let draft = self.input.take();
            if draft.is_empty() {
                self.drafts.remove(&old);
            } else {
                self.drafts.insert(old, draft);
            }
        }
        self.input = Input::with_text(
            &self
                .drafts
                .remove(&(inst.clone(), channel))
                .unwrap_or_default(),
        );
        self.reply_to = None;
        self.editing = None;
        self.msg_sel = None;
        self.member_sel = 0;
        self.current = Some((inst.clone(), channel));
        if let Some(i) = self
            .sidebar
            .iter()
            .position(|item| item.channel() == Some((&inst, channel)))
        {
            self.sidebar_sel = i;
        }
        self.core.set_visible_channels([(inst.clone(), channel)]);
        let core = self.core.clone();
        let tasks = self.tasks.clone();
        tokio::spawn(async move {
            if let Err(err) = core.open_channel(&inst, channel).await {
                let _ = tasks.send(TaskMsg::Error(err.to_string()));
            }
        });
        self.mark_current_read();
    }

    /// Acks the newest message of the open channel (it is on screen).
    pub fn mark_current_read(&self) {
        let Some((inst, channel)) = &self.current else {
            return;
        };
        let newest = {
            let store = self.core.store();
            store
                .instance(inst)
                .and_then(|s| s.channel(*channel))
                .and_then(|c| c.last_message_id)
        };
        if let Some(id) = newest {
            self.core.mark_read(inst, *channel, id);
        }
    }

    // ---- messages -------------------------------------------------------------------------

    /// Number of entries (messages + pending) in the open channel.
    pub fn entry_count(&self) -> usize {
        let Some((inst, channel)) = &self.current else {
            return 0;
        };
        let store = self.core.store();
        store
            .instance(inst)
            .and_then(|s| s.window(*channel))
            .map_or(0, |w| w.messages.len() + w.pending.len())
    }

    /// The selected message (not pending): `(id, author, content)`.
    fn selected_message(&self) -> Option<(Id, Id, String)> {
        let (inst, channel) = self.current.as_ref()?;
        let store = self.core.store();
        let w = store.instance(inst)?.window(*channel)?;
        let i = self.msg_sel?;
        w.messages
            .get(i)
            .map(|m| (m.id, m.author.id, m.content.clone()))
    }

    fn selected_pending(&self) -> Option<(LocalId, PendingState)> {
        let (inst, channel) = self.current.as_ref()?;
        let store = self.core.store();
        let w = store.instance(inst)?.window(*channel)?;
        let i = self.msg_sel?.checked_sub(w.messages.len())?;
        w.pending.get(i).map(|p| (p.local_id, p.state.clone()))
    }

    fn me(&self, inst: &str) -> Option<Id> {
        self.core.store().instance(inst).and_then(|s| s.me_id())
    }

    fn move_selection(&mut self, delta: isize) {
        let count = self.entry_count();
        if count == 0 {
            return;
        }
        let current = self.msg_sel.unwrap_or(count) as isize;
        let next = current + delta;
        if next >= count as isize {
            self.msg_sel = None;
            return;
        }
        let next = next.max(0) as usize;
        self.msg_sel = Some(next);
        if next == 0 {
            self.load_older();
        }
    }

    fn load_older(&self) {
        let Some((inst, channel)) = self.current.clone() else {
            return;
        };
        let first = {
            let store = self.core.store();
            let Some(w) = store.instance(&inst).and_then(|s| s.window(channel)) else {
                return;
            };
            if w.reached_start {
                return;
            }
            w.messages.first().map(|m| m.id)
        };
        let Some(first) = first else {
            return;
        };
        let core = self.core.clone();
        let tasks = self.tasks.clone();
        tokio::spawn(async move {
            if let Err(err) = core
                .load_history(&inst, channel, HistoryAnchor::Before(first))
                .await
            {
                let _ = tasks.send(TaskMsg::Error(err.to_string()));
            }
        });
    }

    // ---- events ---------------------------------------------------------------------------

    pub fn on_core_event(&mut self, event: CoreEvent) {
        match event {
            CoreEvent::InstanceStatus { inst, status } => {
                match &status {
                    ConnStatus::LoggedOut => {
                        self.error(format!("{inst}: signed out (:login to sign in again)"));
                    }
                    ConnStatus::Failed { reason } => self.error(format!("{inst}: {reason}")),
                    _ => {}
                }
                self.rebuild_sidebar();
            }
            CoreEvent::StoreChanged { what, .. } => {
                use hermes_client_core::store::Changed;
                if matches!(
                    what,
                    Changed::All
                        | Changed::Spaces
                        | Changed::Space(_)
                        | Changed::Channel(_)
                        | Changed::ReadState(_)
                        | Changed::Dms
                ) {
                    self.rebuild_sidebar();
                }
                if let Changed::Messages(channel) = what
                    && self.current.as_ref().is_some_and(|(_, c)| *c == channel)
                    && self.msg_sel.is_none()
                {
                    self.mark_current_read();
                }
                if let Some((inst, channel)) = &self.current {
                    let gone = self
                        .core
                        .store()
                        .instance(inst)
                        .is_none_or(|s| s.channel(*channel).is_none());
                    if gone && !matches!(what, Changed::All) {
                        self.current = None;
                    }
                }
            }
            CoreEvent::Notification { notification, .. } => {
                // Terminal bell; the status line names the message.
                print!("\x07");
                self.info(format!(
                    "{} in {}: {}",
                    notification.author, notification.title, notification.preview
                ));
            }
            CoreEvent::IncomingCall { .. } => self.info("Incoming call (voice comes later)".into()),
            CoreEvent::ConfigReloaded { warnings } => {
                let (keymap, key_warnings) = Keymap::new(&self.core.config().tui.keys);
                self.keymap = keymap;
                match warnings.into_iter().chain(key_warnings).next() {
                    Some(w) => self.error(format!("config reloaded: {w}")),
                    None => self.info("config reloaded".into()),
                }
            }
            CoreEvent::Error(e) => self.error(e),
        }
    }

    pub fn on_task(&mut self, msg: TaskMsg) {
        match msg {
            TaskMsg::Info(text) => self.info(text),
            TaskMsg::Error(text) => self.error(text),
            TaskMsg::Probed { probe, invite } => match probe.cert {
                CertStatus::Trusted | CertStatus::Pinned => self.login_form(probe.host, invite),
                CertStatus::Unknown => {
                    self.popup = Some(Popup::Confirm {
                        text: format!(
                            "{} uses a self-signed certificate.\nFingerprint:\n{}\nTrust it? (y/n)",
                            probe.host,
                            probe.fingerprint.colon_hex()
                        ),
                        action: ConfirmAction::Trust { probe, invite },
                    });
                }
                CertStatus::Changed { pinned } => {
                    self.popup = Some(Popup::Confirm {
                        text: format!(
                            "WARNING: the certificate of {} CHANGED.\nPinned: {}\nNow:    {}\n\
                             Only accept if the server owner re-generated it. Trust the new one? (y/n)",
                            probe.host,
                            pinned.colon_hex(),
                            probe.fingerprint.colon_hex()
                        ),
                        action: ConfirmAction::Trust { probe, invite },
                    });
                }
            },
            TaskMsg::SignedIn { inst } => {
                if matches!(self.popup, Some(Popup::Form(_))) {
                    self.popup = None;
                }
                self.info(format!("signed in to {inst}"));
                self.rebuild_sidebar();
            }
            TaskMsg::FormFailed(err) => {
                if let Some(Popup::Form(form)) = &mut self.popup {
                    form.busy = false;
                }
                self.error(err);
            }
            TaskMsg::Open { inst, channel } => {
                self.rebuild_sidebar();
                self.open(inst, channel);
                self.focus = Focus::Messages;
            }
        }
    }

    pub fn on_mouse(&mut self, event: MouseEvent) {
        match event.kind {
            MouseEventKind::ScrollUp => self.move_selection(-3),
            MouseEventKind::ScrollDown => self.move_selection(3),
            _ => {}
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        match self.mode {
            Mode::Insert => self.input.insert_str(text),
            Mode::Command => self.command.insert_str(&text.replace('\n', " ")),
            Mode::Normal => {}
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.popup.is_some() {
            self.popup_key(key);
            return;
        }
        match self.mode {
            Mode::Normal => self.normal_key(key),
            Mode::Insert => self.insert_key(key),
            Mode::Command => self.command_key(key),
        }
    }

    fn normal_key(&mut self, event: KeyEvent) {
        let key = Key::from_event(&event);
        if key.code == KeyCode::Esc {
            self.pending_key = None;
            self.msg_sel = None;
            return;
        }
        let action = match self.keymap.lookup(self.pending_key.take(), key) {
            Lookup::Action(a) => a,
            Lookup::Prefix => {
                self.pending_key = Some(key);
                return;
            }
            Lookup::None => {
                // Arrow keys always move.
                match key.code {
                    KeyCode::Down => Action::MoveDown,
                    KeyCode::Up => Action::MoveUp,
                    KeyCode::Left => Action::FocusLeft,
                    KeyCode::Right => Action::FocusRight,
                    _ => return,
                }
            }
        };
        self.action(action);
    }

    fn action(&mut self, action: Action) {
        match action {
            Action::MoveDown | Action::MoveUp => {
                let delta = if action == Action::MoveDown { 1 } else { -1 };
                match self.focus {
                    Focus::Sidebar => {
                        let n = self.sidebar.len();
                        if n > 0 {
                            self.sidebar_sel = (self.sidebar_sel as isize + delta)
                                .clamp(0, n as isize - 1)
                                as usize;
                        }
                    }
                    Focus::Messages => self.move_selection(delta),
                    Focus::Members => {
                        self.member_sel = (self.member_sel as isize + delta).max(0) as usize;
                    }
                }
            }
            Action::FocusLeft => {
                self.focus = match self.focus {
                    Focus::Members => Focus::Messages,
                    _ => Focus::Sidebar,
                }
            }
            Action::FocusRight => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Messages,
                    _ if self.show_members => Focus::Members,
                    f => f,
                }
            }
            Action::FocusNext => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Messages,
                    Focus::Messages if self.show_members => Focus::Members,
                    _ => Focus::Sidebar,
                }
            }
            Action::Open => match self.focus {
                Focus::Sidebar => match self.sidebar.get(self.sidebar_sel).cloned() {
                    Some(SidebarItem::Channel {
                        inst,
                        channel,
                        kind,
                        ..
                    }) => {
                        if kind == ChannelKind::Category {
                            return;
                        }
                        self.open(inst, channel);
                        self.focus = Focus::Messages;
                    }
                    Some(SidebarItem::Instance {
                        inst,
                        status: ConnStatus::LoggedOut,
                    }) => self.login(Some(inst)),
                    _ => {}
                },
                Focus::Messages => {
                    if let Some((_, PendingState::Failed(_))) = self.selected_pending() {
                        self.action(Action::Retry);
                    }
                }
                Focus::Members => {}
            },
            Action::Insert => {
                if self.current.is_some() {
                    self.mode = Mode::Insert;
                } else {
                    self.error("open a channel first (Enter in the sidebar)".into());
                }
            }
            Action::Reply => {
                if let Some((id, _, _)) = self.selected_message() {
                    self.reply_to = Some(id);
                    self.editing = None;
                    self.mode = Mode::Insert;
                }
            }
            Action::Edit => self.start_edit(self.selected_message()),
            Action::Delete => self.ask_delete(),
            Action::React => {
                if let (Some((inst, channel)), Some((id, _, _))) =
                    (self.current.clone(), self.selected_message())
                {
                    self.popup = Some(Popup::Emoji {
                        query: Input::default(),
                        results: Vec::new(),
                        selected: 0,
                        target: (inst, channel, id),
                    });
                }
            }
            Action::Retry => {
                if let (Some((inst, channel)), Some((local_id, PendingState::Failed(_)))) =
                    (self.current.clone(), self.selected_pending())
                    && let Err(e) = self.core.retry_message(&inst, channel, local_id)
                {
                    self.error(e.to_string());
                }
            }
            Action::Top => match self.focus {
                Focus::Messages => {
                    if self.entry_count() > 0 {
                        self.msg_sel = Some(0);
                        self.load_older();
                    }
                }
                _ => self.sidebar_sel = 0,
            },
            Action::Bottom => match self.focus {
                Focus::Messages => self.msg_sel = None,
                _ => self.sidebar_sel = self.sidebar.len().saturating_sub(1),
            },
            Action::PageUp => self.move_selection(-(self.page.max(1) as isize)),
            Action::PageDown => self.move_selection(self.page.max(1) as isize),
            Action::ToggleMute | Action::ToggleDeafen | Action::JoinVoice | Action::LeaveVoice => {
                self.info("Voice comes in a later version.".into())
            }
            Action::ToggleMembers => {
                self.show_members = !self.show_members;
                if !self.show_members && self.focus == Focus::Members {
                    self.focus = Focus::Messages;
                }
            }
            Action::Search => self.info("Search comes in a later version.".into()),
            Action::Command => {
                self.command.clear();
                self.mode = Mode::Command;
            }
            Action::Quit => self.quit = true,
        }
    }

    fn start_edit(&mut self, selected: Option<(Id, Id, String)>) {
        let (Some((inst, _)), Some((id, author, content))) = (self.current.clone(), selected)
        else {
            return;
        };
        if Some(author) != self.me(&inst) {
            self.error("you can only edit your own messages".into());
            return;
        }
        self.editing = Some(id);
        self.reply_to = None;
        self.input = Input::with_text(&content);
        self.mode = Mode::Insert;
    }

    fn ask_delete(&mut self) {
        let Some((inst, channel)) = self.current.clone() else {
            return;
        };
        if let Some((local_id, _)) = self.selected_pending() {
            self.popup = Some(Popup::Confirm {
                text: "Discard this unsent message? (y/n)".into(),
                action: ConfirmAction::Discard {
                    inst,
                    channel,
                    local_id,
                },
            });
            return;
        }
        let Some((message, author, _)) = self.selected_message() else {
            return;
        };
        let can_manage = {
            let store = self.core.store();
            store.instance(&inst).is_some_and(|s| {
                s.space_of(channel).is_some() && view::can(s, channel, Permissions::MANAGE_MESSAGES)
            })
        };
        if Some(author) != self.me(&inst) && !can_manage {
            self.error("you can only delete your own messages".into());
            return;
        }
        self.popup = Some(Popup::Confirm {
            text: "Delete this message? (y/n)".into(),
            action: ConfirmAction::DeleteMessage {
                inst,
                channel,
                message,
            },
        });
    }

    fn insert_key(&mut self, event: KeyEvent) {
        let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
        let alt = event.modifiers.contains(KeyModifiers::ALT);
        let shift = event.modifiers.contains(KeyModifiers::SHIFT);
        match event.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                if self.editing.take().is_some() {
                    self.input.clear();
                }
            }
            KeyCode::Enter if alt || shift => self.input.insert('\n'),
            KeyCode::Enter => self.submit(),
            KeyCode::Tab => self.autocomplete(),
            KeyCode::Up if self.input.is_empty() => {
                // Edit my last message.
                let last = self.current.as_ref().and_then(|(inst, channel)| {
                    let me = self.me(inst)?;
                    let store = self.core.store();
                    let w = store.instance(inst)?.window(*channel)?;
                    w.messages
                        .iter()
                        .rev()
                        .find(|m| m.author.id == me && !m.deleted)
                        .map(|m| (m.id, m.author.id, m.content.clone()))
                });
                self.start_edit(last);
            }
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            KeyCode::Char('w') if ctrl => self.input.delete_word(),
            KeyCode::Char('u') if ctrl => self.input.clear(),
            KeyCode::Char('c') if ctrl => {
                self.input.clear();
                self.reply_to = None;
                self.editing = None;
            }
            KeyCode::Char(c) if !ctrl => {
                self.input.insert(c);
                if let Some((inst, channel)) = &self.current
                    && !self.input.text().starts_with('/')
                {
                    self.core.typing(inst, *channel);
                }
            }
            _ => {}
        }
    }

    fn autocomplete(&mut self) {
        let Some((inst, channel)) = &self.current else {
            return;
        };
        let token = self.input.current_token().to_owned();
        if token.chars().count() < 2 && !token.starts_with('/') {
            return;
        }
        let items = {
            let store = self.core.store();
            let Some(s) = store.instance(inst) else {
                return;
            };
            view::complete(s, *channel, &token, 8)
        };
        match items.len() {
            0 => {}
            1 => self.input.replace_token(&items[0].insert),
            _ => {
                self.popup = Some(Popup::Complete { items, selected: 0 });
            }
        }
    }

    fn submit(&mut self) {
        let Some((inst, channel)) = self.current.clone() else {
            return;
        };
        let text = self.input.text().to_owned();
        if let Some(message) = self.editing.take() {
            self.input.clear();
            self.mode = Mode::Normal;
            let core = self.core.clone();
            let tasks = self.tasks.clone();
            tokio::spawn(async move {
                if let Err(e) = core.edit_message(&inst, channel, message, &text).await {
                    let _ = tasks.send(TaskMsg::Error(e.to_string()));
                }
            });
            return;
        }
        if text.trim().is_empty() {
            return;
        }
        let command = match slash::parse(&text) {
            Ok(c) => c,
            Err(e) => {
                self.error(e.to_string());
                return;
            }
        };
        self.input.clear();
        if let Some(content) = command.message_text() {
            let draft = Draft {
                content,
                reply_to: self.reply_to.take(),
                attachments: Vec::new(),
            };
            if let Err(e) = self.core.send_message(&inst, channel, draft) {
                self.error(e.to_string());
            }
            self.msg_sel = None;
            return;
        }
        match command {
            SlashCommand::Send(_) | SlashCommand::Me(_) => unreachable!("handled above"),
            SlashCommand::Join(_)
            | SlashCommand::Leave
            | SlashCommand::Mute
            | SlashCommand::Deafen => {
                self.info("Voice comes in a later version.".into());
            }
            SlashCommand::Status(status) => {
                self.core.set_presence(status);
                self.info(format!("status: {status:?}").to_lowercase());
            }
            SlashCommand::Nick(nick) => self.set_nick(&inst, channel, nick),
            SlashCommand::Dm(user) => self.open_dm(&inst, &user),
            SlashCommand::Invite => self.create_invite(&inst, channel),
        }
    }

    fn set_nick(&mut self, inst: &str, channel: Id, nick: Option<String>) {
        let Some(space) = self
            .core
            .store()
            .instance(inst)
            .and_then(|s| s.space_of(channel))
        else {
            self.error("nicknames are per space; open a space channel".into());
            return;
        };
        let rest = match self.core.rest(inst) {
            Ok(r) => r,
            Err(e) => return self.error(e.to_string()),
        };
        let tasks = self.tasks.clone();
        tokio::spawn(async move {
            let req = UpdateMemberRequest {
                nickname: Some(nick),
                role_ids: None,
            };
            let msg = match rest.update_member(space, None, &req).await {
                Ok(_) => TaskMsg::Info("nickname changed".into()),
                Err(e) => TaskMsg::Error(e.to_string()),
            };
            let _ = tasks.send(msg);
        });
    }

    fn open_dm(&mut self, inst: &str, name: &str) {
        let wanted = name.to_lowercase();
        let user = {
            let store = self.core.store();
            store.instance(inst).and_then(|s| {
                s.users
                    .values()
                    .find(|u| u.username == wanted)
                    .or_else(|| s.users.values().find(|u| u.name().to_lowercase() == wanted))
                    .map(|u| u.id)
            })
        };
        let Some(user) = user else {
            self.error(format!("no user {name:?} known here"));
            return;
        };
        let rest = match self.core.rest(inst) {
            Ok(r) => r,
            Err(e) => return self.error(e.to_string()),
        };
        let tasks = self.tasks.clone();
        let inst = inst.to_owned();
        tokio::spawn(async move {
            let msg = match rest.create_dm(vec![user]).await {
                Ok(dm) => TaskMsg::Open {
                    inst,
                    channel: dm.id,
                },
                Err(e) => TaskMsg::Error(e.to_string()),
            };
            let _ = tasks.send(msg);
        });
    }

    fn create_invite(&mut self, inst: &str, channel: Id) {
        let Some(space) = self
            .core
            .store()
            .instance(inst)
            .and_then(|s| s.space_of(channel))
        else {
            self.error("open a space channel to create an invite".into());
            return;
        };
        let core = self.core.clone();
        let tasks = self.tasks.clone();
        let inst = inst.to_owned();
        tokio::spawn(async move {
            let msg = match core.rest(&inst) {
                Ok(rest) => match rest
                    .create_invite(space, &CreateInviteRequest::default())
                    .await
                {
                    Ok(invite) => TaskMsg::Info(format!(
                        "invite (7 days): {}",
                        core.invite_link(&inst, &invite.code).unwrap_or(invite.code)
                    )),
                    Err(e) => TaskMsg::Error(e.to_string()),
                },
                Err(e) => TaskMsg::Error(e.to_string()),
            };
            let _ = tasks.send(msg);
        });
    }

    fn command_key(&mut self, event: KeyEvent) {
        match event.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => {
                let line = self.command.take();
                self.mode = Mode::Normal;
                match commands::parse(&line) {
                    Ok(cmd) => self.run(cmd),
                    Err(e) if !e.is_empty() => self.error(e),
                    Err(_) => {}
                }
            }
            KeyCode::Backspace if self.command.is_empty() => self.mode = Mode::Normal,
            KeyCode::Backspace => self.command.backspace(),
            KeyCode::Left => self.command.left(),
            KeyCode::Right => self.command.right(),
            KeyCode::Home => self.command.home(),
            KeyCode::End => self.command.end(),
            KeyCode::Char(c) if !event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.command.insert(c);
            }
            _ => {}
        }
    }

    pub fn run(&mut self, cmd: Command) {
        match cmd {
            Command::Quit => self.quit = true,
            Command::Help => self.popup = Some(Popup::Help),
            Command::Join(name) => self.join_by_name(&name),
            Command::Dm(user) => match self.selected_instance() {
                Some(inst) => self.open_dm(&inst, &user),
                None => self.error("no instance".into()),
            },
            Command::Voice(_) | Command::Leave | Command::Mute | Command::Deafen => {
                self.info("Voice comes in a later version.".into());
            }
            Command::Status(status) => {
                self.core.set_presence(status);
                self.info(format!("status: {status:?}").to_lowercase());
            }
            Command::Upload(path) => self.upload(PathBuf::from(path)),
            Command::Open(n) => self.open_attachment(n),
            Command::AddInstance(target) => self.add_instance(&target),
            Command::RemoveInstance(id) => {
                let Some(inst) = id.or_else(|| self.selected_instance()) else {
                    return self.error("no instance selected".into());
                };
                self.popup = Some(Popup::Confirm {
                    text: format!("Sign out of {inst} and remove it? (y/n)"),
                    action: ConfirmAction::RemoveInstance(inst),
                });
            }
            Command::Login => self.login(self.selected_instance()),
            Command::Logout => {
                let Some(inst) = self.selected_instance() else {
                    return;
                };
                if self.current.as_ref().is_some_and(|(i, _)| *i == inst) {
                    self.current = None;
                }
                let core = self.core.clone();
                tokio::spawn(async move {
                    let _ = core.logout(&inst).await;
                });
            }
            Command::Invite => match self.current.clone() {
                Some((inst, channel)) => self.create_invite(&inst, channel),
                None => self.error("open a space channel first".into()),
            },
            Command::Retry => self.action(Action::Retry),
            Command::Set(key, value) => match self.core.set_setting(&key, &value) {
                Ok(()) => {
                    if key.starts_with("tui.keys") {
                        self.keymap = Keymap::new(&self.core.config().tui.keys).0;
                    }
                    self.info(format!("{key} = {value}"));
                }
                Err(e) => self.error(e.to_string()),
            },
            Command::Offline | Command::Online => {
                let online = cmd == Command::Online;
                if let Some(inst) = self.selected_instance()
                    && let Err(e) = self.core.set_online(&inst, online)
                {
                    self.error(e.to_string());
                }
            }
        }
    }

    fn join_by_name(&mut self, name: &str) {
        let wanted = name.to_lowercase();
        let current_space = self.current.as_ref().and_then(|(inst, c)| {
            self.core
                .store()
                .instance(inst)
                .and_then(|s| s.space_of(*c))
        });
        // Prefer the current space, then any channel with that name.
        let found = self
            .sidebar
            .iter()
            .filter_map(|item| match item {
                SidebarItem::Channel {
                    inst,
                    channel,
                    name,
                    kind,
                    ..
                } if name.to_lowercase() == wanted && kind.has_messages() => {
                    let space = self
                        .core
                        .store()
                        .instance(inst)
                        .and_then(|s| s.space_of(*channel));
                    Some((
                        space == current_space && space.is_some(),
                        inst.clone(),
                        *channel,
                    ))
                }
                _ => None,
            })
            .max_by_key(|(preferred, _, _)| *preferred);
        match found {
            Some((_, inst, channel)) => {
                self.open(inst, channel);
                self.focus = Focus::Messages;
            }
            None => self.error(format!("no channel named {name:?}")),
        }
    }

    fn upload(&mut self, path: PathBuf) {
        let Some((inst, channel)) = self.current.clone() else {
            return self.error("open a channel first".into());
        };
        let core = self.core.clone();
        let tasks = self.tasks.clone();
        let reply_to = self.reply_to.take();
        self.info(format!("uploading {}…", path.display()));
        tokio::spawn(async move {
            match core.upload(&inst, &path).await {
                Ok(attachment) => {
                    let draft = Draft {
                        content: String::new(),
                        reply_to,
                        attachments: vec![attachment],
                    };
                    if let Err(e) = core.send_message(&inst, channel, draft) {
                        let _ = tasks.send(TaskMsg::Error(e.to_string()));
                    } else {
                        let _ = tasks.send(TaskMsg::Info("uploaded".into()));
                    }
                }
                Err(e) => {
                    let _ = tasks.send(TaskMsg::Error(e.to_string()));
                }
            }
        });
    }

    fn open_attachment(&mut self, n: usize) {
        let Some((inst, channel)) = self.current.clone() else {
            return;
        };
        let attachment = {
            let store = self.core.store();
            let w = store.instance(&inst).and_then(|s| s.window(channel));
            let message = match self.msg_sel {
                Some(i) => w.and_then(|w| w.messages.get(i)),
                None => w.and_then(|w| w.messages.iter().rev().find(|m| !m.attachments.is_empty())),
            };
            message.and_then(|m| m.attachments.get(n - 1).cloned())
        };
        let Some(attachment) = attachment else {
            return self.error(format!("no attachment {n} on the selected message"));
        };
        let core = self.core.clone();
        let tasks = self.tasks.clone();
        tokio::spawn(async move {
            let result = async {
                let bytes = core
                    .rest(&inst)
                    .map_err(|e| e.to_string())?
                    .download(attachment.id, &attachment.filename)
                    .await
                    .map_err(|e| e.to_string())?;
                let dir = std::env::temp_dir()
                    .join("hermes-tui")
                    .join(attachment.id.to_string());
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                // The name comes from another user: no directory parts or odd characters.
                let name = hermes_client_core::files::safe_filename(&attachment.filename);
                let path = dir.join(&name);
                std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
                // Opening a program would run it; only save those.
                if hermes_client_core::files::is_program(&name) {
                    return Ok::<_, String>((path, false));
                }
                open_with_os(&path).map_err(|e| e.to_string())?;
                Ok((path, true))
            }
            .await;
            let _ = tasks.send(match result {
                Ok((path, true)) => TaskMsg::Info(format!("opened {}", path.display())),
                Ok((path, false)) => TaskMsg::Info(format!(
                    "saved {} (not opened: it is a program)",
                    path.display()
                )),
                Err(e) => TaskMsg::Error(e),
            });
        });
    }

    fn add_instance(&mut self, target: &str) {
        let target = match AddTarget::parse(target) {
            Ok(t) => t,
            Err(e) => return self.error(e.to_string()),
        };
        let (host, invite, fp) = match target {
            AddTarget::Address(host) => (host, None, None),
            AddTarget::Invite(InviteLink {
                host,
                code,
                fingerprint,
            }) => (host, Some(code), fingerprint),
        };
        // An invite's fingerprint is trusted without asking (section 13).
        if let Some(fp) = fp
            && let Err(e) = self.core.trust_fingerprint(&host, fp)
        {
            return self.error(e.to_string());
        }
        self.info(format!("connecting to {host}…"));
        let core = self.core.clone();
        let tasks = self.tasks.clone();
        tokio::spawn(async move {
            let msg = match core.probe_instance(&host).await {
                Ok(probe) => TaskMsg::Probed {
                    probe: Box::new(probe),
                    invite,
                },
                Err(e) => TaskMsg::Error(format!("{host}: {e}")),
            };
            let _ = tasks.send(msg);
        });
    }

    fn login(&mut self, inst: Option<InstanceId>) {
        let Some(inst) = inst else {
            return self.error("no instance; use :instance add <address>".into());
        };
        let Some(entry) = self.core.config().instance(&inst).cloned() else {
            return self.error(format!("no instance {inst}"));
        };
        self.login_form(entry.url, None);
        if let Some(Popup::Form(form)) = &mut self.popup {
            form.fields[0].input = Input::with_text(&entry.username);
            form.focus = 1;
        }
    }

    fn login_form(&mut self, host: HostPort, invite: Option<String>) {
        self.popup = Some(Popup::Form(Form {
            host,
            register: invite.is_some(),
            invite,
            fields: vec![
                Field {
                    label: "Username",
                    input: Input::default(),
                    secret: false,
                },
                Field {
                    label: "Password",
                    input: Input::default(),
                    secret: true,
                },
            ],
            focus: 0,
            busy: false,
        }));
    }

    fn submit_form(&mut self) {
        let Some(Popup::Form(form)) = &mut self.popup else {
            return;
        };
        if form.busy {
            return;
        }
        let username = form.fields[0].input.text().trim().to_owned();
        let password = form.fields[1].input.text().to_owned();
        if username.is_empty() || password.is_empty() {
            self.error("enter a username and a password".into());
            return;
        }
        form.busy = true;
        let (host, invite, register) = (form.host.clone(), form.invite.clone(), form.register);
        let core = self.core.clone();
        let tasks = self.tasks.clone();
        tokio::spawn(async move {
            let result = if register {
                core.register(&host, &username, &password, invite.as_deref())
                    .await
            } else {
                match core.login(&host, &username, &password).await {
                    // Signed in with an existing account: use the invite to join its space.
                    Ok(inst) => match &invite {
                        Some(code) => match core.join_invite(&inst, code).await {
                            Ok(_) | Err(CoreError::Rest(_)) => Ok(inst),
                            Err(e) => Err(e),
                        },
                        None => Ok(inst),
                    },
                    Err(e) => Err(e),
                }
            };
            let _ = tasks.send(match result {
                Ok(inst) => TaskMsg::SignedIn { inst },
                Err(e) => TaskMsg::FormFailed(e.to_string()),
            });
        });
    }

    fn popup_key(&mut self, event: KeyEvent) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
        match popup {
            Popup::Help => self.popup = None,
            Popup::Confirm { action, .. } => match event.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let action = action.clone();
                    self.popup = None;
                    self.confirmed(action);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.popup = None,
                _ => {}
            },
            Popup::Complete { items, selected } => match event.code {
                KeyCode::Tab | KeyCode::Down => *selected = (*selected + 1) % items.len(),
                KeyCode::BackTab | KeyCode::Up => {
                    *selected = (*selected + items.len() - 1) % items.len();
                }
                KeyCode::Enter => {
                    let insert = items[*selected].insert.clone();
                    self.popup = None;
                    self.input.replace_token(&insert);
                }
                KeyCode::Esc => self.popup = None,
                _ => {
                    // Keep typing: close the popup and handle the key normally.
                    self.popup = None;
                    self.insert_key(event);
                }
            },
            Popup::Emoji {
                query,
                results,
                selected,
                target,
            } => match event.code {
                KeyCode::Esc => self.popup = None,
                KeyCode::Down | KeyCode::Tab => {
                    if !results.is_empty() {
                        *selected = (*selected + 1) % results.len();
                    }
                }
                KeyCode::Up | KeyCode::BackTab => {
                    if !results.is_empty() {
                        *selected = (*selected + results.len() - 1) % results.len();
                    }
                }
                KeyCode::Enter => {
                    let Some((emoji, _)) = results.get(*selected).cloned() else {
                        return;
                    };
                    let (inst, channel, message) = target.clone();
                    self.popup = None;
                    let core = self.core.clone();
                    let tasks = self.tasks.clone();
                    tokio::spawn(async move {
                        // Reacting again with the same emoji removes it.
                        let mine = core
                            .store()
                            .instance(&inst)
                            .and_then(|s| s.window(channel))
                            .and_then(|w| w.get(message))
                            .is_some_and(|m| m.reactions.iter().any(|r| r.emoji == emoji && r.me));
                        let result = if mine {
                            core.unreact(&inst, channel, message, &emoji).await
                        } else {
                            core.react(&inst, channel, message, &emoji).await
                        };
                        if let Err(e) = result {
                            let _ = tasks.send(TaskMsg::Error(e.to_string()));
                        }
                    });
                }
                KeyCode::Backspace => {
                    query.backspace();
                    *results = view::search_emoji(query.text(), 8);
                    *selected = 0;
                }
                KeyCode::Char(c) if !ctrl => {
                    query.insert(c);
                    *results = view::search_emoji(query.text(), 8);
                    *selected = 0;
                }
                _ => {}
            },
            Popup::Form(form) => match event.code {
                KeyCode::Esc => self.popup = None,
                KeyCode::Tab | KeyCode::Down => form.focus = (form.focus + 1) % form.fields.len(),
                KeyCode::BackTab | KeyCode::Up => {
                    form.focus = (form.focus + form.fields.len() - 1) % form.fields.len();
                }
                KeyCode::Char('r') if ctrl => form.register = !form.register,
                KeyCode::Enter => {
                    if form.focus + 1 < form.fields.len() {
                        form.focus += 1;
                    } else {
                        self.submit_form();
                    }
                }
                KeyCode::Backspace => form.fields[form.focus].input.backspace(),
                KeyCode::Left => form.fields[form.focus].input.left(),
                KeyCode::Right => form.fields[form.focus].input.right(),
                KeyCode::Char(c) if !ctrl => form.fields[form.focus].input.insert(c),
                _ => {}
            },
        }
    }

    fn confirmed(&mut self, action: ConfirmAction) {
        let core = self.core.clone();
        let tasks = self.tasks.clone();
        match action {
            ConfirmAction::DeleteMessage {
                inst,
                channel,
                message,
            } => {
                tokio::spawn(async move {
                    if let Err(e) = core.delete_message(&inst, channel, message).await {
                        let _ = tasks.send(TaskMsg::Error(e.to_string()));
                    }
                });
            }
            ConfirmAction::Discard {
                inst,
                channel,
                local_id,
            } => {
                core.discard_message(&inst, channel, local_id);
                self.msg_sel = None;
            }
            ConfirmAction::Trust { probe, invite } => {
                match core.trust_fingerprint(&probe.host, probe.fingerprint) {
                    Ok(()) => self.login_form(probe.host.clone(), invite),
                    Err(e) => self.error(e.to_string()),
                }
            }
            ConfirmAction::RemoveInstance(inst) => {
                if self.current.as_ref().is_some_and(|(i, _)| *i == inst) {
                    self.current = None;
                }
                tokio::spawn(async move {
                    let msg = match core.remove_instance(&inst).await {
                        Ok(()) => TaskMsg::Info(format!("removed {inst}")),
                        Err(e) => TaskMsg::Error(e.to_string()),
                    };
                    let _ = tasks.send(msg);
                });
                self.rebuild_sidebar();
            }
        }
    }
}

/// Opens a file with the system's default application. On Windows this calls `ShellExecuteW`
/// directly: going through `cmd /C start` would let `&` and similar characters in a filename run
/// other commands.
#[cfg(windows)]
#[allow(unsafe_code)]
fn open_with_os(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{PCWSTR, w};

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call; the other
    // pointers are static strings or null.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    if result.0 as usize > 32 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "could not open the file (error {})",
            result.0 as usize
        )))
    }
}

/// Opens a file with the system's default application (no shell involved).
#[cfg(not(windows))]
fn open_with_os(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path);
        c
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}
