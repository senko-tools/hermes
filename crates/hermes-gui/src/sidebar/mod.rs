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
use iced::{Alignment, Border, Color, Element, Fill, Length};

use self::collapsed::Node;
use self::menu::Target;
use self::tree::{Badge, Row};
use crate::buffer::channel::message::{Images, Img};
use crate::drag::Item;
use crate::font::Fonts;
use crate::message::Message;
use crate::theme::{Theme, style};
use crate::voice::{JoinOn, VoiceMsg};
use crate::widget::icon::{self, icon};
use crate::widget::{avatar, icon_button, label, small, strong, tip, tip_at};

/// Indent per tree level.
const INDENT: f32 = 10.0;
/// Width of the chevron / channel icon column, so names line up across row kinds.
const LEAD: f32 = 16.0;

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
    images: &Images,
) -> Element<'a, Message> {
    let join_on = JoinOn::from_config(&core.config().voice.join_on);
    let mut list = column![].spacing(2).padding([8, 8]);
    for row in rows {
        let (element, drag) = match row {
            Row::Channel { drag, .. } | Row::Category { drag, .. } => {
                (row_view(row, join_on, fonts, theme), drag.as_ref())
            }
            _ => (row_view(row, join_on, fonts, theme), None),
        };
        list = list.push(draggable(element, drag, drop_target, theme));
    }
    list = list.push(Space::new().height(style::GAP));
    list = list.push(
        button(
            row![
                icon(icon::PLUS, 14.0),
                small("Add an instance", fonts).width(Fill)
            ]
            .spacing(style::GAP)
            .align_y(Alignment::Center),
        )
        .padding(style::ROW_PAD)
        .width(Fill)
        .style(move |_, s| style::ghost(theme, s, false))
        .on_press(Message::OpenAddInstance),
    );

    let header = container(
        row![
            icon(icon::FEATHER, 18.0).color(theme.buttons.primary.background.0),
            text("Hermes").font(fonts.bold()).size(fonts.size + 3.0),
        ]
        .spacing(style::GAP)
        .align_y(Alignment::Center),
    )
    .padding([0, 16])
    .height(style::HEADER)
    .width(Fill)
    .align_y(Alignment::Center)
    .style(move |_| style::pane_header(theme));

    let mut content = column![
        header,
        scrollable(list)
            .height(Fill)
            .style(move |_, s| style::scrollbar(theme, s)),
    ]
    .height(Fill);
    if let Some(update) = update {
        content = content.push(
            container(
                button(
                    row![
                        icon(icon::ARROW_DOWN, 14.0),
                        small(format!("Hermes {} is available", update.version), fonts),
                    ]
                    .spacing(style::GAP)
                    .align_y(Alignment::Center),
                )
                .width(Fill)
                .padding(style::ROW_PAD)
                .style(move |_, s| style::ghost(theme, s, true))
                .on_press(Message::OpenUpdate),
            )
            .padding([4, 8]),
        );
    }
    if let Some(panel) = voice_panel::view(core, ptt_down, fonts, theme) {
        content = content.push(panel);
    }
    container(content.push(user_panel(core, fonts, theme, images)))
        .width(f32::from(width))
        .height(Fill)
        .style(|_| style::sidebar(theme))
        .into()
}

/// Me: avatar with presence, name and status text (a click opens the status menu), then the
/// mentions and settings buttons.
fn user_panel<'a>(
    core: &Core,
    fonts: &Fonts,
    theme: &'a Theme,
    images: &Images,
) -> Element<'a, Message> {
    let first = core.instance_ids().first().cloned();
    let (name, status_text, picture) = first
        .as_ref()
        .and_then(|inst| {
            let store = core.store();
            let me = store.instance(inst)?.me.as_ref()?.user.clone();
            let picture =
                me.avatar
                    .as_ref()
                    .and_then(|a| match images.get(&(inst.clone(), a.id)) {
                        Some(Img::Ready(handle)) => Some(handle.clone()),
                        _ => None,
                    });
            Some((
                me.name().to_owned(),
                me.status_text.clone().unwrap_or_default(),
                picture,
            ))
        })
        .or_else(|| {
            first.as_ref().and_then(|id| {
                core.config()
                    .instance(id)
                    .map(|i| (i.username.clone(), String::new(), None))
            })
        })
        .unwrap_or_else(|| ("Not signed in".to_owned(), String::new(), None));
    use hermes_client_core::proto::models::PresenceStatus as P;
    let (dot_color, presence) = match core.presence() {
        P::Online => (theme.text.success.0, "Online"),
        P::Idle => (theme.text.warning.0, "Idle"),
        P::Dnd => (theme.text.error.0, "Do not disturb"),
        P::Invisible => (theme.text.tertiary.0, "Invisible"),
    };
    let subtitle = if status_text.is_empty() {
        presence.to_owned()
    } else {
        status_text
    };
    let footer = style::sidebar_footer(theme)
        .background
        .and_then(|b| match b {
            iced::Background::Color(c) => Some(c),
            _ => None,
        })
        .unwrap_or(theme.sidebar.background.0);
    let me_target = Target::Me(first);
    let me = mouse_area(
        button(
            row![
                avatar::with_dot(
                    avatar::view(&name, picture.as_ref(), 32.0, fonts),
                    32.0,
                    dot_color,
                    footer,
                ),
                column![
                    strong(name, fonts).wrapping(text::Wrapping::None),
                    small(subtitle, fonts)
                        .color(theme.text.tertiary.0)
                        .wrapping(text::Wrapping::None),
                ]
                .spacing(1),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding([4, 6])
        .width(Fill)
        .style(|_, s| style::sidebar_item(theme, s, false))
        .on_press(Message::OpenMenu(me_target.clone())),
    )
    .on_right_press(Message::OpenMenu(me_target));
    container(
        row![
            me,
            tip_at(
                icon_button(icon::AT_SIGN, false, theme)
                    .on_press(Message::OpenResults(crate::buffer::results::Key::Mentions)),
                "Mentions",
                iced::widget::tooltip::Position::Top,
                fonts,
                theme,
            ),
            tip_at(
                icon_button(icon::SETTINGS, false, theme).on_press(Message::OpenSettings),
                "Settings (Ctrl+,)",
                iced::widget::tooltip::Position::Top,
                fonts,
                theme,
            ),
        ]
        .spacing(2)
        .align_y(Alignment::Center),
    )
    .padding([8, 8])
    .style(move |_| style::sidebar_footer(theme))
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
            let color = status_color(status, theme);
            let mut content = row![
                chevron(*collapsed, theme),
                icon(icon::SERVER, 14.0).color(color),
                strong(inst.clone(), fonts)
                    .wrapping(text::Wrapping::None)
                    .width(Fill),
            ]
            .spacing(style::GAP)
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
        } => column![
            Space::new().height(4),
            item(
                row![
                    avatar::space_icon(name, 22.0, fonts),
                    text(name.clone())
                        .font(fonts.bold())
                        .size(fonts.size + 1.0)
                        .wrapping(text::Wrapping::None)
                        .width(Fill),
                    if *collapsed {
                        badge_view(*badge, fonts, theme)
                    } else {
                        Space::new().into()
                    },
                    // No tooltip: it would cover the menu this opens.
                    button(icon(icon::ELLIPSIS, 14.0))
                        .padding([3, 4])
                        .style(move |_, s| style::ghost(theme, s, false))
                        .on_press(Message::OpenMenu(Target::Space(inst.clone(), *id))),
                ]
                .spacing(style::GAP)
                .align_y(Alignment::Center),
                INDENT,
                false,
                theme,
                Message::Toggle(Node::Space(inst.clone(), *id)),
                Some(Target::Space(inst.clone(), *id)),
            ),
        ]
        .into(),
        Row::Category {
            inst,
            id,
            name,
            collapsed,
            badge,
            ..
        } => column![
            Space::new().height(6),
            item(
                row![
                    chevron(*collapsed, theme),
                    section_title(name, fonts, theme),
                    if *collapsed {
                        badge_view(*badge, fonts, theme)
                    } else {
                        Space::new().into()
                    },
                ]
                .spacing(style::GAP_S)
                .align_y(Alignment::Center),
                INDENT * 2.0,
                false,
                theme,
                Message::Toggle(Node::Category(inst.clone(), *id)),
                Some(Target::Channel(inst.clone(), *id)),
            ),
        ]
        .into(),
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
            let glyph = if *kind == ChannelKind::Voice {
                icon::VOLUME_2
            } else {
                icon::HASH
            };
            let unread = badge.unread && !*muted;
            let name = label(name.clone(), fonts).wrapping(text::Wrapping::None);
            let (name, icon_color) = if *selected {
                (
                    name.font(fonts.medium()).color(theme.text.primary.0),
                    theme.text.secondary.0,
                )
            } else if unread {
                (
                    name.font(fonts.bold()).color(theme.text.primary.0),
                    theme.text.secondary.0,
                )
            } else if *muted {
                (
                    name.color(theme.text.tertiary.0.scale_alpha(0.8)),
                    theme.text.tertiary.0.scale_alpha(0.8),
                )
            } else {
                (name.color(theme.text.secondary.0), theme.text.tertiary.0)
            };
            let content = row![
                icon(glyph, LEAD).color(icon_color),
                name.width(Fill),
                mention_count(badge.mentions, fonts, theme),
            ]
            .spacing(style::GAP)
            .align_y(Alignment::Center);
            let indent = INDENT * if *nested { 3.0 } else { 2.0 };
            let row = if *kind == ChannelKind::Voice {
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
            };
            with_unread_pill(row, unread && !*selected, theme)
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
            let ring = if *speaking {
                theme.voice.speaking_ring.0
            } else {
                Color::TRANSPARENT
            };
            let face = container(avatar::view(name, None, 20.0, fonts))
                .padding(2)
                .style(move |_| container::Style {
                    border: Border {
                        color: ring,
                        width: 2.0,
                        radius: 12.0.into(),
                    },
                    ..container::Style::default()
                });
            let mut content = row![
                face,
                small(name.clone(), fonts)
                    .color(if *speaking {
                        theme.text.primary.0
                    } else {
                        theme.text.secondary.0
                    })
                    .wrapping(text::Wrapping::None)
                    .width(Fill),
            ]
            .spacing(style::GAP)
            .align_y(Alignment::Center);
            if *deafened {
                content = content.push(icon(icon::HEADPHONE_OFF, 14.0).color(theme.voice.muted.0));
            } else if *muted {
                content = content.push(icon(icon::MIC_OFF, 14.0).color(theme.voice.muted.0));
            }
            let row = container(content).padding(iced::Padding {
                top: 1.0,
                bottom: 1.0,
                left: 8.0 + INDENT * 2.0 + LEAD + style::GAP,
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
                    chevron(*collapsed, theme),
                    section_title("Direct messages", fonts, theme),
                    if *collapsed {
                        badge_view(*badge, fonts, theme)
                    } else {
                        Space::new().into()
                    },
                ]
                .spacing(style::GAP_S)
                .align_y(Alignment::Center),
                INDENT,
                false,
                theme,
                Message::Toggle(Node::Dms(inst.clone())),
                None,
            );
            column![
                Space::new().height(8),
                row![
                    header,
                    tip(
                        button(icon(icon::PLUS, 14.0))
                            .padding([3, 4])
                            .style(|_, s| style::ghost(theme, s, false))
                            .on_press(Message::NewDm(inst.clone())),
                        "New direct message or group",
                        fonts,
                        theme,
                    ),
                ]
                .spacing(2)
                .align_y(Alignment::Center),
            ]
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
            let unread = badge.unread;
            let label_text = label(name.clone(), fonts).wrapping(text::Wrapping::None);
            let label_text = if unread && !*selected {
                label_text.font(fonts.bold()).color(theme.text.primary.0)
            } else if *selected {
                label_text.font(fonts.medium()).color(theme.text.primary.0)
            } else {
                label_text.color(theme.text.secondary.0)
            };
            let face: Element<'a, Message> = if *group {
                container(icon(icon::USERS, 13.0).color(theme.text.secondary.0))
                    .center_x(24)
                    .center_y(24)
                    .style(move |_| container::Style {
                        background: Some(theme.buttons.secondary.background.0.into()),
                        border: Border {
                            radius: 12.0.into(),
                            ..Border::default()
                        },
                        ..container::Style::default()
                    })
                    .into()
            } else {
                avatar::view(name, None, 24.0, fonts)
            };
            let row = item(
                row![
                    face,
                    label_text.width(Fill),
                    mention_count(badge.mentions, fonts, theme),
                ]
                .spacing(style::GAP)
                .align_y(Alignment::Center),
                INDENT * 2.0,
                *selected,
                theme,
                Message::Select(inst.clone(), *id),
                Some(Target::Channel(inst.clone(), *id)),
            );
            with_unread_pill(row, unread && !*selected, theme)
        }
    }
}

/// A small uppercase heading (categories, direct messages).
fn section_title<'a>(name: &str, fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
    text(name.to_uppercase())
        .font(fonts.bold())
        .size(fonts.small() - 0.5)
        .color(theme.text.tertiary.0)
        .wrapping(text::Wrapping::None)
        .width(Fill)
        .into()
}

/// A row with a short bar on its left edge, marking unread messages.
fn with_unread_pill<'a>(
    row: Element<'a, Message>,
    unread: bool,
    theme: &'a Theme,
) -> Element<'a, Message> {
    if !unread {
        return row;
    }
    let pill = container(Space::new().width(3).height(8)).style(move |_| container::Style {
        background: Some(theme.general.unread_indicator.0.into()),
        border: Border {
            radius: 2.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    });
    iced::widget::stack![row, container(pill).height(Fill).align_y(Alignment::Center),].into()
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
                    radius: style::RADIUS.into(),
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
    .padding(style::ROW_PAD)
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
    .padding(style::ROW_PAD)
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

fn chevron<'a>(collapsed: bool, theme: &'a Theme) -> Element<'a, Message> {
    icon(
        if collapsed {
            icon::CHEVRON_RIGHT
        } else {
            icon::CHEVRON_DOWN
        },
        12.0,
    )
    .color(theme.text.tertiary.0)
    .width(LEAD)
    .into()
}

/// A collapsed node: mention count if any, else an unread dot.
fn badge_view<'a>(badge: Badge, fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
    if badge.mentions > 0 {
        mention_count(badge.mentions, fonts, theme)
    } else if badge.unread {
        container(Space::new().width(6).height(6))
            .style(move |_| container::Style {
                background: Some(theme.general.unread_indicator.0.into()),
                border: Border {
                    radius: 3.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            })
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
        .size(fonts.small() - 1.0)
        .color(text_color),
    )
    .padding([1, 6])
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

fn status_color(status: &ConnStatus, theme: &Theme) -> Color {
    match status {
        ConnStatus::Connected => theme.text.success.0,
        ConnStatus::Connecting | ConnStatus::Reconnecting { .. } => theme.text.warning.0,
        ConnStatus::Failed { .. } | ConnStatus::LoggedOut => theme.text.error.0,
        ConnStatus::Offline => theme.text.tertiary.0,
    }
}
