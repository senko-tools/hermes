//! The TUI's right-click: an action menu for the selection (`a`, `:menu` or a right-click),
//! result lists with jump-to (search, mentions, pins), and the space management commands
//! (PLAN.md section 16.2 and Phase 6).

use hermes_client_core::proto::api::{
    BanRequest, CreateChannelRequest, NotificationSettingRequest, SearchQuery,
    UpdateChannelRequest, UpdateMeRequest, UpdateMemberRequest,
};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{
    ChannelKind, Message as ChatMessage, NotificationLevel, NotificationSetting,
};
use hermes_client_core::proto::permissions::Permissions;
use hermes_client_core::store::HistoryAnchor;
use hermes_client_core::{InstanceId, view};

use crate::app::{App, ConfirmAction, Focus, Mode, Popup, SidebarItem, TaskMsg};
use crate::commands::{Command, CreateKind, Notify, ProfileField};
use crate::input::Input;
use crate::keymap::Action;

/// One entry of an action menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    /// A normal-mode action on the current selection.
    Key(Action),
    Run(Command),
    /// Opens `:` with this text, for actions that need more input.
    Prefill(String),
    /// Opens the channel first, then does the inner item.
    In(InstanceId, Id, Box<MenuItem>),
    /// Adds `@username ` to the input.
    Mention(String),
    Dm(InstanceId, String),
    /// Pin (`true`) or unpin the selected message.
    Pin(bool),
    /// Kick a member (asks first): instance, space, user, name.
    Kick(InstanceId, Id, Id, String),
}

pub type Menu = Vec<(String, MenuItem)>;

fn item(label: &str, item: MenuItem) -> (String, MenuItem) {
    (label.to_owned(), item)
}

impl App {
    /// The menu for what has the focus: sidebar row, selected message or selected member.
    pub fn open_menu(&mut self) {
        let menu = match self.focus {
            Focus::Sidebar => self.sidebar_menu(),
            Focus::Messages => self.message_menu(),
            Focus::Members => self.member_menu(),
        };
        match menu {
            Some((title, items)) if !items.is_empty() => {
                self.popup = Some(Popup::Menu {
                    title,
                    items,
                    selected: 0,
                });
            }
            _ => self.info("nothing to do here".into()),
        }
    }

    fn sidebar_menu(&self) -> Option<(String, Menu)> {
        let row = self.sidebar.get(self.sidebar_sel)?.clone();
        let mut out = Menu::new();
        match row {
            SidebarItem::Instance { inst, status } => {
                use hermes_client_core::gateway::ConnStatus;
                if status == ConnStatus::LoggedOut {
                    out.push(item("Sign in", MenuItem::Run(Command::Login)));
                } else {
                    out.push(item("Create a space…", MenuItem::Prefill("space ".into())));
                    out.push(item(
                        "Edit my name…",
                        MenuItem::Prefill("profile name ".into()),
                    ));
                    out.push(item(
                        "Set my status text…",
                        MenuItem::Prefill("profile status ".into()),
                    ));
                    out.push(item("Mentions", MenuItem::Run(Command::Mentions)));
                    if status == ConnStatus::Offline {
                        out.push(item("Go online", MenuItem::Run(Command::Online)));
                    } else {
                        out.push(item("Go offline", MenuItem::Run(Command::Offline)));
                    }
                    out.push(item("Log out", MenuItem::Run(Command::Logout)));
                }
                out.push(item(
                    "Remove instance",
                    MenuItem::Run(Command::RemoveInstance(Some(inst.clone()))),
                ));
                Some((inst, out))
            }
            SidebarItem::Space { inst, name } => {
                // Space commands work on the open channel, so open the space's first channel.
                let first = {
                    let store = self.core.store();
                    let s = store.instance(&inst)?;
                    let space = s
                        .spaces
                        .iter()
                        .find(|(_, sp)| sp.space.name == name)
                        .map(|(id, _)| *id)?;
                    view::channel_tree(s, space)
                        .into_iter()
                        .find(|c| c.kind != ChannelKind::Category)
                        .map(|c| c.id)?
                };
                let within = |m: MenuItem| MenuItem::In(inst.clone(), first, Box::new(m));
                out.push(item(
                    "Mark all as read",
                    within(MenuItem::Run(Command::MarkRead)),
                ));
                out.push(item(
                    "Invite people",
                    within(MenuItem::Run(Command::Invite)),
                ));
                out.push(item(
                    "Search in this space…",
                    within(MenuItem::Prefill("search ".into())),
                ));
                out.push(item(
                    "Create a text channel…",
                    within(MenuItem::Prefill("create text ".into())),
                ));
                out.push(item(
                    "Create a voice channel…",
                    within(MenuItem::Prefill("create voice ".into())),
                ));
                for (label, n) in [
                    ("Notify: all messages", Notify::All),
                    ("Notify: only @mentions", Notify::Mentions),
                    ("Notify: nothing", Notify::Nothing),
                    ("Mute space", Notify::Mute),
                    ("Unmute space", Notify::Unmute),
                ] {
                    out.push(item(label, within(MenuItem::Run(Command::Notify(n, true)))));
                }
                Some((name, out))
            }
            SidebarItem::Channel {
                inst,
                channel,
                kind,
                name,
                ..
            } => {
                if kind == ChannelKind::Category {
                    return None;
                }
                let within = |m: MenuItem| MenuItem::In(inst.clone(), channel, Box::new(m));
                out.push(item("Open", within(MenuItem::Key(Action::Open))));
                if kind.has_voice() {
                    out.push(item("Join voice", MenuItem::Key(Action::JoinVoice)));
                }
                out.push(item(
                    "Mark as read",
                    within(MenuItem::Run(Command::MarkRead)),
                ));
                out.push(item(
                    "Pinned messages",
                    within(MenuItem::Run(Command::Pins)),
                ));
                for (label, n) in [
                    ("Notify: like the space", Notify::Default),
                    ("Notify: all messages", Notify::All),
                    ("Notify: only @mentions", Notify::Mentions),
                    ("Notify: nothing", Notify::Nothing),
                    ("Mute", Notify::Mute),
                    ("Unmute", Notify::Unmute),
                ] {
                    if n == Notify::Default && kind.is_private() {
                        continue;
                    }
                    out.push(item(
                        label,
                        within(MenuItem::Run(Command::Notify(n, false))),
                    ));
                }
                let manage = {
                    let store = self.core.store();
                    store
                        .instance(&inst)
                        .is_some_and(|s| view::can(s, channel, Permissions::MANAGE_CHANNELS))
                };
                if kind == ChannelKind::Dm {
                    out.push(item(
                        "Close conversation",
                        within(MenuItem::Run(Command::CloseDm)),
                    ));
                }
                if manage && !kind.is_private() {
                    out.push(item(
                        "Rename…",
                        within(MenuItem::Prefill(format!("rename {name}"))),
                    ));
                    out.push(item(
                        "Set topic…",
                        within(MenuItem::Prefill("topic ".into())),
                    ));
                    out.push(item(
                        "Delete channel",
                        within(MenuItem::Run(Command::DeleteChannel)),
                    ));
                }
                Some((name, out))
            }
            SidebarItem::VoiceUser {
                inst, user, name, ..
            } => {
                let username = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.user(user).map(|u| u.username.clone()))?;
                out.push(item("Message", MenuItem::Dm(inst, username.clone())));
                out.push(item("Mention", MenuItem::Mention(username)));
                Some((name, out))
            }
            SidebarItem::DmHeader => None,
        }
    }

    fn message_menu(&self) -> Option<(String, Menu)> {
        let (inst, channel) = self.current.clone()?;
        let (id, author, _) = self.selected_message()?;
        let store = self.core.store();
        let s = store.instance(&inst)?;
        let msg = s.window(channel)?.get(id)?;
        let own = s.me_id() == Some(author);
        let dm = s.dms.contains_key(&channel);
        let manage = view::can(s, channel, Permissions::MANAGE_MESSAGES);
        let mut out = Menu::new();
        if view::can(s, channel, Permissions::SEND_MESSAGES) {
            out.push(item("Reply", MenuItem::Key(Action::Reply)));
        }
        out.push(item("React", MenuItem::Key(Action::React)));
        if own {
            out.push(item("Edit", MenuItem::Key(Action::Edit)));
        }
        if manage || dm {
            if msg.pinned {
                out.push(item("Unpin", MenuItem::Pin(false)));
            } else {
                out.push(item("Pin", MenuItem::Pin(true)));
            }
        }
        if !msg.attachments.is_empty() {
            out.push(item("Open the attachment", MenuItem::Run(Command::Open(1))));
        }
        if !own {
            let username = msg.author.username.clone();
            out.push(item(
                "Message the author",
                MenuItem::Dm(inst.clone(), username.clone()),
            ));
            out.push(item("Mention the author", MenuItem::Mention(username)));
        }
        if own || (manage && !dm) {
            out.push(item("Delete", MenuItem::Key(Action::Delete)));
        }
        let title = format!("message by {}", s.display_name(s.space_of(channel), author));
        Some((title, out))
    }

    /// The members of the open channel in list order (as the member list shows them).
    pub fn member_order(&self) -> Vec<Id> {
        let Some((inst, channel)) = &self.current else {
            return Vec::new();
        };
        let store = self.core.store();
        let Some(s) = store.instance(inst) else {
            return Vec::new();
        };
        match s.space_of(*channel) {
            Some(space) => view::member_groups(s, space)
                .into_iter()
                .flat_map(|g| g.members.into_iter().map(|m| m.user_id))
                .collect(),
            None => s
                .channel(*channel)
                .map(|c| c.recipients.iter().map(|u| u.id).collect())
                .unwrap_or_default(),
        }
    }

    fn member_menu(&self) -> Option<(String, Menu)> {
        let (inst, channel) = self.current.clone()?;
        let user = *self.member_order().get(self.member_sel)?;
        let store = self.core.store();
        let s = store.instance(&inst)?;
        let space = s.space_of(channel);
        let username = s.user(user)?.username.clone();
        let me = s.me_id() == Some(user);
        let mut out = Menu::new();
        if !me {
            out.push(item(
                "Message",
                MenuItem::Dm(inst.clone(), username.clone()),
            ));
        }
        out.push(item("Mention", MenuItem::Mention(username.clone())));
        if let Some(space) = space {
            let can = |p| view::can_in_space(s, space, p);
            let above = view::outranks(s, space, user);
            if can(Permissions::MANAGE_ROLES) {
                out.push(item(
                    "Give a role…",
                    MenuItem::Prefill(format!("role {username} +")),
                ));
                out.push(item(
                    "Take a role…",
                    MenuItem::Prefill(format!("role {username} -")),
                ));
            }
            if above && can(Permissions::KICK_MEMBERS) {
                out.push(item(
                    "Kick",
                    MenuItem::Kick(inst.clone(), space, user, s.display_name(Some(space), user)),
                ));
            }
            if above && can(Permissions::BAN_MEMBERS) {
                out.push(item("Ban…", MenuItem::Prefill(format!("ban {username} "))));
            }
        }
        Some((s.display_name(space, user), out))
    }

    pub fn run_menu(&mut self, choice: MenuItem) {
        match choice {
            MenuItem::Key(action) => self.action(action),
            MenuItem::Run(command) => self.run(command),
            MenuItem::Prefill(text) => {
                self.command = Input::with_text(&text);
                self.mode = Mode::Command;
            }
            MenuItem::In(inst, channel, inner) => {
                if self.current != Some((inst.clone(), channel)) {
                    self.open(inst, channel);
                }
                if *inner != MenuItem::Key(Action::Open) {
                    self.run_menu(*inner);
                } else {
                    self.focus = Focus::Messages;
                }
            }
            MenuItem::Mention(username) => {
                if self.current.is_none() {
                    return self.error("open a channel first".into());
                }
                self.input.insert_str(&format!("@{username} "));
                self.mode = Mode::Insert;
            }
            MenuItem::Dm(inst, username) => self.open_dm(&inst, &username),
            MenuItem::Pin(pin) => {
                let (Some((inst, channel)), Some((id, _, _))) =
                    (self.current.clone(), self.selected_message())
                else {
                    return;
                };
                self.rest(
                    &inst,
                    if pin { "pinned" } else { "unpinned" },
                    move |r| async move {
                        if pin {
                            r.pin(channel, id).await
                        } else {
                            r.unpin(channel, id).await
                        }
                    },
                );
            }
            MenuItem::Kick(inst, space, user, name) => {
                self.popup = Some(Popup::Confirm {
                    text: format!(
                        "Kick {name} from the space? They can rejoin with an invite. (y/n)"
                    ),
                    action: ConfirmAction::Kick { inst, space, user },
                });
            }
        }
    }

    /// Runs a REST call in the background; the status line shows `ok` or the error.
    fn rest<T, Fut>(
        &self,
        inst: &InstanceId,
        ok: &str,
        f: impl FnOnce(hermes_client_core::rest::Rest) -> Fut + Send + 'static,
    ) where
        T: Send + 'static,
        Fut: Future<Output = Result<T, hermes_client_core::rest::Error>> + Send + 'static,
    {
        let rest = self.core.rest(inst);
        let tasks = self.tasks.clone();
        let ok = ok.to_owned();
        tokio::spawn(async move {
            let msg = match rest {
                Ok(rest) => match f(rest).await {
                    Ok(_) => TaskMsg::Info(ok),
                    Err(err) => TaskMsg::Error(err.to_string()),
                },
                Err(err) => TaskMsg::Error(err.to_string()),
            };
            let _ = tasks.send(msg);
        });
    }

    /// The open channel and its space (`None` space for DMs), or an error on the status line.
    fn here(&mut self) -> Option<(InstanceId, Id, Option<Id>)> {
        let Some((inst, channel)) = self.current.clone() else {
            self.error("open a channel first".into());
            return None;
        };
        let space = self
            .core
            .store()
            .instance(&inst)
            .and_then(|s| s.space_of(channel));
        Some((inst, channel, space))
    }

    fn here_space(&mut self) -> Option<(InstanceId, Id, Id)> {
        let (inst, channel, space) = self.here()?;
        match space {
            Some(space) => Some((inst, channel, space)),
            None => {
                self.error("open a channel of a space first".into());
                None
            }
        }
    }

    /// A member of `space` by username or display name.
    fn find_member(&mut self, inst: &str, space: Id, name: &str) -> Option<Id> {
        let wanted = name.to_lowercase();
        let found = {
            let store = self.core.store();
            store.instance(inst).and_then(|s| {
                s.spaces.get(&space)?.members.values().find_map(|m| {
                    (m.user.username.to_lowercase() == wanted
                        || s.display_name(Some(space), m.user.id).to_lowercase() == wanted)
                        .then_some(m.user.id)
                })
            })
        };
        if found.is_none() {
            self.error(format!("nobody called {name:?} in this space"));
        }
        found
    }

    /// Runs the commands added with Phase 6; gives others back.
    pub fn run_more(&mut self, cmd: Command) -> Option<Command> {
        match cmd {
            Command::Menu => self.open_menu(),
            Command::Search(q) | Command::SearchAll(q) if q.trim().is_empty() => {
                self.error("usage: :search <words>".into());
            }
            Command::Search(q) => {
                let (inst, _, space) = self.here()?;
                self.search(inst, space, q);
            }
            Command::SearchAll(q) => {
                let inst = self.selected_instance()?;
                self.search(inst, None, q);
            }
            Command::Mentions => {
                let inst = self
                    .current
                    .as_ref()
                    .map(|(i, _)| i.clone())
                    .or_else(|| self.selected_instance())?;
                self.results(&inst, "mentions".into(), move |r| async move {
                    r.mentions(None, 50).await
                });
            }
            Command::Pins => {
                let (inst, channel, _) = self.here()?;
                self.results(&inst, "pinned messages".into(), move |r| async move {
                    r.pins(channel).await
                });
            }
            Command::Notify(n, space) => {
                let (inst, channel, sp) = self.here()?;
                let target = if space {
                    match sp {
                        Some(sp) => sp,
                        None => {
                            self.error("a DM has no space".into());
                            return None;
                        }
                    }
                } else {
                    channel
                };
                self.notify(inst, target, n);
            }
            Command::Create(kind, name) => {
                let (inst, channel, space) = self.here_space()?;
                let parent = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.channel(channel).and_then(|c| c.parent_id));
                let req = CreateChannelRequest {
                    kind: match kind {
                        CreateKind::Text => ChannelKind::Text,
                        CreateKind::Voice => ChannelKind::Voice,
                        CreateKind::Category => ChannelKind::Category,
                    },
                    name: if kind == CreateKind::Text {
                        name.trim()
                            .to_lowercase()
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join("-")
                    } else {
                        name.trim().to_owned()
                    },
                    parent_id: if kind == CreateKind::Category {
                        None
                    } else {
                        parent
                    },
                    topic: None,
                    bitrate: None,
                    user_limit: None,
                };
                self.rest(
                    &inst,
                    &format!("created {}", req.name),
                    move |r| async move { r.create_channel(space, &req).await },
                );
            }
            Command::CreateSpace(name) => {
                let inst = self.selected_instance()?;
                self.rest(
                    &inst,
                    &format!("created the space {name}"),
                    move |r| async move { r.create_space(&name).await },
                );
            }
            Command::Rename(name) => {
                let (inst, channel, _) = self.here()?;
                let req = UpdateChannelRequest {
                    name: Some(name),
                    ..UpdateChannelRequest::default()
                };
                self.rest(&inst, "renamed", move |r| async move {
                    r.update_channel(channel, &req).await
                });
            }
            Command::Topic(topic) => {
                let (inst, channel, _) = self.here()?;
                let topic = topic.trim().to_owned();
                let req = UpdateChannelRequest {
                    topic: Some((!topic.is_empty()).then_some(topic)),
                    ..UpdateChannelRequest::default()
                };
                self.rest(&inst, "topic changed", move |r| async move {
                    r.update_channel(channel, &req).await
                });
            }
            Command::DeleteChannel => {
                let (inst, channel, _) = self.here_space()?;
                let name = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.channel(channel).and_then(|c| c.name.clone()))
                    .unwrap_or_default();
                self.popup = Some(Popup::Confirm {
                    text: format!("Delete #{name} and all its messages for everyone? (y/n)"),
                    action: ConfirmAction::DeleteChannel { inst, channel },
                });
            }
            Command::CloseDm => {
                let (inst, channel, _) = self.here()?;
                let dm = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.channel(channel))
                    .is_some_and(|c| c.kind == ChannelKind::Dm);
                if !dm {
                    self.error(":close works in a 1:1 DM (leave group DMs from their menu)".into());
                    return None;
                }
                self.current = None;
                self.rest(&inst, "conversation closed", move |r| async move {
                    r.close_dm(channel).await
                });
            }
            Command::Kick(name) => {
                let (inst, _, space) = self.here_space()?;
                let user = self.find_member(&inst, space, &name)?;
                self.run_menu(MenuItem::Kick(inst, space, user, name));
            }
            Command::Ban(name, reason) => {
                let (inst, _, space) = self.here_space()?;
                let user = self.find_member(&inst, space, &name)?;
                let req = BanRequest {
                    reason: (!reason.is_empty()).then_some(reason),
                    delete_messages_hours: None,
                };
                self.rest(&inst, &format!("banned {name}"), move |r| async move {
                    r.ban(space, user, &req).await
                });
            }
            Command::Unban(name) => {
                let (inst, _, space) = self.here_space()?;
                let tasks = self.tasks.clone();
                let rest = self.core.rest(&inst);
                tokio::spawn(async move {
                    let result = async {
                        let rest = rest.map_err(|e| e.to_string())?;
                        let bans = rest.bans(space).await.map_err(|e| e.to_string())?;
                        let ban = bans
                            .iter()
                            .find(|b| b.user.username.eq_ignore_ascii_case(&name))
                            .ok_or_else(|| format!("{name} is not banned"))?;
                        rest.unban(space, ban.user.id)
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok::<_, String>(format!("unbanned {name}"))
                    }
                    .await;
                    let _ = tasks.send(match result {
                        Ok(text) => TaskMsg::Info(text),
                        Err(err) => TaskMsg::Error(err),
                    });
                });
            }
            Command::Role(name, role, add) => {
                let (inst, _, space) = self.here_space()?;
                let user = self.find_member(&inst, space, &name)?;
                let (role_id, mut roles) = {
                    let store = self.core.store();
                    let s = store.instance(&inst)?;
                    let st = s.spaces.get(&space)?;
                    let role_id = st
                        .roles
                        .iter()
                        .find(|r| !r.is_everyone() && r.name.eq_ignore_ascii_case(&role))
                        .map(|r| r.id);
                    (
                        role_id,
                        st.members
                            .get(&user)
                            .map(|m| m.role_ids.clone())
                            .unwrap_or_default(),
                    )
                };
                let Some(role_id) = role_id else {
                    self.error(format!("there is no role called {role:?}"));
                    return None;
                };
                roles.retain(|r| *r != role_id);
                if add {
                    roles.push(role_id);
                }
                let req = UpdateMemberRequest {
                    nickname: None,
                    role_ids: Some(roles),
                };
                self.rest(
                    &inst,
                    &format!("{} {role}", if add { "gave" } else { "took away" }),
                    move |r| async move { r.update_member(space, Some(user), &req).await },
                );
            }
            Command::Profile(field, text) => {
                let inst = self.selected_instance()?;
                let value = (!text.is_empty()).then_some(text);
                let req = match field {
                    ProfileField::Name => UpdateMeRequest {
                        display_name: Some(value),
                        ..UpdateMeRequest::default()
                    },
                    ProfileField::Status => UpdateMeRequest {
                        status_text: Some(value),
                        ..UpdateMeRequest::default()
                    },
                };
                self.rest(&inst, "profile saved", move |r| async move {
                    r.update_me(&req).await
                });
            }
            Command::MarkRead => {
                let (inst, channel, space) = self.here()?;
                let channels: Vec<(Id, Id)> = {
                    let store = self.core.store();
                    store
                        .instance(&inst)
                        .map(|s| {
                            let ids: Vec<Id> = match space {
                                Some(sp) => s
                                    .spaces
                                    .get(&sp)
                                    .map(|st| st.channels.keys().copied().collect())
                                    .unwrap_or_default(),
                                None => vec![channel],
                            };
                            ids.into_iter()
                                .filter_map(|c| Some((c, s.channel(c)?.last_message_id?)))
                                .collect()
                        })
                        .unwrap_or_default()
                };
                for (c, last) in channels {
                    self.core.mark_read(&inst, c, last);
                }
                self.info("marked as read".into());
            }
            other => return Some(other),
        }
        None
    }

    fn notify(&mut self, inst: InstanceId, target: Id, n: Notify) {
        let current = self
            .core
            .store()
            .instance(&inst)
            .and_then(|s| s.notification_settings.get(&target).copied());
        let level = current.map_or(NotificationLevel::Mentions, |c| c.level);
        let muted = current.and_then(|c| c.muted_until);
        let req = match n {
            Notify::All => Some((NotificationLevel::All, muted)),
            Notify::Mentions => Some((NotificationLevel::Mentions, muted)),
            Notify::Nothing => Some((NotificationLevel::None, muted)),
            Notify::Mute => Some((level, Some(NotificationSetting::MUTED_FOREVER))),
            Notify::Unmute => Some((level, None)),
            Notify::Default => None,
        };
        match req {
            Some((level, muted_until)) => {
                let req = NotificationSettingRequest { level, muted_until };
                self.rest(&inst, "notifications changed", move |r| async move {
                    r.set_notification_setting(target, &req).await
                });
            }
            None => self.rest(
                &inst,
                "notifications back to the default",
                move |r| async move { r.delete_notification_setting(target).await },
            ),
        }
    }

    fn search(&mut self, inst: InstanceId, space: Option<Id>, q: String) {
        let title = format!("search: {q}");
        let query = SearchQuery {
            q,
            space_id: space,
            limit: Some(50),
            ..SearchQuery::default()
        };
        self.results(&inst, title, move |r| async move {
            r.search(&query).await.map(|res| res.messages)
        });
    }

    /// Loads a message list in the background and shows it in a results popup.
    fn results<Fut>(
        &mut self,
        inst: &InstanceId,
        title: String,
        f: impl FnOnce(hermes_client_core::rest::Rest) -> Fut + Send + 'static,
    ) where
        Fut: Future<Output = Result<Vec<ChatMessage>, hermes_client_core::rest::Error>>
            + Send
            + 'static,
    {
        self.info(format!("loading {title}…"));
        let rest = self.core.rest(inst);
        let tasks = self.tasks.clone();
        let inst = inst.clone();
        tokio::spawn(async move {
            let msg = match rest {
                Ok(rest) => match f(rest).await {
                    Ok(items) => TaskMsg::Results {
                        title,
                        items: items.into_iter().map(|m| (inst.clone(), m)).collect(),
                    },
                    Err(err) => TaskMsg::Error(err.to_string()),
                },
                Err(err) => TaskMsg::Error(err.to_string()),
            };
            let _ = tasks.send(msg);
        });
    }

    /// One line per result: time, author, place and text.
    pub fn result_line(&self, inst: &str, m: &ChatMessage) -> String {
        let store = self.core.store();
        let Some(s) = store.instance(inst) else {
            return String::new();
        };
        let space = s.space_of(m.channel_id);
        let place = s
            .channel(m.channel_id)
            .map(|c| view::channel_title(s, c))
            .unwrap_or_default();
        format!(
            "{} {} {place}: {}",
            hermes_client_core::time::format(m.created_at() as i64, "%m-%d %H:%M", self.utc_offset),
            s.display_name(space, m.author.id),
            view::plain_text(s, space, &m.content, 200)
        )
    }

    /// Opens `channel` at `message` (loading the page around it) and selects it.
    pub fn jump(&mut self, inst: InstanceId, channel: Id, message: Id) {
        if self.current != Some((inst.clone(), channel)) {
            self.open(inst.clone(), channel);
        }
        self.focus = Focus::Messages;
        let loaded = self
            .core
            .store()
            .instance(&inst)
            .and_then(|s| s.window(channel))
            .is_some_and(|w| w.get(message).is_some());
        if loaded {
            self.select_message(message);
            return;
        }
        let core = self.core.clone();
        let tasks = self.tasks.clone();
        tokio::spawn(async move {
            let msg = match core
                .load_history(&inst, channel, HistoryAnchor::Around(message))
                .await
            {
                Ok(()) => TaskMsg::Jumped {
                    inst,
                    channel,
                    message,
                },
                Err(err) => TaskMsg::Error(err.to_string()),
            };
            let _ = tasks.send(msg);
        });
    }

    pub fn select_message(&mut self, message: Id) {
        let Some((inst, channel)) = self.current.clone() else {
            return;
        };
        let index = self
            .core
            .store()
            .instance(&inst)
            .and_then(|s| s.window(channel))
            .and_then(|w| w.messages.iter().position(|m| m.id == message));
        if let Some(i) = index {
            self.msg_sel = Some(i);
        }
    }

    /// At the newest loaded message of a window that does not reach the present (after a
    /// jump): the next page.
    pub fn load_newer(&self) -> bool {
        let Some((inst, channel)) = self.current.clone() else {
            return false;
        };
        let last = {
            let store = self.core.store();
            let Some(w) = store.instance(&inst).and_then(|s| s.window(channel)) else {
                return false;
            };
            if w.reached_end {
                return false;
            }
            w.messages.last().map(|m| m.id)
        };
        let Some(last) = last else {
            return false;
        };
        let core = self.core.clone();
        let tasks = self.tasks.clone();
        tokio::spawn(async move {
            if let Err(err) = core
                .load_history(&inst, channel, HistoryAnchor::After(last))
                .await
            {
                let _ = tasks.send(TaskMsg::Error(err.to_string()));
            }
        });
        true
    }
}
