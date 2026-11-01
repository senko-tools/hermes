//! The certificate prompt (PLAN.md sections 13 and 15.4): trust on first use for an unknown
//! self-signed certificate, and a warning when a pinned certificate changed.

use hermes_client_core::address::Fingerprint;
use iced::widget::{button, column, container, row, space, text};
use iced::{Alignment, Element};

use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::{ButtonKind, text_button};
use crate::widget::{caption, heading, label};

/// `pinned` is set when the certificate changed.
pub fn view<'a, M: Clone + 'a>(
    host: String,
    now: Fingerprint,
    pinned: Option<Fingerprint>,
    fonts: &Fonts,
    theme: &'a Theme,
    on_trust: M,
    on_cancel: M,
) -> Element<'a, M> {
    let fingerprint = |fp: Fingerprint| {
        container(
            text(fp.colon_hex())
                .font(fonts.mono)
                .size(fonts.caption())
                .wrapping(text::Wrapping::Glyph),
        )
        .padding(style::GAP_S)
        .style(move |_| container::Style {
            background: Some(theme.text.code_bg.0.into()),
            ..container::Style::default()
        })
    };
    let mut content = column![].spacing(style::GAP);
    match pinned {
        None => {
            content = content
                .push(heading("Trust this server?", fonts))
                .push(label(
                    format!(
                        "{host} uses a self-signed certificate. If you know the server owner, \
                         compare this fingerprint with theirs before trusting it."
                    ),
                    fonts,
                ))
                .push(fingerprint(now));
        }
        Some(pinned) => {
            content = content
                .push(heading("The certificate changed", fonts).color(theme.text.error.0))
                .push(label(
                    format!(
                        "The certificate of {host} is not the one you trusted before. Someone may \
                         be intercepting the connection. Only continue if the server owner told \
                         you they re-generated the certificate."
                    ),
                    fonts,
                ))
                .push(caption("Trusted before:", fonts))
                .push(fingerprint(pinned))
                .push(caption("Now:", fonts))
                .push(fingerprint(now));
        }
    }
    let trust_label = if pinned.is_some() {
        "I re-generated the certificate, trust the new one"
    } else {
        "Trust"
    };
    let changed = pinned.is_some();
    content
        .push(
            row![
                space::horizontal(),
                text_button("Cancel", ButtonKind::Secondary, fonts, theme).on_press(on_cancel),
                button(label(trust_label, fonts))
                    .style(move |_, s| {
                        if changed {
                            style::danger(theme, s)
                        } else {
                            style::primary(theme, s)
                        }
                    })
                    .on_press(on_trust),
            ]
            .spacing(style::GAP_S)
            .align_y(Alignment::Center),
        )
        .into()
}
