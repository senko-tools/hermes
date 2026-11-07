//! The voice panel at the bottom of the sidebar (PLAN.md section 15.1), shown while in voice:
//! channel and space, connection quality, push-to-talk indicator, and mute, deafen, screen
//! share and disconnect buttons.

use hermes_client_core::{Core, VoiceStatus, view};
use iced::widget::{button, column, container, row};
use iced::{Alignment, Element, Fill};

use crate::font::Fonts;
use crate::message::Message;
use crate::theme::{Theme, style};
use crate::voice::VoiceMsg;
use crate::widget::icon::{self, icon};
use crate::widget::{caption, tip_at};

/// The panel's text, computed without widgets so it can be tested.
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    /// `Lounge · Friends`, or the DM's name.
    pub place: String,
    /// `38 ms · 0.0 % loss`, `Connecting…`, `Reconnecting (try 2)…`.
    pub quality: String,
    pub connected: bool,
    pub muted: bool,
    pub deafened: bool,
    /// `Some(held)` in push-to-talk mode.
    pub ptt: Option<bool>,
    /// My screen is being shared.
    pub sharing: bool,
}

/// `None` while not in voice.
pub fn panel(core: &Core, ptt_down: bool) -> Option<Panel> {
    let (inst, channel) = core.voice_channel()?;
    let place = {
        let store = core.store();
        let s = store.instance(&inst);
        match s.and_then(|s| Some((s, s.channel(channel)?))) {
            Some((s, c)) => match s.space_of(channel).and_then(|id| s.spaces.get(&id)) {
                Some(space) => {
                    format!("{} · {}", c.name.as_deref().unwrap_or(""), space.space.name)
                }
                None => view::channel_title(s, c),
            },
            None => "Voice".to_owned(),
        }
    };
    let status = core.voice_status();
    let quality = match &status {
        VoiceStatus::Connected { ping_ms, loss_pct } => {
            format!("{ping_ms} ms · {loss_pct:.1} % loss")
        }
        VoiceStatus::Connecting => "Connecting…".to_owned(),
        VoiceStatus::Reconnecting { attempt } => format!("Reconnecting (try {attempt})…"),
        VoiceStatus::Disconnected => "Disconnected".to_owned(),
    };
    let ptt = (core.audio_settings().mode == hermes_client_core::audio::InputMode::PushToTalk
        || core.voice_requires_ptt())
    .then_some(ptt_down);
    Some(Panel {
        place,
        quality,
        connected: matches!(status, VoiceStatus::Connected { .. }),
        muted: core.self_mute() || core.self_deaf(),
        deafened: core.self_deaf(),
        ptt,
        sharing: core.sharing_screen(),
    })
}

pub fn view<'a>(
    core: &Core,
    ptt_down: bool,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Option<Element<'a, Message>> {
    let p = panel(core, ptt_down)?;
    let (inst, channel) = core.voice_channel()?;
    let state_color = if p.connected {
        theme.text.success.0
    } else {
        theme.text.warning.0
    };
    let mut heading = row![
        icon(icon::WAVEFORM, 14.0).color(state_color),
        caption(
            if p.connected {
                "Voice connected"
            } else {
                "Voice connecting"
            },
            fonts
        )
        .font(fonts.bold())
        .color(state_color)
        .width(Fill),
    ]
    .spacing(style::GAP_S)
    .align_y(Alignment::Center);
    if let Some(held) = p.ptt {
        heading = heading.push(
            container(caption("PTT", fonts).font(fonts.bold()).color(if held {
                theme.buttons.primary.text.0
            } else {
                theme.text.tertiary.0
            }))
            .padding([0.0, style::GAP_S])
            .style(move |_| iced::widget::container::Style {
                background: held.then(|| theme.voice.speaking_ring.0.into()),
                border: iced::Border {
                    color: theme.general.border.0,
                    width: 1.0,
                    radius: style::radius_small(theme).into(),
                },
                ..iced::widget::container::Style::default()
            }),
        );
    }
    let place = button(
        column![
            heading,
            caption(p.place, fonts)
                .color(theme.text.secondary.0)
                .wrapping(iced::widget::text::Wrapping::None),
            caption(p.quality, fonts)
                .color(theme.text.tertiary.0)
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(2),
    )
    .padding([style::GAP_XS, style::GAP_S])
    .width(Fill)
    .style(move |_, s| style::sidebar_item(theme, s, false))
    .on_press(Message::Select(inst, channel));
    let toggle = |glyph: crate::widget::icon::Glyph, on: bool, hint: &'a str, message: VoiceMsg| {
        tip_at(
            button(
                container(icon(glyph, style::ICON))
                    .center_x(Fill)
                    .center_y(style::ICON_BUTTON),
            )
            .padding(0)
            .width(Fill)
            .style(move |_, s| {
                if on {
                    style::danger(theme, s)
                } else {
                    style::secondary(theme, s)
                }
            })
            .on_press(Message::Voice(message)),
            hint,
            iced::widget::tooltip::Position::Top,
            fonts,
            theme,
        )
    };
    let buttons = row![
        toggle(
            if p.muted { icon::MIC_OFF } else { icon::MIC },
            p.muted,
            if p.muted { "Unmute" } else { "Mute" },
            VoiceMsg::ToggleMute
        ),
        toggle(
            if p.deafened {
                icon::HEADPHONES_OFF
            } else {
                icon::HEADPHONES
            },
            p.deafened,
            if p.deafened { "Undeafen" } else { "Deafen" },
            VoiceMsg::ToggleDeafen
        ),
        // Sharing asks what to share first (the menu); a running share stops right away.
        toggle(
            icon::MONITOR_SHARE,
            p.sharing,
            if p.sharing {
                "Stop sharing"
            } else {
                "Share screen"
            },
            if p.sharing {
                VoiceMsg::ToggleShare
            } else {
                VoiceMsg::OpenShareMenu
            }
        ),
        tip_at(
            button(
                container(icon(icon::HANG_UP, style::ICON))
                    .center_x(Fill)
                    .center_y(style::ICON_BUTTON),
            )
            .padding(0)
            .width(Fill)
            .style(move |_, s| style::danger(theme, s))
            .on_press(Message::Voice(VoiceMsg::Leave)),
            "Leave voice",
            iced::widget::tooltip::Position::Top,
            fonts,
            theme,
        ),
    ]
    .spacing(style::GAP_S);
    Some(
        container(column![place, buttons].spacing(style::GAP_S))
            .padding([style::GAP_S, style::GAP_S])
            .width(Fill)
            .style(move |_| style::sidebar_footer(theme))
            .into(),
    )
}
