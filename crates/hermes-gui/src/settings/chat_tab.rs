//! Chat: messages, layout, motion, the sidebar and files.

use super::*;
use crate::ui::{ButtonKind, Segment};

const STATUS_SECONDS: [u32; 3] = [3, 5, 10];
const MEDIA_SCALES: [(u32, &str); 3] = [(75, "Small"), (100, "Normal"), (150, "Large")];
/// How wide a slider on the right of its row is.
const SLIDER_W: f32 = 240.;

impl Shell {
    /// A switch row for a `bool` in the config; use [`flag!`], which names the field once.
    pub(super) fn flag(
        &self,
        name: &'static str,
        hint: &'static str,
        on: bool,
        id: &'static str,
        set: fn(&mut ClientConfig, bool),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        setting_fit(
            name,
            hint,
            crate::ui::switch(
                self,
                id,
                on,
                true,
                move |this, _, cx| this.set_config(|c| set(c, !on), cx),
                cx,
            ),
        )
    }

    pub(super) fn tab_chat(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.config.buffer;
        let messages = section(
            "Messages",
            "",
            vec![
                flag!(self, cx, "Show avatars", "", buffer.show_avatars),
                flag!(self, cx, "Show image previews", "", buffer.image_previews),
                flag!(self, cx, "Show who is typing", "", buffer.show_typing),
                flag!(
                    self,
                    cx,
                    "Format messages",
                    "Off: bold, code blocks and the like show as they were typed.",
                    buffer.markdown
                ),
                flag!(
                    self,
                    cx,
                    "Link previews",
                    "Show the title and picture of linked pages. This computer fetches \
                    them itself, so the site sees your address (the Hermes server never \
                    fetches them).",
                    buffer.link_previews
                ),
                flag!(
                    self,
                    cx,
                    "Preview while writing",
                    "The message shows formatted above the message box. Also the eye \
                    button next to it.",
                    buffer.composer_preview
                ),
                flag!(
                    self,
                    cx,
                    "Enter sends",
                    "Shift+Enter starts a new line.",
                    buffer.enter_sends
                ),
            ],
        );
        // Side by side: each is a field with its presets under it.
        let times = section(
            "Time and date",
            "",
            vec![setting_pair(
                self.pattern_row(page, Pattern::Time, cx),
                self.pattern_row(page, Pattern::Date, cx),
            )],
        );
        let layout = section(
            "Layout",
            "",
            vec![
                flag!(
                    self,
                    cx,
                    "Compact messages",
                    "Time, a small picture, the name and the text on one line.",
                    buffer.compact
                ),
                setting_fit(
                    "Group messages",
                    &format!("Within {} minutes", b.group_minutes),
                    sized(SLIDER_W, self.slider(page, SliderKind::GroupMinutes)),
                ),
                setting_fit(
                    "Status line stays",
                    "",
                    crate::ui::segmented(
                        self,
                        "status-seconds",
                        STATUS_SECONDS
                            .iter()
                            .map(|secs| Segment::new(format!("{secs} s")))
                            .collect(),
                        STATUS_SECONDS
                            .iter()
                            .position(|&s| s == self.config.general.status_seconds),
                        true,
                        |this, n, _, cx| {
                            let secs = STATUS_SECONDS[n];
                            this.set_config(|c| c.general.status_seconds = secs, cx);
                        },
                        cx,
                    ),
                ),
                setting_fit(
                    "Quick reactions",
                    &self.config.buffer.quick_reactions,
                    crate::ui::button(
                        self,
                        "edit-reactions",
                        "Change",
                        ButtonKind::Secondary,
                        true,
                        |this, window, cx| this.open_form_quick_reactions(window, cx),
                        cx,
                    ),
                ),
            ],
        );
        page_body(
            "Chat",
            None,
            vec![
                messages,
                times,
                layout,
                self.sidebar_group(cx),
                self.files_group(page, cx),
            ],
        )
    }

    pub(super) fn motion_group(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let g = &self.config.general;
        let scroll_speed = g.scroll_speed;
        group(
            "Motion",
            vec![
                flag!(
                    self,
                    cx,
                    "Reduce motion",
                    "No sliding or fading.",
                    general.reduce_motion
                ),
                setting_fit(
                    "Motion speed",
                    "How long things take to slide and fade.",
                    crate::ui::segmented(
                        self,
                        "settings-motion",
                        theme::MOTION_SPEEDS
                            .iter()
                            .map(|(_, label, _)| Segment::new(*label))
                            .collect(),
                        theme::MOTION_SPEEDS
                            .iter()
                            .position(|(v, _, _)| *v == g.motion_speed),
                        true,
                        |this, n, _, cx| {
                            let value = theme::MOTION_SPEEDS[n].0;
                            this.set_config(|c| c.general.motion_speed = value, cx);
                        },
                        cx,
                    ),
                ),
                flag!(
                    self,
                    cx,
                    "Smooth scrolling",
                    "The mouse wheel glides instead of jumping.",
                    general.smooth_scroll
                ),
                flag!(
                    self,
                    cx,
                    "Animate pictures",
                    "GIFs move while the window has the focus.",
                    general.animate_pictures
                ),
                self.more(
                    "motion",
                    "More motion options",
                    |cx| {
                        vec![
                            setting_fit(
                                "Scroll speed",
                                &format!("{scroll_speed:.2} x"),
                                sized(220., self.slider(page, SliderKind::ScrollSpeed)),
                            ),
                            flag!(
                                self,
                                cx,
                                "Animate in the background",
                                "GIFs keep moving while another window has the focus.",
                                general.animate_unfocused
                            ),
                        ]
                    },
                    cx,
                ),
            ],
        )
    }

    pub(super) fn sidebar_group(&self, cx: &mut Context<Self>) -> AnyElement {
        group(
            "Sidebar",
            vec![
                flag!(self, cx, "Show unread dots", "", sidebar.show_unread_dots),
                flag!(
                    self,
                    cx,
                    "Show mention counts",
                    "",
                    sidebar.show_mention_counts
                ),
                flag!(
                    self,
                    cx,
                    "Show who is in voice channels",
                    "",
                    sidebar.show_voice_users
                ),
            ],
        )
    }

    pub(super) fn files_group(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let g = &self.config.general;
        let folder = if g.download_dir.is_empty() {
            "The Downloads folder".to_owned()
        } else {
            g.download_dir.clone()
        };
        let media_scale = crate::ui::segmented(
            self,
            "media-scale",
            MEDIA_SCALES
                .iter()
                .map(|(_, name)| Segment::new(*name))
                .collect(),
            MEDIA_SCALES
                .iter()
                .position(|(s, _)| *s == self.config.buffer.media_scale),
            true,
            |this, n, _, cx| {
                let scale = MEDIA_SCALES[n].0;
                this.set_config(|c| c.buffer.media_scale = scale, cx);
            },
            cx,
        );
        group(
            "Files and media",
            vec![
                setting_pair(
                    setting_below(
                        "Media volume",
                        &format!("{:.0} %", self.config.voice.media_volume * 100.),
                        self.slider(page, SliderKind::MediaVolume),
                    ),
                    setting_below(
                        "Size of pictures and videos",
                        &format!("{} %", self.config.buffer.media_scale),
                        media_scale,
                    ),
                ),
                setting_fit(
                    "Download folder",
                    &folder,
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(crate::ui::button(
                            self,
                            "download-choose",
                            "Choose…",
                            ButtonKind::Secondary,
                            true,
                            |this, _, cx| this.choose_download_dir(cx),
                            cx,
                        ))
                        // Only when there is something to reset.
                        .when(!g.download_dir.is_empty(), |d| {
                            d.child(crate::ui::button(
                                self,
                                "download-reset",
                                "Reset",
                                ButtonKind::Secondary,
                                true,
                                |this, _, cx| {
                                    this.set_config(|c| c.general.download_dir.clear(), cx);
                                },
                                cx,
                            ))
                        }),
                ),
                flag!(
                    self,
                    cx,
                    "Ask where to save",
                    "Choose a place for each download.",
                    general.ask_where_to_save
                ),
                setting_fit(
                    "Saved on this device",
                    "Thumbnails come back as needed, the search index fills up again.",
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(crate::ui::button(
                            self,
                            "clear-thumbs",
                            "Clear pictures",
                            ButtonKind::Secondary,
                            true,
                            |this, _, cx| {
                                this.clear_pictures();
                                this.success("Pictures cleared", cx);
                            },
                            cx,
                        ))
                        .child(crate::ui::button(
                            self,
                            "clear-index",
                            "Clear search index",
                            ButtonKind::Secondary,
                            true,
                            |this, _, cx| {
                                for inst in this.core.instance_ids() {
                                    this.core.clear_search_index(&inst);
                                }
                                this.success("Search index cleared", cx);
                            },
                            cx,
                        )),
                ),
            ],
        )
    }

    pub(super) fn choose_download_dir(&mut self, cx: &mut Context<Self>) {
        self.run(
            cx,
            async {
                rfd::AsyncFileDialog::new()
                    .pick_folder()
                    .await
                    .map(|folder| folder.path().display().to_string())
            },
            |this, picked, _, cx| {
                if let Some(folder) = picked {
                    this.set_config(|c| c.general.download_dir = folder, cx);
                }
            },
        );
    }
}
