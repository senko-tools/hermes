//! The Members tab: searchable member list; for the picked member their roles (checkboxes),
//! nickname, kick, ban, voice moderation and ownership transfer, each shown only when I may do
//! it.

use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Member;
use hermes_client_core::proto::permissions::Permissions;
use hermes_client_core::store::InstanceStore;
use hermes_client_core::{time, view};
use iced::widget::{button, checkbox, column, container, row, scrollable, text_input};
use iced::{Alignment, Element, Fill};

use super::{Ctx, Message, rgb};
use crate::theme::style;
use crate::widget::{ButtonKind, text_button};
use crate::widget::{caption, label, strong};

/// At most this many rows are built; the search narrows bigger spaces.
const MAX_ROWS: usize = 300;

/// Members matching `query` (name, nickname or username, case-insensitive), by name.
pub fn filter<'s>(s: &'s InstanceStore, space: Id, query: &str) -> Vec<&'s Member> {
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

/// The color of the member's highest colored role.
fn color(s: &InstanceStore, space: Id, member: &Member) -> Option<u32> {
    s.spaces.get(&space).and_then(|st| {
        st.roles
            .iter()
            .filter(|r| member.role_ids.contains(&r.id))
            .find_map(|r| r.color)
    })
}

pub fn view<'a>(ctx: &Ctx<'a, '_>) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let members = filter(ctx.s, ctx.space, &ctx.state.member_query);
    let total = members.len();
    let mut list = column![].spacing(1);
    for m in members.into_iter().take(MAX_ROWS) {
        let selected = ctx.state.member == Some(m.user.id);
        let name = ctx.s.display_name(Some(ctx.space), m.user.id);
        let name_color = color(ctx.s, ctx.space, m).map_or(theme.text.primary.0, rgb);
        list = list.push(
            button(
                row![
                    crate::widget::avatar::view(
                        &name,
                        crate::widget::avatar::picture(ctx.images, ctx.inst, Some(&m.user))
                            .as_ref(),
                        28.0,
                        fonts,
                    ),
                    column![
                        label(name.clone(), fonts).color(name_color),
                        caption(format!("@{}", m.user.username), fonts)
                            .color(theme.text.tertiary.0),
                    ]
                    .spacing(0),
                ]
                .spacing(style::GAP_S)
                .align_y(Alignment::Center),
            )
            .padding([style::GAP_XS, style::GAP_S])
            .width(Fill)
            .style(move |_, s| style::sidebar_item(theme, s, selected))
            .on_press(Message::SelectMember(m.user.id)),
        );
    }
    if total > MAX_ROWS {
        list = list.push(
            caption(
                format!("…and {} more. Search to narrow.", total - MAX_ROWS),
                fonts,
            )
            .color(theme.text.tertiary.0),
        );
    }
    let left = container(
        column![
            text_input("Search members", &ctx.state.member_query)
                .padding(style::INPUT_PAD)
                .style(move |_, st| style::input(theme, st))
                .on_input(Message::MemberQuery)
                .font(fonts.ui)
                .size(fonts.size),
            caption(format!("{total} member{}", super::plural(total)), fonts)
                .color(theme.text.tertiary.0),
            scrollable(list)
                .style(move |_, st| style::scrollbar(theme, st))
                .height(Fill),
        ]
        .spacing(style::GAP_S),
    )
    .padding(style::GAP_S)
    .width(super::SIDE_LIST)
    .height(Fill)
    .style(move |_| style::sidebar(theme));

    let detail: Element<'a, Message> =
        match ctx.state.member.and_then(|u| ctx.s.member(ctx.space, u)) {
            Some(m) => detail(ctx, m),
            None => container(
                label("Pick a member to see what you can do.", fonts).color(theme.text.secondary.0),
            )
            .padding(style::GAP_XL)
            .into(),
        };
    row![left, detail].height(Fill).into()
}

fn detail<'a>(ctx: &Ctx<'a, '_>, m: &Member) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let st = &ctx.s.spaces[&ctx.space];
    let user = m.user.id;
    let me = ctx.me() == Some(user);
    let above = view::outranks(ctx.s, ctx.space, user);
    let owner = st.space.owner_id == user;

    let mut col = column![].spacing(style::GAP).max_width(720);
    let mut title = column![
        strong(ctx.s.display_name(Some(ctx.space), user), fonts),
        caption(
            format!(
                "@{} · joined {}{}",
                m.user.username,
                time::format(m.joined_at, "%Y-%m-%d", ctx.utc_offset),
                if owner { " · owner" } else { "" }
            ),
            fonts
        )
        .color(theme.text.tertiary.0),
    ]
    .spacing(2);
    if let Some(status) = m.user.status_text.as_deref().filter(|t| !t.is_empty()) {
        title = title.push(caption(status.to_owned(), fonts).color(theme.text.secondary.0));
    }
    col = col.push(title);

    let mut actions = row![].spacing(style::GAP_S).align_y(Alignment::Center);
    if !me {
        actions = actions.push(
            text_button("Message", ButtonKind::Primary, fonts, theme)
                .on_press(Message::DmMember(user)),
        );
    }
    let nick = if me {
        ctx.can(Permissions::CHANGE_NICKNAME) || ctx.can(Permissions::MANAGE_NICKNAMES)
    } else {
        above && ctx.can(Permissions::MANAGE_NICKNAMES)
    };
    if nick {
        actions = actions.push(
            text_button("Change nickname…", ButtonKind::Secondary, fonts, theme)
                .on_press(Message::Nickname(user)),
        );
    }
    col = col.push(actions);
    // Kick, ban and transfer go in a danger zone at the bottom, away from everyday buttons.
    let mut danger = Vec::new();
    if above && ctx.can(Permissions::KICK_MEMBERS) {
        danger.push(crate::buffer::page::setting(
            "Kick",
            "Removes them from the space; they can come back with a new invite.",
            text_button("Kick", ButtonKind::Danger, fonts, theme).on_press(Message::Kick(user)),
            fonts,
            theme,
        ));
    }
    if above && ctx.can(Permissions::BAN_MEMBERS) {
        danger.push(crate::buffer::page::setting(
            "Ban",
            "Removes them and keeps them out.",
            text_button("Ban…", ButtonKind::Danger, fonts, theme).on_press(Message::Ban(user)),
            fonts,
            theme,
        ));
    }

    // Roles.
    let mut roles = column![strong("Roles", fonts)].spacing(style::GAP_S);
    let mut any = false;
    for role in st.roles.iter().filter(|r| !r.is_everyone()) {
        any = true;
        let has = m.role_ids.contains(&role.id);
        let may =
            view::can_manage_role(ctx.s, ctx.space, role.id) && (me || above || ctx.is_owner());
        let id = role.id;
        let mut c = checkbox(has)
            .label(role.name.clone())
            .font(fonts.ui)
            .text_size(fonts.size);
        if may {
            c = c.on_toggle(move |on| Message::MemberRole(user, id, on));
        }
        roles = roles.push(c);
    }
    if !any {
        roles = roles.push(
            caption(
                "This space has no roles yet. Create them in the Roles tab.",
                fonts,
            )
            .color(theme.text.tertiary.0),
        );
    }
    col = col.push(super::field_block(roles.into(), theme));

    // Voice moderation, while they are in a voice channel of this space.
    if let Some(vs) = ctx
        .s
        .voice_states
        .get(&user)
        .filter(|v| ctx.s.space_of(v.channel_id) == Some(ctx.space))
    {
        let channel = ctx
            .s
            .channel(vs.channel_id)
            .and_then(|c| c.name.clone())
            .unwrap_or_default();
        let mut voice = row![].spacing(style::GAP_S).align_y(Alignment::Center);
        if ctx.can(Permissions::MUTE_MEMBERS) {
            voice = voice.push(
                button(label(
                    if vs.server_mute {
                        "Server unmute"
                    } else {
                        "Server mute"
                    },
                    fonts,
                ))
                .padding(style::BUTTON_PAD)
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::ServerMute(user, !vs.server_mute)),
            );
        }
        if ctx.can(Permissions::DEAFEN_MEMBERS) {
            voice = voice.push(
                button(label(
                    if vs.server_deaf {
                        "Server undeafen"
                    } else {
                        "Server deafen"
                    },
                    fonts,
                ))
                .padding(style::BUTTON_PAD)
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::ServerDeafen(user, !vs.server_deaf)),
            );
        }
        if ctx.can(Permissions::MOVE_MEMBERS) {
            voice = voice.push(
                text_button("Disconnect", ButtonKind::Danger, fonts, theme)
                    .on_press(Message::Disconnect(user)),
            );
        }
        col = col.push(super::field_block(
            column![strong(format!("In voice: {channel}"), fonts), voice]
                .spacing(style::GAP_S)
                .into(),
            theme,
        ));
    }

    if ctx.is_owner() && !me {
        danger.push(crate::buffer::page::setting(
            "Transfer ownership",
            "Makes them the owner of the space. You keep your roles but can no longer delete \
             the space.",
            text_button("Transfer…", ButtonKind::Danger, fonts, theme)
                .on_press(Message::Transfer(user)),
            fonts,
            theme,
        ));
    }
    if !danger.is_empty() {
        col = col.push(crate::buffer::page::group(
            "Danger zone",
            danger,
            fonts,
            theme,
        ));
    }
    scrollable(
        container(col)
            .padding(iced::Padding::ZERO.left(style::GAP_XL).right(style::GAP_L))
            .width(Fill),
    )
    .style(move |_, st| style::scrollbar(theme, st))
    .height(Fill)
    .into()
}
