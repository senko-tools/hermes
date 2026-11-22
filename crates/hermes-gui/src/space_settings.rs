//! The space settings page (PLAN.md sections 15.1 and 15.4), over the whole window: Overview, Roles,
//! Channels & permissions, Members, Invites, Bans and Audit log. Everything is read from the store;
//! the invite, ban and audit lists are loaded over REST when their tab opens. Changes go through
//! REST and come back as gateway events, so the store stays the only source of truth.

use crate::ui::ButtonKind;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Entity, FontWeight, SharedString, Subscription, Window, div};
use hermes_client_core::proto::api::{
    CreateRoleRequest, OverrideRequest, RolePosition, UpdateMemberRequest, UpdateRoleRequest,
    UpdateSpaceRequest, VoiceStatePatchRequest,
};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{
    AuditAction, AuditEntry, Ban, ChannelKind, Invite, Member, Override, Role,
};
use hermes_client_core::proto::permissions::{CATALOG, PermissionGroup, Permissions};
use hermes_client_core::store::InstanceStore;
use hermes_client_core::{CoreError, InstanceId, now_ms, time, view};

use crate::assets::{self, icon};
use crate::drag::{Ghost, Item, Kind, Shape};
use crate::menu_entries::{Action, Target};
use crate::results::Remote;
use crate::settings::{
    DeviceChoice, centered, frame, group, group_card, muted, page_body, page_body_with, page_part,
    setting_below, setting_fit, setting_pair,
};
use crate::shell::Shell;
use crate::theme::{self, px};

const AUDIT_PAGE: u32 = 50;
/// How many members the list shows at first, and how many more each "Show more" adds.
const MEMBER_PAGE: usize = 50;
/// Preset colors offered next to the hex field.
const SWATCHES: [u32; 12] = [
    0x1ABC9C, 0x2ECC71, 0x3498DB, 0x9B59B6, 0xE91E63, 0xF1C40F, 0xE67E22, 0xE74C3C, 0x95A5A6,
    0x607D8B, 0x11806A, 0x206694,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Overview,
    Roles,
    Channels,
    Members,
    Encryption,
    Invites,
    Bans,
    Reports,
    Audit,
}

impl Tab {
    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Roles => "Roles",
            Tab::Channels => "Channels",
            Tab::Members => "Members",
            Tab::Encryption => "Encryption",
            Tab::Invites => "Invites",
            Tab::Bans => "Bans",
            Tab::Reports => "Reports",
            Tab::Audit => "Audit log",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tab::Overview => assets::INFO,
            Tab::Roles => assets::SHIELD,
            Tab::Channels => assets::POUND,
            Tab::Members => assets::USERS,
            Tab::Encryption => assets::LOCK,
            Tab::Invites => assets::LINK,
            Tab::Bans => assets::BAN,
            Tab::Reports => assets::BELL,
            Tab::Audit => assets::FILE,
        }
    }

    /// The tabs I can use in a space.
    pub fn available(s: &InstanceStore, space: Id) -> Vec<Tab> {
        let mut tabs = vec![Tab::Overview, Tab::Roles, Tab::Channels, Tab::Members];
        if s.instance
            .as_ref()
            .is_some_and(|i| i.limits.channels_encrypted)
        {
            tabs.push(Tab::Encryption);
        }
        if view::can_in_space(s, space, Permissions::CREATE_INVITES)
            || view::can_in_space(s, space, Permissions::MANAGE_INVITES)
        {
            tabs.push(Tab::Invites);
        }
        if view::can_in_space(s, space, Permissions::BAN_MEMBERS) {
            tabs.push(Tab::Bans);
        }
        if view::can_in_space(s, space, Permissions::MANAGE_MESSAGES) {
            tabs.push(Tab::Reports);
        }
        if view::can_in_space(s, space, Permissions::VIEW_AUDIT_LOG) {
            tabs.push(Tab::Audit);
        }
        tabs
    }
}

/// One permission in an override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tri {
    Deny,
    Inherit,
    Allow,
}

impl Tri {
    pub fn of(ov: Option<&Override>, flag: Permissions) -> Tri {
        match ov {
            Some(o) if o.allow.contains(flag) => Tri::Allow,
            Some(o) if o.deny.contains(flag) => Tri::Deny,
            _ => Tri::Inherit,
        }
    }

    /// `(allow, deny)` with `flag` set to `self`.
    pub fn apply(
        self,
        allow: Permissions,
        deny: Permissions,
        flag: Permissions,
    ) -> (Permissions, Permissions) {
        let (allow, deny) = (allow - flag, deny - flag);
        match self {
            Tri::Allow => (allow | flag, deny),
            Tri::Deny => (allow, deny | flag),
            Tri::Inherit => (allow, deny),
        }
    }
}

/// The bits I may allow or deny in `role`'s override in `channel`, or `None` if I may not change
/// that override at all. As the server checks it: Manage channels and Manage roles in the
/// channel, and the role is `@everyone`, one of mine or below my highest role; an override may
/// only hold bits I have there myself.
pub fn override_access(s: &InstanceStore, space: Id, channel: Id, role: Id) -> Option<Permissions> {
    let have = s.channel_permissions(channel);
    if !have.contains(Permissions::MANAGE_CHANNELS | Permissions::MANAGE_ROLES) {
        return None;
    }
    let st = s.spaces.get(&space)?;
    let role = st.role(role)?;
    let me = st.members.get(&s.me_id()?)?;
    let allowed = role.is_everyone()
        || me.role_ids.contains(&role.id)
        || hermes_client_core::proto::permissions::can_manage_role(&st.space, &st.roles, me, role);
    allowed.then_some(have)
}

/// Whether setting `flag` to `tri` in an override (`ov`, if there is one) is something the server
/// takes, with `access` from [`override_access`]. Emptying the override deletes it, which needs no
/// bits of mine.
pub fn override_change_allowed(
    access: Option<Permissions>,
    ov: Option<&Override>,
    flag: Permissions,
    tri: Tri,
) -> bool {
    access.is_some_and(|have| {
        let (allow, deny) = ov.map_or_else(Default::default, |o| (o.allow, o.deny));
        let (allow, deny) = tri.apply(allow, deny, flag);
        let bits = allow | deny;
        bits.is_empty() || have.contains(bits)
    })
}

/// The role being edited, unsaved.
pub struct RoleDraft {
    pub id: Id,
    name: Entity<InputState>,
    color: Entity<InputState>,
    pub hoist: bool,
    pub mentionable: bool,
    pub permissions: Permissions,
    _inputs: Vec<Subscription>,
}

/// What a space's settings page remembers (one per space, kept while the app runs).
pub struct SpacePage {
    pub inst: InstanceId,
    pub space: Id,
    pub tab: Tab,
    name: Entity<InputState>,
    description: Entity<InputState>,
    member_query: Entity<InputState>,
    /// The overview's fields differ from the store.
    overview_dirty: bool,
    /// The name and description the overview's fields were filled with: a save sends only
    /// what was edited since, so it cannot undo someone else's change to the other field.
    overview_loaded: (String, String),
    pub role: Option<RoleDraft>,
    /// The channel and role whose overrides are shown.
    pub channel: Option<Id>,
    pub override_role: Option<Id>,
    /// The member whose actions are shown.
    pub member: Option<Id>,
    /// How many members the list shows (grows by [`MEMBER_PAGE`]).
    member_limit: usize,
    /// The channel page's role dropdown and its subscription.
    override_select: Option<(Entity<SelectState<Vec<DeviceChoice>>>, Subscription)>,
    pub invites: Remote<Vec<Invite>>,
    pub bans: Remote<Vec<Ban>>,
    pub reports: Remote<Vec<hermes_client_core::proto::models::Report>>,
    pub audit: Remote<Vec<AuditEntry>>,
    /// The audit log has older entries to load.
    pub audit_more: bool,
    /// The Encryption tab.
    pub keys: crate::key_settings::KeySettings,
    _inputs: Vec<Subscription>,
}

/// `0xRRGGBB` as `#RRGGBB`.
pub fn color_hex(color: u32) -> String {
    format!("#{:06X}", color & 0xFF_FFFF)
}

/// `#RRGGBB` or `RRGGBB` (also `#RGB`); `None` if empty, `Err` if malformed.
pub fn parse_color(text: &str) -> Result<Option<u32>, ()> {
    let hex = text.trim().trim_start_matches('#');
    if hex.is_empty() {
        return Ok(None);
    }
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(());
    }
    let hex = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 => hex.to_owned(),
        _ => return Err(()),
    };
    u32::from_str_radix(&hex, 16).map(Some).map_err(|_| ())
}

/// The new positions after dropping role `moved` onto `onto` (`roles` highest first): the role
/// takes `onto`'s place. Positions are renumbered from 1 at the bottom; only changes are returned.
/// `@everyone` never moves.
pub fn reorder_roles(roles: &[Role], moved: Id, onto: Id) -> Vec<RolePosition> {
    let mut list: Vec<&Role> = roles.iter().filter(|r| !r.is_everyone()).collect();
    let (Some(from), Some(to)) = (
        list.iter().position(|r| r.id == moved),
        list.iter().position(|r| r.id == onto),
    ) else {
        return Vec::new();
    };
    if from == to {
        return Vec::new();
    }
    let role = list.remove(from);
    list.insert(to, role);
    let n = list.len() as i32;
    list.iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let position = n - i as i32;
            (r.position != position).then_some(RolePosition { id: r.id, position })
        })
        .collect()
}

/// Members matching `query` (name, nickname or username, case-insensitive), by name, with the
/// name they show as. Runs on every frame of the Members tab, so each name is lowercased once.
pub fn filter_members<'s>(
    s: &'s InstanceStore,
    space: Id,
    query: &str,
) -> Vec<(String, &'s Member)> {
    let Some(st) = s.spaces.get(&space) else {
        return Vec::new();
    };
    let q = query.trim().to_lowercase();
    let mut out: Vec<(String, String, &Member)> = st
        .members
        .values()
        .filter_map(|m| {
            let name = s.display_name(Some(space), m.user.id);
            let lower = name.to_lowercase();
            let matches =
                q.is_empty() || lower.contains(&q) || m.user.username.to_lowercase().contains(&q);
            matches.then_some((lower, name, m))
        })
        .collect();
    // Same names keep one order from frame to frame.
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.user.id.cmp(&b.2.user.id)));
    out.into_iter().map(|(_, name, m)| (name, m)).collect()
}

/// A readable line for an audit entry ("Alice kicked Bob") and its details.
pub fn describe(s: &InstanceStore, space: Id, entry: &AuditEntry) -> (String, String) {
    let actor = s.display_name(Some(space), entry.actor_id);
    let detail_name = entry
        .details
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let target = entry.target_id.map(|id| {
        if let Some(role) = s.spaces.get(&space).and_then(|st| st.role(id)) {
            format!("role {}", role.name)
        } else if let Some(c) = s.channel(id) {
            format!("#{}", c.name.clone().unwrap_or_default())
        } else if s.user(id).is_some() || s.member(space, id).is_some() {
            s.display_name(Some(space), id)
        } else {
            detail_name.clone().unwrap_or_else(|| format!("#{id}"))
        }
    });
    let t = target.unwrap_or_default();
    use AuditAction as A;
    let what = match entry.action {
        A::SpaceUpdate => "changed the server settings".to_owned(),
        A::SpaceTransfer => format!("gave the server to {t}"),
        A::MemberUpdate => format!("updated {t}"),
        A::MemberKick => format!("kicked {t}"),
        A::MemberBan => format!("banned {t}"),
        A::MemberUnban => format!("unbanned {t}"),
        A::RoleCreate => format!("created {t}"),
        A::RoleUpdate => format!("changed {t}"),
        A::RoleDelete => format!(
            "deleted role {}",
            detail_name.clone().unwrap_or_else(|| t.clone())
        ),
        A::RolesReorder => "reordered the roles".to_owned(),
        A::ChannelCreate => format!("created {t}"),
        A::ChannelUpdate => format!("changed {t}"),
        A::ChannelDelete => format!(
            "deleted channel {}",
            detail_name.clone().unwrap_or_else(|| t.clone())
        ),
        A::ChannelsReorder => "reordered the channels".to_owned(),
        A::OverrideUpdate => format!("changed the permissions of {t}"),
        A::OverrideDelete => format!("removed a permission override from {t}"),
        A::MessageDelete => "deleted a message".to_owned(),
        A::MemberVoiceUpdate => format!("changed the voice mute / deafen of {t}"),
        A::MemberMove => format!("moved {t} to another voice channel"),
        A::MemberDisconnect => format!("disconnected {t} from voice"),
        A::MemberTimeout => format!("changed the timeout of {t}"),
        A::MessagePin => "pinned a message".to_owned(),
        A::MessageUnpin => "unpinned a message".to_owned(),
        A::InviteCreate => "created an invite".to_owned(),
        A::InviteRevoke => "revoked an invite".to_owned(),
        A::KeyPolicyUpdate => "changed the encryption rules".to_owned(),
        A::MessageDeleteBulk => "deleted several messages".to_owned(),
        A::AttachmentRemove => "removed a file from a message".to_owned(),
        A::Unknown => format!("did something this version of Hermes doesn't know ({t})"),
    };
    let details = match entry.details.as_object() {
        Some(map) if !map.is_empty() => {
            if let Some(reason) = map.get("reason").and_then(|r| r.as_str()) {
                format!("reason: {reason}")
            } else {
                let keys: Vec<&str> = map
                    .keys()
                    .map(String::as_str)
                    .filter(|k| *k != "name")
                    .collect();
                if keys.is_empty() {
                    String::new()
                } else {
                    format!("changed: {}", keys.join(", ").replace('_', " "))
                }
            }
        }
        _ => String::new(),
    };
    (format!("{actor} {what}"), details)
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

impl Shell {
    pub fn open_space_settings(
        &mut self,
        inst: InstanceId,
        space: Id,
        tab: Tab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = (inst.clone(), space);
        if !self.space_pages.contains_key(&key) {
            let (name, description) = {
                let store = self.core.store();
                store
                    .instance(&inst)
                    .and_then(|s| s.spaces.get(&space))
                    .map(|st| {
                        (
                            st.space.name.clone(),
                            st.space.description.clone().unwrap_or_default(),
                        )
                    })
                    .unwrap_or_default()
            };
            let field = |value: String,
                         placeholder: &'static str,
                         window: &mut Window,
                         cx: &mut Context<Self>| {
                cx.new(|cx| {
                    let mut input = InputState::new(window, cx).placeholder(placeholder);
                    input.set_value(value, window, cx);
                    input
                })
            };
            let overview_loaded = (name.clone(), description.clone());
            let name = field(name, "Name", window, cx);
            let description = field(description, "What is this server about?", window, cx);
            let member_query = field(String::new(), "Search members", window, cx);
            let page_key = key.clone();
            let notify_key = key.clone();
            let subscribe_overview =
                move |this: &mut Shell,
                      event: &InputEvent,
                      key: &(InstanceId, Id),
                      cx: &mut Context<Shell>| {
                    match event {
                        InputEvent::Change => {
                            if let Some(page) = this.space_pages.get_mut(key) {
                                page.overview_dirty = true;
                            }
                            cx.notify();
                        }
                        InputEvent::PressEnter { .. } => this.save_overview(key.clone(), cx),
                        _ => {}
                    }
                };
            let first_key = page_key.clone();
            let inputs = vec![
                cx.subscribe_in(&name, window, move |this, _, event, _, cx| {
                    subscribe_overview(this, event, &first_key, cx);
                }),
                cx.subscribe_in(&description, window, move |this, _, event, _, cx| {
                    subscribe_overview(this, event, &page_key, cx);
                }),
                cx.subscribe_in(&member_query, window, move |_, _, _: &InputEvent, _, cx| {
                    let _ = &notify_key;
                    cx.notify();
                }),
            ];
            self.space_pages.insert(
                key.clone(),
                SpacePage {
                    inst: inst.clone(),
                    space,
                    tab,
                    name,
                    description,
                    member_query,
                    overview_dirty: false,
                    overview_loaded,
                    role: None,
                    channel: None,
                    override_role: None,
                    member: None,
                    member_limit: MEMBER_PAGE,
                    override_select: None,
                    invites: Remote::Idle,
                    bans: Remote::Idle,
                    reports: Remote::Idle,
                    audit: Remote::Idle,
                    audit_more: false,
                    keys: Default::default(),
                    _inputs: inputs,
                },
            );
        } else if let Some(page) = self.space_pages.get(&key)
            && !page.overview_dirty
        {
            // The page is kept while the app runs: show the name and description as they are
            // now, not as they were when it was first opened.
            let current = {
                let store = self.core.store();
                store
                    .instance(&inst)
                    .and_then(|s| s.spaces.get(&space))
                    .map(|st| {
                        (
                            st.space.name.clone(),
                            st.space.description.clone().unwrap_or_default(),
                        )
                    })
            };
            if let Some((name, description)) = current {
                let (name_input, description_input) = (page.name.clone(), page.description.clone());
                if let Some(page) = self.space_pages.get_mut(&key) {
                    page.overview_loaded = (name.clone(), description.clone());
                }
                // `set_value` sends no `Change`, so the page does not turn dirty.
                name_input.update(cx, |input, cx| input.set_value(name, window, cx));
                description_input.update(cx, |input, cx| input.set_value(description, window, cx));
            }
        }
        self.open_space_page = Some(key);
        self.set_space_tab(tab, false, cx);
        cx.notify();
    }

    pub fn close_space_settings(&mut self, cx: &mut Context<Self>) {
        self.open_space_page = None;
        cx.notify();
    }

    fn open_page(&mut self) -> Option<&mut SpacePage> {
        let key = self.open_space_page.clone()?;
        self.space_pages.get_mut(&key)
    }

    /// Shows `tab` and loads its REST list (unless already loaded and not `force`).
    fn set_space_tab(&mut self, tab: Tab, force: bool, cx: &mut Context<Self>) {
        let Some(done) = self.open_space_page.clone() else {
            return;
        };
        let (inst, space) = done.clone();
        let manage_invites = {
            let store = self.core.store();
            store
                .instance(&inst)
                .is_some_and(|s| view::can_in_space(s, space, Permissions::MANAGE_INVITES))
        };
        let core = self.core.clone();
        let Some(page) = self.space_pages.get_mut(&done) else {
            return;
        };
        if !force {
            // Its list, not the page of what was open in it last time.
            page.role = None;
            page.channel = None;
            page.member = None;
            page.member_limit = MEMBER_PAGE;
        }
        page.tab = tab;
        crate::settings::reset_page_scroll();
        match tab {
            Tab::Encryption => self.load_key_settings(done, cx),
            Tab::Invites => {
                if !manage_invites
                    || (!force && matches!(page.invites, Remote::Ready(_) | Remote::Loading))
                {
                    return;
                }
                page.invites = Remote::Loading;
                self.run(
                    cx,
                    async move {
                        let rest = core.rest(&inst)?;
                        rest.invites(space).await.map_err(|e| e.to_string())
                    },
                    move |this, result, _, _| {
                        if let Some(page) = this.space_pages.get_mut(&done) {
                            page.invites = match result {
                                Ok(list) => Remote::Ready(list),
                                Err(err) => Remote::Failed(err),
                            };
                        }
                    },
                );
            }
            Tab::Bans => {
                if !force && matches!(page.bans, Remote::Ready(_) | Remote::Loading) {
                    return;
                }
                page.bans = Remote::Loading;
                self.run(
                    cx,
                    async move {
                        let rest = core.rest(&inst)?;
                        rest.bans(space).await.map_err(|e| e.to_string())
                    },
                    move |this, result, _, _| {
                        if let Some(page) = this.space_pages.get_mut(&done) {
                            page.bans = match result {
                                Ok(list) => Remote::Ready(list),
                                Err(err) => Remote::Failed(err),
                            };
                        }
                    },
                );
            }
            Tab::Reports => {
                if !force && matches!(page.reports, Remote::Ready(_) | Remote::Loading) {
                    return;
                }
                page.reports = Remote::Loading;
                self.run(
                    cx,
                    async move {
                        let rest = core.rest(&inst)?;
                        rest.reports(space).await.map_err(|e| e.to_string())
                    },
                    move |this, result, _, _| {
                        if let Some(page) = this.space_pages.get_mut(&done) {
                            page.reports = match result {
                                Ok(list) => Remote::Ready(list),
                                Err(err) => Remote::Failed(err),
                            };
                        }
                    },
                );
            }
            Tab::Audit => {
                if !force && matches!(page.audit, Remote::Ready(_) | Remote::Loading) {
                    return;
                }
                page.audit = Remote::Loading;
                self.load_audit(None, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn load_audit(&mut self, before: Option<Id>, cx: &mut Context<Self>) {
        let Some(key) = self.open_space_page.clone() else {
            return;
        };
        let append = before.is_some();
        if append {
            // Hides "Load older entries" until this page is in, so it is not loaded twice.
            match self.space_pages.get_mut(&key) {
                Some(page) if page.audit_more => page.audit_more = false,
                _ => return,
            }
        }
        let core = self.core.clone();
        let (inst, space) = key.clone();
        self.run(
            cx,
            async move {
                let rest = core.rest(&inst)?;
                rest.audit_log(space, before, AUDIT_PAGE)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |this, result, _, cx| {
                let Some(page) = this.space_pages.get_mut(&key) else {
                    return;
                };
                // An older page belongs to the list whose last entry it was asked after; a
                // reload since then replaced that list.
                let same_list =
                    !append || page.audit.ready().and_then(|l| l.last()).map(|e| e.id) == before;
                if !same_list {
                    return;
                }
                match (result, append) {
                    (Ok(list), _) => {
                        page.audit_more = list.len() as u32 >= AUDIT_PAGE;
                        match (&mut page.audit, append) {
                            (Remote::Ready(old), true) => old.extend(list),
                            _ => page.audit = Remote::Ready(list),
                        }
                    }
                    // The entries already shown stay; the button comes back to try again.
                    (Err(err), true) => {
                        page.audit_more = true;
                        this.error(format!("could not load older entries: {err}"), cx);
                    }
                    (Err(err), false) => page.audit = Remote::Failed(err),
                }
            },
        );
    }

    /// An invite was made from the form: the list shows it.
    pub fn invite_created(&mut self, inst: InstanceId, space: Id, cx: &mut Context<Self>) {
        if self.open_space_page == Some((inst, space)) {
            self.set_space_tab(Tab::Invites, true, cx);
        }
    }

    fn save_overview(&mut self, key: (InstanceId, Id), cx: &mut Context<Self>) {
        let Some(page) = self.space_pages.get(&key) else {
            return;
        };
        let name = page.name.read(cx).value().trim().to_owned();
        let description = page.description.read(cx).value().trim().to_owned();
        // Only the fields edited since they were filled in, so saving one does not undo
        // someone else's change to the other.
        let (loaded_name, loaded_description) = &page.overview_loaded;
        let req = UpdateSpaceRequest {
            name: (name != loaded_name.trim()).then(|| name.clone()),
            description: (description != loaded_description.trim())
                .then(|| (!description.is_empty()).then(|| description.clone())),
            icon_id: None,
        };
        let core = self.core.clone();
        let (inst, space) = key.clone();
        self.run(
            cx,
            async move {
                core.rest(&inst)?.update_space(space, &req).await?;
                Ok::<_, CoreError>(())
            },
            move |this, result, _, cx| match result {
                Ok(()) => {
                    if let Some(page) = this.space_pages.get_mut(&key) {
                        page.overview_dirty = false;
                        page.overview_loaded = (name, description);
                    }
                    this.success("Server saved", cx);
                }
                Err(err) => this.error(format!("could not save: {err}"), cx),
            },
        );
    }

    fn pick_space_icon(&mut self, inst: InstanceId, space: Id, cx: &mut Context<Self>) {
        self.pick_picture(crate::crop::CropFor::SpaceIcon(inst, space), cx);
    }

    // ---- roles --------------------------------------------------------------------------------

    /// Starts editing `role` from the store.
    pub fn select_role(&mut self, role: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some((inst, space)) = self.open_space_page.clone() else {
            return;
        };
        let found = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| s.spaces.get(&space)?.role(role).cloned())
        };
        let Some(found) = found else {
            return;
        };
        let field = |value: String,
                     placeholder: &'static str,
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = InputState::new(window, cx).placeholder(placeholder);
                input.set_value(value, window, cx);
                input
            })
        };
        let name = field(found.name.clone(), "Role name", window, cx);
        let color = field(
            found.color.map(color_hex).unwrap_or_default(),
            "#RRGGBB or empty",
            window,
            cx,
        );
        let events = vec![
            cx.subscribe_in(&name, window, |this, _, event, _, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.save_role(cx),
                _ => {}
            }),
            cx.subscribe_in(&color, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        if let Some(page) = self.open_page() {
            page.role = Some(RoleDraft {
                id: found.id,
                name,
                color,
                hoist: found.hoist,
                mentionable: found.mentionable,
                permissions: found.permissions,
                _inputs: events,
            });
        }
        cx.notify();
    }

    fn create_role(&mut self, cx: &mut Context<Self>) {
        let Some((inst, space)) = self.open_space_page.clone() else {
            return;
        };
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                let req = CreateRoleRequest {
                    name: "new role".into(),
                    color: None,
                    hoist: None,
                    mentionable: None,
                    permissions: None,
                };
                let role = core.rest(&inst)?.create_role(space, &req).await?;
                Ok::<_, CoreError>(role.id)
            },
            |this, result, window, cx| match result {
                Ok(role) => {
                    this.select_role(role, window, cx);
                    this.info("Role created: give it a name and permissions", cx);
                }
                Err(err) => this.error(format!("could not create the role: {err}"), cx),
            },
        );
    }

    /// The PATCH for what changed against the stored role; `Err` if the color does not parse.
    fn role_changes(
        &self,
        draft: &RoleDraft,
        role: &Role,
        cx: &Context<Self>,
    ) -> Result<UpdateRoleRequest, ()> {
        let color = parse_color(&draft.color.read(cx).value())?;
        let name = draft.name.read(cx).value().trim().to_owned();
        Ok(UpdateRoleRequest {
            name: (name != role.name && !role.is_everyone()).then_some(name),
            color: (color != role.color).then_some(color),
            hoist: (draft.hoist != role.hoist).then_some(draft.hoist),
            mentionable: (draft.mentionable != role.mentionable).then_some(draft.mentionable),
            permissions: (draft.permissions != role.permissions).then_some(draft.permissions),
        })
    }

    fn save_role(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.open_space_page.clone() else {
            return;
        };
        let (inst, space) = key.clone();
        let Some(draft) = self.space_pages.get(&key).and_then(|p| p.role.as_ref()) else {
            return;
        };
        let id = draft.id;
        let role = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| s.spaces.get(&space)?.role(id).cloned())
        };
        let Some(req) = role.and_then(|r| self.role_changes(draft, &r, cx).ok()) else {
            self.error("the color must be six hex digits, like #3498DB", cx);
            return;
        };
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                let role = core.rest(&inst)?.update_role(space, id, &req).await?;
                Ok::<_, CoreError>(role.name)
            },
            |this, result, _, cx| match result {
                Ok(name) => this.success(format!("Role {name} saved"), cx),
                Err(err) => this.error(format!("could not save the role: {err}"), cx),
            },
        );
    }

    fn move_role(&mut self, role: Id, delta: isize, cx: &mut Context<Self>) {
        let Some((inst, space)) = self.open_space_page.clone() else {
            return;
        };
        let moves = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| s.spaces.get(&space))
                .map(|st| {
                    let list: Vec<_> = st.roles.iter().filter(|r| !r.is_everyone()).collect();
                    let i = list.iter().position(|r| r.id == role);
                    let j = i.and_then(|i| i.checked_add_signed(delta));
                    match (i, j.and_then(|j| list.get(j))) {
                        (Some(_), Some(onto)) => reorder_roles(&st.roles, role, onto.id),
                        _ => Vec::new(),
                    }
                })
                .unwrap_or_default()
        };
        self.send_role_order(&inst, space, moves, cx);
    }

    fn send_role_order(
        &self,
        inst: &InstanceId,
        space: Id,
        moves: Vec<RolePosition>,
        cx: &mut Context<Self>,
    ) {
        if moves.is_empty() {
            return;
        }
        self.call_status(
            inst,
            "Roles reordered",
            move |r| async move { r.reorder_roles(space, &moves).await },
            cx,
        );
    }

    /// A role was dropped on another.
    pub fn drop_role(&mut self, dragged: Item, target: Item, cx: &mut Context<Self>) {
        let moves = {
            let store = self.core.store();
            store.instance(&dragged.inst).and_then(|s| {
                if !view::can_manage_role(s, dragged.space, dragged.id) {
                    return None;
                }
                s.spaces
                    .get(&dragged.space)
                    .map(|st| reorder_roles(&st.roles, dragged.id, target.id))
            })
        };
        self.send_role_order(&dragged.inst, dragged.space, moves.unwrap_or_default(), cx);
    }

    // ---- channels -----------------------------------------------------------------------------

    pub fn select_settings_channel(
        &mut self,
        channel: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((inst, space)) = self.open_space_page.clone() else {
            return;
        };
        let (first, roles) = {
            let store = self.core.store();
            let s = store.instance(&inst);
            let first = s
                .and_then(|s| s.channel(channel))
                .and_then(|c| c.overrides.first().map(|o| o.role_id));
            let roles: Vec<(Id, String)> = s
                .and_then(|s| s.spaces.get(&space))
                .map(|st| st.roles.iter().map(|r| (r.id, r.name.clone())).collect())
                .unwrap_or_default();
            (first, roles)
        };
        let current = first.unwrap_or(space);
        // The roles whose overrides can be shown; which have some is said under it, so it
        // stays right as overrides come and go.
        let choices: Vec<DeviceChoice> = roles
            .iter()
            .map(|(id, name)| DeviceChoice {
                value: id.to_string(),
                name: SharedString::from(name.clone()),
            })
            .collect();
        let selected = roles.iter().position(|(id, _)| *id == current).unwrap_or(0);
        let state = cx.new(|cx| {
            SelectState::new(
                choices,
                Some(gpui_kit::component::IndexPath::new(selected)),
                window,
                cx,
            )
        });
        let events = cx.subscribe(
            &state,
            |this, _, event: &SelectEvent<Vec<DeviceChoice>>, cx| {
                if let SelectEvent::Confirm(Some(value)) = event
                    && let Ok(role) = value.parse::<u64>()
                {
                    if let Some(page) = this.open_page() {
                        page.override_role = Some(Id::from(role));
                    }
                    cx.notify();
                }
            },
        );
        if let Some(page) = self.open_page() {
            page.channel = Some(channel);
            page.override_role = Some(current);
            page.override_select = Some((state, events));
        }
        cx.notify();
    }

    fn set_override(&mut self, flag: Permissions, tri: Tri, cx: &mut Context<Self>) {
        let Some(key) = self.open_space_page.clone() else {
            return;
        };
        let Some((channel, role)) = self
            .space_pages
            .get(&key)
            .and_then(|p| Some((p.channel?, p.override_role?)))
        else {
            return;
        };
        let inst = key.0;
        let current = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| s.channel(channel))
                .and_then(|c| c.overrides.iter().find(|o| o.role_id == role).copied())
        };
        let (allow, deny) = current.map_or_else(Default::default, |o| (o.allow, o.deny));
        let (allow, deny) = tri.apply(allow, deny, flag);
        if allow.is_empty() && deny.is_empty() {
            self.call_status(
                &inst,
                "Permission reset",
                move |r| async move { r.delete_override(channel, role).await },
                cx,
            );
            return;
        }
        let req = OverrideRequest { allow, deny };
        let text = match tri {
            Tri::Allow => "Allowed",
            Tri::Deny => "Denied",
            Tri::Inherit => "Permission reset",
        };
        self.call_status(
            &inst,
            text,
            move |r| async move { r.set_override(channel, role, &req).await },
            cx,
        );
    }

    fn move_channel_step(&mut self, channel: Id, delta: i32, cx: &mut Context<Self>) {
        let Some((inst, space)) = self.open_space_page.clone() else {
            return;
        };
        let moves = {
            let store = self.core.store();
            store
                .instance(&inst)
                .map(|s| view::step_channel(s, space, channel, delta))
                .unwrap_or_default()
        };
        if moves.is_empty() {
            return;
        }
        self.call_status(
            &inst,
            "Channel moved",
            move |r| async move { r.reorder_channels(space, &moves).await },
            cx,
        );
    }

    // ---- members ------------------------------------------------------------------------------

    fn set_member_role(&mut self, user: Id, role: Id, on: bool, cx: &mut Context<Self>) {
        let Some((inst, space)) = self.open_space_page.clone() else {
            return;
        };
        let roles = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| s.member(space, user))
                .map(|m| m.role_ids.clone())
        };
        let Some(mut roles) = roles else {
            return;
        };
        roles.retain(|r| *r != role);
        if on {
            roles.push(role);
        }
        let req = UpdateMemberRequest {
            nickname: None,
            role_ids: Some(roles),
        };
        self.call_status(
            &inst,
            if on { "Role given" } else { "Role taken away" },
            move |r| async move { r.update_member(space, Some(user), &req).await },
            cx,
        );
    }

    fn patch_voice(
        &mut self,
        user: Id,
        req: VoiceStatePatchRequest,
        ok: &str,
        cx: &mut Context<Self>,
    ) {
        let Some((inst, space)) = self.open_space_page.clone() else {
            return;
        };
        self.call_status(
            &inst,
            ok,
            move |r| async move { r.update_voice_state(space, user, &req).await },
            cx,
        );
    }

    /// Menu actions that open or act on the space settings. `false` if this is not one.
    pub fn run_space_action(
        &mut self,
        target: &Target,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match (target.clone(), action) {
            (Target::Space(inst, space), Action::SpaceSettings) => {
                self.open_space_settings(inst, space, Tab::Overview, window, cx);
            }
            (Target::Channel(inst, channel), Action::ChannelPermissions) => {
                let space = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.space_of(channel));
                if let Some(space) = space {
                    self.open_space_settings(inst, space, Tab::Channels, window, cx);
                    self.select_settings_channel(channel, window, cx);
                }
            }
            (Target::Member(inst, Some(space), user), Action::ManageRoles) => {
                self.open_space_settings(inst, space, Tab::Members, window, cx);
                if let Some(page) = self.open_page() {
                    page.member = Some(user);
                }
            }
            (Target::Role(inst, space, role), Action::EditRole) => {
                self.open_space_settings(inst, space, Tab::Roles, window, cx);
                self.select_role(role, window, cx);
            }
            (Target::Role(inst, space, role), Action::DeleteRole) => {
                if let Some(page) = self.space_pages.get_mut(&(inst.clone(), space))
                    && page.role.as_ref().is_some_and(|r| r.id == role)
                {
                    page.role = None;
                }
                self.call_status(
                    &inst,
                    "Role deleted",
                    move |r| async move { r.delete_role(space, role).await },
                    cx,
                );
            }
            (Target::Invite(inst, _, code), Action::CopyInviteLink) => {
                if let Some(link) = self.core.invite_link(&inst, &code) {
                    self.copy(link, "Invite link", cx);
                }
            }
            (Target::Invite(inst, space, code), Action::RevokeInvite) => {
                let core = self.core.clone();
                let target = inst.clone();
                self.run(
                    cx,
                    async move {
                        core.rest(&target)?.delete_invite(&code).await?;
                        Ok::<_, CoreError>(())
                    },
                    move |this, result, _, cx| match result {
                        Ok(()) => this.invite_created(inst, space, cx),
                        Err(err) => this.error(format!("could not revoke: {err}"), cx),
                    },
                );
            }
            _ => return false,
        }
        true
    }

    // ---- drawing ------------------------------------------------------------------------------

    pub fn render_space_settings(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let key = self.open_space_page.as_ref()?;
        let page = self.space_pages.get(key)?;
        let store = self.core.store();
        let s = store.instance(&page.inst)?;
        let st = s.spaces.get(&page.space)?;
        let nav: Vec<AnyElement> = Tab::available(s, page.space)
            .into_iter()
            .map(|tab| {
                crate::ui::nav_item(
                    self,
                    tab.icon(),
                    tab.title(),
                    page.tab == tab,
                    format!("space-tab-{}", tab.title()),
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_space_tab(tab, false, cx)))
                .into_any_element()
            })
            .collect();
        let content = match page.tab {
            Tab::Overview => self.tab_overview(page, s, cx),
            Tab::Roles => self.tab_roles(page, s, cx),
            Tab::Channels => self.tab_channels(page, s, cx),
            Tab::Members => self.tab_members(page, s, cx),
            Tab::Encryption => self.tab_encryption(page, s, cx),
            Tab::Invites => self.tab_invites(page, s, cx),
            Tab::Bans => self.tab_bans(page, s, cx),
            Tab::Reports => self.tab_reports(page, s, cx),
            Tab::Audit => self.tab_audit(page, s, cx),
        };
        Some(frame(
            st.space.name.clone(),
            nav,
            crate::theme::enter(
                gpui_kit::SharedString::from(format!("space-settings-{:?}", page.tab)),
                content,
            ),
            cx.listener(|this, _, _, cx| this.close_space_settings(cx)),
        ))
    }

    fn tab_overview(
        &self,
        page: &SpacePage,
        s: &InstanceStore,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let st = &s.spaces[&page.space];
        let (inst, space) = (page.inst.clone(), page.space);
        let editable = view::can_in_space(s, space, Permissions::MANAGE_SPACE);
        let is_owner = Some(st.space.owner_id) == s.me_id();

        // The icon on the left, with its buttons under it; the name and description beside it.
        let has_icon = st.space.icon.is_some();
        let marker = st.space.icon.as_ref().and_then(|file| {
            self.images
                .profile_marker(&inst, file, crate::images::AVATAR_PIXELS)
        });
        // Read from the image cache, not through the store again (it is held here).
        let picture = st.space.icon.as_ref().and_then(|icon| {
            match self.images.avatars.get(&(inst.clone(), icon.id)) {
                Some(crate::images::Img::Ready(image)) => Some(image.shown(self.animating())),
                _ => None,
            }
        });
        const ICON: f32 = 88.;
        let initial = st
            .space
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default();
        let icon_box = div()
            .relative()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(px(ICON))
            .rounded(theme::radius_large())
            .bg(theme::accent_soft())
            .text_size(px(32.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme::accent())
            .overflow_hidden()
            .children(marker)
            .child(match picture {
                Some(picture) => gpui_kit::img(picture)
                    .size(px(ICON))
                    .object_fit(gpui_kit::ObjectFit::Cover)
                    .id("space-icon-picture")
                    .into_any_element(),
                None => div().child(initial).into_any_element(),
            });
        let (pick_inst, remove_inst) = (inst.clone(), inst.clone());
        let icon_column = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .flex_shrink_0()
            .child(icon_box)
            .when(editable, |d| {
                d.child(crate::ui::button(
                    self,
                    "space-icon",
                    if has_icon { "Change" } else { "Upload" },
                    ButtonKind::Secondary,
                    true,
                    move |this, _, cx| {
                        this.pick_space_icon(pick_inst.clone(), space, cx);
                    },
                    cx,
                ))
                .when(has_icon, |d| {
                    d.child(crate::ui::button(
                        self,
                        "space-icon-remove",
                        "Remove",
                        ButtonKind::Secondary,
                        true,
                        move |this, _, cx| {
                            let req = UpdateSpaceRequest {
                                icon_id: Some(None),
                                ..UpdateSpaceRequest::default()
                            };
                            this.call_status(
                                &remove_inst,
                                "Icon removed",
                                move |r| async move { r.update_space(space, &req).await },
                                cx,
                            );
                        },
                        cx,
                    ))
                })
            });
        let fields = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(240.))
            .child(setting_below(
                "Server name",
                "",
                div()
                    .w_full()
                    .child(Input::new(&page.name).disabled(!editable)),
            ))
            .child(setting_below(
                "Description",
                "Shown on invite pages.",
                div()
                    .w_full()
                    .child(Input::new(&page.description).disabled(!editable)),
            ))
            // What the icon may be, under the fields rather than squeezed under the icon.
            .when(editable, |d| {
                d.child(muted(format!(
                    "Icon: {}",
                    crate::crop::CropFor::SpaceIcon(inst.clone(), space).rules()
                )))
            });
        let mut rows = vec![
            div()
                .flex()
                .flex_wrap()
                .items_start()
                .gap(px(24.))
                .py(px(12.))
                .child(icon_column)
                .child(fields)
                .into_any_element(),
        ];
        if editable && page.overview_dirty {
            let save_key = (inst.clone(), space);
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .py(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(theme::secondary_size())
                            .text_color(theme::warning())
                            .child("You have unsaved changes."),
                    )
                    .child(crate::ui::button(
                        self,
                        "space-save",
                        "Save changes",
                        ButtonKind::Primary,
                        true,
                        move |this, _, cx| {
                            this.save_overview(save_key.clone(), cx);
                        },
                        cx,
                    ))
                    .into_any_element(),
            );
        }
        let members = st.members.len();
        let channels = st
            .channels
            .values()
            .filter(|c| c.kind.has_messages())
            .count();
        let owner = s.display_name(Some(space), st.space.owner_id);
        let stat = |label: &'static str, value: String| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(120.))
                .gap(px(2.))
                .px(px(14.))
                .py(px(10.))
                .rounded(theme::radius_small())
                .bg(theme::background())
                .child(
                    div()
                        .text_size(theme::caption_size())
                        .text_color(theme::text_tertiary())
                        .child(label),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(theme::body_size())
                        .font_weight(FontWeight::MEDIUM)
                        .child(value),
                )
        };
        let mut about = vec![
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .py(px(12.))
                .child(stat("Owner", owner))
                .child(stat("Members", members.to_string()))
                .child(stat("Channels", channels.to_string()))
                .into_any_element(),
            setting_fit(
                "Server ID",
                "For bug reports and bots.",
                crate::ui::button(
                    self,
                    "space-copy-id",
                    self.copy_label("space-copy-id", "Copy"),
                    ButtonKind::Secondary,
                    true,
                    move |this, _, cx| {
                        this.copy_from("space-copy-id", space.to_string(), "Server ID", cx)
                    },
                    cx,
                ),
            ),
        ];
        if view::can_in_space(s, space, Permissions::CREATE_INVITES) {
            let invite_inst = inst.clone();
            about.push(setting_fit(
                "Invite people",
                "Creates a link that works for 7 days and copies it.",
                crate::ui::button(
                    self,
                    "space-invite",
                    "Copy invite link",
                    ButtonKind::Primary,
                    true,
                    move |this, window, cx| {
                        this.run_action(
                            Target::Space(invite_inst.clone(), space),
                            Action::InvitePeople,
                            window,
                            cx,
                        );
                    },
                    cx,
                ),
            ));
        }
        let danger_inst = inst.clone();
        let danger = if is_owner {
            setting_fit(
                "Delete server",
                "Deletes every channel and message for everyone. This cannot be undone. To keep \
                 the space, give it to someone else from the Members tab first.",
                crate::ui::button(
                    self,
                    "space-delete",
                    "Delete server",
                    ButtonKind::Danger,
                    true,
                    move |this, _, cx| {
                        this.cancel_closing("confirm");
                        this.confirm = Some((
                            Target::Space(danger_inst.clone(), space),
                            Action::DeleteSpace,
                        ));
                        cx.notify();
                    },
                    cx,
                ),
            )
        } else {
            setting_fit(
                "Leave server",
                "You will need a new invite to come back.",
                crate::ui::button(
                    self,
                    "space-leave",
                    "Leave server",
                    ButtonKind::Danger,
                    true,
                    move |this, _, cx| {
                        this.cancel_closing("confirm");
                        this.confirm = Some((
                            Target::Space(danger_inst.clone(), space),
                            Action::LeaveSpace,
                        ));
                        cx.notify();
                    },
                    cx,
                ),
            )
        };
        page_body(
            "Overview",
            (!editable).then_some("You need the Manage server permission to change these."),
            vec![
                group("Server profile", rows),
                group("About", about),
                group("Danger zone", vec![danger]),
            ],
        )
    }

    fn tab_roles(&self, page: &SpacePage, s: &InstanceStore, cx: &mut Context<Self>) -> AnyElement {
        let st = &s.spaces[&page.space];
        if let Some(draft) = page.role.as_ref().filter(|d| st.role(d.id).is_some()) {
            return self.role_editor(page, s, draft, cx);
        }
        let space = page.space;
        let can_manage = view::can_in_space(s, space, Permissions::MANAGE_ROLES);
        // How many members have each role, in one pass over the members.
        let mut counts = std::collections::HashMap::<Id, usize>::new();
        for role in st.members.values().flat_map(|m| &m.role_ids) {
            *counts.entry(*role).or_default() += 1;
        }
        let rows: Vec<AnyElement> = st
            .roles
            .iter()
            .map(|role| {
                let count = if role.is_everyone() {
                    st.members.len()
                } else {
                    counts.get(&role.id).copied().unwrap_or_default()
                };
                let id = role.id;
                let draggable = !role.is_everyone() && view::can_manage_role(s, space, role.id);
                let element = list_row(self, format!("role-{id}"), false, cx)
                    .h(px(40.))
                    .child(div().w(px(14.)).flex_shrink_0().when(draggable, |d| {
                        d.child(icon(assets::DRAG, 14., theme::text_tertiary()))
                    }))
                    .child(
                        div()
                            .size(px(10.))
                            .flex_shrink_0()
                            .rounded(px(5.))
                            .bg(role.color.map_or(theme::text_tertiary(), theme::rgb_u32)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(role.name.clone()))
                    .child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(format!("{count} member{}", plural(count))),
                    )
                    .child(opens())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_role(id, window, cx);
                    }));
                let element = if draggable {
                    self.with_list_drag(element, page, Kind::Role, id, role.name.clone(), cx)
                } else {
                    element
                };
                self.with_menu(element, Target::Role(page.inst.clone(), space, id), cx)
                    .into_any_element()
            })
            .collect();
        let create = can_manage.then(|| {
            crate::ui::button(
                self,
                "role-create",
                "Create role",
                ButtonKind::Secondary,
                true,
                |this, _, cx| this.create_role(cx),
                cx,
            )
            .into_any_element()
        });
        page_body_with(
            "Roles",
            create,
            Some(
                "Roles give members permissions: each of their roles' and @everyone's. Higher \
                 roles can manage lower ones; drag a role to change the order.",
            ),
            vec![list_card(rows)],
        )
    }

    fn role_editor(
        &self,
        page: &SpacePage,
        s: &InstanceStore,
        draft: &RoleDraft,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let st = &s.spaces[&page.space];
        let space = page.space;
        let role = st.role(draft.id).expect("checked by the caller");
        let id = role.id;
        let everyone = role.is_everyone();
        let editable = view::can_manage_role(s, space, id);
        // You cannot grant what you do not have (the owner and administrators have everything).
        let mine = s.permissions(space, None);
        let mut col: Vec<AnyElement> = Vec::new();
        let title = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .size(px(12.))
                    .flex_shrink_0()
                    .rounded(px(6.))
                    .bg(role.color.map_or(theme::text_tertiary(), theme::rgb_u32)),
            )
            .child(heading(if everyone {
                "@everyone".to_owned()
            } else {
                role.name.clone()
            }))
            .into_any_element();
        if everyone {
            col.push(muted("What every member can do."));
        }
        if !editable {
            col.push(
                div()
                    .text_size(theme::caption_size())
                    .text_color(theme::warning())
                    .child(
                        "You can only edit roles below your own highest role, with Manage roles.",
                    )
                    .into_any_element(),
            );
        }
        let mut role_rows: Vec<AnyElement> = Vec::new();
        if !everyone {
            let name_row = setting_below(
                "Name",
                "",
                div()
                    .w_full()
                    .child(Input::new(&draft.name).disabled(!editable)),
            );
            let color_text = draft.color.read(cx).value().to_string();
            let parsed = parse_color(&color_text);
            let color_input = draft.color.clone();
            let swatches = SWATCHES
                .into_iter()
                .enumerate()
                .map(|(n, color)| {
                    let chosen = parsed == Ok(Some(color));
                    let input = color_input.clone();
                    div()
                        .id(("swatch", n))
                        .size(px(20.))
                        .rounded(px(10.))
                        .bg(theme::rgb_u32(color))
                        .border_2()
                        .border_color(if chosen {
                            theme::text()
                        } else {
                            gpui_kit::rgba(0)
                        })
                        .when(editable, |d| d.cursor_pointer())
                        .on_click(cx.listener(move |_, _, window, cx| {
                            if editable {
                                input.update(cx, |input, cx| {
                                    input.set_value(color_hex(color), window, cx);
                                });
                            }
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            role_rows.push(setting_pair(
                name_row,
                setting_below(
                    "Color",
                    if parse_color(&color_text).is_err() {
                        "Use six hex digits, like #3498DB."
                    } else {
                        "Names show in their highest colored role's color."
                    },
                    div()
                        .flex()
                        .flex_col()
                        .w_full()
                        .gap(px(8.))
                        .child(Input::new(&draft.color).disabled(!editable))
                        .child(div().flex().flex_wrap().gap(px(6.)).children(swatches)),
                ),
            ));
            role_rows.push(setting_fit(
                "Show separately",
                "Members with this role get their own group in the member list.",
                {
                    let on = draft.hoist;
                    crate::ui::switch(
                        self,
                        "role-hoist",
                        on,
                        editable,
                        move |this, _, cx| {
                            if let Some(d) = this.open_page().and_then(|p| p.role.as_mut()) {
                                d.hoist = !on;
                            }
                            cx.notify();
                        },
                        cx,
                    )
                },
            ));
            role_rows.push(setting_fit(
                "Mentionable",
                "Anyone can @mention this role.",
                {
                    let on = draft.mentionable;
                    crate::ui::switch(
                        self,
                        "role-mention",
                        on,
                        editable,
                        move |this, _, cx| {
                            if let Some(d) = this.open_page().and_then(|p| p.role.as_mut()) {
                                d.mentionable = !on;
                            }
                            cx.notify();
                        },
                        cx,
                    )
                },
            ));
            if editable {
                role_rows.push(setting_fit(
                    "Position",
                    "Higher roles can manage lower ones.",
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(crate::ui::button(
                            self,
                            "role-up",
                            "Move up",
                            ButtonKind::Secondary,
                            true,
                            move |this, _, cx| this.move_role(id, -1, cx),
                            cx,
                        ))
                        .child(crate::ui::button(
                            self,
                            "role-down",
                            "Move down",
                            ButtonKind::Secondary,
                            true,
                            move |this, _, cx| this.move_role(id, 1, cx),
                            cx,
                        )),
                ));
            }
            col.push(group("Role", role_rows));
        }
        for permission_group in PermissionGroup::ALL {
            let rows: Vec<AnyElement> = CATALOG
                .iter()
                .filter(|i| i.group == permission_group)
                .map(|info| {
                    let on = draft.permissions.contains(info.flag);
                    let grantable = editable && (on || mine.contains(info.flag));
                    let flag = info.flag;
                    setting_fit(
                        info.name,
                        info.description,
                        crate::ui::switch(
                            self,
                            format!("perm-{}", info.name),
                            on,
                            grantable,
                            move |this, _, cx| {
                                if let Some(d) = this.open_page().and_then(|p| p.role.as_mut()) {
                                    d.permissions.set(flag, !on);
                                }
                                cx.notify();
                            },
                            cx,
                        ),
                    )
                })
                .collect();
            col.push(group(permission_group.title(), rows));
        }
        let draft_name = draft.name.read(cx).value().trim().to_owned();
        let color_ok = parse_color(&draft.color.read(cx).value());
        let dirty = draft_name != role.name
            || color_ok != Ok(role.color)
            || draft.hoist != role.hoist
            || draft.mentionable != role.mentionable
            || draft.permissions != role.permissions;
        let valid = !draft_name.is_empty() && color_ok.is_ok();
        let mut buttons = div().flex().flex_wrap().items_center().gap(px(8.));
        if editable && !everyone {
            let inst = page.inst.clone();
            buttons = buttons.child(crate::ui::button(
                self,
                "role-delete",
                "Delete role",
                ButtonKind::Danger,
                true,
                move |this, _, cx| {
                    this.confirm =
                        Some((Target::Role(inst.clone(), space, id), Action::DeleteRole));
                    cx.notify();
                },
                cx,
            ));
        }
        if editable && dirty {
            buttons = buttons
                .child(crate::ui::button(
                    self,
                    "role-reset",
                    "Reset",
                    ButtonKind::Secondary,
                    true,
                    move |this, window, cx| this.select_role(id, window, cx),
                    cx,
                ))
                .when(valid, |d| {
                    d.child(crate::ui::button(
                        self,
                        "role-save",
                        "Save changes",
                        ButtonKind::Primary,
                        true,
                        |this, _, cx| this.save_role(cx),
                        cx,
                    ))
                });
        }
        item_page(
            self.back_line("Roles", |page| page.role = None, cx),
            title,
            buttons.into_any_element(),
            col,
        )
    }

    fn tab_channels(
        &self,
        page: &SpacePage,
        s: &InstanceStore,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let space = page.space;
        if let Some(channel) = page.channel.and_then(|id| s.channel(id))
            && channel.space_id == Some(space)
        {
            return self.channel_detail(page, s, channel.id, cx);
        }
        let inst = page.inst.clone();
        let manage = view::can_in_space(s, space, Permissions::MANAGE_CHANNELS);
        let tree = view::channel_tree(s, space);
        // What each row is, for where a drop near it lands: a category is open when channels
        // follow it (this list never folds).
        {
            let mut shapes = self.sidebar_order.borrow_mut();
            shapes.clear();
            for (n, c) in tree.iter().enumerate() {
                let shape = match c.kind {
                    ChannelKind::Category => Shape::Category {
                        open: tree.get(n + 1).is_some_and(|next| next.depth > 0),
                    },
                    _ => Shape::Channel,
                };
                shapes.insert(c.id, (shape, c.depth > 0));
            }
        }
        let rows: Vec<AnyElement> = tree
            .into_iter()
            .map(|c| {
                let id = c.id;
                let glyph = match c.kind {
                    ChannelKind::Category => assets::CHEVRON_DOWN,
                    ChannelKind::Voice => assets::VOLUME,
                    _ => assets::POUND,
                };
                let category = c.kind == ChannelKind::Category;
                let element = list_row(self, format!("settings-channel-{id}"), false, cx)
                    .h(px(36.))
                    .when(c.depth > 0, |d| d.pl(px(28.)))
                    .child(icon(glyph, 16., theme::text_tertiary()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(category, |d| {
                                d.text_size(theme::caption_size())
                                    .text_color(theme::category_text())
                            })
                            .child(if category {
                                c.name.to_uppercase()
                            } else {
                                c.name.clone()
                            }),
                    )
                    .child(opens())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_settings_channel(id, window, cx);
                    }));
                let element = if manage {
                    let item = Item {
                        inst: page.inst.clone(),
                        space,
                        kind: Kind::Channel,
                        id,
                    };
                    self.channel_drag(element, item, c.name.clone(), cx)
                } else {
                    element
                };
                self.with_menu(element, Target::Channel(inst.clone(), id), cx)
                    .into_any_element()
            })
            .collect();
        let actions = manage.then(|| {
            let (channel_inst, category_inst) = (inst.clone(), inst.clone());
            // Not `settings::wrap()`: that is as wide as its parent, and it squeezed the title
            // to a letter per line.
            div()
                .flex()
                .gap(px(8.))
                .child(crate::ui::button(
                    self,
                    "channel-create",
                    "+ Channel",
                    ButtonKind::Secondary,
                    true,
                    move |this, window, cx| {
                        this.open_form_create_channel(
                            channel_inst.clone(),
                            space,
                            None,
                            window,
                            cx,
                        );
                    },
                    cx,
                ))
                .child(crate::ui::button(
                    self,
                    "category-create",
                    "+ Category",
                    ButtonKind::Secondary,
                    true,
                    move |this, window, cx| {
                        this.open_form_create_category(category_inst.clone(), space, window, cx);
                    },
                    cx,
                ))
                .into_any_element()
        });
        let rows = if manage {
            // The whole list takes a dragged channel, gaps and the space under it too.
            let list = div()
                .id("settings-channel-list")
                .flex()
                .flex_col()
                .flex_grow(1.)
                .gap(px(1.))
                .children(rows)
                .child(self.channel_end_drop(
                    div().id("channel-list-end").flex_grow(1.).min_h(px(8.)),
                    cx,
                ));
            vec![
                self.channel_list_drop(list, inst.clone(), space, cx)
                    .into_any_element(),
            ]
        } else {
            rows
        };
        page_body_with(
            "Channels",
            actions,
            Some(if manage {
                "Drag channels to reorder them, or onto a category to move them into it. Open \
                 one to change what each role may do in it."
            } else {
                "Open a channel to see what each role may do in it. Changing them needs the \
                 Manage channels permission."
            }),
            vec![list_card(rows)],
        )
    }

    fn channel_detail(
        &self,
        page: &SpacePage,
        s: &InstanceStore,
        id: Id,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let channel = s.channel(id).expect("checked by the caller");
        let st = &s.spaces[&page.space];
        let space = page.space;
        let inst = page.inst.clone();
        // Reordering is checked space-wide on the server, editing and deleting in the channel.
        let manage = view::can_in_space(s, space, Permissions::MANAGE_CHANNELS);
        let manage_here = view::can(s, id, Permissions::MANAGE_CHANNELS);
        let category = channel.kind == ChannelKind::Category;
        let kind = match channel.kind {
            ChannelKind::Category => "Category",
            ChannelKind::Voice => "Voice channel",
            _ => "Text channel",
        };
        let glyph = match channel.kind {
            ChannelKind::Category => assets::CHEVRON_DOWN,
            ChannelKind::Voice => assets::VOLUME,
            _ => assets::POUND,
        };
        let title = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(icon(glyph, 20., theme::text_tertiary()))
            .child(heading(channel.name.clone().unwrap_or_default()))
            .into_any_element();
        let about = match channel.topic.as_deref() {
            Some(topic) if !topic.is_empty() => format!("{kind} · {topic}"),
            _ => kind.to_owned(),
        };
        let mut col: Vec<AnyElement> = Vec::new();
        let actions = if manage || manage_here {
            let (edit_inst, here_inst, delete_inst) = (inst.clone(), inst.clone(), inst.clone());
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .when(manage_here, |d| {
                    d.child(crate::ui::button(
                        self,
                        "channel-edit",
                        "Edit…",
                        ButtonKind::Secondary,
                        true,
                        move |this, window, cx| {
                            this.open_form_edit_channel(edit_inst.clone(), id, window, cx);
                        },
                        cx,
                    ))
                })
                .when(manage, |d| {
                    d.child(crate::ui::button(
                        self,
                        "channel-up",
                        "Move up",
                        ButtonKind::Secondary,
                        true,
                        move |this, _, cx| this.move_channel_step(id, -1, cx),
                        cx,
                    ))
                    .child(crate::ui::button(
                        self,
                        "channel-down",
                        "Move down",
                        ButtonKind::Secondary,
                        true,
                        move |this, _, cx| this.move_channel_step(id, 1, cx),
                        cx,
                    ))
                })
                .when(category && manage, |d| {
                    d.child(crate::ui::button(
                        self,
                        "channel-here",
                        "+ Channel here",
                        ButtonKind::Secondary,
                        true,
                        move |this, window, cx| {
                            this.open_form_create_channel(
                                here_inst.clone(),
                                space,
                                Some(id),
                                window,
                                cx,
                            );
                        },
                        cx,
                    ))
                })
                .when(manage_here, |d| {
                    d.child(crate::ui::button(
                        self,
                        "channel-delete",
                        if category {
                            "Delete category"
                        } else {
                            "Delete channel"
                        },
                        ButtonKind::Danger,
                        true,
                        move |this, _, cx| {
                            this.cancel_closing("confirm");
                            this.confirm = Some((
                                Target::Channel(delete_inst.clone(), id),
                                Action::DeleteChannel,
                            ));
                            cx.notify();
                        },
                        cx,
                    ))
                })
                .into_any_element()
        } else {
            div().into_any_element()
        };
        col.push(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(muted(about))
                .child(muted(if category {
                    "What each role may do in this category: \u{2715} denies, / follows the \
                     role's normal permissions, \u{2713} allows. Channels inside it keep their \
                     own settings."
                } else {
                    "What each role may do in this channel: \u{2715} denies it here, / follows \
                     the role's normal permissions, \u{2713} allows it here."
                }))
                .into_any_element(),
        );
        // Every role can be picked from the dropdown; the ones with overrides here are named
        // under it.
        let current = page.override_role;
        let with_overrides: Vec<String> = st
            .roles
            .iter()
            .filter(|r| channel.overrides.iter().any(|o| o.role_id == r.id))
            .map(|r| r.name.clone())
            .collect();
        if let Some(role_id) = current.filter(|r| st.role(*r).is_some()) {
            let ov = channel.overrides.iter().find(|o| o.role_id == role_id);
            let role_name = st.role(role_id).map(|r| r.name.clone()).unwrap_or_default();
            let access = override_access(s, space, id, role_id);
            // Some toggles are off because they would allow or deny what I do not have.
            let mut limited = false;
            let clear_inst = inst.clone();
            let _ = &role_name;
            let picker = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .children(
                    page.override_select
                        .as_ref()
                        .map(|(state, _)| crate::ui::dropdown(state, crate::ui::DropWidth::Medium)),
                )
                .when(access.is_some() && ov.is_some(), |d| {
                    d.child(crate::ui::button(
                        self,
                        "override-clear",
                        "Remove override",
                        ButtonKind::Secondary,
                        true,
                        move |this, _, cx| {
                            this.call_status(
                                &clear_inst,
                                "Override removed",
                                move |r| async move { r.delete_override(id, role_id).await },
                                cx,
                            );
                        },
                        cx,
                    ))
                });
            col.push(group(
                "Permissions",
                vec![setting_fit(
                    "Role",
                    &if with_overrides.is_empty() {
                        "No role has overrides here yet.".to_owned()
                    } else {
                        format!("Overrides here: {}", with_overrides.join(", "))
                    },
                    picker,
                )],
            ));
            for permission_group in PermissionGroup::ALL {
                let entries: Vec<_> = CATALOG
                    .iter()
                    .filter(|i| i.group == permission_group && i.channel)
                    // Text channels do not use voice bits.
                    .filter(|i| {
                        channel.kind != ChannelKind::Text || i.group != PermissionGroup::Voice
                    })
                    .collect();
                if entries.is_empty() {
                    continue;
                }
                let rows = entries
                    .into_iter()
                    .map(|info| {
                        let state = Tri::of(ov, info.flag);
                        let flag = info.flag;
                        // Short: a cross, a slash and a tick (as the line above explains),
                        // so a long column of them does not run across the card.
                        const TRIS: [Tri; 3] = [Tri::Deny, Tri::Inherit, Tri::Allow];
                        let options = TRIS
                            .iter()
                            .map(|tri| {
                                let allowed = override_change_allowed(access, ov, flag, *tri);
                                limited |= access.is_some() && !allowed;
                                match tri {
                                    Tri::Deny => crate::ui::Segment::with(
                                        crate::ui::icon_preview(assets::CLOSE),
                                        "",
                                    ),
                                    Tri::Inherit => crate::ui::Segment::new("/"),
                                    Tri::Allow => crate::ui::Segment::with(
                                        crate::ui::icon_preview(assets::CHECK),
                                        "",
                                    ),
                                }
                                .enabled(allowed)
                            })
                            .collect();
                        setting_fit(
                            info.name,
                            info.description,
                            crate::ui::segmented(
                                self,
                                format!("override-{}", info.name),
                                options,
                                TRIS.iter().position(|t| *t == state),
                                true,
                                move |this, n, _, cx| this.set_override(flag, TRIS[n], cx),
                                cx,
                            ),
                        )
                    })
                    .collect();
                col.push(group(permission_group.title(), rows));
            }
            let have = s.channel_permissions(id);
            if !have.contains(Permissions::MANAGE_CHANNELS | Permissions::MANAGE_ROLES) {
                col.push(muted(
                    "Changing these needs both Manage channels and Manage roles.",
                ));
            } else if access.is_none() {
                col.push(muted(
                    "You can only change the overrides of @everyone, your own roles and roles \
                     below your highest one.",
                ));
            } else if limited {
                col.push(muted(
                    "You can only allow or deny permissions you have in this channel yourself.",
                ));
            }
        }
        item_page(
            self.back_line("Channels", |page| page.channel = None, cx),
            title,
            actions,
            col,
        )
    }

    fn tab_members(
        &self,
        page: &SpacePage,
        s: &InstanceStore,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let space = page.space;
        if let Some(m) = page.member.and_then(|u| s.member(space, u)) {
            return self.member_detail(page, s, m, cx);
        }
        let st = &s.spaces[&space];
        let query = page.member_query.read(cx).value().to_string();
        let members = filter_members(s, space, &query);
        let total = members.len();
        let shown = total.min(page.member_limit);
        let rows: Vec<AnyElement> = members
            .into_iter()
            .take(shown)
            .map(|(name, m)| {
                let user = m.user.id;
                let name_color = hermes_client_core::view::role_color(s, space, user)
                    .map_or(theme::text(), theme::rgb_u32);
                let roles: Vec<&str> = st
                    .roles
                    .iter()
                    .filter(|r| !r.is_everyone() && m.role_ids.contains(&r.id))
                    .map(|r| r.name.as_str())
                    .collect();
                list_row(self, format!("settings-member-{user}"), false, cx)
                    .h(px(52.))
                    .child(self.avatar(&page.inst, Some(&m.user), &name, 28.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().text_color(name_color).child(name))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(theme::caption_size())
                                    .text_color(theme::text_tertiary())
                                    .child(format!("@{}", m.user.username)),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink(1.)
                            .min_w_0()
                            .max_w(px(220.))
                            .truncate()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(roles.join(", ")),
                    )
                    .child(opens())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(page) = this.open_page() {
                            page.member = Some(user);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let search = div()
            .w(px(240.))
            .max_w_full()
            .child(Input::new(&page.member_query))
            .into_any_element();
        let mut parts = vec![if rows.is_empty() {
            muted("Nobody matches.")
        } else {
            list_card(rows)
        }];
        if shown < total {
            parts.push(centered(crate::ui::button(
                self,
                "members-more",
                format!("Show {} more", (total - shown).min(MEMBER_PAGE)),
                ButtonKind::Secondary,
                true,
                |this, _, cx| {
                    if let Some(page) = this.open_page() {
                        page.member_limit += MEMBER_PAGE;
                    }
                    cx.notify();
                },
                cx,
            )));
        }
        page_body_with(
            "Members",
            Some(search),
            Some(&if shown < total {
                format!("{total} member{}, {shown} shown", plural(total))
            } else {
                format!("{total} member{}", plural(total))
            }),
            parts,
        )
    }

    fn member_detail(
        &self,
        page: &SpacePage,
        s: &InstanceStore,
        m: &Member,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let st = &s.spaces[&page.space];
        let space = page.space;
        let inst = page.inst.clone();
        let user = m.user.id;
        let me = s.me_id() == Some(user);
        let above = view::outranks(s, space, user);
        let owner = st.space.owner_id == user;
        let is_owner = Some(st.space.owner_id) == s.me_id();
        let can = |p| view::can_in_space(s, space, p);
        let title = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(self.avatar(
                &inst,
                Some(&m.user),
                &s.display_name(Some(space), user),
                36.,
            ))
            .child(heading(s.display_name(Some(space), user)))
            .into_any_element();
        let mut col: Vec<AnyElement> = vec![
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(muted(format!(
                    "@{} · joined {}{}",
                    m.user.username,
                    time::format(m.joined_at, "%Y-%m-%d", self.utc_offset),
                    if owner { " · owner" } else { "" }
                )))
                .children(
                    m.user
                        .status_text
                        .as_deref()
                        .filter(|t| !t.is_empty())
                        .map(|status| muted(status.to_owned())),
                )
                .into_any_element(),
        ];
        let nick = if me {
            can(Permissions::CHANGE_NICKNAME) || can(Permissions::MANAGE_NICKNAMES)
        } else {
            above && can(Permissions::MANAGE_NICKNAMES)
        };
        let (dm_inst, nick_inst) = (inst.clone(), inst.clone());
        let actions = div()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .when(!me, |d| {
                d.child(crate::ui::button(
                    self,
                    "member-dm",
                    "Message",
                    ButtonKind::Primary,
                    true,
                    move |this, window, cx| {
                        this.close_space_settings(cx);
                        this.message_user(dm_inst.clone(), user, window, cx);
                    },
                    cx,
                ))
            })
            .when(nick, |d| {
                d.child(crate::ui::button(
                    self,
                    "member-nick",
                    "Change nickname…",
                    ButtonKind::Secondary,
                    true,
                    move |this, window, cx| {
                        this.open_form_nickname(nick_inst.clone(), space, user, window, cx);
                    },
                    cx,
                ))
            })
            .into_any_element();

        let role_rows: Vec<AnyElement> = st
            .roles
            .iter()
            .filter(|r| !r.is_everyone())
            .map(|role| {
                let has = m.role_ids.contains(&role.id);
                let may = view::can_manage_role(s, space, role.id) && (me || above || is_owner);
                let id = role.id;
                // A chip per role, filled while they have it: all of them fit in a few lines.
                let chip = crate::ui::chip(
                    self,
                    SharedString::from(format!("member-role-{id}")),
                    role.name.clone(),
                    has,
                    cx,
                );
                if may {
                    chip.on_click(cx.listener(move |this, _, _, cx| {
                        this.set_member_role(user, id, !has, cx);
                    }))
                    .into_any_element()
                } else {
                    chip.opacity(0.5).cursor_not_allowed().into_any_element()
                }
            })
            .collect();
        col.push(group(
            "Roles",
            vec![if role_rows.is_empty() {
                div()
                    .py(px(12.))
                    .child(muted(
                        "This server has no roles yet. Create them in the Roles tab.",
                    ))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .py(px(12.))
                    .children(role_rows)
                    .into_any_element()
            }],
        ));

        // Voice moderation, while they are in a voice channel of this space.
        if let Some(vs) = s
            .voice_states
            .get(&user)
            .filter(|v| s.space_of(v.channel_id) == Some(space))
        {
            let channel = s
                .channel(vs.channel_id)
                .and_then(|c| c.name.clone())
                .unwrap_or_default();
            let (server_mute, server_deaf) = (vs.server_mute, vs.server_deaf);
            let voice = div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .when(can(Permissions::MUTE_MEMBERS), |d| {
                    d.child(crate::ui::button(
                        self,
                        "member-mute",
                        if server_mute {
                            "Server unmute"
                        } else {
                            "Server mute"
                        },
                        ButtonKind::Secondary,
                        true,
                        move |this, _, cx| {
                            this.patch_voice(
                                user,
                                VoiceStatePatchRequest {
                                    server_mute: Some(!server_mute),
                                    ..VoiceStatePatchRequest::default()
                                },
                                if server_mute {
                                    "Server unmuted"
                                } else {
                                    "Server muted"
                                },
                                cx,
                            );
                        },
                        cx,
                    ))
                })
                .when(can(Permissions::DEAFEN_MEMBERS), |d| {
                    d.child(crate::ui::button(
                        self,
                        "member-deafen",
                        if server_deaf {
                            "Server undeafen"
                        } else {
                            "Server deafen"
                        },
                        ButtonKind::Secondary,
                        true,
                        move |this, _, cx| {
                            this.patch_voice(
                                user,
                                VoiceStatePatchRequest {
                                    server_deaf: Some(!server_deaf),
                                    ..VoiceStatePatchRequest::default()
                                },
                                if server_deaf {
                                    "Server undeafened"
                                } else {
                                    "Server deafened"
                                },
                                cx,
                            );
                        },
                        cx,
                    ))
                })
                .when(can(Permissions::MOVE_MEMBERS), |d| {
                    d.child(crate::ui::button(
                        self,
                        "member-disconnect",
                        "Disconnect",
                        ButtonKind::Danger,
                        true,
                        move |this, _, cx| {
                            this.patch_voice(
                                user,
                                VoiceStatePatchRequest {
                                    channel_id: Some(None),
                                    ..VoiceStatePatchRequest::default()
                                },
                                "Disconnected from voice",
                                cx,
                            );
                        },
                        cx,
                    ))
                });
            col.push(group(
                &format!("In voice: {channel}"),
                vec![voice.py(px(12.)).into_any_element()],
            ));
        }

        // Kick, ban and transfer go in a danger zone at the bottom, away from everyday buttons.
        let mut danger: Vec<AnyElement> = Vec::new();
        let target = Target::Member(inst.clone(), Some(space), user);
        if above && can(Permissions::KICK_MEMBERS) {
            let kick = target.clone();
            danger.push(setting_fit(
                "Kick",
                "Removes them from the server; they can come back with a new invite.",
                crate::ui::button(
                    self,
                    "member-kick",
                    "Kick",
                    ButtonKind::Danger,
                    true,
                    move |this, _, cx| {
                        this.cancel_closing("confirm");
                        this.confirm = Some((kick.clone(), Action::Kick));
                        cx.notify();
                    },
                    cx,
                ),
            ));
        }
        if above && can(Permissions::BAN_MEMBERS) {
            let ban_inst = inst.clone();
            danger.push(setting_fit(
                "Ban",
                "Removes them and keeps them out.",
                crate::ui::button(
                    self,
                    "member-ban",
                    "Ban…",
                    ButtonKind::Danger,
                    true,
                    move |this, window, cx| {
                        this.open_form_ban(ban_inst.clone(), space, user, window, cx);
                    },
                    cx,
                ),
            ));
        }
        if is_owner && !me {
            danger.push(setting_fit(
                "Transfer ownership",
                "Makes them the owner of the space. You keep your roles but can no longer delete \
                 the space.",
                crate::ui::button(
                    self,
                    "member-transfer",
                    "Transfer…",
                    ButtonKind::Danger,
                    true,
                    move |this, _, cx| {
                        this.cancel_closing("confirm");
                        this.confirm = Some((target.clone(), Action::TransferOwnership));
                        cx.notify();
                    },
                    cx,
                ),
            ));
        }
        if !danger.is_empty() {
            col.push(group("Danger zone", danger));
        }
        item_page(
            self.back_line("Members", |page| page.member = None, cx),
            title,
            actions,
            col,
        )
    }

    /// "Loading…", an error, or the empty text for a REST list; `None` once there are rows.
    fn list_placeholder<T>(
        &self,
        remote: &Remote<T>,
        empty: bool,
        empty_text: &'static str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let text = match remote {
            Remote::Idle | Remote::Loading => return Some(theme::skeleton("list", 4)),
            Remote::Failed(err) => format!("Could not load: {err}"),
            Remote::Ready(_) if empty => empty_text.to_owned(),
            Remote::Ready(_) => return None,
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(muted(text))
                .child(crate::ui::button(
                    self,
                    "list-refresh",
                    "Refresh",
                    ButtonKind::Secondary,
                    true,
                    |this, _, cx| {
                        if let Some(tab) = this.open_page().map(|p| p.tab) {
                            this.set_space_tab(tab, true, cx);
                        }
                    },
                    cx,
                ))
                .into_any_element(),
        )
    }

    fn tab_invites(
        &self,
        page: &SpacePage,
        s: &InstanceStore,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let space = page.space;
        let inst = page.inst.clone();
        let manage = view::can_in_space(s, space, Permissions::MANAGE_INVITES);
        let create_inst = inst.clone();
        let create = view::can_in_space(s, space, Permissions::CREATE_INVITES).then(|| {
            crate::ui::button(
                self,
                "invite-create",
                "Create invite…",
                ButtonKind::Primary,
                true,
                move |this, window, cx| {
                    this.open_form_invite(create_inst.clone(), space, window, cx);
                },
                cx,
            )
            .into_any_element()
        });
        let text = Some("Anyone with a valid link can join.");
        let mut rows: Vec<AnyElement> = Vec::new();
        if !manage {
            rows.push(muted(
                "Seeing everyone's invites needs the Manage invites permission.",
            ));
            return page_body_with("Invites", create, text, rows);
        }
        let list = page.invites.ready();
        if let Some(p) = self.list_placeholder(
            &page.invites,
            list.is_some_and(Vec::is_empty),
            "No invites yet.",
            cx,
        ) {
            rows.push(p);
            return page_body_with("Invites", create, text, rows);
        }
        let mut entries: Vec<AnyElement> = Vec::new();
        let now = now_ms();
        for (n, invite) in list.into_iter().flatten().enumerate() {
            let uses = match invite.max_uses {
                Some(max) if max > 0 => format!("{} / {max} uses", invite.uses),
                _ => format!("{} uses", invite.uses),
            };
            let expires = match invite.expires_at {
                Some(at) if at <= now => "expired".to_owned(),
                Some(at) => format!(
                    "expires {}",
                    time::format(at, "%Y-%m-%d %H:%M", self.utc_offset)
                ),
                None => "never expires".to_owned(),
            };
            let creator = invite.creator_id.map_or_else(
                || "the server".to_owned(),
                |c| s.display_name(Some(space), c),
            );
            let (copy_inst, copy_code) = (inst.clone(), invite.code.clone());
            let (revoke_inst, revoke_code) = (inst.clone(), invite.code.clone());
            let row = space_settings_list_row(
                div()
                    .font_family(theme::mono_font())
                    .text_size(theme::secondary_size())
                    .child(invite.code.clone()),
                muted(format!("{uses} · {expires} · by {creator}")),
            );
            entries.push(
                row.child(crate::ui::button(
                    self,
                    format!("invite-copy-{n}"),
                    self.copy_label(&format!("invite-copy-{}", invite.code), "Copy link"),
                    ButtonKind::Secondary,
                    true,
                    move |this, _, cx| {
                        if let Some(link) = this.core.invite_link(&copy_inst, &copy_code) {
                            this.copy_from(
                                format!("invite-copy-{copy_code}"),
                                link,
                                "Invite link",
                                cx,
                            );
                        }
                    },
                    cx,
                ))
                .child(crate::ui::button(
                    self,
                    format!("invite-revoke-{n}"),
                    "Revoke",
                    ButtonKind::Danger,
                    true,
                    move |this, _, cx| {
                        this.cancel_closing("confirm");
                        this.confirm = Some((
                            Target::Invite(revoke_inst.clone(), space, revoke_code.clone()),
                            Action::RevokeInvite,
                        ));
                        cx.notify();
                    },
                    cx,
                ))
                .into_any_element(),
            );
        }
        rows.push(group_card(entries).into_any_element());
        page_body_with("Invites", create, text, rows)
    }

    fn tab_reports(
        &self,
        page: &SpacePage,
        s: &InstanceStore,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const TEXT: Option<&str> = Some(
            "Messages people reported. A report shows what the reporter chose to share: the server \
             cannot read encrypted messages.",
        );
        let mut rows: Vec<AnyElement> = Vec::new();
        let list = page.reports.ready();
        if let Some(p) = self.list_placeholder(
            &page.reports,
            list.is_some_and(Vec::is_empty),
            "Nothing has been reported.",
            cx,
        ) {
            rows.push(p);
            return page_body("Reports", TEXT, rows);
        }
        let mut entries: Vec<AnyElement> = Vec::new();
        for report in list.into_iter().flatten() {
            let (inst, space, id) = (page.inst.clone(), page.space, report.id);
            let jump_inst = page.inst.clone();
            let (channel, message) = (report.channel_id, report.message_id);
            let who = |user: Id| s.display_name(Some(space), user);
            let row = space_settings_list_row(
                div().text_size(theme::secondary_size()).child(format!(
                    "{} about {}: {}",
                    who(report.reporter_id),
                    who(report.author_id),
                    report.reason
                )),
                muted(report.excerpt.clone()),
            );
            entries.push(
                row.child(crate::ui::button(
                    self,
                    format!("report-open-{id}"),
                    "Go to it",
                    ButtonKind::Secondary,
                    true,
                    move |this, window, cx| {
                        this.close_pages(cx);
                        this.jump_to(jump_inst.clone(), channel, message, window, cx);
                    },
                    cx,
                ))
                .child(crate::ui::button(
                    self,
                    format!("report-done-{id}"),
                    "Dealt with",
                    ButtonKind::Secondary,
                    true,
                    move |this, _, cx| {
                        let core = this.core.clone();
                        let inst = inst.clone();
                        this.run(
                            cx,
                            async move {
                                core.rest(&inst)?
                                    .resolve_report(space, id)
                                    .await
                                    .map_err(|e| e.to_string())
                            },
                            |this, result, _, cx| match result {
                                Ok(()) => this.set_space_tab(Tab::Reports, true, cx),
                                Err(err) => this.error(err, cx),
                            },
                        );
                    },
                    cx,
                ))
                .into_any_element(),
            );
        }
        rows.push(group_card(entries).into_any_element());
        page_body("Reports", TEXT, rows)
    }

    fn tab_bans(&self, page: &SpacePage, _s: &InstanceStore, cx: &mut Context<Self>) -> AnyElement {
        let inst = page.inst.clone();
        let space = page.space;
        const TEXT: Option<&str> = Some(
            "Banned people cannot rejoin with an invite. Ban someone from the Members tab or by \
             right-clicking them.",
        );
        let mut rows: Vec<AnyElement> = Vec::new();
        let list = page.bans.ready();
        if let Some(p) = self.list_placeholder(
            &page.bans,
            list.is_some_and(Vec::is_empty),
            "Nobody is banned.",
            cx,
        ) {
            rows.push(p);
            return page_body("Bans", TEXT, rows);
        }
        let mut entries: Vec<AnyElement> = Vec::new();
        for ban in list.into_iter().flatten() {
            let reason = ban
                .reason
                .clone()
                .filter(|r| !r.is_empty())
                .unwrap_or_else(|| "no reason given".to_owned());
            let user = ban.user.id;
            let unban_inst = inst.clone();
            let row = space_settings_list_row(
                div().text_size(theme::secondary_size()).child(format!(
                    "{} (@{})",
                    ban.user.name(),
                    ban.user.username
                )),
                muted(format!(
                    "{reason} · {}",
                    time::format(ban.created_at, "%Y-%m-%d", self.utc_offset)
                )),
            );
            entries.push(
                row.child(crate::ui::button(
                    self,
                    format!("unban-{user}"),
                    "Unban",
                    ButtonKind::Secondary,
                    true,
                    move |this, _, cx| {
                        let core = this.core.clone();
                        let inst = unban_inst.clone();
                        this.run(
                            cx,
                            async move {
                                core.rest(&inst)?.unban(space, user).await?;
                                Ok::<_, CoreError>(())
                            },
                            |this, result, _, cx| match result {
                                Ok(()) => this.set_space_tab(Tab::Bans, true, cx),
                                Err(err) => {
                                    this.error(format!("could not unban: {err}"), cx);
                                }
                            },
                        );
                    },
                    cx,
                ))
                .into_any_element(),
            );
        }
        rows.push(group_card(entries).into_any_element());
        page_body("Bans", TEXT, rows)
    }

    fn tab_audit(&self, page: &SpacePage, s: &InstanceStore, cx: &mut Context<Self>) -> AnyElement {
        let refresh = crate::ui::button(
            self,
            "audit-refresh",
            "Refresh",
            ButtonKind::Secondary,
            true,
            |this, _, cx| this.set_space_tab(Tab::Audit, true, cx),
            cx,
        )
        .into_any_element();
        let list = page.audit.ready();
        if let Some(p) = self.list_placeholder(
            &page.audit,
            list.is_some_and(Vec::is_empty),
            "Nothing has been changed yet.",
            cx,
        ) {
            return page_body_with("Audit log", Some(refresh), None, vec![p]);
        }
        let entries: Vec<AnyElement> = list
            .into_iter()
            .flatten()
            .map(|entry| {
                let (text, details) = describe(s, page.space, entry);
                crate::settings::log_entry(
                    text,
                    details,
                    time::format(entry.created_at, "%Y-%m-%d %H:%M", self.utc_offset),
                )
            })
            .collect();
        let mut parts = vec![group_card(entries).into_any_element()];
        if page.audit_more {
            let before = list.and_then(|l| l.last()).map(|e| e.id);
            parts.push(centered(crate::ui::button(
                self,
                "audit-more",
                "Load older entries",
                ButtonKind::Secondary,
                true,
                move |this, _, cx| this.load_audit(before, cx),
                cx,
            )));
        }
        page_body_with("Audit log", Some(refresh), None, parts)
    }

    /// Lets a role row be dragged onto another of the roles' list.
    fn with_list_drag(
        &self,
        element: gpui_kit::Stateful<gpui_kit::Div>,
        page: &SpacePage,
        kind: Kind,
        id: Id,
        name: String,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let item = Item {
            inst: page.inst.clone(),
            space: page.space,
            kind,
            id,
        };
        let (target, over) = (item.clone(), item.clone());
        element
            .on_drag(item, move |_, _, _, cx| cx.new(|_| Ghost(name.clone())))
            .drag_over::<Item>(move |style, dragged, _, _| {
                // Where it would land: tinted with the accent.
                if dragged.can_drop_on(&over) {
                    style.bg(theme::alpha(theme::accent(), 0.18))
                } else {
                    style
                }
            })
            .on_drop(cx.listener(move |this, dragged: &Item, _, cx| {
                if dragged.can_drop_on(&target) {
                    this.drop_role(dragged.clone(), target.clone(), cx);
                }
            }))
    }
}

fn space_settings_list_row(title: gpui_kit::Div, detail: AnyElement) -> gpui_kit::Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(8.))
        .py(px(8.))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(title)
                .child(detail),
        )
}

/// A row of the lists here: [`crate::ui::list_row`], a little smaller.
fn list_row(
    shell: &Shell,
    id: String,
    selected: bool,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    crate::ui::list_row(shell, id, selected, 32., cx)
        .rounded(theme::radius_small())
        .text_size(theme::secondary_size())
}

/// A page's title (an item's name).
fn heading(text: String) -> AnyElement {
    div()
        .min_w_0()
        .truncate()
        .text_size(theme::heading_size())
        .font_weight(FontWeight::MEDIUM)
        .child(text)
        .into_any_element()
}

/// The items of a list tab on one card; each row opens the item's own page (#62).
fn list_card(rows: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .p(px(6.))
        .rounded(theme::radius_large())
        .border_1()
        .border_color(theme::border())
        .bg(theme::sidebar())
        .children(rows)
        .into_any_element()
}

/// The `›` at the end of a row that opens a page.
fn opens() -> impl IntoElement {
    icon(assets::CHEVRON_DOWN, 16., theme::text_tertiary()).with_transformation(
        gpui_kit::Transformation::rotate(gpui_kit::radians(-std::f32::consts::FRAC_PI_2)),
    )
}

/// An item's own page: the line back to its list, its title with buttons on the right, and its
/// parts, in the same centered column as every page.
fn item_page(
    back: AnyElement,
    title: AnyElement,
    actions: AnyElement,
    parts: Vec<AnyElement>,
) -> AnyElement {
    div()
        .id("item-page")
        .flex()
        .flex_col()
        .gap(px(20.))
        .size_full()
        .p(px(32.))
        .overflow_y_scroll()
        .child(page_part(back))
        .child(page_part(crate::settings::title_bar(title, Some(actions))))
        .children(parts.into_iter().map(page_part))
        .into_any_element()
}

impl Shell {
    /// "‹ Roles": back from an item's page to its list; `close` forgets the open item.
    fn back_line(
        &self,
        list: &'static str,
        close: impl Fn(&mut SpacePage) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("item-back")
            .group("item-back")
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(24.))
            .cursor_pointer()
            .text_size(theme::secondary_size())
            .text_color(theme::text_tertiary())
            .hover(|d| d.text_color(theme::accent()))
            .child(
                icon(assets::ARROW_LEFT, 16., theme::text_tertiary())
                    .group_hover("item-back", |s| s.text_color(theme::accent())),
            )
            .child(list)
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(page) = this.open_page() {
                    close(page);
                }
                crate::settings::reset_page_scroll();
                cx.notify();
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(id: u64, position: i32) -> Role {
        Role {
            id: Id(id),
            space_id: Id(1),
            name: format!("r{id}"),
            color: None,
            hoist: false,
            mentionable: false,
            position,
            permissions: Permissions::empty(),
        }
    }

    #[test]
    fn a_dropped_role_takes_the_target_place() {
        // Highest first: A(3) B(2) C(1) @everyone(0).
        let roles = [role(10, 3), role(11, 2), role(12, 1), role(1, 0)];
        // A onto C: A goes to the bottom, B and C move up.
        let moves = reorder_roles(&roles, Id(10), Id(12));
        assert_eq!(
            moves,
            [
                RolePosition {
                    id: Id(11),
                    position: 3
                },
                RolePosition {
                    id: Id(12),
                    position: 2
                },
                RolePosition {
                    id: Id(10),
                    position: 1
                },
            ]
        );
        // C onto B: only the two swap.
        assert_eq!(reorder_roles(&roles, Id(12), Id(11)).len(), 2);
        assert!(reorder_roles(&roles, Id(10), Id(10)).is_empty());
        assert!(
            reorder_roles(&roles, Id(10), Id(1)).is_empty(),
            "never onto @everyone"
        );
    }

    #[test]
    fn colors_parse_and_print() {
        assert_eq!(parse_color(""), Ok(None));
        assert_eq!(parse_color("#ff8800"), Ok(Some(0xFF8800)));
        assert_eq!(parse_color("F80"), Ok(Some(0xFF8800)));
        assert_eq!(parse_color("#12345"), Err(()));
        assert_eq!(parse_color("zzzzzz"), Err(()));
        // `from_str_radix` alone would take a sign.
        assert_eq!(parse_color("+12345"), Err(()));
        assert_eq!(parse_color("#+F8"), Err(()));
        assert_eq!(color_hex(0xFF8800), "#FF8800");
    }

    #[test]
    fn override_editing_follows_the_server_rules() {
        let mut s = hermes_client_core::testing::fixtures::store();
        let (space, general) = (Id(10), Id(11));
        let owner = override_access(&s, space, general, space);
        assert!(
            owner.is_some_and(|have| have == Permissions::all()),
            "the owner"
        );

        // Bob owns the space now. I am a moderator (position 2) with Manage channels and Manage
        // roles but not Mention everyone; there is an admin role above me and a helper below.
        let role = |id: u64, position: i32, permissions: Permissions| Role {
            id: Id(id),
            space_id: space,
            name: format!("r{id}"),
            color: None,
            hoist: false,
            mentionable: false,
            position,
            permissions,
        };
        let st = s.spaces.get_mut(&space).unwrap();
        st.space.owner_id = Id(3);
        let everyone = st.roles.pop().unwrap();
        st.roles = vec![
            role(40, 3, Permissions::empty()),
            role(
                41,
                2,
                Permissions::MANAGE_CHANNELS | Permissions::MANAGE_ROLES,
            ),
            role(42, 1, Permissions::empty()),
            everyone,
        ];
        st.members.get_mut(&Id(1)).unwrap().role_ids = vec![Id(41)];
        let have = override_access(&s, space, general, space).expect("@everyone");
        assert!(!have.contains(Permissions::MENTION_EVERYONE));
        assert!(
            override_access(&s, space, general, Id(41)).is_some(),
            "my own role"
        );
        assert!(
            override_access(&s, space, general, Id(42)).is_some(),
            "below mine"
        );
        assert_eq!(
            override_access(&s, space, general, Id(40)),
            None,
            "above mine"
        );

        let send = Permissions::SEND_MESSAGES;
        let everyone_ping = Permissions::MENTION_EVERYONE;
        let access = Some(have);
        assert!(override_change_allowed(access, None, send, Tri::Deny));
        assert!(!override_change_allowed(
            access,
            None,
            everyone_ping,
            Tri::Allow
        ));
        // An override holding a bit I lack can still be emptied, but not changed otherwise.
        let theirs = Override {
            role_id: Id(42),
            allow: everyone_ping,
            deny: Permissions::empty(),
        };
        assert!(override_change_allowed(
            access,
            Some(&theirs),
            everyone_ping,
            Tri::Inherit
        ));
        assert!(!override_change_allowed(
            access,
            Some(&theirs),
            send,
            Tri::Deny
        ));
        assert!(!override_change_allowed(None, None, send, Tri::Deny));

        // Without Manage roles in the channel, nothing.
        let st = s.spaces.get_mut(&space).unwrap();
        st.channels.get_mut(&general).unwrap().overrides = vec![Override {
            role_id: Id(41),
            allow: Permissions::empty(),
            deny: Permissions::MANAGE_ROLES,
        }];
        assert_eq!(override_access(&s, space, general, space), None);
    }

    #[test]
    fn members_filter_and_sort_by_shown_name() {
        let s = hermes_client_core::testing::fixtures::store();
        let names: Vec<String> = filter_members(&s, Id(10), "")
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, ["Bobby", "me"], "case-insensitive order");
        let found = filter_members(&s, Id(10), "BOB");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1.user.id, Id(3), "by username too");
        assert!(filter_members(&s, Id(10), "nobody").is_empty());
    }

    #[test]
    fn tri_state_round_trips() {
        let send = Permissions::SEND_MESSAGES;
        let view = Permissions::VIEW_CHANNEL;
        let (allow, deny) = Tri::Deny.apply(Permissions::empty(), Permissions::empty(), view);
        assert_eq!((allow, deny), (Permissions::empty(), view));
        let (allow, deny) = Tri::Allow.apply(allow, deny, send);
        assert_eq!((allow, deny), (send, view));
        let (allow, deny) = Tri::Allow.apply(allow, deny, view);
        assert_eq!((allow, deny), (send | view, Permissions::empty()));
        let ov = Override {
            role_id: Id(1),
            allow,
            deny,
        };
        assert_eq!(Tri::of(Some(&ov), view), Tri::Allow);
        assert_eq!(Tri::of(Some(&ov), Permissions::ATTACH_FILES), Tri::Inherit);
        assert_eq!(Tri::of(None, view), Tri::Inherit);
        let (allow, deny) = Tri::Inherit.apply(allow, deny, view);
        assert_eq!((allow, deny), (send, Permissions::empty()));
    }
}
