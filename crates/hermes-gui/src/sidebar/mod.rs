//! The sidebar (PLAN.md section 15.1): one scrollable tree of instances, spaces, categories,
//! channels with their voice occupants, and the direct messages of each instance, then the user
//! panel. Right-click opens a context menu.

pub mod collapsed;
pub mod menu;
pub mod tree;

use hermes_client_core::Core;
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::models::ChannelKind;
use iced::widget::{Space, button, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Border, Element, Fill, Length};

use self::collapsed::Node;
use self::menu::Target;
use self::tree::{Badge, Row};
use crate::font::Fonts;
use crate::message::Message;
use crate::theme::{Theme, style};
use crate::widget::{label, small, strong};

const INDENT: f32 = 12.0;

pub fn view<'a>(
    core: &Core,
    rows: &[Row],
    fonts: &Fonts,
    theme: &'a Theme,
    width: u16,
) -> Element<'a, Message> {
    let mut list = column![].spacing(1).padding([0, 4]);
    for row in rows {
        list = list.push(row_view(row, fonts, theme));
    }
    list = list.push(
        button(small("+ Add instance", fonts).color(theme.text.secondary.0))
            .padding([6, 8])
            .width(Fill)
            .style(|_, s| style::sidebar_item(theme, s, false))
            .on_press(Message::OpenAddInstance),
    );

    let user = core
        .instance_ids()
        .first()
        .and_then(|id| core.config().instance(id).map(|i| i.username.clone()))
        .unwrap_or_else(|| "Not signed in".to_owned());
    let user_panel = container(
        row![
            strong(user, fonts).width(Fill),
            button(small("Settings", fonts))
                .style(|_, status| style::secondary(theme, status))
                .on_press(Message::OpenSettings),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding(8)
    .style(|_| style::pane_header(theme));

    container(
        column![
            container(strong("Hermes", fonts)).padding([10, 12]),
            scrollable(list).height(Fill),
            user_panel,
        ]
        .height(Fill),
    )
    .width(f32::from(width))
    .height(Fill)
    .style(|_| style::sidebar(theme))
    .into()
}

fn row_view<'a>(row: &Row, fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
    match row {
        Row::Instance {
            inst,
            status,
            collapsed,
        } => {
            let (dot, color) = status_dot(status, theme);
            let mut content = row![
                arrow(*collapsed, fonts, theme),
                label(dot, fonts).color(color),
                strong(inst.clone(), fonts).width(Fill),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            let logged_out = *status == ConnStatus::LoggedOut;
            if logged_out {
                content = content.push(small("Sign in", fonts).color(theme.text.link.0));
            }
            let on_press = if logged_out {
                Message::SignIn(inst.clone())
            } else {
                Message::Toggle(Node::Instance(inst.clone()))
            };
            item(
                content,
                0.0,
                false,
                theme,
                on_press,
                Some(Target::Instance(inst.clone())),
            )
        }
        Row::Space {
            inst,
            id,
            name,
            collapsed,
            badge,
        } => item(
            row![
                arrow(*collapsed, fonts, theme),
                label(name.clone(), fonts).font(fonts.medium()).width(Fill),
                badge_view(*badge, fonts, theme),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            INDENT,
            false,
            theme,
            Message::Toggle(Node::Space(inst.clone(), *id)),
            Some(Target::Space(inst.clone(), *id)),
        ),
        Row::Category {
            inst,
            id,
            name,
            collapsed,
            badge,
        } => item(
            row![
                arrow(*collapsed, fonts, theme),
                small(name.to_uppercase(), fonts)
                    .color(theme.text.tertiary.0)
                    .width(Fill),
                badge_view(*badge, fonts, theme),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            INDENT * 2.0,
            false,
            theme,
            Message::Toggle(Node::Category(inst.clone(), *id)),
            None,
        ),
        Row::Channel {
            inst,
            id,
            kind,
            name,
            nested,
            badge,
            selected,
        } => {
            let icon = if *kind == ChannelKind::Voice {
                "♪"
            } else {
                "#"
            };
            let name = label(name.clone(), fonts);
            let name = if badge.unread || *selected {
                name.font(fonts.bold()).color(theme.text.primary.0)
            } else {
                name.color(theme.text.secondary.0)
            };
            item(
                row![
                    label(icon, fonts).color(theme.text.tertiary.0),
                    name.width(Fill),
                    mention_count(badge.mentions, fonts, theme),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                INDENT * if *nested { 3.0 } else { 2.0 },
                *selected,
                theme,
                Message::Select(inst.clone(), *id),
                Some(Target::Channel(inst.clone(), *id)),
            )
        }
        Row::VoiceUser {
            name,
            muted,
            deafened,
            ..
        } => {
            let mut content = row![
                small("●", fonts).color(theme.text.success.0),
                small(name.clone(), fonts).width(Fill),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            if *deafened {
                content = content.push(small("deaf", fonts).color(theme.voice.muted.0));
            } else if *muted {
                content = content.push(small("muted", fonts).color(theme.voice.muted.0));
            }
            container(content)
                .padding(iced::Padding {
                    top: 2.0,
                    bottom: 2.0,
                    left: 8.0 + INDENT * 4.0,
                    right: 8.0,
                })
                .into()
        }
        Row::Dms {
            inst,
            collapsed,
            badge,
        } => {
            let header = item(
                row![
                    arrow(*collapsed, fonts, theme),
                    small("DIRECT MESSAGES", fonts)
                        .color(theme.text.tertiary.0)
                        .width(Fill),
                    badge_view(*badge, fonts, theme),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                INDENT,
                false,
                theme,
                Message::Toggle(Node::Dms(inst.clone())),
                None,
            );
            row![
                header,
                button(label("+", fonts).color(theme.text.secondary.0))
                    .padding([2, 8])
                    .style(|_, s| style::sidebar_item(theme, s, false))
                    .on_press(Message::NewDm(inst.clone())),
            ]
            .align_y(Alignment::Center)
            .into()
        }
        Row::Dm {
            inst,
            id,
            name,
            group,
            badge,
            selected,
        } => {
            let name = label(name.clone(), fonts);
            let name = if badge.unread || *selected {
                name.font(fonts.bold()).color(theme.text.primary.0)
            } else {
                name.color(theme.text.secondary.0)
            };
            item(
                row![
                    label(if *group { "≡" } else { "@" }, fonts).color(theme.text.tertiary.0),
                    name.width(Fill),
                    mention_count(badge.mentions, fonts, theme),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                INDENT * 2.0,
                *selected,
                theme,
                Message::Select(inst.clone(), *id),
                Some(Target::Channel(inst.clone(), *id)),
            )
        }
    }
}

/// A clickable row; right-click opens the menu for `target`.
fn item<'a>(
    content: impl Into<Element<'a, Message>>,
    indent: f32,
    selected: bool,
    theme: &'a Theme,
    on_press: Message,
    target: Option<Target>,
) -> Element<'a, Message> {
    let button = button(row![
        Space::new().width(Length::Fixed(indent)),
        content.into()
    ])
    .padding([4, 8])
    .width(Fill)
    .style(move |_, s| style::sidebar_item(theme, s, selected))
    .on_press(on_press);
    match target {
        Some(target) => mouse_area(button)
            .on_right_press(Message::OpenMenu(target))
            .into(),
        None => button.into(),
    }
}

fn arrow<'a>(collapsed: bool, fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
    container(small(if collapsed { "▸" } else { "▾" }, fonts).color(theme.text.tertiary.0))
        .width(10)
        .into()
}

/// A collapsed node: mention count if any, else an unread dot.
fn badge_view<'a>(badge: Badge, fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
    if badge.mentions > 0 {
        mention_count(badge.mentions, fonts, theme)
    } else if badge.unread {
        small("●", fonts)
            .color(theme.general.unread_indicator.0)
            .into()
    } else {
        Space::new().into()
    }
}

fn mention_count<'a>(count: u32, fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
    if count == 0 {
        return Space::new().into();
    }
    let text_color = theme.buttons.danger.text.0;
    container(
        text(if count > 99 {
            "99+".to_owned()
        } else {
            count.to_string()
        })
        .font(fonts.bold())
        .size(fonts.small())
        .color(text_color),
    )
    .padding([0, 6])
    .style(move |_| container::Style {
        background: Some(theme.buttons.danger.background.0.into()),
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    })
    .into()
}

fn status_dot(status: &ConnStatus, theme: &Theme) -> (&'static str, iced::Color) {
    match status {
        ConnStatus::Connected => ("●", theme.text.success.0),
        ConnStatus::Connecting | ConnStatus::Reconnecting { .. } => ("●", theme.text.warning.0),
        ConnStatus::Failed { .. } | ConnStatus::LoggedOut => ("●", theme.text.error.0),
        ConnStatus::Offline => ("○", theme.text.tertiary.0),
    }
}
