//! The Invites, Bans and Audit log tabs: lists loaded over REST when the tab opens.

use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{AuditAction, AuditEntry};
use hermes_client_core::proto::permissions::Permissions;
use hermes_client_core::store::InstanceStore;
use hermes_client_core::time;
use iced::widget::{Space, column, container, row, scrollable};
use iced::{Alignment, Element, Fill};

use super::{Ctx, Message, placeholder};
use crate::theme::style;
use crate::widget::{ButtonKind, text_button};
use crate::widget::{caption, heading, label};

fn page<'a>(content: Element<'a, Message>, theme: &'a crate::theme::Theme) -> Element<'a, Message> {
    scrollable(container(content).padding(style::GAP_XL).width(Fill))
        .style(move |_, st| style::scrollbar(theme, st))
        .height(Fill)
        .into()
}

pub fn invites<'a>(ctx: &Ctx<'a, '_>) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let manage = ctx.can(Permissions::MANAGE_INVITES);
    let mut col = column![
        row![
            column![
                heading("Invites", fonts),
                caption(
                    "Anyone with a link can join while it is valid. Revoke a link to stop it working.",
                    fonts
                )
                .color(theme.text.tertiary.0),
            ]
            .spacing(2)
            .width(Fill),
            text_button("Create invite…", ButtonKind::Primary, fonts, theme)
                .on_press_maybe(ctx.can(Permissions::CREATE_INVITES).then_some(Message::CreateInvite)),
        ]
        .spacing(style::GAP)
        .align_y(Alignment::Center)
    ]
    .spacing(style::GAP_S)
    .max_width(820);
    if !manage {
        col = col.push(
            caption(
                "Seeing everyone's invites needs the Manage invites permission.",
                fonts,
            )
            .color(theme.text.tertiary.0),
        );
        return page(col.into(), theme);
    }
    let list = ctx.state.invites.ready();
    if let Some(p) = placeholder(
        &ctx.state.invites,
        list.is_some_and(Vec::is_empty),
        "No invites yet.",
        fonts,
        theme,
    ) {
        return page(col.push(p).into(), theme);
    }
    let now = hermes_client_core::now_ms();
    for invite in list.into_iter().flatten() {
        let uses = match invite.max_uses {
            Some(max) if max > 0 => format!("{} / {max} uses", invite.uses),
            _ => format!("{} uses", invite.uses),
        };
        let expires = match invite.expires_at {
            Some(at) if at <= now => "expired".to_owned(),
            Some(at) => format!(
                "expires {}",
                time::format(at, "%Y-%m-%d %H:%M", ctx.utc_offset)
            ),
            None => "never expires".to_owned(),
        };
        let creator = invite
            .creator_id
            .map(|c| ctx.s.display_name(Some(ctx.space), c))
            .unwrap_or_else(|| "the server".to_owned());
        let mine = invite.creator_id == ctx.me();
        col = col.push(super::field_block(
            row![
                column![
                    label(invite.code.clone(), fonts).font(fonts.mono),
                    caption(format!("{uses} · {expires} · by {creator}"), fonts)
                        .color(theme.text.tertiary.0),
                ]
                .spacing(2)
                .width(Fill),
                text_button("Copy link", ButtonKind::Secondary, fonts, theme)
                    .on_press(Message::CopyInvite(invite.code.clone())),
                text_button("Revoke", ButtonKind::Danger, fonts, theme).on_press_maybe(
                    (manage || mine).then(|| Message::RevokeInvite(invite.code.clone()))
                ),
            ]
            .spacing(style::GAP_S)
            .align_y(Alignment::Center)
            .into(),
            theme,
        ));
    }
    page(col.into(), theme)
}

pub fn bans<'a>(ctx: &Ctx<'a, '_>) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let mut col = column![
        column![
            heading("Bans", fonts),
            caption(
                "Banned people cannot rejoin with an invite. Ban someone from the Members tab or \
                 by right-clicking them.",
                fonts
            )
            .color(theme.text.tertiary.0),
        ]
        .spacing(2)
    ]
    .spacing(style::GAP_S)
    .max_width(820);
    let list = ctx.state.bans.ready();
    if let Some(p) = placeholder(
        &ctx.state.bans,
        list.is_some_and(Vec::is_empty),
        "Nobody is banned.",
        fonts,
        theme,
    ) {
        return page(col.push(p).into(), theme);
    }
    for ban in list.into_iter().flatten() {
        let reason = ban
            .reason
            .clone()
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| "no reason given".to_owned());
        col = col.push(super::field_block(
            row![
                column![
                    label(
                        format!("{} (@{})", ban.user.name(), ban.user.username),
                        fonts
                    ),
                    caption(
                        format!(
                            "{reason} · {}",
                            time::format(ban.created_at, "%Y-%m-%d", ctx.utc_offset)
                        ),
                        fonts
                    )
                    .color(theme.text.tertiary.0),
                ]
                .spacing(2)
                .width(Fill),
                text_button("Unban", ButtonKind::Secondary, fonts, theme)
                    .on_press(Message::Unban(ban.user.id)),
            ]
            .spacing(style::GAP_S)
            .align_y(Alignment::Center)
            .into(),
            theme,
        ));
    }
    page(col.into(), theme)
}

pub fn audit<'a>(ctx: &Ctx<'a, '_>) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let mut col = column![
        row![
            heading("Audit log", fonts).width(Fill),
            text_button("Refresh", ButtonKind::Secondary, fonts, theme).on_press(Message::Refresh),
        ]
        .align_y(Alignment::Center)
    ]
    .spacing(style::GAP_S)
    .max_width(820);
    let list = ctx.state.audit.ready();
    if let Some(p) = placeholder(
        &ctx.state.audit,
        list.is_some_and(Vec::is_empty),
        "Nothing has been changed yet.",
        fonts,
        theme,
    ) {
        return page(col.push(p).into(), theme);
    }
    for entry in list.into_iter().flatten() {
        let (text, details) = describe(ctx.s, ctx.space, entry);
        let mut line = column![
            row![
                caption(
                    time::format(entry.created_at, "%Y-%m-%d %H:%M", ctx.utc_offset),
                    fonts
                )
                .color(theme.text.tertiary.0)
                .width(120),
                label(text, fonts),
            ]
            .spacing(style::GAP_S)
        ]
        .spacing(2);
        if !details.is_empty() {
            line = line.push(row![
                Space::new().width(128),
                caption(details, fonts).color(theme.text.tertiary.0)
            ]);
        }
        col = col.push(super::field_block(line.into(), theme));
    }
    if ctx.state.audit_more {
        col = col.push(
            text_button("Load older entries", ButtonKind::Secondary, fonts, theme)
                .on_press(Message::MoreAudit),
        );
    }
    page(col.into(), theme)
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
