//! The Channels & permissions tab: the channel tree (drag to reorder or move into a category),
//! create / edit / delete, and per-role overrides with tri-state toggles (deny / inherit /
//! allow).

use std::fmt;

use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{ChannelKind, Override};
use hermes_client_core::proto::permissions::{CATALOG, PermissionGroup, Permissions};
use hermes_client_core::view;
use iced::widget::{Space, button, column, container, mouse_area, pick_list, row, scrollable};
use iced::{Alignment, Element, Fill};

use super::{Ctx, Message, side_list};
use crate::drag::Kind;
use crate::theme::style;
use crate::widget::{label, small, strong};

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

/// A role in the "add a role" picker.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RoleChoice {
    id: Id,
    name: String,
}

impl fmt::Display for RoleChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

pub fn view<'a>(ctx: &Ctx<'a, '_>) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let manage = ctx.can(Permissions::MANAGE_CHANNELS);
    let selected = ctx.state.channel;

    let mut rows = Vec::new();
    for c in view::channel_tree(ctx.s, ctx.space) {
        let is_selected = selected == Some(c.id);
        let content: Element<'a, Message> = if c.kind == ChannelKind::Category {
            small(c.name.to_uppercase(), fonts)
                .color(theme.text.tertiary.0)
                .into()
        } else {
            row![
                label(
                    if c.kind == ChannelKind::Voice {
                        "♪"
                    } else {
                        "#"
                    },
                    fonts
                )
                .color(theme.text.tertiary.0),
                label(c.name.clone(), fonts),
            ]
            .spacing(6)
            .into()
        };
        let targeted = ctx.drop_target == Some((Kind::Channel, c.id));
        let indent = if c.depth > 0 { 14.0 } else { 0.0 };
        let handle = if manage {
            small("⠿", fonts).color(theme.text.tertiary.0)
        } else {
            small("", fonts)
        };
        let b = button(
            row![Space::new().width(indent), handle, content]
                .spacing(6)
                .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .width(Fill)
        .style(move |_, s| {
            let mut st = style::sidebar_item(theme, s, is_selected || targeted);
            if targeted {
                st.border = iced::Border {
                    color: theme.pane.focused_border.0,
                    width: 1.0,
                    radius: 4.0.into(),
                };
            }
            st
        })
        .on_press(Message::SelectChannel(c.id));
        let mut area = mouse_area(b);
        if manage {
            area = area
                .on_enter(Message::Hover(Some((Kind::Channel, c.id))))
                .on_exit(Message::Leave((Kind::Channel, c.id)));
        }
        rows.push(area.into());
    }
    let actions: Element<'a, Message> = if manage {
        row![
            button(small("+ Channel", fonts))
                .padding([2, 6])
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::CreateChannel(None)),
            button(small("+ Category", fonts))
                .padding([2, 6])
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::CreateCategory),
        ]
        .spacing(4)
        .into()
    } else {
        Space::new().into()
    };
    let list = side_list("Channels", actions, rows, fonts, theme);

    let detail: Element<'a, Message> = match selected.and_then(|id| ctx.s.channel(id)) {
        Some(channel) if channel.space_id == Some(ctx.space) => detail(ctx, channel.id),
        _ => container(
            column![
                label("Pick a channel to edit it.", fonts).color(theme.text.secondary.0),
                small(
                    if manage {
                        "Drag channels to reorder them, or onto a category to move them into it. \
                         Each channel can change what a role may do in it."
                    } else {
                        "You need the Manage channels permission to change channels."
                    },
                    fonts
                )
                .color(theme.text.tertiary.0),
            ]
            .spacing(8)
            .max_width(520),
        )
        .padding(20)
        .into(),
    };
    row![list, detail].height(Fill).into()
}

fn detail<'a>(ctx: &Ctx<'a, '_>, id: Id) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let channel = ctx.s.channel(id).expect("checked by the caller");
    let st = &ctx.s.spaces[&ctx.space];
    let manage = ctx.can(Permissions::MANAGE_CHANNELS);
    let edit_overrides = manage && ctx.can(Permissions::MANAGE_ROLES);
    let category = channel.kind == ChannelKind::Category;
    let name = channel.name.clone().unwrap_or_default();

    let mut col = column![].spacing(14).max_width(720);
    let kind = match channel.kind {
        ChannelKind::Category => "Category",
        ChannelKind::Voice => "Voice channel",
        _ => "Text channel",
    };
    col = col.push(
        column![
            strong(name, fonts),
            small(
                match channel.topic.as_deref() {
                    Some(topic) if !topic.is_empty() => format!("{kind} · {topic}"),
                    _ => kind.to_owned(),
                },
                fonts
            )
            .color(theme.text.tertiary.0),
        ]
        .spacing(2),
    );
    if manage {
        let mut buttons = row![
            button(label("Edit…", fonts))
                .padding(style::BUTTON_PAD)
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::EditChannel(id)),
            button(label("Move up", fonts))
                .padding(style::BUTTON_PAD)
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::MoveChannel(id, -1)),
            button(label("Move down", fonts))
                .padding(style::BUTTON_PAD)
                .style(move |_, s| style::secondary(theme, s))
                .on_press(Message::MoveChannel(id, 1)),
        ]
        .spacing(8);
        if category {
            buttons = buttons.push(
                button(label("+ Channel here", fonts))
                    .padding(style::BUTTON_PAD)
                    .style(move |_, s| style::secondary(theme, s))
                    .on_press(Message::CreateChannel(Some(id))),
            );
        }
        buttons = buttons.push(Space::new().width(Fill)).push(
            button(label(
                if category {
                    "Delete category"
                } else {
                    "Delete channel"
                },
                fonts,
            ))
            .padding(style::BUTTON_PAD)
            .style(move |_, s| style::danger(theme, s))
            .on_press(Message::DeleteChannel(id)),
        );
        col = col.push(buttons);
    }

    col = col.push(super::field_block(
        column![
            strong("Permissions", fonts),
            small(
                if category {
                    "What each role may do in this category. Channels inside it keep their own \
                     settings."
                } else {
                    "What each role may do in this channel. Inherit uses the role's normal \
                     permissions; Deny and Allow override them here."
                },
                fonts
            )
            .color(theme.text.tertiary.0),
        ]
        .spacing(4)
        .into(),
        theme,
    ));

    // Roles with an override first, then a picker for the others.
    let with: Vec<Id> = channel.overrides.iter().map(|o| o.role_id).collect();
    let mut chips = row![].spacing(4).align_y(Alignment::Center);
    let current = ctx.state.override_role;
    for role in st.roles.iter().filter(|r| with.contains(&r.id)) {
        let chosen = current == Some(role.id);
        chips = chips.push(
            button(small(role.name.clone(), fonts))
                .padding([3, 8])
                .style(move |_, s| style::tab(theme, s, chosen))
                .on_press(Message::OverrideRole(role.id)),
        );
    }
    let others: Vec<RoleChoice> = st
        .roles
        .iter()
        .filter(|r| !with.contains(&r.id))
        .map(|r| RoleChoice {
            id: r.id,
            name: r.name.clone(),
        })
        .collect();
    let picked = current
        .filter(|id| !with.contains(id))
        .and_then(|id| others.iter().find(|r| r.id == id).cloned());
    if !others.is_empty() {
        chips = chips.push(
            pick_list(others, picked, |r: RoleChoice| Message::OverrideRole(r.id))
                .padding(style::INPUT_PAD)
                .style(move |_, st| style::pick_list(theme, st))
                .menu_style(move |_| style::menu(theme))
                .placeholder("Add a role…")
                .font(fonts.ui)
                .text_size(fonts.small()),
        );
    }
    col = col.push(
        scrollable(chips)
            .style(move |_, st| style::scrollbar(theme, st))
            .direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::new().width(4).scroller_width(4),
            )),
    );

    if let Some(role_id) = current.filter(|r| st.role(*r).is_some()) {
        let ov = channel.overrides.iter().find(|o| o.role_id == role_id);
        let role_name = st.role(role_id).map(|r| r.name.clone()).unwrap_or_default();
        let mut perms = column![
            row![
                label(format!("Overrides for {role_name}"), fonts).width(Fill),
                button(small("Remove override", fonts))
                    .padding(style::BUTTON_PAD)
                    .style(move |_, s| style::secondary(theme, s))
                    .on_press_maybe(
                        (edit_overrides && ov.is_some()).then_some(Message::ClearOverride)
                    ),
            ]
            .align_y(Alignment::Center)
        ]
        .spacing(10);
        for group in PermissionGroup::ALL {
            let entries: Vec<_> = CATALOG
                .iter()
                .filter(|i| i.group == group && i.channel)
                .filter(|i| {
                    // Text channels do not use voice bits and the other way round.
                    match channel.kind {
                        ChannelKind::Text => i.group != PermissionGroup::Voice,
                        ChannelKind::Voice => true,
                        _ => true,
                    }
                })
                .collect();
            if entries.is_empty() {
                continue;
            }
            perms =
                perms.push(small(group.title().to_uppercase(), fonts).color(theme.text.tertiary.0));
            for info in entries {
                let state = Tri::of(ov, info.flag);
                let flag = info.flag;
                let toggle = |t: Tri, text: &'static str| {
                    let on = state == t;
                    let b = button(small(text, fonts))
                        .padding([2, 8])
                        .style(move |_, s| match (on, t) {
                            (true, Tri::Deny) => style::danger(theme, s),
                            (true, Tri::Allow) => style::primary(theme, s),
                            (true, Tri::Inherit) => style::tab(theme, s, true),
                            _ => style::secondary(theme, s),
                        });
                    if edit_overrides {
                        b.on_press(Message::SetOverride(flag, t))
                    } else {
                        b
                    }
                };
                perms = perms.push(
                    row![
                        column![
                            label(info.name, fonts),
                            small(info.description, fonts).color(theme.text.tertiary.0),
                        ]
                        .spacing(2)
                        .width(Fill),
                        row![
                            toggle(Tri::Deny, "✕ Deny"),
                            toggle(Tri::Inherit, "/ Inherit"),
                            toggle(Tri::Allow, "✓ Allow"),
                        ]
                        .spacing(2),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                );
            }
        }
        if !edit_overrides {
            perms = perms.push(
                small(
                    "Changing these needs both Manage channels and Manage roles.",
                    fonts,
                )
                .color(theme.text.warning.0),
            );
        }
        col = col.push(perms);
    }
    scrollable(container(col).padding(20).width(Fill))
        .style(move |_, st| style::scrollbar(theme, st))
        .height(Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

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
