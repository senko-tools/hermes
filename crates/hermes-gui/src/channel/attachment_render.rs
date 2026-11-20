//! Drawing a message's files and the uploads waiting to go.

use super::*;

impl Shell {
    pub(super) fn render_attachment(
        &self,
        inst: &InstanceId,
        place: FilePlace,
        a: &Attachment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if is_image(a) && self.config.buffer.image_previews {
            let (w, h) = display_size(a);
            let picture = self.images.thumbs.get(&(inst.clone(), a.id));
            if let Some(Img::Ready(image)) = picture {
                let (open_inst, attachment) = (inst.clone(), a.clone());
                let picture = div()
                    .id(SharedString::from(format!("image-{}", a.id)))
                    .relative()
                    .w(px(w))
                    .h(px(h))
                    .rounded(theme::row_radius())
                    .overflow_hidden()
                    .cursor_pointer()
                    .group(IMAGE_GROUP)
                    .child(self.images.attachment_marker(inst, a))
                    .child({
                        let picture = img(image.shown(self.animating()))
                            .w(px(w))
                            .h(px(h))
                            .rounded(theme::row_radius())
                            .id(SharedString::from(format!("image-pic-{}", a.id)));
                        // It just arrived: it fades in over its placeholder.
                        let just_arrived = self
                            .images
                            .thumb_arrived
                            .get(&(inst.clone(), a.id))
                            .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(400));
                        if just_arrived {
                            div()
                                .child(picture)
                                .with_animation(
                                    SharedString::from(format!("image-in-{}", a.id)),
                                    gpui_kit::Animation::new(theme::fade() * 2)
                                        .with_easing(gpui_kit::ease_out_quint()),
                                    |d, t| d.opacity(t),
                                )
                                .into_any_element()
                        } else {
                            picture.into_any_element()
                        }
                    })
                    // What opening or saving it downloads, under the mouse.
                    .child(
                        div()
                            .absolute()
                            .bottom(px(6.))
                            .right(px(6.))
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(8.))
                            .bg(gpui_kit::hsla(0., 0., 0., 0.65))
                            .text_size(theme::caption_size())
                            .text_color(gpui_kit::rgb(0xffffff))
                            .opacity(0.)
                            .group_hover(IMAGE_GROUP, |s| s.opacity(1.))
                            .child(view::human_size(a.size)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_image(open_inst.clone(), attachment.clone(), cx);
                    }));
                return self
                    .with_file_menu(picture, inst, place, a.id, cx)
                    .into_any_element();
            }
            let failed = matches!(picture, Some(Img::Failed));
            let text = if failed {
                "Could not load · click to open"
            } else {
                "Loading…"
            };
            let (open_inst, attachment) = (inst.clone(), a.clone());
            // While it loads, the space it will fill breathes softly.
            let shimmer = (!failed).then(|| {
                div()
                    .absolute()
                    .inset_0()
                    .rounded(theme::row_radius())
                    .with_animation(
                        SharedString::from(format!("image-wait-{}", a.id)),
                        gpui_kit::Animation::new(std::time::Duration::from_millis(1400))
                            .repeat()
                            .with_easing(gpui_kit::pulsating_between(0., 1.)),
                        |d, t| d.bg(theme::alpha(theme::hover(), 0.6 * t)),
                    )
            });
            let waiting = div()
                .id(SharedString::from(format!("image-{}", a.id)))
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .cursor_default()
                .w(px(w))
                .h(px(h))
                .rounded(theme::row_radius())
                .bg(theme::code_background())
                .text_size(theme::caption_size())
                .text_color(theme::text_tertiary())
                .children(shimmer)
                .child(text)
                .child(self.images.attachment_marker(inst, a))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if failed {
                        this.images
                            .thumbs
                            .remove(&(open_inst.clone(), attachment.id));
                    }
                    this.open_image(open_inst.clone(), attachment.clone(), cx);
                }));
            return self
                .with_file_menu(waiting, inst, place, a.id, cx)
                .into_any_element();
        }
        if crate::media::is_video(&a.content_type, &a.filename) {
            return self.render_video_card(inst, place, a, cx);
        }
        if crate::media::is_sound(&a.content_type, &a.filename) {
            return self.render_audio_card(inst, place, a, cx);
        }
        let (name, name_record) = self.selectable_file_name(inst, Some(place), &a.filename);
        let card = div()
            .id(SharedString::from(format!("file-{}", a.id)))
            .flex()
            .items_center()
            .gap(px(12.))
            .cursor_default()
            .max_w(px(420.))
            .px(px(12.))
            .py(px(8.))
            .rounded(theme::row_radius())
            .bg(theme::sidebar())
            .border_1()
            .border_color(theme::border())
            .child(icon(assets::FILE, 22., theme::accent()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .relative()
                            .truncate()
                            .text_size(theme::secondary_size())
                            .text_color(theme::link())
                            .child(name)
                            .children(name_record),
                    )
                    .child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(view::human_size(a.size)),
                    ),
            )
            .child(self.download_button(inst, a, cx));
        self.with_file_menu(card, inst, place, a.id, cx)
            .into_any_element()
    }

    pub(super) fn render_pending(
        &self,
        s: &InstanceStore,
        inst: &InstanceId,
        channel: Id,
        p: &PendingRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(pending) = s.window(channel).and_then(|w| w.pending(p.local_id)) else {
            return div().into_any_element();
        };
        let (color, note) = match &pending.state {
            // The row itself is dimmed while on its way: the text keeps its colour.
            PendingState::Sending => (theme::text(), None),
            PendingState::Queued => (theme::text(), Some("waiting for connection")),
            PendingState::Failed(_) => (theme::danger(), Some("not sent")),
        };
        let me = s.me_id();
        let name = me.map(|id| s.display_name(None, id)).unwrap_or_default();
        let local_id: LocalId = p.local_id;
        let compact = self.config.buffer.compact;
        let time_text = time::format(
            pending.created_at,
            &self.config.buffer.timestamp_format,
            self.utc_offset,
        );
        let mut body = div().flex().flex_col().gap(px(2.)).flex_1().min_w_0();
        let content = div()
            .text_size(theme::body_size())
            .text_color(color)
            .child(pending.content.clone())
            .into_any_element();
        if compact {
            let mut head = Vec::new();
            if !p.grouped {
                if self.config.buffer.show_avatars {
                    let user = me.and_then(|id| s.user(id));
                    head.push(self.avatar(inst, user, &name, COMPACT_AVATAR));
                }
                head.push(
                    div()
                        .text_size(theme::body_size())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::role_default())
                        .child(name.clone())
                        .into_any_element(),
                );
            }
            body = body.child(compact_line(head, vec![content]));
        } else if !p.grouped {
            body = body.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_baseline()
                    .child(
                        div()
                            .text_size(theme::body_size())
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::role_default())
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(time_text.clone()),
                    ),
            );
            body = body.child(content);
        } else {
            body = body.child(content);
        }
        if let Some(note) = note {
            let mut line = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(theme::caption_size())
                .text_color(color)
                .child(note);
            if let PendingState::Failed(reason) = &pending.state {
                let (retry_inst, discard_inst) = (inst.clone(), inst.clone());
                line = line
                    .child(
                        div()
                            .text_color(theme::text_tertiary())
                            .child(reason.clone()),
                    )
                    .child(
                        Button::new(("pending-retry", local_id as usize))
                            .cursor_pointer()
                            .small()
                            .label("Retry")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Err(err) =
                                    this.core.retry_message(&retry_inst, channel, local_id)
                                {
                                    this.error(err.to_string(), cx);
                                }
                            })),
                    )
                    .child(
                        Button::new(("pending-discard", local_id as usize))
                            .cursor_pointer()
                            .small()
                            .danger()
                            .label("Delete")
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.core.discard_message(&discard_inst, channel, local_id);
                            })),
                    );
            }
            body = body.child(line);
        }
        let gutter = if compact {
            let shown = (!p.grouped).then(|| {
                div()
                    .text_size(theme::caption_size())
                    .text_color(theme::text_tertiary())
                    .child(time_text.clone())
                    .into_any_element()
            });
            compact_time_slot(&time_text, shown)
        } else if !p.grouped && self.config.buffer.show_avatars {
            let user = me.and_then(|id| s.user(id));
            self.avatar(inst, user, &name, theme::AVATAR)
        } else {
            div().into_any_element()
        };
        let (top, bottom) = crate::rows::pending_padding(p, compact);
        // On its way it is dimmed as a whole; one that failed is not, so its buttons read.
        let on_its_way = !matches!(pending.state, PendingState::Failed(_));
        div()
            .flex()
            .w_full()
            .pt(px(top))
            .pb(px(bottom))
            .px(px(16.))
            .when(on_its_way, |d| d.opacity(0.6))
            .child(
                div()
                    .flex_shrink_0()
                    .when(!compact, |d| d.w(px(GUTTER)))
                    .child(gutter),
            )
            .child(body)
            .into_any_element()
    }
}
