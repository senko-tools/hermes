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
use iced::widget::{Space, button, column, container, row, rule, scrollable, space, text_input};
use iced::{Alignment, Element, Fill, Length};

use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::{heading, label, small, strong};

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
            Tab::Channels => "Channels & permissions",
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
    use crate::widget::icon::{self as i, icon};
    let mut nav = column![
        row![
            crate::widget::avatar::space_icon(&space.space.name, 24.0, fonts),
            iced::widget::text(space.space.name.to_uppercase())
                .font(fonts.bold())
                .size(fonts.small() - 0.5)
                .color(theme.text.tertiary.0)
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(style::GAP)
        .align_y(Alignment::Center)
        .padding([4, 10]),
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
        nav = nav.push(
            button(
                row![
                    icon(glyph, 16.0).color(if selected {
                        theme.buttons.primary.background.0
                    } else {
                        theme.text.tertiary.0
                    }),
                    label(tab.title(), fonts).color(if selected {
                        theme.text.primary.0
                    } else {
                        theme.text.secondary.0
                    }),
                ]
                .spacing(style::GAP_L)
                .align_y(Alignment::Center),
            )
            .padding([8, 12])
            .width(Fill)
            .style(move |_, s| style::sidebar_item(theme, s, selected))
            .on_press(Message::Tab(tab)),
        );
    }
    let content: Element<'a, Message> = match current {
        Tab::Overview => scrollable(container(overview(ctx)).padding(20).width(Fill))
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
            .padding(iced::Padding::ZERO.top(28.0))
            .into(),
        Message::Close,
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
    let mut col = column![
        heading("Overview", fonts),
        field(
            "Space name",
            "",
            input("Name", &name, Message::Name),
            fonts,
            theme
        ),
        field(
            "Description",
            "Shown on invite pages.",
            input(
                "What is this space about?",
                &description,
                Message::Description
            ),
            fonts,
            theme,
        ),
    ]
    .spacing(14)
    .max_width(720);
    if editable {
        let icon = if st.space.icon.is_some() {
            row![
                button(label("Change icon…", fonts))
                    .padding(style::BUTTON_PAD)
                    .style(move |_, s| style::secondary(theme, s))
                    .on_press(Message::PickIcon),
                button(label("Remove icon", fonts))
                    .padding(style::BUTTON_PAD)
                    .style(move |_, s| style::secondary(theme, s))
                    .on_press(Message::RemoveIcon),
            ]
        } else {
            row![
                button(label("Upload icon…", fonts))
                    .padding(style::BUTTON_PAD)
                    .style(move |_, s| style::secondary(theme, s))
                    .on_press(Message::PickIcon),
            ]
        };
        col = col
            .push(field(
                "Icon",
                "A square PNG or JPEG works best.",
                icon.spacing(8),
                fonts,
                theme,
            ))
            .push(
                row![
                    space::horizontal(),
                    button(label("Reset", fonts))
                        .padding(style::BUTTON_PAD)
                        .style(move |_, s| style::secondary(theme, s))
                        .on_press_maybe(dirty.then_some(Message::ResetOverview)),
                    button(label("Save changes", fonts))
                        .padding(style::BUTTON_PAD)
                        .style(move |_, s| style::primary(theme, s))
                        .on_press_maybe(dirty.then_some(Message::SaveOverview)),
                ]
                .spacing(8),
            );
    } else {
        col = col.push(
            small(
                "You need the Manage space permission to change these.",
                fonts,
            )
            .color(theme.text.tertiary.0),
        );
    }
    let members = st.members.len();
    let channels = st
        .channels
        .values()
        .filter(|c| c.kind.has_messages())
        .count();
    let owner = ctx.s.display_name(Some(ctx.space), st.space.owner_id);
    col = col.push(field(
        "About",
        &format!(
            "Owned by {owner} · {members} member{} · {channels} channel{}",
            plural(members),
            plural(channels)
        ),
        row![
            button(label("Copy space ID", fonts))
                .padding(style::BUTTON_PAD)
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::CopyId),
        ],
        fonts,
        theme,
    ));
    if ctx.can(Permissions::CREATE_INVITES) {
        col = col.push(field(
            "Invite people",
            "Creates a link that works for 7 days and copies it.",
            button(label("Copy invite link", fonts))
                .padding(style::BUTTON_PAD)
                .style(move |_, s| style::primary(theme, s))
                .on_press(Message::InvitePeople),
            fonts,
            theme,
        ));
    }
    col = col
        .push(Space::new().height(style::GAP_XL))
        .push(strong("Danger zone", fonts).color(theme.text.error.0));
    if ctx.is_owner() {
        col = col.push(field(
            "Delete space",
            "Deletes every channel and message for everyone. This cannot be undone. To keep the \
             space, give it to someone else from the Members tab first.",
            button(label("Delete space", fonts))
                .padding(style::BUTTON_PAD)
                .style(move |_, s| style::danger(theme, s))
                .on_press(Message::DeleteSpace),
            fonts,
            theme,
        ));
    } else {
        col = col.push(field(
            "Leave space",
            "You will need a new invite to come back.",
            button(label("Leave space", fonts))
                .padding(style::BUTTON_PAD)
                .style(move |_, s| style::danger(theme, s))
                .on_press(Message::LeaveSpace),
            fonts,
            theme,
        ));
    }
    col.into()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// A labelled row: name and hint on the left, the control on the right.
fn field<'a>(
    name: &str,
    hint: &str,
    control: impl Into<Element<'a, Message>>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    column![
        rule::horizontal(1).style(move |_| style::rule(theme)),
        row![
            column![
                label(name.to_owned(), fonts),
                small(hint.to_owned(), fonts).color(theme.text.tertiary.0)
            ]
            .spacing(2)
            .width(Length::FillPortion(2)),
            container(control.into()).width(Length::FillPortion(3)),
        ]
        .spacing(16)
        .align_y(Alignment::Center),
    ]
    .spacing(10)
    .into()
}

/// A full-width block under a rule.
fn field_block<'a>(content: Element<'a, Message>, theme: &'a Theme) -> Element<'a, Message> {
    column![
        rule::horizontal(1).style(move |_| style::rule(theme)),
        content
    ]
    .spacing(10)
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
                small(title.to_uppercase(), fonts)
                    .color(theme.text.tertiary.0)
                    .width(Fill),
                actions
            ]
            .align_y(Alignment::Center)
            .padding([0, 4]),
            scrollable(list)
                .style(move |_, st| style::scrollbar(theme, st))
                .height(Fill),
        ]
        .spacing(6),
    )
    .padding(8)
    .width(240)
    .height(Fill)
    .style(move |_| style::sidebar(theme))
    .into()
}

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
            button(small("Refresh", fonts))
                .padding(style::BUTTON_PAD)
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::Refresh),
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
