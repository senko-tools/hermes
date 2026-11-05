//! The sidebar (PLAN.md section 15.1): two tabs, Spaces (a scrollable tree of instances,
//! spaces, categories and channels with their voice occupants) and Messages (the direct
//! messages of each instance), then the voice panel (while in voice) and the user panel.
//! Right-click opens a context menu.

pub mod collapsed;
pub mod menu;
pub mod tree;
pub mod voice_panel;

use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::{Core, InstanceId};
use iced::widget::{Space, button, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Border, Color, Element, Fill};

use self::collapsed::Node;
use self::menu::Target;
use self::tree::{Badge, Row, Tab};
use crate::buffer::channel::message::Images;
use crate::drag::Item;
use crate::font::Fonts;
use crate::message::Message;
use crate::theme::Theme;
use crate::theme::style::{self, RowState};
use crate::voice::{JoinOn, VoiceMsg};
use crate::widget::icon::{self, icon};
use crate::widget::{avatar, caption, icon_button, label, strong, tip_at};

/// Size of row icons.
const LEAD: f32 = style::ROW_ICON;

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
    menu: Option<&Target>,
    tab: Tab,
    badges: (Badge, Badge),
) -> Element<'a, Message> {
    let join_on = JoinOn::from_config(&core.config().voice.join_on);
    let mut list = column![].spacing(2).padding([style::GAP_S, style::GAP_S]);
    {
        // Dropped before the panels below, which read the store themselves: a second read
        // lock on this thread can deadlock while a writer waits.
        let store = core.store();
        let face = |inst: &InstanceId, user: Id| {
            avatar::picture(
                images,
                inst,
                store.instance(inst).and_then(|s| s.user(user)),
            )
        };
        for row in rows {
            let (element, drag) = match row {
                Row::Channel { drag, .. } | Row::Category { drag, .. } => (
                    row_view(row, join_on, fonts, theme, menu, &face),
                    drag.as_ref(),
                ),
                _ => (row_view(row, join_on, fonts, theme, menu, &face), None),
            };
            list = list.push(draggable(element, drag, drop_target, theme));
        }
    }
    list = list.push(Space::new().height(style::GAP_S));
    list = list.push(
        row_button(
            row![
                slot(icon(icon::PLUS, LEAD).color(theme.icons.default.0)),
                label("Add an instance", fonts)
                    .color(theme.text.secondary.0)
                    .width(Fill)
            ]
            .spacing(style::GAP_S)
            .align_y(Alignment::Center),
            0,
            RowState::Plain,
            false,
            theme,
        )
        .on_press(Message::OpenAddInstance),
    );

    let header = container(
        row![
            icon(icon::FEATHER, 18.0).color(theme.accent.base.0),
            crate::widget::title("Hermes", fonts),
        ]
        .spacing(style::GAP_S)
        .align_y(Alignment::Center),
    )
    .padding([0.0, style::GAP_L])
    .height(style::HEADER)
    .width(Fill)
    .align_y(Alignment::Center)
    .style(move |_| style::pane_header(theme));

    let mut content = column![
        header,
        tab_bar(tab, badges, fonts, theme),
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
                        caption(format!("Hermes {} is available", update.version), fonts),
                    ]
                    .spacing(style::GAP_S)
                    .align_y(Alignment::Center),
                )
                .width(Fill)
                .padding(style::ROW_PAD)
                .style(move |_, s| style::ghost(theme, s, true))
                .on_press(Message::OpenUpdate),
            )
            .padding([style::GAP_XS, style::GAP_S]),
        );
    }
    if let Some(panel) = voice_panel::view(core, ptt_down, fonts, theme) {
        content = content.push(panel);
    }
    container(content.push(user_panel(core, fonts, theme, images, menu)))
        .width(f32::from(width))
        .height(Fill)
        .style(|_| style::sidebar(theme))
        .into()
}

/// Spaces | Messages. The other tab shows a dot or the mention count when something there is
/// unread.
fn tab_bar<'a>(
    tab: Tab,
    (spaces, dms): (Badge, Badge),
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let one = |name: &'static str, glyph: char, this: Tab, badge: Badge| {
        let selected = tab == this;
        let mut content = row![
            icon(glyph, 16.0),
            label(name, fonts)
                .wrapping(text::Wrapping::None)
                .width(Fill),
        ]
        .spacing(style::GAP_S)
        .align_y(Alignment::Center);
        if !selected {
            if badge.mentions > 0 {
                content = content.push(mention_count(badge.mentions, fonts, theme));
            } else if badge.unread {
                content = content.push(icon(icon::DOT, 12.0).color(theme.text.primary.0));
            }
        }
        button(container(content).center_y(Fill))
            .height(style::ROW_H)
            .width(Fill)
            .padding([0.0, style::GAP_S])
            .style(move |_, s| style::tab(theme, s, selected))
            .on_press(Message::SidebarTab(this))
    };
    row![
        one("Spaces", icon::HOME, Tab::Spaces, spaces),
        one("Messages", icon::CHAT, Tab::Dms, dms),
    ]
    .spacing(style::GAP_XS)
    .padding(iced::Padding::new(style::GAP_S).bottom(0.0))
    .into()
}

/// Me: avatar with presence, name and status text (a click opens the status menu), then the
/// mentions and settings buttons.
fn user_panel<'a>(
    core: &Core,
    fonts: &Fonts,
    theme: &'a Theme,
    images: &Images,
    menu: Option<&Target>,
) -> Element<'a, Message> {
    let first = core.instance_ids().first().cloned();
    let (name, status_text, picture) = first
        .as_ref()
        .and_then(|inst| {
            let store = core.store();
            let me = store.instance(inst)?.me.as_ref()?.user.clone();
            let picture = avatar::picture(images, inst, Some(&me));
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
    let dot_color = style::my_presence(theme, core.presence());
    let presence = match core.presence() {
        P::Online => "Online",
        P::Idle => "Idle",
        P::Dnd => "Do not disturb",
        P::Invisible => "Invisible",
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
        .unwrap_or(theme.surface.deep.0);
    let me_target = Target::Me(first);
    let open = menu == Some(&me_target);
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
                    caption(subtitle, fonts)
                        .color(theme.text.tertiary.0)
                        .wrapping(text::Wrapping::None),
                ]
                .spacing(1),
            ]
            .spacing(style::GAP_S)
            .align_y(Alignment::Center),
        )
        .padding([style::GAP_XS, style::GAP_S])
        .width(Fill)
        .style(move |_, s| {
            if open {
                style::menu_open(theme)
            } else {
                style::sidebar_item(theme, s, false)
            }
        })
        .on_press(Message::OpenMenu(me_target.clone())),
    )
    .on_right_press(Message::OpenMenu(me_target));
    container(
        row![
            me,
            tip_at(
                icon_button(icon::AT, false, theme)
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
    .padding([style::GAP_S, style::GAP_S])
    .style(move |_| style::sidebar_footer(theme))
    .into()
}

fn row_view<'a>(
    row: &Row,
    join_on: JoinOn,
    fonts: &Fonts,
    theme: &'a Theme,
    menu: Option<&Target>,
    face: &dyn Fn(&InstanceId, Id) -> Option<iced::widget::image::Handle>,
) -> Element<'a, Message> {
    match row {
        Row::Instance {
            inst,
            status,
            collapsed,
        } => {
            let mut content = row![
                slot(icon(icon::SERVER, LEAD).color(status_color(status, theme))),
                strong(inst.clone(), fonts)
                    .wrapping(text::Wrapping::None)
                    .width(Fill),
            ]
            .spacing(style::GAP_S)
            .align_y(Alignment::Center);
            let logged_out = *status == ConnStatus::LoggedOut;
            content = if logged_out {
                content.push(caption("Sign in", fonts).color(theme.text.link.0))
            } else {
                content.push(chevron(*collapsed, theme))
            };
            let on_press = if logged_out {
                Message::SignIn(inst.clone())
            } else {
                Message::Toggle(Node::Instance(inst.clone()))
            };
            item(
                content,
                0,
                RowState::Plain,
                theme,
                on_press,
                Some(Target::Instance(inst.clone())),
                menu,
            )
        }
        Row::Space {
            inst,
            id,
            name,
            collapsed,
            badge,
        } => {
            let state = if *collapsed {
                RowState::of(false, badge.unread, badge.mentions > 0, false)
            } else {
                RowState::Plain
            };
            let content = row![
                slot(avatar::space_icon(name, SLOT, fonts)),
                crate::widget::title(name.clone(), fonts)
                    .color(theme.accent.highlight.0)
                    .wrapping(text::Wrapping::None)
                    .width(Fill),
                if *collapsed {
                    mention_count(badge.mentions, fonts, theme)
                } else {
                    Space::new().into()
                },
                // No tooltip: it would cover the menu this opens.
                button(icon(icon::MORE, LEAD))
                    .padding(2)
                    .style(move |_, s| style::ghost(theme, s, false))
                    .on_press(Message::OpenMenu(Target::Space(inst.clone(), *id))),
            ]
            .spacing(style::GAP_S)
            .align_y(Alignment::Center);
            column![
                Space::new().height(style::GAP_XS),
                item(
                    content,
                    1,
                    state,
                    theme,
                    Message::Toggle(Node::Space(inst.clone(), *id)),
                    Some(Target::Space(inst.clone(), *id)),
                    menu,
                ),
            ]
            .into()
        }
        Row::Category {
            inst,
            id,
            name,
            collapsed,
            badge,
            ..
        } => {
            let state = if *collapsed {
                RowState::of(false, badge.unread, badge.mentions > 0, false)
            } else {
                RowState::Plain
            };
            column![
                Space::new().height(style::GAP_S),
                item(
                    row![
                        slot(chevron(*collapsed, theme)),
                        section_title(name, fonts, theme),
                        if *collapsed {
                            mention_count(badge.mentions, fonts, theme)
                        } else {
                            Space::new().into()
                        },
                    ]
                    .spacing(style::GAP_S)
                    .align_y(Alignment::Center),
                    2,
                    state,
                    theme,
                    Message::Toggle(Node::Category(inst.clone(), *id)),
                    Some(Target::Channel(inst.clone(), *id)),
                    menu,
                ),
            ]
            .into()
        }
        Row::Channel {
            inst,
            id,
            kind,
            name,
            nested,
            badge,
            selected,
            muted,
            connected,
            ..
        } => {
            let glyph = if *kind == ChannelKind::Voice {
                icon::VOLUME
            } else {
                icon::HASH
            };
            let state = if *muted {
                RowState::of(*selected, false, false, *connected)
            } else {
                RowState::of(*selected, badge.unread, badge.mentions > 0, *connected)
            };
            let muted = *muted;
            let content = || -> Element<'a, Message> {
                row![
                    slot(icon(glyph, LEAD).color(glyph_color(state, muted, theme))),
                    name_text(name, state, muted, fonts, theme),
                    mention_count(badge.mentions, fonts, theme),
                ]
                .spacing(style::GAP_S)
                .align_y(Alignment::Center)
                .into()
            };
            let level = if *nested { 3 } else { 2 };
            if *kind == ChannelKind::Voice {
                voice_item(content, level, state, theme, inst, *id, join_on, menu)
            } else {
                item(
                    content(),
                    level,
                    state,
                    theme,
                    Message::Select(inst.clone(), *id),
                    Some(Target::Channel(inst.clone(), *id)),
                    menu,
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
            nested,
            dm,
            ..
        } => {
            let ring = if *speaking {
                theme.voice.speaking_ring.0
            } else {
                Color::TRANSPARENT
            };
            let picture = face(inst, *user);
            let face = container(avatar::view(name, picture.as_ref(), SLOT - 4.0, fonts))
                .padding(2)
                .style(move |_| container::Style {
                    border: Border {
                        color: ring,
                        width: 2.0,
                        radius: (SLOT / 2.0).into(),
                    },
                    ..container::Style::default()
                });
            let mut content = row![
                slot(face),
                label(name.clone(), fonts)
                    .color(if *speaking {
                        theme.text.primary.0
                    } else {
                        theme.text.secondary.0
                    })
                    .wrapping(text::Wrapping::None)
                    .width(Fill),
            ]
            .spacing(style::GAP_S)
            .align_y(Alignment::Center);
            if *deafened {
                content = content.push(icon(icon::HEADPHONES_OFF, 14.0).color(theme.voice.muted.0));
            } else if *muted {
                content = content.push(icon(icon::MIC_OFF, 14.0).color(theme.voice.muted.0));
            }
            let level = if *dm {
                2
            } else if *nested {
                4
            } else {
                3
            };
            // No vertical padding: the row's fixed height centers the avatar, which padding
            // would squeeze into an oval.
            let row = container(row![Space::new().width(indent(level)), content])
                .padding([0.0, style::ROW_PAD.left])
                .center_y(style::VOICE_ROW_H);
            if *me {
                row.into()
            } else {
                mouse_area(row)
                    .on_right_press(Message::Voice(VoiceMsg::UserMenu(inst.clone(), *user)))
                    .into()
            }
        }
        Row::Requests { count, .. } => column![
            Space::new().height(style::GAP_S),
            container(section_title(&format!("Requests ({count})"), fonts, theme))
                .padding([0.0, style::ROW_PAD.left + indent(1)]),
        ]
        .into(),
        Row::Request {
            inst,
            id,
            name,
            peer,
            group,
            selected,
        } => {
            let state = RowState::of(*selected, false, false, false);
            let target = Target::Channel(inst.clone(), *id);
            let face: Element<'a, Message> = if *group {
                icon(icon::USERS, 14.0).color(theme.text.secondary.0).into()
            } else {
                let picture = peer.and_then(|p| face(inst, p));
                avatar::view(name, picture.as_ref(), SLOT, fonts)
            };
            let small = |glyph: char, color: Color, message: Message| {
                button(icon(glyph, 14.0).color(color))
                    .padding(2)
                    .style(move |_, s| style::ghost(theme, s, false))
                    .on_press(message)
            };
            // Ignoring a group request leaves it (confirmed); a 1:1 request is closed for good.
            let ignore = if *group {
                menu::Action::LeaveGroup
            } else {
                menu::Action::CloseDm
            };
            item(
                row![
                    slot(face),
                    name_text(name, state, false, fonts, theme),
                    tip_at(
                        small(
                            icon::CHECK,
                            theme.accent.base.0,
                            Message::Menu(target.clone(), menu::Action::AcceptRequest),
                        ),
                        "Accept",
                        iced::widget::tooltip::Position::Top,
                        fonts,
                        theme,
                    ),
                    tip_at(
                        small(
                            icon::CLOSE,
                            theme.text.secondary.0,
                            Message::Menu(target.clone(), ignore),
                        ),
                        "Ignore",
                        iced::widget::tooltip::Position::Top,
                        fonts,
                        theme,
                    ),
                ]
                .spacing(style::GAP_XS)
                .align_y(Alignment::Center),
                1,
                state,
                theme,
                Message::Select(inst.clone(), *id),
                Some(target),
                menu,
            )
        }
        // Starting a conversation is the first entry of an instance's DMs.
        Row::NewDm { inst } => row_button(
            row![
                slot(icon(icon::ADD_USER, LEAD).color(theme.icons.default.0)),
                label("New message", fonts)
                    .color(theme.text.secondary.0)
                    .wrapping(text::Wrapping::None)
                    .width(Fill),
            ]
            .spacing(style::GAP_S)
            .align_y(Alignment::Center),
            1,
            RowState::Plain,
            false,
            theme,
        )
        .on_press(Message::NewDm(inst.clone()))
        .into(),
        Row::Dm {
            inst,
            id,
            name,
            peer,
            group,
            badge,
            selected,
            connected,
        } => {
            let state = RowState::of(*selected, badge.unread, badge.mentions > 0, *connected);
            let face: Element<'a, Message> = if *group {
                container(icon(icon::USERS, 12.0).color(theme.text.secondary.0))
                    .center_x(SLOT)
                    .center_y(SLOT)
                    .style(move |_| container::Style {
                        background: Some(theme.buttons.secondary.background.0.into()),
                        border: Border {
                            radius: (SLOT / 2.0).into(),
                            ..Border::default()
                        },
                        ..container::Style::default()
                    })
                    .into()
            } else {
                let picture = peer.and_then(|p| face(inst, p));
                avatar::view(name, picture.as_ref(), SLOT, fonts)
            };
            let row = item(
                row![
                    slot(face),
                    name_text(name, state, false, fonts, theme),
                    mention_count(badge.mentions, fonts, theme),
                ]
                .spacing(style::GAP_S)
                .align_y(Alignment::Center),
                1,
                state,
                theme,
                Message::Select(inst.clone(), *id),
                Some(Target::Channel(inst.clone(), *id)),
                menu,
            );
            if *group {
                return row;
            }
            // An X on hover closes a 1:1 DM (group DMs are left from their menu, confirmed).
            let close = button(icon(icon::CLOSE, 14.0))
                .padding(2)
                .style(move |_, s| style::ghost(theme, s, false))
                .on_press(Message::Menu(
                    Target::Channel(inst.clone(), *id),
                    menu::Action::CloseDm,
                ));
            iced::widget::hover(
                row,
                container(close)
                    .align_right(Fill)
                    .center_y(style::ROW_H)
                    .padding([0.0, style::GAP_XS]),
            )
        }
    }
}

/// Every row's leading glyph (chevron, icon, small avatar) sits in a slot this wide, so the names
/// after them line up.
const SLOT: f32 = 20.0;

/// Left padding of a row at tree `level`.
fn indent(level: u8) -> f32 {
    f32::from(level) * style::INDENT
}

fn slot<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content).center_x(SLOT).into()
}

/// A channel or DM name: bold when it has news, brighter when it is open.
fn name_text<'a>(
    name: &str,
    state: RowState,
    muted: bool,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let name = label(name.to_owned(), fonts).wrapping(text::Wrapping::None);
    match state {
        RowState::Unread | RowState::Mention => name.font(fonts.bold()).color(theme.text.primary.0),
        RowState::Selected | RowState::Connected => {
            name.font(fonts.medium()).color(theme.text.primary.0)
        }
        RowState::Plain if muted => name.color(theme.text.tertiary.0.scale_alpha(0.8)),
        RowState::Plain => name.color(theme.text.secondary.0),
    }
    .width(Fill)
    .into()
}

/// The channel icon takes the color of the row's state bar.
fn glyph_color(state: RowState, muted: bool, theme: &Theme) -> Color {
    match style::state_bar(theme, state) {
        Some(color) => color.scale_alpha(1.0 / color.a.max(0.01)),
        None if muted => theme.text.tertiary.0.scale_alpha(0.8),
        None => theme.icons.default.0,
    }
}

/// A small uppercase heading (categories, direct messages).
fn section_title<'a>(name: &str, fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
    crate::widget::section_title(name, fonts)
        .color(theme.text.tertiary.0)
        .width(Fill)
        .into()
}

/// `row` with the bar of its state along its left edge. The bar is always there (empty for
/// states without one), so the row keeps the same widget tree when its state changes: a new
/// tree would reset the row's widget state and swallow the first click of a double-click.
fn with_state_bar<'a>(
    row: impl Into<Element<'a, Message>>,
    state: RowState,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let color = style::state_bar(theme, state).unwrap_or(Color::TRANSPARENT);
    let width = if color.a > 0.0 {
        f32::from(theme.shape.indicator_width)
    } else {
        0.0
    };
    let bar = container(Space::new().width(width).height(Fill)).style(move |_| container::Style {
        background: Some(color.into()),
        border: Border {
            radius: if theme.shape.row_radius > 0 {
                (width / 2.0).into()
            } else {
                0.0.into()
            },
            ..Border::default()
        },
        ..container::Style::default()
    });
    iced::widget::stack![row.into(), bar].into()
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
                    radius: style::row_radius(theme).into(),
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

/// The look of a row at tree `level`: `ROW_H` tall, its content centered.
fn row_button<'a>(
    content: impl Into<Element<'a, Message>>,
    level: u8,
    state: RowState,
    open: bool,
    theme: &'a Theme,
) -> iced::widget::Button<'a, Message> {
    button(
        container(
            row![Space::new().width(indent(level)), content.into()].align_y(Alignment::Center),
        )
        .center_y(Fill),
    )
    .height(style::ROW_H)
    .padding([0.0, style::ROW_PAD.left])
    .width(Fill)
    .style(move |_, s| {
        if open {
            style::menu_open(theme)
        } else {
            style::sidebar_row(theme, s, state)
        }
    })
}

/// A clickable row; right-click opens the menu for `target`.
fn item<'a>(
    content: impl Into<Element<'a, Message>>,
    level: u8,
    state: RowState,
    theme: &'a Theme,
    on_press: Message,
    target: Option<Target>,
    menu: Option<&Target>,
) -> Element<'a, Message> {
    let open = menu.is_some() && menu == target.as_ref();
    let button = row_button(content, level, state, open, theme).on_press(on_press);
    let row: Element<'a, Message> = match target {
        Some(target) => mouse_area(button)
            .on_right_press(Message::OpenMenu(target))
            .into(),
        None => button.into(),
    };
    with_state_bar(row, state, theme)
}

#[allow(clippy::too_many_arguments)]
/// A voice channel row: a click opens its voice room; joining takes a double-click unless
/// `voice.join_on = "single_click"`.
fn voice_item<'a>(
    content: impl Fn() -> Element<'a, Message>,
    level: u8,
    state: RowState,
    theme: &'a Theme,
    inst: &InstanceId,
    channel: Id,
    join_on: JoinOn,
    menu: Option<&Target>,
) -> Element<'a, Message> {
    let open = menu == Some(&Target::Channel(inst.clone(), channel));
    // Not a button: a button would take the clicks the double-click needs. The row is drawn
    // twice, plain and hovered, and `hover` shows the second while the pointer is over it.
    let look = |status| {
        let st = if open {
            style::menu_open(theme)
        } else {
            style::sidebar_row(theme, status, state)
        };
        container(row![Space::new().width(indent(level)), content()].align_y(Alignment::Center))
            .center_y(style::ROW_H)
            .width(Fill)
            .padding([0.0, style::ROW_PAD.left])
            .style(move |_| container::Style {
                background: st.background,
                text_color: Some(st.text_color),
                border: st.border,
                ..container::Style::default()
            })
    };
    let look = iced::widget::hover(
        look(iced::widget::button::Status::Active),
        look(iced::widget::button::Status::Hovered),
    );
    let area = mouse_area(look)
        .on_right_press(Message::OpenMenu(Target::Channel(inst.clone(), channel)))
        .interaction(iced::mouse::Interaction::Pointer);
    let area = match join_on {
        JoinOn::SingleClick => area.on_press(Message::Voice(VoiceMsg::SelectAndJoin(
            inst.clone(),
            channel,
        ))),
        JoinOn::DoubleClick => area
            .on_press(Message::Select(inst.clone(), channel))
            .on_double_click(Message::Voice(VoiceMsg::Join(inst.clone(), channel))),
    };
    with_state_bar(area, state, theme)
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
    .into()
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
        .size(fonts.caption())
        .color(text_color),
    )
    .padding([1.0, style::GAP_S])
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
