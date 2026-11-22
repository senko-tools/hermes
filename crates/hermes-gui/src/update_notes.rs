//! "What's new", opened by clicking the update notice: what changed in every version between this
//! one and the update, newest first, each version on its own, and the Update button. The notes
//! come from each release (CHANGELOG.md), read by `hermes_client_core::update::parse_notes`:
//! headings, and `-` lists nested by indent.

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, div};
use hermes_client_core::update::{NoteLine, ReleaseNotes};

use crate::modal::{dialog, heading, paragraph};
use crate::shell::Shell;
use crate::theme::{self, px};

/// The popup's id, for its animations and `close_later`.
pub const POPUP: &str = "update-notes";
/// How far each level of a nested list goes in.
const INDENT: f32 = 16.;

/// One line of notes.
fn note_line(line: &NoteLine) -> AnyElement {
    match line {
        NoteLine::Heading(text) => div()
            .pt(px(6.))
            .text_size(theme::caption_size())
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::accent())
            .child(text.to_uppercase())
            .into_any_element(),
        // A plain `-` in front, and the text's wrapped lines under the text, not under the `-`.
        NoteLine::Item { depth, text } => div()
            .flex()
            .pl(px(INDENT * *depth as f32))
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(14.))
                    .text_color(theme::text_tertiary())
                    .child("-"),
            )
            .child(div().flex_1().min_w_0().child(text.clone()))
            .into_any_element(),
        NoteLine::Text(text) => div()
            .text_color(theme::text_secondary())
            .child(text.clone())
            .into_any_element(),
    }
}

/// One version's notes: its name and date over its lines.
fn version_block(notes: &ReleaseNotes) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .text_size(theme::body_size())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("Hermes {}", notes.version)),
                )
                .children(notes.date.clone().map(|date| {
                    div()
                        .text_size(theme::caption_size())
                        .text_color(theme::text_tertiary())
                        .child(date)
                })),
        )
        .when(notes.lines.is_empty(), |d| {
            d.child(
                div()
                    .text_color(theme::text_secondary())
                    .child("No notes for this version."),
            )
        })
        .children(notes.lines.iter().map(note_line))
        .into_any_element()
}

impl Shell {
    pub fn render_update_notes(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.update_notes_open {
            return None;
        }
        let update = self.update.as_ref()?;
        // This build updates itself: Update starts it, else the release page is the way.
        let self_update = hermes_client_core::updater::available();
        let close = |this: &mut Shell, cx: &mut Context<Shell>| {
            this.close_later(POPUP, |s| s.update_notes_open = false, cx);
        };
        let mut versions = div()
            .id("update-notes-list")
            .flex()
            .flex_col()
            .max_h(px(380.))
            .overflow_y_scroll()
            .pr(px(6.))
            .text_size(theme::secondary_size())
            .text_color(theme::text());
        if update.notes.is_empty() {
            versions = versions.child(
                div()
                    .text_color(theme::text_secondary())
                    .child("No notes for this update."),
            );
        }
        for (n, notes) in update.notes.iter().enumerate() {
            if n > 0 {
                versions = versions.child(div().my(px(12.)).h(px(1.)).bg(theme::border()));
            }
            versions = versions.child(version_block(notes));
        }
        let page = update.url.clone();
        let release_page = Button::new("update-notes-page")
            .cursor_pointer()
            .label(if self_update {
                "Release page"
            } else {
                "Open the release page"
            })
            .when(!self_update, |b| b.primary())
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Err(err) = crate::open::url(&page) {
                    this.error(format!("could not open the browser: {err}"), cx);
                }
            }));
        let (version, url) = (update.version.clone(), update.url.clone());
        let start = self_update.then(|| {
            Button::new("update-notes-start")
                .cursor_pointer()
                .primary()
                .label("Update")
                .on_click(cx.listener(move |this, _, window, cx| {
                    close(this, cx);
                    this.start_update(version.clone(), url.clone(), window, cx);
                }))
        });
        Some(dialog(
            POPUP,
            520.,
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(heading(format!("Hermes {} is available", update.version)))
                        .child(paragraph(format!(
                            "You have {}. This is what changed since.",
                            env!("CARGO_PKG_VERSION")
                        ))),
                )
                .child(versions)
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            Button::new("update-notes-close")
                                .cursor_pointer()
                                .label("Not now")
                                .large()
                                .on_click(cx.listener(move |this, _, _, cx| close(this, cx))),
                        )
                        .child(release_page.large())
                        .children(start.map(|b| b.large())),
                ),
        ))
    }
}
