//! The sidebar (PLAN.md section 15.1). For now: the instances with their connection status and
//! the user panel; the space/channel tree comes next.

use hermes_client_core::Core;
use hermes_client_core::gateway::ConnStatus;
use iced::widget::{button, column, container, row, scrollable};
use iced::{Alignment, Element, Fill};

use crate::font::Fonts;
use crate::message::Message;
use crate::theme::{Theme, style};
use crate::widget::{label, small, strong};

pub fn view<'a>(core: &Core, fonts: &Fonts, theme: &'a Theme, width: u16) -> Element<'a, Message> {
    let mut instances = column![].spacing(2);
    let ids = core.instance_ids();
    if ids.is_empty() {
        instances = instances.push(
            container(small("No instances yet", fonts).color(theme.text.tertiary.0))
                .padding([4, 8]),
        );
    }
    for id in ids {
        let status = core.status(&id);
        let (dot, color) = status_dot(&status, theme);
        let logged_out = status == ConnStatus::LoggedOut;
        let mut item = row![
            label(dot, fonts).color(color),
            label(id.clone(), fonts).width(Fill)
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if logged_out {
            item = item.push(small("Sign in", fonts).color(theme.text.link.0));
        }
        instances = instances.push(
            button(item)
                .padding([4, 8])
                .width(Fill)
                .style(|_, s| style::sidebar_item(theme, s, false))
                .on_press_maybe(logged_out.then_some(Message::SignIn(id))),
        );
    }
    instances = instances.push(
        button(small("+ Add instance", fonts).color(theme.text.secondary.0))
            .padding([4, 8])
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
            scrollable(instances).height(Fill),
            user_panel,
        ]
        .height(Fill),
    )
    .width(f32::from(width))
    .height(Fill)
    .style(|_| style::sidebar(theme))
    .into()
}

fn status_dot(status: &ConnStatus, theme: &Theme) -> (&'static str, iced::Color) {
    match status {
        ConnStatus::Connected => ("●", theme.text.success.0),
        ConnStatus::Connecting | ConnStatus::Reconnecting { .. } => ("●", theme.text.warning.0),
        ConnStatus::Failed { .. } | ConnStatus::LoggedOut => ("●", theme.text.error.0),
        _ => ("○", theme.text.tertiary.0),
    }
}
