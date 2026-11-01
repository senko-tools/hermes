//! The Roles tab: the role list (drag to reorder, or Move up / Move down) and the editor with
//! name, color, hoist, mentionable and the permission checkboxes grouped with descriptions.

use hermes_client_core::proto::api::RolePosition;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Role;
use hermes_client_core::proto::permissions::{CATALOG, PermissionGroup, Permissions};
use hermes_client_core::view;
use iced::widget::{
    Space, button, checkbox, column, container, mouse_area, row, scrollable, text_input,
};
use iced::{Alignment, Border, Element, Fill, Length};

use super::{Ctx, Message, color_hex, parse_color, rgb, side_list};
use crate::drag::Kind;
use crate::theme::style;
use crate::widget::{ButtonKind, text_button};
use crate::widget::{caption, label, strong};

/// Preset colors offered next to the hex field.
const SWATCHES: [u32; 12] = [
    0x1ABC9C, 0x2ECC71, 0x3498DB, 0x9B59B6, 0xE91E63, 0xF1C40F, 0xE67E22, 0xE74C3C, 0x95A5A6,
    0x607D8B, 0x11806A, 0x206694,
];

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

pub fn view<'a>(ctx: &Ctx<'a, '_>) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let st = &ctx.s.spaces[&ctx.space];
    let can_manage = ctx.can(Permissions::MANAGE_ROLES);
    let selected = ctx.state.role.as_ref().map(|r| r.id);

    let mut rows = Vec::new();
    for role in &st.roles {
        let count = if role.is_everyone() {
            st.members.len()
        } else {
            st.members
                .values()
                .filter(|m| m.role_ids.contains(&role.id))
                .count()
        };
        let manageable = view::can_manage_role(ctx.s, ctx.space, role.id);
        let dot = caption("●", fonts).color(role.color.map_or(theme.text.tertiary.0, rgb));
        let handle = if manageable && !role.is_everyone() {
            caption("⠿", fonts).color(theme.text.tertiary.0)
        } else {
            caption(" ", fonts)
        };
        let is_selected = selected == Some(role.id);
        let targeted = ctx.drop_target == Some((Kind::Role, role.id));
        let row_button = button(
            row![
                handle,
                dot,
                label(role.name.clone(), fonts).width(Fill),
                caption(count.to_string(), fonts).color(theme.text.tertiary.0),
            ]
            .spacing(style::GAP_S)
            .align_y(Alignment::Center),
        )
        .padding([style::GAP_XS, style::GAP_S])
        .width(Fill)
        .style(move |_, s| {
            let mut st = style::sidebar_item(theme, s, is_selected || targeted);
            if targeted {
                st.border = Border {
                    color: theme.pane.focused_border.0,
                    width: 1.0,
                    radius: 4.0.into(),
                };
            }
            st
        })
        .on_press(Message::SelectRole(role.id));
        let mut area = mouse_area(row_button);
        if !role.is_everyone() {
            area = area
                .on_enter(Message::Hover(Some((Kind::Role, role.id))))
                .on_exit(Message::Leave((Kind::Role, role.id)));
        }
        rows.push(area.into());
    }
    let create: Element<'a, Message> = if can_manage {
        text_button("+ Create role", ButtonKind::Secondary, fonts, theme)
            .on_press(Message::CreateRole)
            .into()
    } else {
        Space::new().into()
    };
    let list = side_list("Roles", create, rows, fonts, theme);

    let editor: Element<'a, Message> = match &ctx.state.role {
        Some(draft) if st.role(draft.id).is_some() => editor(ctx, draft),
        _ => container(
            column![
                label("Pick a role to edit it.", fonts).color(theme.text.secondary.0),
                caption(
                    "Roles give members permissions. A member has every permission of each of \
                     their roles, plus @everyone's. Higher roles in the list can manage lower ones; \
                     drag a role (or use Move up / down) to change the order.",
                    fonts
                )
                .color(theme.text.tertiary.0),
            ]
            .spacing(style::GAP_S)
            .max_width(520),
        )
        .padding(style::GAP_XL)
        .into(),
    };
    row![list, editor].height(Fill).into()
}

fn editor<'a>(ctx: &Ctx<'a, '_>, draft: &'a super::RoleDraft) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let st = &ctx.s.spaces[&ctx.space];
    let role = st.role(draft.id).expect("checked by the caller");
    let everyone = role.is_everyone();
    let editable = view::can_manage_role(ctx.s, ctx.space, role.id);
    // You cannot grant what you do not have (the owner and administrators have everything).
    let mine = ctx.s.permissions(ctx.space, None);

    let mut col = column![].spacing(style::GAP).max_width(720);
    let title = if everyone {
        "@everyone: what every member can do".to_owned()
    } else {
        format!("Edit role: {}", role.name)
    };
    col = col.push(strong(title, fonts));
    if !editable {
        col = col.push(
            caption(
                "You can only edit roles below your own highest role, with Manage roles.",
                fonts,
            )
            .color(theme.text.warning.0),
        );
    }
    if !everyone {
        let name = text_input("Role name", &draft.name)
            .padding(style::INPUT_PAD)
            .style(move |_, st| style::input(theme, st))
            .font(fonts.ui)
            .size(fonts.size);
        let name = if editable {
            name.on_input(Message::RoleName)
                .on_submit(Message::SaveRole)
        } else {
            name
        };
        col = col.push(super::field("Name", "", name, fonts, theme));

        let parsed = parse_color(&draft.color);
        let mut swatches = row![].spacing(style::GAP_XS);
        for color in SWATCHES {
            let chosen = parsed == Ok(Some(color));
            let swatch = button(Space::new().width(18).height(18))
                .padding(0)
                .style(move |_, _| button::Style {
                    background: Some(rgb(color).into()),
                    border: Border {
                        color: if chosen {
                            theme.text.primary.0
                        } else {
                            iced::Color::TRANSPARENT
                        },
                        width: 2.0,
                        radius: 4.0.into(),
                    },
                    ..button::Style::default()
                });
            swatches = swatches.push(if editable {
                swatch.on_press(Message::RoleColor(color_hex(color)))
            } else {
                swatch
            });
        }
        let hex = text_input("#RRGGBB or empty", &draft.color)
            .padding(style::INPUT_PAD)
            .style(move |_, st| style::input(theme, st))
            .font(fonts.mono)
            .size(fonts.size)
            .width(110);
        let hex = if editable {
            hex.on_input(Message::RoleColor)
        } else {
            hex
        };
        let preview = caption("●", fonts).color(match parsed {
            Ok(Some(c)) => rgb(c),
            _ => theme.text.tertiary.0,
        });
        col = col.push(super::field(
            "Color",
            if parsed.is_err() {
                "Use six hex digits, like #3498DB."
            } else {
                "Member names show in the color of their highest colored role."
            },
            column![
                swatches,
                row![preview, hex]
                    .spacing(style::GAP_S)
                    .align_y(Alignment::Center)
            ]
            .spacing(style::GAP_S),
            fonts,
            theme,
        ));
        let check = |on: bool, text: &'static str, f: fn(bool) -> Message| {
            let c = checkbox(on)
                .label(text)
                .font(fonts.ui)
                .text_size(fonts.size);
            if editable { c.on_toggle(f) } else { c }
        };
        col = col.push(super::field(
            "Display",
            "",
            column![
                check(
                    draft.hoist,
                    "Show members with this role separately in the member list",
                    Message::RoleHoist
                ),
                check(
                    draft.mentionable,
                    "Anyone can @mention this role",
                    Message::RoleMentionable
                ),
            ]
            .spacing(style::GAP_S),
            fonts,
            theme,
        ));
    }

    for group in PermissionGroup::ALL {
        let mut list = column![strong(group.title(), fonts)].spacing(style::GAP_S);
        for info in CATALOG.iter().filter(|i| i.group == group) {
            let on = draft.permissions.contains(info.flag);
            let grantable = editable && (on || mine.contains(info.flag));
            let flag = info.flag;
            let mut c = checkbox(on)
                .label(info.name)
                .font(fonts.medium())
                .text_size(fonts.size);
            if grantable {
                c = c.on_toggle(move |v| Message::RolePermission(flag, v));
            }
            list = list.push(
                column![
                    c,
                    container(caption(info.description, fonts).color(theme.text.tertiary.0))
                        .padding(iced::Padding {
                            left: 26.0,
                            ..iced::Padding::ZERO
                        }),
                ]
                .spacing(2),
            );
        }
        col = col.push(super::field_block(list.into(), theme));
    }

    let dirty = draft_dirty(role, draft);
    let valid = !draft.name.trim().is_empty() && parse_color(&draft.color).is_ok();
    let mut buttons = row![].spacing(style::GAP_S).align_y(Alignment::Center);
    if editable && !everyone {
        buttons = buttons
            .push(
                text_button("Move up", ButtonKind::Secondary, fonts, theme)
                    .on_press(Message::MoveRole(role.id, -1)),
            )
            .push(
                text_button("Move down", ButtonKind::Secondary, fonts, theme)
                    .on_press(Message::MoveRole(role.id, 1)),
            )
            .push(
                text_button("Delete role", ButtonKind::Danger, fonts, theme)
                    .on_press(Message::DeleteRole(role.id)),
            );
    }
    buttons = buttons.push(Space::new().width(Fill));
    if editable {
        buttons = buttons
            .push(
                text_button("Reset", ButtonKind::Secondary, fonts, theme)
                    .on_press_maybe(dirty.then_some(Message::ResetRole)),
            )
            .push(
                text_button("Save changes", ButtonKind::Primary, fonts, theme)
                    .on_press_maybe((dirty && valid).then_some(Message::SaveRole)),
            );
    }
    col = col.push(buttons);
    scrollable(container(col).padding(style::GAP_XL).width(Fill))
        .style(move |_, st| style::scrollbar(theme, st))
        .height(Fill)
        .width(Length::Fill)
        .into()
}

/// Whether the draft differs from the stored role.
pub fn draft_dirty(role: &Role, draft: &super::RoleDraft) -> bool {
    draft.name.trim() != role.name
        || parse_color(&draft.color) != Ok(role.color)
        || draft.hoist != role.hoist
        || draft.mentionable != role.mentionable
        || draft.permissions != role.permissions
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
        let moves = reorder_roles(&roles, Id(12), Id(11));
        assert_eq!(moves.len(), 2);
        assert!(reorder_roles(&roles, Id(10), Id(10)).is_empty());
        assert!(
            reorder_roles(&roles, Id(10), Id(1)).is_empty(),
            "never onto @everyone"
        );
    }
}
