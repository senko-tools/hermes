//! The VoiceRoom buffer (PLAN.md section 15.1): the participants of a voice room with speaking
//! rings and mute/deafen state. Right-click a participant for the volume menu.

use hermes_client_core::proto::id::Id;
use hermes_client_core::{Core, InstanceId};
use iced::widget::{Space, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Border, Color, Element, Fill, Length};

use crate::font::Fonts;
use crate::message::Message;
use crate::theme::{Theme, style};
use crate::voice::VoiceMsg;
use crate::widget::{ButtonKind, text_button};
use crate::widget::{caption, label};

const AVATAR: f32 = 64.0;
const CARD_WIDTH: f32 = 120.0;

/// One participant card.
#[derive(Debug, Clone, PartialEq)]
pub struct Participant {
    pub user: Id,
    pub name: String,
    pub speaking: bool,
    pub muted: bool,
    pub deafened: bool,
    /// Your volume for them (1.0 = 100 %), or `None` for yourself.
    pub volume: Option<f32>,
    pub locally_muted: bool,
}

/// The people in the room of `channel`, in join order.
pub fn participants(core: &Core, inst: &InstanceId, channel: Id) -> Vec<Participant> {
    let store = core.store();
    let Some(s) = store.instance(inst) else {
        return Vec::new();
    };
    let me = s.me_id();
    let space = s.space_of(channel);
    let mut states: Vec<_> = s
        .voice_states
        .values()
        .filter(|v| v.channel_id == channel)
        .copied()
        .collect();
    states.sort_by_key(|v| (v.joined_at, v.user_id));
    states
        .into_iter()
        .map(|v| {
            let own = Some(v.user_id) == me;
            Participant {
                user: v.user_id,
                name: s.display_name(space, v.user_id),
                speaking: core.is_speaking(inst, v.user_id),
                muted: v.self_mute || v.server_mute,
                deafened: v.self_deaf || v.server_deaf,
                volume: (!own).then(|| core.user_volume(inst, v.user_id)),
                locally_muted: !own && core.user_muted(inst, v.user_id),
            }
        })
        .collect()
}

pub fn view<'a>(
    core: &Core,
    inst: &InstanceId,
    channel: Id,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let joined = core.voice_channel() == Some((inst.clone(), channel));
    let people = participants(core, inst, channel);

    let action = if joined {
        text_button("Disconnect", ButtonKind::Danger, fonts, theme)
            .on_press(Message::Voice(VoiceMsg::Leave))
    } else {
        text_button("Join voice", ButtonKind::Primary, fonts, theme)
            .on_press(Message::Voice(VoiceMsg::Join(inst.clone(), channel)))
    };
    let summary = match people.len() {
        0 => "Nobody is here yet".to_owned(),
        1 => "1 person".to_owned(),
        n => format!("{n} people"),
    };
    let top = row![
        label(summary, fonts)
            .color(theme.text.secondary.0)
            .width(Fill),
        action,
    ]
    .spacing(style::GAP_S)
    .align_y(Alignment::Center);

    let mut grid = row![].spacing(style::GAP);
    for p in people {
        grid = grid.push(card(inst, p, fonts, theme));
    }
    column![
        top,
        scrollable(grid.wrap().vertical_spacing(12))
            .style(move |_, st| style::scrollbar(theme, st))
            .height(Fill)
    ]
    .spacing(style::GAP_L)
    .padding(style::GAP_L)
    .into()
}

fn card<'a>(
    inst: &InstanceId,
    p: Participant,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let ring = if p.speaking {
        theme.voice.speaking_ring.0
    } else {
        Color::TRANSPARENT
    };
    let initial: String = p
        .name
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    let avatar = container(text(initial).font(fonts.bold()).size(fonts.heading()))
        .center(Length::Fixed(AVATAR))
        .style(move |_| container::Style {
            text_color: Some(theme.text.primary.0),
            background: Some(theme.surface.hover.0.into()),
            border: Border {
                color: ring,
                width: 3.0,
                radius: (AVATAR / 2.0).into(),
            },
            ..container::Style::default()
        });
    let state = if p.deafened {
        "deafened"
    } else if p.muted {
        "muted"
    } else if p.locally_muted {
        "muted for you"
    } else {
        ""
    };
    let mut info = column![label(p.name, fonts)]
        .spacing(2)
        .align_x(Alignment::Center);
    if !state.is_empty() {
        info = info.push(caption(state, fonts).color(theme.voice.muted.0));
    }
    if let Some(volume) = p.volume.filter(|v| (*v - 1.0).abs() > 0.001) {
        info = info.push(
            caption(format!("volume {:.0} %", volume * 100.0), fonts).color(theme.text.tertiary.0),
        );
    }
    let content = column![avatar, info, Space::new().height(4)]
        .spacing(style::GAP_S)
        .align_x(Alignment::Center)
        .width(Length::Fixed(CARD_WIDTH));
    let area = mouse_area(content);
    if p.volume.is_some() {
        area.on_right_press(Message::Voice(VoiceMsg::UserMenu(inst.clone(), p.user)))
            .into()
    } else {
        area.into()
    }
}
