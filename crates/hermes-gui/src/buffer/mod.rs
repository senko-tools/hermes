//! Buffers: what a pane shows (PLAN.md section 15.1).

pub mod channel;
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

/// A settings page over the whole window: the navigation on a sidebar-colored left part, the
/// content on the right, and a round close button (Esc also closes) in the top right corner.
pub fn page_frame<'a, M: Clone + 'a>(
    nav: iced::Element<'a, M>,
    content: iced::Element<'a, M>,
    on_close: M,
    fonts: &crate::font::Fonts,
    theme: &'a crate::theme::Theme,
) -> iced::Element<'a, M> {
    use iced::widget::{button, column, container, row};
    use iced::{Alignment, Border, Fill, FillPortion};

    use crate::theme::style;

    let close = column![
        button(
            container(crate::widget::icon::icon(crate::widget::icon::CLOSE, 20.0))
                .center_x(38)
                .center_y(38),
        )
        .padding(0)
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: hovered.then_some(theme.surface.hover.0.into()),
                text_color: if hovered {
                    theme.text.primary.0
                } else {
                    theme.text.secondary.0
                },
                border: Border {
                    color: theme.text.tertiary.0,
                    width: 2.0,
                    radius: 19.0.into(),
                },
                ..button::Style::default()
            }
        })
        .on_press(on_close),
        crate::widget::caption("ESC", fonts)
            .font(fonts.bold())
            .color(theme.text.tertiary.0),
    ]
    .spacing(style::GAP_S)
    .align_x(Alignment::Center);
    row![
        container(container(nav).width(232).height(Fill))
            .width(FillPortion(2))
            .height(Fill)
            .align_right(Fill)
            .padding(iced::Padding {
                top: 48.0,
                bottom: 16.0,
                left: 12.0,
                right: 12.0,
            })
            .style(move |_| style::sidebar(theme)),
        container(
            row![
                container(content).max_width(760).width(Fill).height(Fill),
                container(close).padding(iced::Padding::ZERO.top(48.0)),
            ]
            .spacing(style::GAP_L),
        )
        .width(FillPortion(5))
        .height(Fill)
        .padding(iced::Padding {
            top: 0.0,
            bottom: 0.0,
            left: 40.0,
            right: 24.0,
        })
        .style(move |_| style::app(theme)),
    ]
    .into()
}
