//! Drawing the message box.

use super::*;

impl Shell {
    pub(super) fn render_composer(
        &self,
        key: &ChannelKey,
        reply_name: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(view) = self.chats.get(key) else {
            return div().into_any_element();
        };
        let mode = match (view.editing, reply_name) {
            (Some(_), _) => Some((assets::PENCIL, "Editing message".to_owned())),
            (None, Some(name)) => Some((assets::REPLY, format!("Replying to {name}"))),
            _ => None,
        };
        let popup = (!view.suggestions.is_empty()).then(|| {
            let rows = view.suggestions.iter().enumerate().map(|(n, suggestion)| {
                let accept = key.clone();
                div()
                    .id(("suggestion", n))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(theme::radius_small())
                    .cursor_pointer()
                    .text_size(theme::secondary_size())
                    // The chosen row keeps its own highlight.
                    .when(n != view.suggestion, |d| d.hover(|d| d.bg(theme::hover())))
                    .child(suggestion.label.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.accept_suggestion(&accept, n, window, cx);
                    }))
                    // The highlight flows from row to row as the arrows move it.
                    .with_spring(
                        ("suggestion-chosen", n),
                        gpui_kit::SpringAnimation::new(crate::theme::paced(
                            crate::hover::HOVER_SPRING,
                        ))
                        .to(n == view.suggestion),
                        |d, phase| d.bg(theme::alpha(theme::selected(), phase.0.clamp(0., 1.))),
                    )
            });
            div()
                .absolute()
                .bottom_full()
                .left(px(16.))
                .right(px(16.))
                .mb(px(6.))
                .p(px(6.))
                .rounded(theme::radius_large())
                .bg(theme::sidebar())
                .border_1()
                .border_color(theme::border())
                .shadow_md()
                .children(rows)
                // It rises out of the message box as it fades in.
                .with_animation(
                    "suggestions-in",
                    gpui_kit::Animation::new(theme::fade()).with_easing(gpui_kit::ease_out_quint()),
                    |d, t| d.opacity(t).mb(px(6. - 4. * (1. - t))),
                )
        });
        let cancel_key = key.clone();
        let mode_bar = mode.map(|(glyph, text)| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .pb(px(6.))
                .mb(px(6.))
                .border_b_1()
                .border_color(theme::border())
                .child(icon(glyph, 16., theme::accent()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(theme::secondary_size())
                        .text_color(theme::text_secondary())
                        .child(text),
                )
                .child(
                    crate::ui::tip_button(self, "composer-cancel", assets::CLOSE, "Cancel", cx)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.composer_cancel(&cancel_key, window, cx);
                        })),
                )
                // It opens out of the message box instead of pushing it at once.
                .overflow_hidden()
                .with_animation(
                    SharedString::from(format!("composer-mode-{glyph}")),
                    gpui_kit::Animation::new(theme::fade() * 2)
                        .with_easing(gpui_kit::ease_out_quint()),
                    |d, t| d.max_h(px(44. * t)).opacity(t),
                )
        });
        let upload_key = key.clone();
        let chips = view.uploads.iter().map(|upload| {
            let (state, color) = match &upload.state {
                UploadState::Uploading => ("uploading…".to_owned(), theme::text_tertiary()),
                UploadState::Ready(_) => (view::human_size(upload.size), theme::text_tertiary()),
                UploadState::Failed(e) => (format!("failed: {e}"), theme::danger()),
            };
            let local = upload.local;
            let remove = upload_key.clone();
            let uploading = matches!(upload.state, UploadState::Uploading);
            let failed = matches!(upload.state, UploadState::Failed(_));
            let state = div().text_color(color).child(state);
            // Still going up: the state breathes.
            let state = if uploading {
                state
                    .with_animation(
                        ("upload-busy", local as usize),
                        gpui_kit::Animation::new(std::time::Duration::from_millis(1200))
                            .repeat()
                            .with_easing(gpui_kit::pulsating_between(0.4, 1.)),
                        |d, t| d.opacity(t),
                    )
                    .into_any_element()
            } else {
                state.into_any_element()
            };
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(8.))
                .py(px(4.))
                .rounded(theme::radius_small())
                .bg(theme::selected())
                .border_1()
                .border_color(if failed {
                    theme::danger()
                } else {
                    theme::alpha(theme::border(), 0.)
                })
                .text_size(theme::caption_size())
                .child(icon(assets::FILE, 14., theme::text_secondary()))
                .child(div().max_w(px(240.)).truncate().child(upload.name.clone()))
                .child(state)
                .child(
                    crate::ui::tip_button(
                        self,
                        ("upload-remove", local as usize),
                        assets::CLOSE,
                        "Remove",
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(view) = this.chats.get_mut(&remove) {
                            view.uploads.retain(|u| u.local != local);
                        }
                        cx.notify();
                    })),
                )
        });
        let uploads = (!view.uploads.is_empty()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .pb(px(6.))
                .children(chips)
        });
        let pick_key = key.clone();
        let emoji_key = key.clone();
        let paste_key = key.clone();
        let max_mb = self
            .core
            .store()
            .instance(&key.0)
            .and_then(|s| s.max_upload_mb());
        let attach_tip = SharedString::from(match max_mb {
            Some(mb) => format!("Attach files up to {mb} MB (or drop them here)"),
            None => "Attach files (or drop them here)".to_owned(),
        });
        let max_length = self
            .core
            .store()
            .instance(&key.0)
            .and_then(|s| s.max_message_length());
        let counter = max_length.and_then(|max| {
            let len = view.input.read(cx).value().chars().count();
            let (text, over) = length_counter(len, max as usize)?;
            Some(
                div()
                    .flex_shrink_0()
                    .px(px(4.))
                    .text_size(theme::caption_size())
                    .text_color(if over {
                        theme::danger()
                    } else {
                        theme::text_tertiary()
                    })
                    .child(text),
            )
        });
        // What is written, formatted, while the preview is on and there is something to show.
        let preview = (self.config.buffer.composer_preview && self.config.buffer.markdown)
            .then(|| view.input.read(cx).value().to_string())
            .filter(|text| !text.trim().is_empty())
            .map(|text| self.composer_preview(key, &text));
        let preview_on = self.config.buffer.composer_preview;
        let shell = cx.entity().downgrade();
        let input = Textarea::new(&view.input)
            .large()
            .appearance(false)
            .on_paste(move |_, _: &mut Window, cx: &mut App| {
                shell
                    .update(cx, |this, cx| this.paste_image(&paste_key, cx))
                    .unwrap_or(false)
            });
        div()
            .relative()
            .flex_shrink_0()
            .children(popup)
            .child(
                // Flush with the panel's sides and bottom, as tall as the user panel next to it.
                div()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .min_h(px(theme::FOOTER_HEIGHT))
                    .px(px(12.))
                    .py(px(4.))
                    .bg(theme::raised())
                    .border_t_1()
                    .border_color(theme::border())
                    .children(mode_bar)
                    .children(uploads)
                    .children(preview)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                crate::ui::icon_button(self, "composer-attach", assets::ATTACH, cx)
                                    .tooltip(move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            attach_tip.clone(),
                                        )
                                        .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.pick_files(pick_key.clone(), cx);
                                    })),
                            )
                            .child(div().flex_1().min_w_0().child(input))
                            .children(counter)
                            .child(
                                crate::ui::icon_button(self, "composer-preview", assets::EYE, cx)
                                    .when(preview_on, |d| d.bg(theme::selected()))
                                    .tooltip(move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(if preview_on {
                                            "Hide the formatted preview"
                                        } else {
                                            "Show it formatted while you write"
                                        })
                                        .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_config(
                                            |c| c.buffer.composer_preview = !preview_on,
                                            cx,
                                        );
                                    })),
                            )
                            .child({
                                let key = emoji_key.clone();
                                crate::ui::icon_button(self, "composer-emoji", assets::EMOJI, cx)
                                    .tooltip(|window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new("Emoji")
                                            .build(window, cx)
                                    })
                                    .on_click(cx.listener(
                                        move |this, event: &gpui_kit::ClickEvent, window, cx| {
                                            this.open_emoji_picker(
                                                crate::emoji_picker::PickFor::Compose(key.clone()),
                                                event.position(),
                                                window,
                                                cx,
                                            );
                                        },
                                    ))
                            }),
                    ),
            )
            .into_any_element()
    }
}
