//! The space settings page (PLAN.md sections 15.1 and 15.4), over the whole window: Overview, Roles,
//! Channels & permissions, Members, Invites, Bans and Audit log. Everything is read from the store;
//! the invite, ban and audit lists are loaded over REST when their tab opens. Changes go through
//! REST and come back as gateway events, so the store stays the only source of truth.

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
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
use crate::channel::icon_button;
use crate::drag::{Ghost, Item, Kind};
use crate::menu_entries::{Action, Target};
use crate::results::Remote;
use crate::settings::{frame, group, muted, nav_item, page_body, setting, wrap};
use crate::shell::Shell;
use crate::theme::{self, px};
use crate::voice::chip;

const AUDIT_PAGE: u32 = 50;
/// At most this many member rows are built; the search narrows bigger spaces.
const MAX_ROWS: usize = 300;
/// Width of the list column of the Roles, Channels and Members tabs.
const SIDE_LIST: f32 = 240.0;
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
    Invites,
    Bans,
    Audit,
}

impl Tab {
    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Roles => "Roles",
            Tab::Channels => "Channels",
            Tab::Members => "Members",
            Tab::Invites => "Invites",
            Tab::Bans => "Bans",
            Tab::Audit => "Audit log",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tab::Overview => assets::INFO,
            Tab::Roles => assets::SHIELD,
            Tab::Channels => assets::POUND,
            Tab::Members => assets::USERS,
            Tab::Invites => assets::LINK,
            Tab::Bans => assets::BAN,
            Tab::Audit => assets::FILE,
        }
    }

    /// The tabs I can use in a space.
    pub fn available(s: &InstanceStore, space: Id) -> Vec<Tab> {
        let mut tabs = vec![Tab::Overview, Tab::Roles, Tab::Channels, Tab::Members];
        if view::can_in_space(s, space, Permissions::CREATE_INVITES)
            || view::can_in_space(s, space, Permissions::MANAGE_INVITES)
        {
            tabs.push(Tab::Invites);
        }
        if view::can_in_space(s, space, Permissions::BAN_MEMBERS) {
            tabs.push(Tab::Bans);
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
    pub role: Option<RoleDraft>,
    /// The channel and role whose overrides are shown.
    pub channel: Option<Id>,
    pub override_role: Option<Id>,
    /// The member whose actions are shown.
    pub member: Option<Id>,
    pub invites: Remote<Vec<Invite>>,
    pub bans: Remote<Vec<Ban>>,
    pub audit: Remote<Vec<AuditEntry>>,
    /// The audit log has older entries to load.
    pub audit_more: bool,
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

/// Members matching `query` (name, nickname or username, case-insensitive), by name.
pub fn filter_members<'s>(s: &'s InstanceStore, space: Id, query: &str) -> Vec<&'s Member> {
    let Some(st) = s.spaces.get(&space) else {
        return Vec::new();
    };
    let q = query.trim().to_lowercase();
    let mut out: Vec<(String, &Member)> = st
        .members
        .values()
        .map(|m| (s.display_name(Some(space), m.user.id), m))
        .filter(|(name, m)| {
            q.is_empty()
                || name.to_lowercase().contains(&q)
                || m.user.username.to_lowercase().contains(&q)
        })
        .collect();
    out.sort_by_key(|(name, _)| name.to_lowercase());
    out.into_iter().map(|(_, m)| m).collect()
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
        A::SpaceUpdate => "changed the space settings".to_owned(),
        A::SpaceTransfer => format!("gave the space to {t}"),
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
            let name = field(name, "Name", window, cx);
            let description = field(description, "What is this space about?", window, cx);
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
                    role: None,
                    channel: None,
                    override_role: None,
                    member: None,
                    invites: Remote::Idle,
                    bans: Remote::Idle,
                    audit: Remote::Idle,
                    audit_more: false,
                    _inputs: inputs,
                },
            );
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
        page.tab = tab;
        match tab {
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
                        let rest = core.rest(&inst).map_err(|e| e.to_string())?;
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
                        let rest = core.rest(&inst).map_err(|e| e.to_string())?;
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
        let core = self.core.clone();
        let (inst, space) = key.clone();
        let append = before.is_some();
        self.run(
            cx,
            async move {
                let rest = core.rest(&inst).map_err(|e| e.to_string())?;
                rest.audit_log(space, before, AUDIT_PAGE)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |this, result, _, _| {
                let Some(page) = this.space_pages.get_mut(&key) else {
                    return;
                };
                match result {
                    Ok(list) => {
                        page.audit_more = list.len() as u32 >= AUDIT_PAGE;
                        match (&mut page.audit, append) {
                            (Remote::Ready(old), true) => old.extend(list),
                            _ => page.audit = Remote::Ready(list),
                        }
                    }
                    Err(err) => page.audit = Remote::Failed(err),
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
        let req = UpdateSpaceRequest {
            name: Some(name),
            description: Some((!description.is_empty()).then_some(description)),
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
                    }
                    this.info("Space saved", cx);
                }
                Err(err) => this.error(format!("could not save: {err}"), cx),
            },
        );
    }

    fn pick_space_icon(&mut self, inst: InstanceId, space: Id, cx: &mut Context<Self>) {
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                let Some(file) = rfd::AsyncFileDialog::new()
                    .set_title("Choose a space icon")
                    .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"])
                    .pick_file()
                    .await
                else {
                    return Ok(None);
                };
                let icon = core.upload(&inst, file.path()).await?;
                core.rest(&inst)?
                    .update_space(
                        space,
                        &UpdateSpaceRequest {
                            icon_id: Some(Some(icon.id)),
                            ..Default::default()
                        },
                    )
                    .await?;
                Ok::<_, CoreError>(Some(()))
            },
            |this, result, _, cx| match result {
                Ok(Some(())) => this.info("Icon changed", cx),
                Ok(None) => {}
                Err(err) => this.error(format!("could not change the icon: {err}"), cx),
            },
        );
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
                Ok(name) => this.info(format!("Role {name} saved"), cx),
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

    pub fn select_settings_channel(&mut self, channel: Id, cx: &mut Context<Self>) {
        let Some((inst, space)) = self.open_space_page.clone() else {
            return;
        };
        let first = {
            let store = self.core.store();
            store
                .instance(&inst)
                .and_then(|s| s.channel(channel))
                .and_then(|c| c.overrides.first().map(|o| o.role_id))
        };
        if let Some(page) = self.open_page() {
            page.channel = Some(channel);
            page.override_role = Some(first.unwrap_or(space));
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
            (Target::Space(inst, space), Action::SearchSpace) => {
                self.open_results(crate::results::Key::Search(inst, Some(space)), window, cx);
            }
            (Target::Channel(inst, channel), Action::ShowPins) => {
                self.open_results(crate::results::Key::Pins(inst, channel), window, cx);
            }
            (Target::Me(_), Action::ShowMentions) => {
                self.open_results(crate::results::Key::Mentions, window, cx);
            }
            (Target::Channel(inst, channel), Action::ChannelPermissions) => {
                let space = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.space_of(channel));
                if let Some(space) = space {
                    self.open_space_settings(inst, space, Tab::Channels, window, cx);
                    self.select_settings_channel(channel, cx);
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
                nav_item(
                    tab.icon(),
                    tab.title(),
                    page.tab == tab,
                    format!("space-tab-{}", tab.title()),
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
            Tab::Invites => self.tab_invites(page, s, cx),
            Tab::Bans => self.tab_bans(page, s, cx),
            Tab::Audit => self.tab_audit(page, s, cx),
        };
        Some(frame(
            st.space.name.clone(),
            nav,
            content,
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
        let mut rows = vec![
            setting("Space name", "", Input::new(&page.name).disabled(!editable)),
            setting(
                "Description",
                "Shown on invite pages.",
                Input::new(&page.description).disabled(!editable),
            ),
        ];
        if editable {
            let has_icon = st.space.icon.is_some();
            let (pick_inst, remove_inst) = (inst.clone(), inst.clone());
            rows.push(setting(
                "Icon",
                "A square PNG or JPEG works best.",
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::new("space-icon")
                            .cursor_pointer()
                            .label(if has_icon {
                                "Change icon…"
                            } else {
                                "Upload icon…"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pick_space_icon(pick_inst.clone(), space, cx);
                            })),
                    )
                    .when(has_icon, |d| {
                        d.child(
                            Button::new("space-icon-remove")
                                .cursor_pointer()
                                .label("Remove icon")
                                .on_click(cx.listener(move |this, _, _, cx| {
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
                                })),
                        )
                    }),
            ));
            if page.overview_dirty {
                let save_key = (inst.clone(), space);
                rows.push(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .text_size(theme::secondary_size())
                                .text_color(theme::warning())
                                .child("You have unsaved changes."),
                        )
                        .child(
                            Button::new("space-save")
                                .cursor_pointer()
                                .primary()
                                .label("Save changes")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_overview(save_key.clone(), cx);
                                })),
                        )
                        .into_any_element(),
                );
            }
        } else {
            rows.push(muted(
                "You need the Manage space permission to change these.",
            ));
        }
        let members = st.members.len();
        let channels = st
            .channels
            .values()
            .filter(|c| c.kind.has_messages())
            .count();
        let owner = s.display_name(Some(space), st.space.owner_id);
        let mut about = vec![setting(
            "About",
            &format!(
                "Owned by {owner} · {members} member{} · {channels} channel{}",
                plural(members),
                plural(channels)
            ),
            Button::new("space-copy-id")
                .cursor_pointer()
                .label("Copy space ID")
                .on_click(
                    cx.listener(move |this, _, _, cx| this.copy(space.to_string(), "Space ID", cx)),
                ),
        )];
        if view::can_in_space(s, space, Permissions::CREATE_INVITES) {
            let invite_inst = inst.clone();
            about.push(setting(
                "Invite people",
                "Creates a link that works for 7 days and copies it.",
                Button::new("space-invite")
                    .cursor_pointer()
                    .primary()
                    .label("Copy invite link")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_action(
                            Target::Space(invite_inst.clone(), space),
                            Action::InvitePeople,
                            window,
                            cx,
                        );
                    })),
            ));
        }
        let danger_inst = inst.clone();
        let danger = if is_owner {
            setting(
                "Delete space",
                "Deletes every channel and message for everyone. This cannot be undone. To keep \
                 the space, give it to someone else from the Members tab first.",
                Button::new("space-delete")
                    .cursor_pointer()
                    .danger()
                    .label("Delete space")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm = Some((
                            Target::Space(danger_inst.clone(), space),
                            Action::DeleteSpace,
                        ));
                        cx.notify();
                    })),
            )
        } else {
            setting(
                "Leave space",
                "You will need a new invite to come back.",
                Button::new("space-leave")
                    .cursor_pointer()
                    .danger()
                    .label("Leave space")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm = Some((
                            Target::Space(danger_inst.clone(), space),
                            Action::LeaveSpace,
                        ));
                        cx.notify();
                    })),
            )
        };
        page_body(
            "Overview",
            None,
            vec![
                group("Space", rows),
                group("About", about),
                group("Danger zone", vec![danger]),
            ],
        )
    }

    fn tab_roles(&self, page: &SpacePage, s: &InstanceStore, cx: &mut Context<Self>) -> AnyElement {
        let st = &s.spaces[&page.space];
        let space = page.space;
        let selected = page.role.as_ref().map(|r| r.id);
        let can_manage = view::can_in_space(s, space, Permissions::MANAGE_ROLES);
        let rows: Vec<AnyElement> = st
            .roles
            .iter()
            .map(|role| {
                let count = if role.is_everyone() {
                    st.members.len()
                } else {
                    st.members
                        .values()
                        .filter(|m| m.role_ids.contains(&role.id))
                        .count()
                };
                let id = role.id;
                let chosen = selected == Some(id);
                let element = list_row(format!("role-{id}"), chosen)
                    .child(
                        div()
                            .size(px(10.))
                            .rounded(px(5.))
                            .bg(role.color.map_or(theme::text_tertiary(), theme::rgb_u32)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(role.name.clone()))
                    .child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_role(id, window, cx);
                    }));
                let draggable = !role.is_everyone() && view::can_manage_role(s, space, role.id);
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
            icon_button("role-create", assets::PLUS)
                .on_click(cx.listener(|this, _, _, cx| this.create_role(cx)))
                .into_any_element()
        });
        let list = side_list("Roles", create, rows);
        let detail = match &page.role {
            Some(draft) if st.role(draft.id).is_some() => self.role_editor(page, s, draft, cx),
            _ => hint_panel(
                "Pick a role to edit it.",
                "Roles give members permissions. A member has every permission of each of their \
                 roles, plus @everyone's. Higher roles in the list can manage lower ones; drag a \
                 role (or use Move up / down) to change the order.",
            ),
        };
        two_columns(list, detail)
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
        col.push(strong(if everyone {
            "@everyone: what every member can do".to_owned()
        } else {
            format!("Edit role: {}", role.name)
        }));
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
        if !everyone {
            col.push(setting(
                "Name",
                "",
                Input::new(&draft.name).disabled(!editable),
            ));
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
            col.push(setting(
                "Color",
                if parse_color(&color_text).is_err() {
                    "Use six hex digits, like #3498DB."
                } else {
                    "Member names show in the color of their highest colored role."
                },
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(px(8.))
                    .child(div().flex().flex_wrap().gap(px(4.)).children(swatches))
                    .child(Input::new(&draft.color).disabled(!editable)),
            ));
            col.push(setting(
                "Show separately",
                "Members with this role get their own group in the member list.",
                Switch::new("role-hoist")
                    .cursor_pointer()
                    .checked(draft.hoist)
                    .disabled(!editable)
                    .when(!editable, |b| b.cursor_default())
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        if let Some(d) = this.open_page().and_then(|p| p.role.as_mut()) {
                            d.hoist = *on;
                        }
                        cx.notify();
                    })),
            ));
            col.push(setting(
                "Mentionable",
                "Anyone can @mention this role.",
                Switch::new("role-mention")
                    .cursor_pointer()
                    .checked(draft.mentionable)
                    .disabled(!editable)
                    .when(!editable, |b| b.cursor_default())
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        if let Some(d) = this.open_page().and_then(|p| p.role.as_mut()) {
                            d.mentionable = *on;
                        }
                        cx.notify();
                    })),
            ));
        }
        for permission_group in PermissionGroup::ALL {
            let rows: Vec<AnyElement> = CATALOG
                .iter()
                .filter(|i| i.group == permission_group)
                .map(|info| {
                    let on = draft.permissions.contains(info.flag);
                    let grantable = editable && (on || mine.contains(info.flag));
                    let flag = info.flag;
                    setting(
                        info.name,
                        info.description,
                        Switch::new(SharedString::from(format!("perm-{}", info.name)))
                            .cursor_pointer()
                            .checked(on)
                            .disabled(!grantable)
                            .when(!grantable, |b| b.cursor_default())
                            .on_click(cx.listener(move |this, value: &bool, _, cx| {
                                if let Some(d) = this.open_page().and_then(|p| p.role.as_mut()) {
                                    d.permissions.set(flag, *value);
                                }
                                cx.notify();
                            })),
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
            buttons = buttons
                .child(
                    Button::new("role-up")
                        .cursor_pointer()
                        .label("Move up")
                        .on_click(cx.listener(move |this, _, _, cx| this.move_role(id, -1, cx))),
                )
                .child(
                    Button::new("role-down")
                        .cursor_pointer()
                        .label("Move down")
                        .on_click(cx.listener(move |this, _, _, cx| this.move_role(id, 1, cx))),
                )
                .child(
                    Button::new("role-delete")
                        .cursor_pointer()
                        .danger()
                        .label("Delete role")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.confirm =
                                Some((Target::Role(inst.clone(), space, id), Action::DeleteRole));
                            cx.notify();
                        })),
                );
        }
        buttons = buttons.child(div().flex_1());
        if editable && dirty {
            buttons =
                buttons
                    .child(
                        Button::new("role-reset")
                            .cursor_pointer()
                            .label("Reset")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_role(id, window, cx)
                            })),
                    )
                    .when(valid, |d| {
                        d.child(
                            Button::new("role-save")
                                .cursor_pointer()
                                .primary()
                                .label("Save changes")
                                .on_click(cx.listener(|this, _, _, cx| this.save_role(cx))),
                        )
                    });
        }
        col.push(buttons.into_any_element());
        detail_panel(col)
    }

    fn tab_channels(
        &self,
        page: &SpacePage,
        s: &InstanceStore,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let space = page.space;
        let inst = page.inst.clone();
        let manage = view::can_in_space(s, space, Permissions::MANAGE_CHANNELS);
        let rows: Vec<AnyElement> = view::channel_tree(s, space)
            .into_iter()
            .map(|c| {
                let id = c.id;
                let glyph = match c.kind {
                    ChannelKind::Category => assets::CHEVRON_DOWN,
                    ChannelKind::Voice => assets::VOLUME,
                    _ => assets::POUND,
                };
                let element = list_row(format!("settings-channel-{id}"), page.channel == Some(id))
                    .when(c.depth > 0, |d| d.ml(px(14.)))
                    .child(icon(glyph, 18., theme::text_tertiary()))
                    .child(div().flex_1().min_w_0().truncate().child(c.name.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_settings_channel(id, cx);
                    }));
                let element = if manage {
                    self.with_list_drag(element, page, Kind::Channel, id, c.name.clone(), cx)
                } else {
                    element
                };
                self.with_menu(element, Target::Channel(inst.clone(), id), cx)
                    .into_any_element()
            })
            .collect();
        let actions = manage.then(|| {
            let (channel_inst, category_inst) = (inst.clone(), inst.clone());
            div()
                .flex()
                .gap(px(2.))
                .child(
                    icon_button("channel-create", assets::PLUS).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.open_form_create_channel(
                                channel_inst.clone(),
                                space,
                                None,
                                window,
                                cx,
                            );
                        },
                    )),
                )
                .child(
                    icon_button("category-create", assets::FOLDER_PLUS).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.open_form_create_category(
                                category_inst.clone(),
                                space,
                                window,
                                cx,
                            );
                        },
                    )),
                )
                .into_any_element()
        });
        let list = side_list("Channels", actions, rows);
        let detail = match page.channel.and_then(|id| s.channel(id)) {
            Some(channel) if channel.space_id == Some(space) => {
                self.channel_detail(page, s, channel.id, cx)
            }
            _ => hint_panel(
                "Pick a channel to edit it.",
                if manage {
                    "Drag channels to reorder them, or onto a category to move them into it. Each \
                     channel can change what a role may do in it."
                } else {
                    "You need the Manage channels permission to change channels."
                },
            ),
        };
        two_columns(list, detail)
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
        let manage = view::can_in_space(s, space, Permissions::MANAGE_CHANNELS);
        let edit_overrides = manage && view::can_in_space(s, space, Permissions::MANAGE_ROLES);
        let category = channel.kind == ChannelKind::Category;
        let kind = match channel.kind {
            ChannelKind::Category => "Category",
            ChannelKind::Voice => "Voice channel",
            _ => "Text channel",
        };
        let mut col: Vec<AnyElement> = vec![
            strong(channel.name.clone().unwrap_or_default()),
            muted(match channel.topic.as_deref() {
                Some(topic) if !topic.is_empty() => format!("{kind} · {topic}"),
                _ => kind.to_owned(),
            }),
        ];
        if manage {
            let (edit_inst, here_inst, delete_inst) = (inst.clone(), inst.clone(), inst.clone());
            col.push(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(
                        Button::new("channel-edit")
                            .cursor_pointer()
                            .label("Edit…")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_form_edit_channel(edit_inst.clone(), id, window, cx);
                            })),
                    )
                    .child(
                        Button::new("channel-up")
                            .cursor_pointer()
                            .label("Move up")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.move_channel_step(id, -1, cx)
                            })),
                    )
                    .child(
                        Button::new("channel-down")
                            .cursor_pointer()
                            .label("Move down")
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.move_channel_step(id, 1, cx)
                                }),
                            ),
                    )
                    .when(category, |d| {
                        d.child(
                            Button::new("channel-here")
                                .cursor_pointer()
                                .label("+ Channel here")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_form_create_channel(
                                        here_inst.clone(),
                                        space,
                                        Some(id),
                                        window,
                                        cx,
                                    );
                                })),
                        )
                    })
                    .child(
                        Button::new("channel-delete")
                            .cursor_pointer()
                            .danger()
                            .label(if category {
                                "Delete category"
                            } else {
                                "Delete channel"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.confirm = Some((
                                    Target::Channel(delete_inst.clone(), id),
                                    Action::DeleteChannel,
                                ));
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        col.push(strong("Permissions".to_owned()));
        col.push(muted(if category {
            "What each role may do in this category. Channels inside it keep their own settings."
        } else {
            "What each role may do in this channel. Inherit uses the role's normal permissions; \
             Deny and Allow override them here."
        }));
        // Every role can be picked; those with an override are marked with a dot.
        let current = page.override_role;
        let chips = st.roles.iter().enumerate().map(|(n, role)| {
            let has = channel.overrides.iter().any(|o| o.role_id == role.id);
            let id = role.id;
            let name = if has {
                format!("● {}", role.name)
            } else {
                role.name.clone()
            };
            chip(("override-role", n), name, current == Some(id)).on_click(cx.listener(
                move |this, _, _, cx| {
                    if let Some(page) = this.open_page() {
                        page.override_role = Some(id);
                    }
                    cx.notify();
                },
            ))
        });
        col.push(wrap().children(chips).into_any_element());
        if let Some(role_id) = current.filter(|r| st.role(*r).is_some()) {
            let ov = channel.overrides.iter().find(|o| o.role_id == role_id);
            let role_name = st.role(role_id).map(|r| r.name.clone()).unwrap_or_default();
            let clear_inst = inst.clone();
            col.push(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(theme::secondary_size())
                            .child(format!("Overrides for {role_name}")),
                    )
                    .when(edit_overrides && ov.is_some(), |d| {
                        d.child(
                            Button::new("override-clear")
                                .cursor_pointer()
                                .label("Remove override")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.call_status(
                                    &clear_inst,
                                    "Override removed",
                                    move |r| async move { r.delete_override(id, role_id).await },
                                    cx,
                                );
                                })),
                        )
                    })
                    .into_any_element(),
            );
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
                        let toggle = |tri: Tri, text: &'static str, cx: &mut Context<Self>| {
                            chip(
                                SharedString::from(format!("{}-{text}", info.name)),
                                text,
                                state == tri,
                            )
                            .when(edit_overrides, |d| {
                                d.on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_override(flag, tri, cx);
                                }))
                            })
                        };
                        setting(
                            info.name,
                            info.description,
                            div()
                                .flex()
                                .gap(px(2.))
                                .child(toggle(Tri::Deny, "Deny", cx))
                                .child(toggle(Tri::Inherit, "Inherit", cx))
                                .child(toggle(Tri::Allow, "Allow", cx)),
                        )
                    })
                    .collect();
                col.push(group(permission_group.title(), rows));
            }
            if !edit_overrides {
                col.push(muted(
                    "Changing these needs both Manage channels and Manage roles.",
                ));
            }
        }
        detail_panel(col)
    }

    fn tab_members(
        &self,
        page: &SpacePage,
        s: &InstanceStore,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let space = page.space;
        let query = page.member_query.read(cx).value().to_string();
        let members = filter_members(s, space, &query);
        let total = members.len();
        let rows: Vec<AnyElement> = members
            .into_iter()
            .take(MAX_ROWS)
            .map(|m| {
                let user = m.user.id;
                let name = s.display_name(Some(space), user);
                let name_color = crate::channel::author_color(s, space, user)
                    .map_or(theme::text(), theme::rgb_u32);
                list_row(format!("settings-member-{user}"), page.member == Some(user))
                    .h(px(46.))
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(page) = this.open_page() {
                            page.member = Some(user);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let more = total.saturating_sub(MAX_ROWS);
        let list = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(8.))
            .w(px(SIDE_LIST))
            .h_full()
            .p(px(8.))
            .bg(theme::sidebar())
            .child(Input::new(&page.member_query))
            .child(muted(format!("{total} member{}", plural(total))))
            .child(
                div()
                    .id("settings-members")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(px(1.))
                    .overflow_y_scroll()
                    .children(rows)
                    .when(more > 0, |d| {
                        d.child(muted(format!("…and {more} more. Search to narrow.")))
                    }),
            )
            .into_any_element();
        let detail = match page.member.and_then(|u| s.member(space, u)) {
            Some(m) => self.member_detail(page, s, m, cx),
            None => hint_panel("Pick a member to see what you can do.", ""),
        };
        two_columns(list, detail)
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
        let mut col: Vec<AnyElement> = vec![
            strong(s.display_name(Some(space), user)),
            muted(format!(
                "@{} · joined {}{}",
                m.user.username,
                time::format(m.joined_at, "%Y-%m-%d", self.utc_offset),
                if owner { " · owner" } else { "" }
            )),
        ];
        if let Some(status) = m.user.status_text.as_deref().filter(|t| !t.is_empty()) {
            col.push(muted(status.to_owned()));
        }
        let nick = if me {
            can(Permissions::CHANGE_NICKNAME) || can(Permissions::MANAGE_NICKNAMES)
        } else {
            above && can(Permissions::MANAGE_NICKNAMES)
        };
        let (dm_inst, nick_inst) = (inst.clone(), inst.clone());
        col.push(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .when(!me, |d| {
                    d.child(
                        Button::new("member-dm")
                            .cursor_pointer()
                            .primary()
                            .label("Message")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.close_space_settings(cx);
                                this.message_user(dm_inst.clone(), user, window, cx);
                            })),
                    )
                })
                .when(nick, |d| {
                    d.child(
                        Button::new("member-nick")
                            .cursor_pointer()
                            .label("Change nickname…")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_form_nickname(nick_inst.clone(), space, user, window, cx);
                            })),
                    )
                })
                .into_any_element(),
        );

        let role_rows: Vec<AnyElement> = st
            .roles
            .iter()
            .filter(|r| !r.is_everyone())
            .map(|role| {
                let has = m.role_ids.contains(&role.id);
                let may = view::can_manage_role(s, space, role.id) && (me || above || is_owner);
                let id = role.id;
                Checkbox::new(SharedString::from(format!("member-role-{id}")))
                    .cursor_pointer()
                    .label(SharedString::from(role.name.clone()))
                    .checked(has)
                    .disabled(!may)
                    .when(!may, |b| b.cursor_default())
                    .on_click(cx.listener(move |this, on: &bool, _, cx| {
                        this.set_member_role(user, id, *on, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        col.push(if role_rows.is_empty() {
            group(
                "Roles",
                vec![muted(
                    "This space has no roles yet. Create them in the Roles tab.",
                )],
            )
        } else {
            group("Roles", role_rows)
        });

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
                    d.child(
                        Button::new("member-mute")
                            .cursor_pointer()
                            .label(if server_mute {
                                "Server unmute"
                            } else {
                                "Server mute"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
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
                            })),
                    )
                })
                .when(can(Permissions::DEAFEN_MEMBERS), |d| {
                    d.child(
                        Button::new("member-deafen")
                            .cursor_pointer()
                            .label(if server_deaf {
                                "Server undeafen"
                            } else {
                                "Server deafen"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
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
                            })),
                    )
                })
                .when(can(Permissions::MOVE_MEMBERS), |d| {
                    d.child(
                        Button::new("member-disconnect")
                            .cursor_pointer()
                            .danger()
                            .label("Disconnect")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.patch_voice(
                                    user,
                                    VoiceStatePatchRequest {
                                        channel_id: Some(None),
                                        ..VoiceStatePatchRequest::default()
                                    },
                                    "Disconnected from voice",
                                    cx,
                                );
                            })),
                    )
                });
            col.push(group(
                &format!("In voice: {channel}"),
                vec![voice.into_any_element()],
            ));
        }

        // Kick, ban and transfer go in a danger zone at the bottom, away from everyday buttons.
        let mut danger: Vec<AnyElement> = Vec::new();
        let target = Target::Member(inst.clone(), Some(space), user);
        if above && can(Permissions::KICK_MEMBERS) {
            let kick = target.clone();
            danger.push(setting(
                "Kick",
                "Removes them from the space; they can come back with a new invite.",
                Button::new("member-kick")
                    .cursor_pointer()
                    .danger()
                    .label("Kick")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm = Some((kick.clone(), Action::Kick));
                        cx.notify();
                    })),
            ));
        }
        if above && can(Permissions::BAN_MEMBERS) {
            let ban_inst = inst.clone();
            danger.push(setting(
                "Ban",
                "Removes them and keeps them out.",
                Button::new("member-ban")
                    .cursor_pointer()
                    .danger()
                    .label("Ban…")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_form_ban(ban_inst.clone(), space, user, window, cx);
                    })),
            ));
        }
        if is_owner && !me {
            danger.push(setting(
                "Transfer ownership",
                "Makes them the owner of the space. You keep your roles but can no longer delete \
                 the space.",
                Button::new("member-transfer")
                    .cursor_pointer()
                    .danger()
                    .label("Transfer…")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm = Some((target.clone(), Action::TransferOwnership));
                        cx.notify();
                    })),
            ));
        }
        if !danger.is_empty() {
            col.push(group("Danger zone", danger));
        }
        detail_panel(col)
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
            Remote::Idle | Remote::Loading => "Loading…".to_owned(),
            Remote::Failed(err) => format!("Could not load: {err}"),
            Remote::Ready(_) if empty => empty_text.to_owned(),
            Remote::Ready(_) => return None,
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(muted(text))
                .child(
                    Button::new("list-refresh")
                        .cursor_pointer()
                        .label("Refresh")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(tab) = this.open_page().map(|p| p.tab) {
                                this.set_space_tab(tab, true, cx);
                            }
                        })),
                )
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
        let mut rows: Vec<AnyElement> = vec![muted(
            "Anyone with a link can join while it is valid. Revoke a link to stop it working.",
        )];
        if view::can_in_space(s, space, Permissions::CREATE_INVITES) {
            rows.push(
                Button::new("invite-create")
                    .cursor_pointer()
                    .primary()
                    .label("Create invite…")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_form_invite(create_inst.clone(), space, window, cx);
                    }))
                    .into_any_element(),
            );
        }
        if !manage {
            rows.push(muted(
                "Seeing everyone's invites needs the Manage invites permission.",
            ));
            return page_body("Invites", None, rows);
        }
        let list = page.invites.ready();
        if let Some(p) = self.list_placeholder(
            &page.invites,
            list.is_some_and(Vec::is_empty),
            "No invites yet.",
            cx,
        ) {
            rows.push(p);
            return page_body("Invites", None, rows);
        }
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
            rows.push(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(8.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_family(theme::mono_font())
                                    .text_size(theme::secondary_size())
                                    .child(invite.code.clone()),
                            )
                            .child(muted(format!("{uses} · {expires} · by {creator}"))),
                    )
                    .child(
                        Button::new(("invite-copy", n))
                            .cursor_pointer()
                            .label("Copy link")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(link) = this.core.invite_link(&copy_inst, &copy_code) {
                                    this.copy(link, "Invite link", cx);
                                }
                            })),
                    )
                    .child(
                        Button::new(("invite-revoke", n))
                            .cursor_pointer()
                            .danger()
                            .label("Revoke")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.confirm = Some((
                                    Target::Invite(revoke_inst.clone(), space, revoke_code.clone()),
                                    Action::RevokeInvite,
                                ));
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        page_body("Invites", None, rows)
    }

    fn tab_bans(&self, page: &SpacePage, _s: &InstanceStore, cx: &mut Context<Self>) -> AnyElement {
        let inst = page.inst.clone();
        let space = page.space;
        let mut rows: Vec<AnyElement> = vec![muted(
            "Banned people cannot rejoin with an invite. Ban someone from the Members tab or by \
             right-clicking them.",
        )];
        let list = page.bans.ready();
        if let Some(p) = self.list_placeholder(
            &page.bans,
            list.is_some_and(Vec::is_empty),
            "Nobody is banned.",
            cx,
        ) {
            rows.push(p);
            return page_body("Bans", None, rows);
        }
        for ban in list.into_iter().flatten() {
            let reason = ban
                .reason
                .clone()
                .filter(|r| !r.is_empty())
                .unwrap_or_else(|| "no reason given".to_owned());
            let user = ban.user.id;
            let unban_inst = inst.clone();
            rows.push(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(8.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(theme::secondary_size()).child(format!(
                                "{} (@{})",
                                ban.user.name(),
                                ban.user.username
                            )))
                            .child(muted(format!(
                                "{reason} · {}",
                                time::format(ban.created_at, "%Y-%m-%d", self.utc_offset)
                            ))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("unban-{user}")))
                            .cursor_pointer()
                            .label("Unban")
                            .on_click(cx.listener(move |this, _, _, cx| {
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
                            })),
                    )
                    .into_any_element(),
            );
        }
        page_body("Bans", None, rows)
    }

    fn tab_audit(&self, page: &SpacePage, s: &InstanceStore, cx: &mut Context<Self>) -> AnyElement {
        let mut rows: Vec<AnyElement> = vec![
            Button::new("audit-refresh")
                .cursor_pointer()
                .label("Refresh")
                .on_click(cx.listener(|this, _, _, cx| this.set_space_tab(Tab::Audit, true, cx)))
                .into_any_element(),
        ];
        let list = page.audit.ready();
        if let Some(p) = self.list_placeholder(
            &page.audit,
            list.is_some_and(Vec::is_empty),
            "Nothing has been changed yet.",
            cx,
        ) {
            rows.push(p);
            return page_body("Audit log", None, rows);
        }
        for entry in list.into_iter().flatten() {
            let (text, details) = describe(s, page.space, entry);
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .gap(px(2.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .w(px(130.))
                                    .text_size(theme::caption_size())
                                    .text_color(theme::text_tertiary())
                                    .child(time::format(
                                        entry.created_at,
                                        "%Y-%m-%d %H:%M",
                                        self.utc_offset,
                                    )),
                            )
                            .child(div().text_size(theme::secondary_size()).child(text)),
                    )
                    .when(!details.is_empty(), |d| {
                        d.child(div().pl(px(138.)).child(muted(details)))
                    })
                    .into_any_element(),
            );
        }
        if page.audit_more {
            let before = list.and_then(|l| l.last()).map(|e| e.id);
            rows.push(
                Button::new("audit-more")
                    .cursor_pointer()
                    .label("Load older entries")
                    .on_click(cx.listener(move |this, _, _, cx| this.load_audit(before, cx)))
                    .into_any_element(),
            );
        }
        page_body("Audit log", None, rows)
    }

    /// Lets a settings list row be dragged onto another of the list.
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
                if dragged.can_drop_on(&over) {
                    style.bg(theme::selected())
                } else {
                    style
                }
            })
            .on_drop(cx.listener(move |this, dragged: &Item, _, cx| {
                if dragged.can_drop_on(&target) {
                    this.drop_item(dragged.clone(), target.clone(), cx);
                }
            }))
    }
}

fn strong(text: String) -> AnyElement {
    div()
        .text_size(theme::body_size())
        .font_weight(FontWeight::MEDIUM)
        .child(text)
        .into_any_element()
}

fn list_row(id: String, selected: bool) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(SharedString::from(id))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(8.))
        .h(px(32.))
        .px(px(8.))
        .rounded(px(4.))
        .cursor_pointer()
        .text_size(theme::secondary_size())
        .when(selected, |d| d.bg(theme::selected()))
        .when(!selected, |d| d.hover(|d| d.bg(theme::hover())))
}

/// A narrow list column with a header and scrolling rows.
fn side_list(title: &str, actions: Option<AnyElement>, rows: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(8.))
        .w(px(SIDE_LIST))
        .h_full()
        .p(px(8.))
        .bg(theme::sidebar())
        .child(
            div()
                .flex()
                .items_center()
                .h(px(28.))
                .child(
                    div()
                        .flex_1()
                        .px(px(4.))
                        .text_size(theme::caption_size())
                        .text_color(theme::text_tertiary())
                        .child(title.to_uppercase()),
                )
                .children(actions),
        )
        .child(
            div()
                .id("side-list-rows")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .gap(px(1.))
                .overflow_y_scroll()
                .children(rows),
        )
        .into_any_element()
}

fn two_columns(list: AnyElement, detail: AnyElement) -> AnyElement {
    div()
        .flex()
        .size_full()
        .child(list)
        .child(div().flex_1().min_w_0().h_full().child(detail))
        .into_any_element()
}

fn detail_panel(content: Vec<AnyElement>) -> AnyElement {
    div()
        .id("detail-panel")
        .flex()
        .flex_col()
        .gap(px(14.))
        .size_full()
        .p(px(20.))
        .overflow_y_scroll()
        .children(content)
        .into_any_element()
}

fn hint_panel(title: &'static str, text: &'static str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .max_w(px(520.))
        .p(px(24.))
        .child(
            div()
                .text_size(theme::secondary_size())
                .text_color(theme::text_secondary())
                .child(title),
        )
        .child(muted(text))
        .into_any_element()
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
        assert_eq!(color_hex(0xFF8800), "#FF8800");
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
