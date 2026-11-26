//! "Codes", opened from the Time and date settings: every code a time or date pattern knows,
//! what it stands for and what it gives right now. The list is
//! `hermes_client_core::time::CODES`, the same one the formatter is tested against.

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, div};
use hermes_client_core::time;

use crate::assets;
use crate::modal::{dialog_closed_outside, heading, paragraph};
use crate::shell::Shell;
use crate::theme::{self, px};

/// The popup's id, for its animations and `close_later`.
pub const POPUP: &str = "time-codes";
/// Wide enough for the whole table and the example on one line each.
const WIDTH: f32 = 520.;
/// The example under the table.
const EXAMPLE: &str = "%A %e %B, %H:%M";

/// One row of the table: the code, what it means, and what it gives now.
fn row(code: &str, meaning: &str, now: String, header: bool) -> AnyElement {
    let cell = |d: gpui_kit::Div| {
        if header {
            d.text_size(theme::caption_size())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text_tertiary())
        } else {
            d
        }
    };
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .py(px(5.))
        .child(
            cell(div().w(px(56.)).flex_shrink_0())
                .when(!header, |d| {
                    d.font_family(theme::mono_font())
                        .text_color(theme::accent())
                })
                .child(code.to_owned()),
        )
        .child(cell(div().flex_1().min_w_0()).child(meaning.to_owned()))
        .child(
            cell(div().w(px(110.)).flex_shrink_0().flex().justify_end())
                .when(!header, |d| d.text_color(theme::text_secondary()))
                .child(now),
        )
        .into_any_element()
}

impl Shell {
    pub fn open_time_codes(&mut self, cx: &mut Context<Self>) {
        self.time_codes_open = true;
        cx.notify();
    }

    fn close_time_codes(&mut self, cx: &mut Context<Self>) {
        self.close_later(POPUP, |s| s.time_codes_open = false, cx);
        cx.notify();
    }

    pub fn render_time_codes(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.time_codes_open {
            return None;
        }
        let now = hermes_client_core::now_ms();
        let offset = self.utc_offset;
        let mut table = div()
            .flex()
            .flex_col()
            .text_size(theme::secondary_size())
            .child(row("Code", "Means", "Now".to_owned(), true));
        for (n, (code, meaning)) in time::CODES.iter().enumerate() {
            table = table
                .child(
                    div()
                        .h(px(1.))
                        .bg(theme::border())
                        .when(n == 0, |d| d.mt(px(2.))),
                )
                .child(row(code, meaning, time::format(now, code, offset), false));
        }
        let example = div()
            .flex()
            .flex_wrap()
            .gap_x(px(8.))
            .text_size(theme::secondary_size())
            .text_color(theme::text_secondary())
            .child("For example")
            .child(
                div()
                    .font_family(theme::mono_font())
                    .text_color(theme::accent())
                    .child(EXAMPLE),
            )
            .child("gives")
            .child(
                div()
                    .text_color(theme::text())
                    .child(time::format(now, EXAMPLE, offset)),
            );
        let close = crate::ui::tip_button(self, "time-codes-close", assets::CLOSE, "Close", cx)
            .on_click(cx.listener(|this, _, _, cx| this.close_time_codes(cx)));
        Some(dialog_closed_outside(
            POPUP,
            WIDTH,
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                // Wraps its text instead of pushing the close button off.
                                .min_w_0()
                                .gap(px(2.))
                                .child(heading("Time and date codes"))
                                .child(paragraph(
                                    "Put these in a time or date pattern. Anything else shows \
                                     as it is written.",
                                )),
                        )
                        .child(close),
                )
                .child(table)
                .child(example),
            Box::new(cx.listener(|this, _, _, cx| this.close_time_codes(cx))),
        ))
    }
}
