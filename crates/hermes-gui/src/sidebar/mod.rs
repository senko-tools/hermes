//! The sidebar (PLAN.md section 15.1): one scrollable tree of instances, spaces, categories,
//! channels with their voice occupants, and the direct messages of each instance, then the voice
//! panel (while in voice) and the user panel. Right-click opens a context menu.

pub mod collapsed;
pub mod menu;
pub mod tree;
pub mod voice_panel;

use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::{Core, InstanceId};
use iced::widget::{Space, button, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Border, Element, Fill, Length};

use self::collapsed::Node;
use self::menu::Target;
use self::tree::{Badge, Row};
use crate::drag::Item;
use crate::font::Fonts;
use crate::message::Message;
use crate::theme::{Theme, style};
use crate::voice::{JoinOn, VoiceMsg};
use crate::widget::{label, small, strong};

const INDENT: f32 = 12.0;

#[allow(clippy::too_many_arguments)]
pub fn view<'a>(
    core: &Core,
    rows: &[Row],
    fonts: &Fonts,
    theme: &'a Theme,
    width: u16,
    ptt_down: bool,
    drop_target: Option<&Item>,
    update: Option<&hermes_client_core::update::Update>,
) -> Element<'a, Message> {
    let join_on = JoinOn::from_config(&core.config().voice.join_on);
    let mut list = column![].spacing(1).padding([0, 4]);
    for row in rows {
        let (element, drag) = match row {
            Row::Channel { drag, .. } | Row::Category { drag, .. } => {
                (row_view(row, join_on, fonts, theme), drag.as_ref())
            }
            _ => (row_view(row, join_on, fonts, theme), None),
        };
        list = list.push(draggable(element, drag, drop_target, theme));
    }
    list = list.push(
        button(small("+ Add instance", fonts).color(theme.text.secondary.0))
            .padding([6, 8])
            .width(Fill)
            .style(|_, s| style::sidebar_item(theme, s, false))
            .on_press(Message::OpenAddInstance),
    );

    // Me: name, presence and status text; a click opens the status menu.
    let first = core.instance_ids().first().cloned();
    let (name, status_text) = first
        .as_ref()
        .and_then(|inst| {
            let store = core.store();
            let me = store.instance(inst)?.me.as_ref()?.user.clone();
            Some((
                me.name().to_owned(),
                me.status_text.clone().unwrap_or_default(),
            ))
        })
        .or_else(|| {
            first.as_ref().and_then(|id| {
                core.config()
                    .instance(id)
                    .map(|i| (i.username.clone(), String::new()))
            })
        })
        .unwrap_or_else(|| ("Not signed in".to_owned(), String::new()));
    use hermes_client_core::proto::models::PresenceStatus as P;
    let (dot, dot_color, presence) = match core.presence() {
        P::Online => ("●", theme.text.success.0, "Online"),
        P::Idle => ("●", theme.text.warning.0, "Idle"),
        P::Dnd => ("●", theme.text.error.0, "Do not disturb"),
        P::Invisible => ("○", theme.text.tertiary.0, "Invisible"),
    };
    let subtitle = if status_text.is_empty() {
        presence.to_owned()
    } else {
        status_text
    };
    let me_target = Target::Me(first);
    let me = mouse_area(
        button(
            row![
                label(dot, fonts).color(dot_color),
                column![
                    strong(name, fonts),
                    small(subtitle, fonts)
                        .color(theme.text.tertiary.0)
                        .wrapping(text::Wrapping::None),
                ]
                .spacing(0),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([2, 4])
        .width(Fill)
        .style(|_, s| style::sidebar_item(theme, s, false))
        .on_press(Message::OpenMenu(me_target.clone())),
    )
    .on_right_press(Message::OpenMenu(me_target));
    let user_panel = container(
        row![
            me,
            crate::widget::tip(
                button(small("@", fonts))
                    .padding([4, 8])
                    .style(|_, status| style::secondary(theme, status))
                    .on_press(Message::OpenResults(crate::buffer::results::Key::Mentions)),
                "Mentions: messages that mention you",
                fonts,
                theme,
            ),
            button(small("Settings", fonts))
                .style(|_, status| style::secondary(theme, status))
                .on_press(Message::OpenSettings),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding(6)
    .style(|_| style::pane_header(theme));

    let mut content = column![
        container(strong("Hermes", fonts)).padding([10, 12]),
        scrollable(list).height(Fill),
    ]
    .height(Fill);
    if let Some(panel) = voice_panel::view(core, ptt_down, fonts, theme) {
        content = content.push(panel);
    }
    if let Some(update) = update {
        content = content.push(
            button(
                small(format!("Hermes {} is available", update.version), fonts)
                    .color(theme.text.link.0),
            )
            .width(Fill)
            .padding([6, 10])
            .style(|_, s| style::sidebar_item(theme, s, false))
            .on_press(Message::OpenUpdate),
        );
    }
    container(content.push(user_panel))
        .width(f32::from(width))
        .height(Fill)
        .style(|_| style::sidebar(theme))
        .into()
}

fn row_view<'a>(
    row: &Row,
    join_on: JoinOn,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
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
                crate::widget::tip(
                    button(small("⋮", fonts).color(theme.text.tertiary.0))
                        .padding([0, 6])
                        .style(move |_, s| style::sidebar_item(theme, s, false))
                        .on_press(Message::OpenMenu(Target::Space(inst.clone(), *id))),
                    "Space menu: settings, invite, channels",
                    fonts,
                    theme,
                ),
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
            ..
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
            Some(Target::Channel(inst.clone(), *id)),
        ),
        Row::Channel {
            inst,
            id,
            kind,
            name,
            nested,
            badge,
            selected,
            muted,
            ..
        } => {
            let icon = if *kind == ChannelKind::Voice {
                "♪"
            } else {
                "#"
            };
            let name = label(name.clone(), fonts);
            let name = if badge.unread || *selected {
                name.font(fonts.bold()).color(theme.text.primary.0)
            } else if *muted {
                name.color(theme.text.tertiary.0)
            } else {
                name.color(theme.text.secondary.0)
            };
            let content = row![
                label(icon, fonts).color(theme.text.tertiary.0),
                name.width(Fill),
                mention_count(badge.mentions, fonts, theme),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            let indent = INDENT * if *nested { 3.0 } else { 2.0 };
            if *kind == ChannelKind::Voice {
                voice_item(content, indent, *selected, theme, inst, *id, join_on)
            } else {
                item(
                    content,
                    indent,
                    *selected,
                    theme,
                    Message::Select(inst.clone(), *id),
                    Some(Target::Channel(inst.clone(), *id)),
                )
            }
        }
        Row::VoiceUser {
            inst,
            user,
            name,
            muted,
            deafened,
            speaking,
            me,
            ..
        } => {
            let (dot, color) = if *speaking {
                ("●", theme.voice.speaking_ring.0)
            } else {
                ("○", theme.text.tertiary.0)
            };
            let mut content = row![
                small(dot, fonts).color(color),
                small(name.clone(), fonts).width(Fill),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            if *deafened {
                content = content.push(small("deaf", fonts).color(theme.voice.muted.0));
            } else if *muted {
                content = content.push(small("muted", fonts).color(theme.voice.muted.0));
            }
            let row = container(content).padding(iced::Padding {
                top: 2.0,
                bottom: 2.0,
                left: 8.0 + INDENT * 4.0,
                right: 8.0,
            });
            if *me {
                row.into()
            } else {
                mouse_area(row)
                    .on_right_press(Message::Voice(VoiceMsg::UserMenu(inst.clone(), *user)))
                    .into()
            }
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

/// A row that can be dragged onto another (reports the pointer entering and leaving it), with
/// a marker while a drag is over it.
fn draggable<'a>(
    element: Element<'a, Message>,
    drag: Option<&Item>,
    drop_target: Option<&Item>,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let Some(item) = drag else {
        return element;
    };
    let targeted = drop_target == Some(item);
    let element: Element<'a, Message> = if targeted {
        container(element)
            .style(move |_| container::Style {
                border: Border {
                    color: theme.pane.focused_border.0,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..container::Style::default()
            })
            .into()
    } else {
        element
    };
    mouse_area(element)
        .on_enter(Message::DragHover(Some(item.clone())))
        .on_exit(Message::DragLeave(item.clone()))
        .into()
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

/// A voice channel row: a click selects it (and opens its chat); joining takes a double-click
/// unless `voice.join_on = "single_click"`.
fn voice_item<'a>(
    content: impl Into<Element<'a, Message>>,
    indent: f32,
    selected: bool,
    theme: &'a Theme,
    inst: &InstanceId,
    channel: Id,
    join_on: JoinOn,
) -> Element<'a, Message> {
    // No `on_press`: the button only draws; the mouse area takes the clicks.
    let look = button(row![
        Space::new().width(Length::Fixed(indent)),
        content.into()
    ])
    .padding([4, 8])
    .width(Fill)
    .style(move |_, s| style::sidebar_item(theme, s, selected));
    let area = mouse_area(look)
        .on_right_press(Message::OpenMenu(Target::Channel(inst.clone(), channel)))
        .interaction(iced::mouse::Interaction::Pointer);
    match join_on {
        JoinOn::SingleClick => area.on_press(Message::Voice(VoiceMsg::SelectAndJoin(
            inst.clone(),
            channel,
        ))),
        JoinOn::DoubleClick => area
            .on_press(Message::Select(inst.clone(), channel))
            .on_double_click(Message::Voice(VoiceMsg::Join(inst.clone(), channel))),
    }
    .into()
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
