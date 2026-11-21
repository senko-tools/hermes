//! About.

use super::*;

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
                    ),
            )
            .into_any_element();
        let licence = setting(
            "Licence",
            "The app is GPL-3.0-or-later; the server is AGPL-3.0-or-later.",
            div(),
        );
        let debug = setting(
            "Debug info",
            "Version, system and connection state for bug reports. No tokens or messages.",
            Button::new("about-debug")
                .cursor_pointer()
                .label(self.copy_label("about-debug", "Copy"))
                .on_click(cx.listener(|this, _, _, cx| {
                    let text = this.debug_info();
                    this.copy_from("about-debug", text, "Debug info", cx);
                })),
        );
        let logs = setting(
            "Logs",
            "Dated log files from this device.",
            Button::new("about-logs")
                .cursor_pointer()
                .label("Open folder")
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(paths) = this.core.paths() {
                        let dir = paths.dir.join("logs");
                        let _ = std::fs::create_dir_all(&dir);
                        if let Err(err) = crate::open::path(&dir) {
                            this.error(format!("could not open {}: {err}", dir.display()), cx);
                        }
                    }
                })),
        );
        let project = setting(
            "Project page",
            "Source code, issues and releases.",
            Button::new("about-project")
                .cursor_pointer()
                .label("Open")
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Err(err) = crate::open::url(env!("CARGO_PKG_REPOSITORY")) {
                        this.error(format!("could not open the browser: {err}"), cx);
                    }
                })),
        );
        page_body(
            "About",
            None,
            vec![
                group("Hermes", vec![header, licence]),
                group("Help", vec![debug, logs, project]),
            ],
        )
    }
}
