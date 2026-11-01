//! The SpaceSettings buffer (PLAN.md sections 15.1 and 15.4): Overview, Roles, Channels &
//! permissions, Members, Invites, Bans and Audit log. Everything is read from the store; the
//! invite, ban and audit lists are loaded over REST when their tab opens. Changes go through
//! REST and come back as gateway events, so the store stays the only source of truth.

mod channels;
mod lists;
mod members;
mod roles;

use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{AuditEntry, Ban, Invite, Role};
use hermes_client_core::proto::permissions::Permissions;
use hermes_client_core::store::InstanceStore;
use hermes_client_core::view;
use iced::widget::{Space, column, container, row, rule, scrollable, text_input};
use iced::{Alignment, Element, Fill};

use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::{ButtonKind, text_button};
use crate::widget::{caption, label};

pub use channels::Tri;
pub use roles::reorder_roles;

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

/// A list loaded over REST.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Remote<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Remote<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Remote::Ready(value) => Some(value),
            _ => None,
        }
    }
}

/// The role being edited, unsaved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleDraft {
    pub id: Id,
    pub name: String,
    /// `#RRGGBB`, or empty for no color.
    pub color: String,
    pub hoist: bool,
    pub mentionable: bool,
    pub permissions: Permissions,
}

/// What a SpaceSettings buffer remembers (one per space, kept while the app runs).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub tab: Tab,
    /// Overview drafts; `None` = unchanged (the store's value is shown).
    pub name: Option<String>,
    pub description: Option<String>,
    pub role: Option<RoleDraft>,
    /// The channel and role whose overrides are shown.
    pub channel: Option<Id>,
    pub override_role: Option<Id>,
    pub member_query: String,
    /// The member whose actions are shown.
    pub member: Option<Id>,
    pub invites: Remote<Vec<Invite>>,
    pub bans: Remote<Vec<Ban>>,
    pub audit: Remote<Vec<AuditEntry>>,
    /// The audit log has older entries to load.
    pub audit_more: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Tab(Tab),
    Close,
    /// The pointer entered a draggable row (or left it).
    Hover(Option<(crate::drag::Kind, Id)>),
    Leave((crate::drag::Kind, Id)),
    // Overview
    Name(String),
    Description(String),
    SaveOverview,
    ResetOverview,
    PickIcon,
    RemoveIcon,
    InvitePeople,
    LeaveSpace,
    DeleteSpace,
    CopyId,
    // Roles
    SelectRole(Id),
    CreateRole,
    RoleName(String),
    RoleColor(String),
    RoleHoist(bool),
    RoleMentionable(bool),
    RolePermission(Permissions, bool),
    SaveRole,
    ResetRole,
    DeleteRole(Id),
    /// Move a role one step up (`-1`) or down (`1`) in the list.
    MoveRole(Id, i32),
    // Channels
    SelectChannel(Id),
    CreateChannel(Option<Id>),
    CreateCategory,
    EditChannel(Id),
    DeleteChannel(Id),
    MoveChannel(Id, i32),
    OverrideRole(Id),
    SetOverride(Permissions, Tri),
    ClearOverride,
    // Members
    MemberQuery(String),
    SelectMember(Id),
    MemberRole(Id, Id, bool),
    Nickname(Id),
    Kick(Id),
    Ban(Id),
    DmMember(Id),
    ServerMute(Id, bool),
    ServerDeafen(Id, bool),
    Disconnect(Id),
    Transfer(Id),
    // Lists
    CreateInvite,
    CopyInvite(String),
    RevokeInvite(String),
    Unban(Id),
    Refresh,
    MoreAudit,
    Invites(Result<Vec<Invite>, String>),
    Bans(Result<Vec<Ban>, String>),
    /// `true` when the entries are older ones to append.
    Audit(bool, Result<Vec<AuditEntry>, String>),
    OverviewSaved(Result<(), String>),
    RoleSaved(Result<Role, String>),
    RoleCreated(Result<Id, String>),
}

impl State {
    /// Starts editing `role` from the store.
    pub fn select_role(&mut self, s: &InstanceStore, space: Id, role: Id) {
        self.role = s
            .spaces
            .get(&space)
            .and_then(|st| st.role(role))
            .map(RoleDraft::of);
    }
}

impl RoleDraft {
    pub fn of(r: &Role) -> RoleDraft {
        RoleDraft {
            id: r.id,
            name: r.name.clone(),
            color: r.color.map(color_hex).unwrap_or_default(),
            hoist: r.hoist,
            mentionable: r.mentionable,
            permissions: r.permissions,
        }
    }

    /// The PATCH for what changed against `role`; `None` if the color does not parse.
    pub fn changes(
        &self,
        role: &Role,
    ) -> Option<hermes_client_core::proto::api::UpdateRoleRequest> {
        let color = parse_color(&self.color).ok()?;
        let name = self.name.trim();
        Some(hermes_client_core::proto::api::UpdateRoleRequest {
            name: (name != role.name && !role.is_everyone()).then(|| name.to_owned()),
            color: (color != role.color).then_some(color),
            hoist: (self.hoist != role.hoist).then_some(self.hoist),
            mentionable: (self.mentionable != role.mentionable).then_some(self.mentionable),
            permissions: (self.permissions != role.permissions).then_some(self.permissions),
        })
    }
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

pub use crate::theme::style::rgb;

/// What the views need.
pub struct Ctx<'a, 'b> {
    pub s: &'b InstanceStore,
    pub space: Id,
    pub state: &'a State,
    pub fonts: &'b Fonts,
    pub theme: &'a Theme,
    pub utc_offset: i64,
    /// The row a drag is over, for the drop marker.
    pub drop_target: Option<(crate::drag::Kind, Id)>,
    /// What closing the page goes back to.
    pub return_to: &'b str,
}

impl Ctx<'_, '_> {
    fn can(&self, perm: Permissions) -> bool {
        view::can_in_space(self.s, self.space, perm)
    }

    fn me(&self) -> Option<Id> {
        self.s.me_id()
    }

    fn is_owner(&self) -> bool {
        self.s
            .spaces
            .get(&self.space)
            .is_some_and(|st| Some(st.space.owner_id) == self.me())
    }
}

pub fn view<'a>(ctx: &Ctx<'a, '_>) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let Some(space) = ctx.s.spaces.get(&ctx.space) else {
        return container(label("This space is not available.", fonts))
            .center(Fill)
            .into();
    };
    let current = ctx.state.tab;
    use crate::widget::icon as i;
    let mut nav = column![
        row![
            crate::widget::avatar::space_icon(&space.space.name, 24.0, fonts),
            crate::widget::section_title(&space.space.name, fonts).color(theme.text.tertiary.0),
        ]
        .spacing(style::GAP_S)
        .align_y(Alignment::Center)
        .padding([style::GAP_XS, style::GAP_S]),
        Space::new().height(style::GAP_S),
    ]
    .spacing(2);
    for tab in Tab::available(ctx.s, ctx.space) {
        let selected = current == tab;
        let glyph = match tab {
            Tab::Overview => i::INFO,
            Tab::Roles => i::SHIELD,
            Tab::Channels => i::HASH,
            Tab::Members => i::USERS,
            Tab::Invites => i::LINK,
            Tab::Bans => i::BAN,
            Tab::Audit => i::FILE,
        };
        nav = nav.push(crate::buffer::page::nav_item(
            glyph,
            tab.title(),
            selected,
            Message::Tab(tab),
            fonts,
            theme,
        ));
    }
    let content: Element<'a, Message> = match current {
        Tab::Overview => scrollable(
            container(overview(ctx))
                .padding(
                    iced::Padding::ZERO
                        .bottom(style::HEADER)
                        .right(style::GAP_L),
                )
                .width(Fill),
        )
        .style(move |_, st| style::scrollbar(theme, st))
        .height(Fill)
        .into(),
        Tab::Roles => roles::view(ctx),
        Tab::Channels => channels::view(ctx),
        Tab::Members => members::view(ctx),
        Tab::Invites => lists::invites(ctx),
        Tab::Bans => lists::bans(ctx),
        Tab::Audit => lists::audit(ctx),
    };
    crate::buffer::page_frame(
        nav.into(),
        container(content)
            .padding(iced::Padding::ZERO.top(style::GAP_XL))
            .into(),
        Message::Close,
        ctx.return_to,
        fonts,
        theme,
    )
}

fn overview<'a>(ctx: &Ctx<'a, '_>) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let st = &ctx.s.spaces[&ctx.space];
    let editable = ctx.can(Permissions::MANAGE_SPACE);
    let name = ctx
        .state
        .name
        .clone()
        .unwrap_or_else(|| st.space.name.clone());
    let description = ctx
        .state
        .description
        .clone()
        .unwrap_or_else(|| st.space.description.clone().unwrap_or_default());
    let dirty = ctx.state.name.is_some() || ctx.state.description.is_some();
    let input = |placeholder: &str, value: &str, f: fn(String) -> Message| {
        let input = text_input(placeholder, value)
            .padding(style::INPUT_PAD)
            .style(move |_, st| style::input(theme, st))
            .font(fonts.ui)
            .size(fonts.size);
        if editable {
            input.on_input(f).on_submit(Message::SaveOverview)
        } else {
            input
        }
    };
    let mut space_rows = vec![
        field(
            "Space name",
            "",
            input("Name", &name, Message::Name),
            fonts,
            theme,
        ),
        field(
            "Description",
            "Shown on invite pages.",
            input(
                "What is this space about?",
                &description,
                Message::Description,
            ),
            fonts,
            theme,
        ),
    ];
    if editable {
        let icon = if st.space.icon.is_some() {
            row![
                text_button("Change icon…", ButtonKind::Secondary, fonts, theme)
                    .on_press(Message::PickIcon),
                text_button("Remove icon", ButtonKind::Secondary, fonts, theme)
                    .on_press(Message::RemoveIcon),
            ]
        } else {
            row![
                text_button("Upload icon…", ButtonKind::Secondary, fonts, theme)
                    .on_press(Message::PickIcon),
            ]
        };
        space_rows.push(field(
            "Icon",
            "A square PNG or JPEG works best.",
            icon.spacing(style::GAP_S),
            fonts,
            theme,
        ));
        if dirty {
            space_rows.push(
                row![
                    caption("You have unsaved changes.", fonts)
                        .color(theme.text.warning.0)
                        .width(Fill),
                    text_button("Reset", ButtonKind::Secondary, fonts, theme)
                        .on_press(Message::ResetOverview),
                    text_button("Save changes", ButtonKind::Primary, fonts, theme)
                        .on_press(Message::SaveOverview),
                ]
                .spacing(style::GAP_S)
                .align_y(Alignment::Center)
                .into(),
            );
        }
    } else {
        space_rows.push(
            caption(
                "You need the Manage space permission to change these.",
                fonts,
            )
            .color(theme.text.tertiary.0)
            .into(),
        );
    }
    let mut about_rows = Vec::new();
    let members = st.members.len();
    let channels = st
        .channels
        .values()
        .filter(|c| c.kind.has_messages())
        .count();
    let owner = ctx.s.display_name(Some(ctx.space), st.space.owner_id);
    about_rows.push(field(
        "About",
        &format!(
            "Owned by {owner} · {members} member{} · {channels} channel{}",
            plural(members),
            plural(channels)
        ),
        row![
            text_button("Copy space ID", ButtonKind::Secondary, fonts, theme)
                .on_press(Message::CopyId),
        ],
        fonts,
        theme,
    ));
    if ctx.can(Permissions::CREATE_INVITES) {
        about_rows.push(field(
            "Invite people",
            "Creates a link that works for 7 days and copies it.",
            text_button("Copy invite link", ButtonKind::Primary, fonts, theme)
                .on_press(Message::InvitePeople),
            fonts,
            theme,
        ));
    }
    let danger = if ctx.is_owner() {
        field(
            "Delete space",
            "Deletes every channel and message for everyone. This cannot be undone. To keep the \
             space, give it to someone else from the Members tab first.",
            text_button("Delete space", ButtonKind::Danger, fonts, theme)
                .on_press(Message::DeleteSpace),
            fonts,
            theme,
        )
    } else {
        field(
            "Leave space",
            "You will need a new invite to come back.",
            text_button("Leave space", ButtonKind::Danger, fonts, theme)
                .on_press(Message::LeaveSpace),
            fonts,
            theme,
        )
    };
    crate::buffer::page::page(
        "Overview",
        None,
        vec![
            group("Space", space_rows, fonts, theme),
            group("About", about_rows, fonts, theme),
            group("Danger zone", vec![danger], fonts, theme),
        ],
        fonts,
        theme,
    )
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// A labelled row: name and hint on the left, the control in the control column.
fn field<'a>(
    name: &str,
    hint: &str,
    control: impl Into<Element<'a, Message>>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    crate::buffer::page::setting(name, hint, control, fonts, theme)
}

fn group<'a>(
    title: &str,
    rows: Vec<Element<'a, Message>>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    crate::buffer::page::group(title, rows, fonts, theme)
}

/// A full-width block under a rule.
fn field_block<'a>(content: Element<'a, Message>, theme: &'a Theme) -> Element<'a, Message> {
    column![
        rule::horizontal(1).style(move |_| style::rule(theme)),
        content
    ]
    .spacing(style::GAP_S)
    .into()
}

/// A narrow list column with a header and scrolling rows.
fn side_list<'a>(
    title: &str,
    actions: Element<'a, Message>,
    rows: Vec<Element<'a, Message>>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let mut list = column![].spacing(1);
    for r in rows {
        list = list.push(r);
    }
    container(
        column![
            row![
                crate::widget::section_title(title, fonts)
                    .color(theme.text.tertiary.0)
                    .width(Fill),
                actions
            ]
            .spacing(style::GAP_XS)
            .align_y(Alignment::Center)
            .height(style::ICON_BUTTON)
            .padding([0.0, style::GAP_XS]),
            scrollable(list)
                .style(move |_, st| style::scrollbar(theme, st))
                .height(Fill),
        ]
        .spacing(style::GAP_S),
    )
    .padding(style::GAP_S)
    .width(SIDE_LIST)
    .height(Fill)
    .style(move |_| style::sidebar(theme))
    .into()
}

/// Width of the list column of the Roles, Channels and Members tabs.
pub const SIDE_LIST: f32 = 240.0;

/// A small "Loading…" / error / empty placeholder for REST lists.
fn placeholder<'a, T>(
    remote: &Remote<T>,
    empty: bool,
    empty_text: &'static str,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Option<Element<'a, Message>> {
    let text = match remote {
        Remote::Idle | Remote::Loading => "Loading…".to_owned(),
        Remote::Failed(err) => format!("Could not load: {err}"),
        Remote::Ready(_) if empty => empty_text.to_owned(),
        Remote::Ready(_) => return None,
    };
    let color = if matches!(remote, Remote::Failed(_)) {
        theme.text.error.0
    } else {
        theme.text.tertiary.0
    };
    Some(
        column![
            label(text, fonts).color(color),
            Space::new().height(8),
            text_button("Refresh", ButtonKind::Secondary, fonts, theme).on_press(Message::Refresh),
        ]
        .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_parse_and_print() {
        assert_eq!(parse_color(""), Ok(None));
        assert_eq!(parse_color("#ff8800"), Ok(Some(0xFF8800)));
        assert_eq!(parse_color("F80"), Ok(Some(0xFF8800)));
        assert_eq!(parse_color("#12345"), Err(()));
        assert_eq!(parse_color("zzzzzz"), Err(()));
        assert_eq!(color_hex(0xFF8800), "#FF8800");
    }
}
