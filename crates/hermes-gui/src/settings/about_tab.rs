//! About.

use super::*;
use crate::shell::UpdateCheck;
use crate::ui::ButtonKind;

/// The settings on the About page, by their names (also what the search finds).
const CHECK: &str = "Check for updates";
const ON_START: &str = "Check for updates on start";
const AUTO: &str = "Auto-update";

impl Shell {
    /// "Check for updates" after the version: text, not a button, saying how the check went.
    fn update_link(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(update) = &self.update {
            return crate::ui::text_link("about-update", format!("{} available", update.version))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.cancel_closing(crate::update_notes::POPUP);
                    this.update_notes_open = true;
                    cx.notify();
                }))
                .into_any_element();
        }
        let plain = |text: &'static str| div().child(text).into_any_element();
        match self.update_check {
            UpdateCheck::Checking => plain("Checking\u{2026}"),
            UpdateCheck::UpToDate => plain("Up to date"),
            UpdateCheck::Idle | UpdateCheck::Failed => crate::ui::text_link(
                "about-update",
                if self.update_check == UpdateCheck::Failed {
                    "Could not check"
                } else {
                    CHECK
                },
            )
            .on_click(cx.listener(|this, _, _, cx| this.check_for_update_now(cx)))
            .into_any_element(),
        }
    }

    /// A switch with its name beside it, for the line under About's version.
    fn about_switch(
        &self,
        id: &'static str,
        name: &'static str,
        on: bool,
        set: fn(&mut ClientConfig, bool),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        glow_if_found(
            name,
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(crate::ui::switch(
                    self,
                    id,
                    on,
                    true,
                    move |this, _, cx| this.set_config(|c| set(c, !on), cx),
                    cx,
                ))
                .child(
                    div()
                        .text_size(theme::secondary_size())
                        .text_color(theme::text_secondary())
                        .child(name),
                ),
        )
    }

    pub(super) fn tab_about(&self, cx: &mut Context<Self>) -> AnyElement {
        let version_line = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(6.))
            .text_size(theme::caption_size())
            .text_color(theme::text_tertiary())
            .child(format!(
                "Version {} \u{b7} {} {}",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
            .child("\u{b7}")
            .child(self.update_link(cx));
        let g = &self.config.general;
        let updates = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(24.))
            .gap_y(px(8.))
            .mt(px(10.))
            .child(self.about_switch(
                "about-check-on-start",
                ON_START,
                g.check_for_updates,
                |c, on| c.general.check_for_updates = on,
                cx,
            ))
            .child(self.about_switch(
                "about-auto-update",
                AUTO,
                g.auto_update,
                |c, on| c.general.auto_update = on,
                cx,
            ));
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
                    .child(glow_if_found(CHECK, version_line))
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
                    )
                    .child(updates),
            )
            .into_any_element();
        // For the search: the header is the page's first part, "Updates".
        note_setting(CHECK, "Look for a newer version now.");
        note_setting(ON_START, "");
        note_setting(AUTO, "Install a newer version found on start by itself.");
        note_group("Updates");
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
