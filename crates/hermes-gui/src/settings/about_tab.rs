//! About.

use super::*;
use crate::ui::ButtonKind;

impl Shell {
    pub(super) fn tab_about(&self, cx: &mut Context<Self>) -> AnyElement {
        let header = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .size(px(56.))
                    .rounded(theme::radius_large())
                    .bg(theme::accent_soft())
                    .child(icon(assets::FEATHER, 28., theme::accent())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(theme::heading_size())
                            .font_weight(FontWeight::MEDIUM)
                            .child("Hermes"),
                    )
                    .child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(format!(
                                "Version {} \u{b7} {} {}",
                                env!("CARGO_PKG_VERSION"),
                                std::env::consts::OS,
                                std::env::consts::ARCH
                            )),
                    )
                    .child(
                        div()
                            .text_size(theme::secondary_size())
                            .text_color(theme::text_secondary())
                            .child("Self-hostable chat with text, voice and screen sharing."),
                    )
                    .child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child("The app is GPL-3.0-or-later; the server is AGPL-3.0-or-later."),
                    ),
            )
            .into_any_element();
        note_part(
            "Licence",
            "GPL-3.0-or-later, AGPL-3.0-or-later",
            "Debug info",
        );
        let debug = setting_fit(
            "Debug info",
            "Version, system and connection state for bug reports. No tokens or messages.",
            crate::ui::button(
                self,
                "about-debug",
                self.copy_label("about-debug", "Copy"),
                ButtonKind::Secondary,
                true,
                |this, _, cx| {
                    let text = this.debug_info();
                    this.copy_from("about-debug", text, "Debug info", cx);
                },
                cx,
            ),
        );
        let logs = setting_fit(
            "Logs",
            "Dated log files from this device.",
            crate::ui::button(
                self,
                "about-logs",
                "Open folder",
                ButtonKind::Secondary,
                true,
                |this, _, cx| {
                    if let Some(paths) = this.core.paths() {
                        let dir = paths.dir.join("logs");
                        let _ = std::fs::create_dir_all(&dir);
                        if let Err(err) = crate::open::path(&dir) {
                            this.error(format!("could not open {}: {err}", dir.display()), cx);
                        }
                    }
                },
                cx,
            ),
        );
        let project = setting_fit(
            "Project page",
            "Source code, issues and releases.",
            crate::ui::button(
                self,
                "about-project",
                "Open",
                ButtonKind::Secondary,
                true,
                |this, _, cx| {
                    if let Err(err) = crate::open::url(env!("CARGO_PKG_REPOSITORY")) {
                        this.error(format!("could not open the browser: {err}"), cx);
                    }
                },
                cx,
            ),
        );
        page_body(
            "About",
            None,
            vec![
                // Who this is, on its own above the cards.
                div().py(px(8.)).child(header).into_any_element(),
                group("Help", vec![debug, logs, project]),
            ],
        )
    }
}
