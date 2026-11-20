//! Chat: messages, layout, motion, the sidebar and files.

use super::*;

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
        setting(
            name,
            hint,
            Switch::new(SharedString::from(id))
                .cursor_pointer()
                .checked(on)
                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                    let on = *on;
                    this.set_config(|c| set(c, on), cx);
                })),
        )
    }

    pub(super) fn tab_chat(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.config.buffer;
        let preview = time::format(
            hermes_client_core::now_ms(),
            &b.timestamp_format,
            self.utc_offset,
        );
        let date_preview = time::format(
            hermes_client_core::now_ms(),
            &b.date_format,
            self.utc_offset,
        );
        page_body(
            "Chat",
            None,
            vec![
                group(
                    "Messages",
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
                ),
                group(
                    "Layout",
                    vec![
                        setting(
                            "Group messages",
                            &format!("Within {} minutes", b.group_minutes),
                            self.slider(page, SliderKind::GroupMinutes),
                        ),
                        setting(
                            "Quick reactions",
                            &self.config.buffer.quick_reactions,
                            Button::new("edit-reactions")
                                .cursor_pointer()
                                .label("Change")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_form_quick_reactions(window, cx);
                                })),
                        ),
                        setting(
                            "Status line stays",
                            &format!("{} seconds", self.config.general.status_seconds),
                            wrap().children([3, 5, 10].into_iter().enumerate().map(|(n, secs)| {
                                crate::ui::chip(
                                    self,
                                    ("status-seconds", n),
                                    format!("{secs} s"),
                                    self.config.general.status_seconds == secs,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.set_config(|c| c.general.status_seconds = secs, cx);
                                    },
                                ))
                            })),
                        ),
                        flag!(
                            self,
                            cx,
                            "Compact messages",
                            "Time, a small picture, the name and the text on one line.",
                            buffer.compact
                        ),
                        self.pattern_row(page, Pattern::Time, &format!("Now: {preview}."), cx),
                        self.pattern_row(
                            page,
                            Pattern::Date,
                            &format!("On the line between days. Now: {date_preview}."),
                            cx,
                        ),
                    ],
                ),
                self.sidebar_group(cx),
                self.files_group(page, cx),
            ],
        )
    }

    pub(super) fn motion_group(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let g = &self.config.general;
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
                setting(
                    "Motion speed",
                    "How long things take to slide and fade.",
                    wrap().children(theme::MOTION_SPEEDS.into_iter().enumerate().map(
                        |(n, (value, label, _))| {
                            crate::ui::chip(
                                self,
                                ("settings-motion", n),
                                label,
                                g.motion_speed == value,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.set_config(|c| c.general.motion_speed = value, cx);
                                },
                            ))
                        },
                    )),
                ),
                flag!(
                    self,
                    cx,
                    "Smooth scrolling",
                    "The mouse wheel glides instead of jumping.",
                    general.smooth_scroll
                ),
                setting(
                    "Scroll speed",
                    &format!("{:.2} x", g.scroll_speed),
                    self.slider(page, SliderKind::ScrollSpeed),
                ),
                flag!(
                    self,
                    cx,
                    "Animate pictures",
                    "GIFs move while the window has the focus.",
                    general.animate_pictures
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
        group(
            "Files",
            vec![
                setting(
                    "Media volume",
                    &format!("{:.0} %", self.config.voice.media_volume * 100.),
                    self.slider(page, SliderKind::MediaVolume),
                ),
                setting(
                    "Download folder",
                    &folder,
                    wrap()
                        .child(
                            Button::new("download-choose")
                                .cursor_pointer()
                                .label("Choose…")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.choose_download_dir(cx)),
                                ),
                        )
                        .child(
                            Button::new("download-reset")
                                .cursor_pointer()
                                .label("Reset")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_config(|c| c.general.download_dir.clear(), cx);
                                })),
                        ),
                ),
                setting(
                    "Size of pictures and videos",
                    &format!("{} %", self.config.buffer.media_scale),
                    wrap().children(
                        [(75, "Small"), (100, "Normal"), (150, "Large")]
                            .into_iter()
                            .enumerate()
                            .map(|(n, (scale, name))| {
                                crate::ui::chip(
                                    self,
                                    ("media-scale", n),
                                    name,
                                    self.config.buffer.media_scale == scale,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.set_config(|c| c.buffer.media_scale = scale, cx);
                                    },
                                ))
                            }),
                    ),
                ),
                setting(
                    "Saved on this device",
                    "Thumbnails come back as needed, the search index fills up again.",
                    wrap()
                        .child(
                            Button::new("clear-thumbs")
                                .cursor_pointer()
                                .label("Clear pictures")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.clear_pictures();
                                    this.info("Pictures cleared", cx);
                                })),
                        )
                        .child(
                            Button::new("clear-index")
                                .cursor_pointer()
                                .label("Clear search index")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    for inst in this.core.instance_ids() {
                                        this.core.clear_search_index(&inst);
                                    }
                                    this.info("Search index cleared", cx);
                                })),
                        ),
                ),
                flag!(
                    self,
                    cx,
                    "Ask where to save",
                    "Choose a place for each download.",
                    general.ask_where_to_save
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
