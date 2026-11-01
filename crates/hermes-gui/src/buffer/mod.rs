//! Buffers: what a pane shows (PLAN.md section 15.1).

pub mod channel;
pub mod page;
pub mod results;
pub mod settings;
pub mod settings_tabs;
pub mod space_settings;
pub mod voice_room;

use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Buffer {
    Empty,
    /// A text channel, a voice channel's text chat, a DM or a group DM.
    Channel {
        inst: InstanceId,
        channel: Id,
    },
    Settings,
    /// A space's settings (PLAN.md sections 15.1 and 15.4).
    SpaceSettings {
        inst: InstanceId,
        space: Id,
    },
    /// Search results, mentions or pins.
    Results(results::Key),
    /// The participants of a voice channel's or DM's voice room.
    VoiceRoom {
        inst: InstanceId,
        channel: Id,
    },
}

impl Buffer {
    /// A list of messages to jump from (search, mentions, pins).
    pub fn is_results(&self) -> bool {
        matches!(self, Buffer::Results(_))
    }
}

/// A settings page over the whole window: a header bar like the main window's (the brand over
/// the navigation, a "Return to …" button over the content; Esc also returns), the navigation on
/// a sidebar-colored left part and the content on the right.
/// Width of a settings page's navigation.
const NAV_WIDTH: f32 = 260.0;

pub fn page_frame<'a, M: Clone + 'a>(
    nav: iced::Element<'a, M>,
    content: iced::Element<'a, M>,
    on_close: M,
    return_to: &str,
    fonts: &crate::font::Fonts,
    theme: &'a crate::theme::Theme,
) -> iced::Element<'a, M> {
    use iced::widget::{button, column, container, row};
    use iced::{Alignment, Fill};

    use crate::theme::style;
    use crate::widget::icon::{self, icon};

    let brand = container(
        row![
            icon(icon::FEATHER, style::ICON).color(theme.accent.base.0),
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
    let back = button(
        row![
            icon(icon::ARROW_LEFT, style::ICON),
            crate::widget::label(format!("Return to {return_to}"), fonts)
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(style::GAP_S)
        .align_y(Alignment::Center),
    )
    .padding(style::BUTTON_PAD)
    .style(move |_, status| style::ghost(theme, status, false))
    .on_press(on_close);
    let header = container(
        row![
            back,
            crate::widget::caption("Esc", fonts).color(theme.text.tertiary.0),
        ]
        .spacing(style::GAP_S)
        .align_y(Alignment::Center),
    )
    .padding([0.0, style::GAP])
    .height(style::HEADER)
    .width(Fill)
    .align_y(Alignment::Center)
    .style(move |_| style::pane_header(theme));
    // A drawn line under each header: the pane header's shadow would be painted over by the
    // content below it.
    let line = || {
        iced::widget::rule::horizontal(1).style(move |_| iced::widget::rule::Style {
            color: theme.general.border.0,
            ..style::rule(theme)
        })
    };
    let left = column![
        brand,
        line(),
        container(nav)
            .height(Fill)
            .width(Fill)
            .padding([style::GAP_L, style::GAP_S]),
    ];
    row![
        container(left)
            .width(NAV_WIDTH)
            .height(Fill)
            .style(move |_| style::sidebar(theme)),
        column![
            header,
            line(),
            container(content)
                .width(Fill)
                .height(Fill)
                .padding(iced::Padding {
                    top: 0.0,
                    bottom: 0.0,
                    left: 40.0,
                    right: style::GAP_XL,
                })
                .style(move |_| style::app(theme)),
        ]
        .width(Fill)
        .height(Fill),
    ]
    .into()
}
