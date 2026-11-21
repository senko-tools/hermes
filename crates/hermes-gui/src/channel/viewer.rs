//! The full view of a picture, and forwarding a message.

use super::*;

impl Shell {
    pub(crate) fn open_image(
        &mut self,
        inst: InstanceId,
        attachment: Attachment,
        cx: &mut Context<Self>,
    ) {
        self.viewer = Some(Viewer {
            inst: inst.clone(),
            attachment: attachment.clone(),
            image: Img::Loading,
            zoom: 1.,
            pan: (0., 0.),
            drag: None,
            laid_out: Default::default(),
        });
        self.request_original((inst, attachment.id), attachment, true, cx);
        cx.notify();
    }

    pub fn render_viewer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let viewer = self.viewer.as_ref()?;
        let layout = viewer.laid_out.clone();
        let zoom = viewer.zoom;
        let zoomed = zoom > 1. && layout.get().fitted.0 > 0.;
        let preview = self
            .images
            .thumbs
            .get(&(viewer.inst.clone(), viewer.attachment.id));
        let shown = match (&viewer.image, preview) {
            (Img::Loading, Some(ready @ Img::Ready(_))) => ready,
            (image, _) => image,
        };
        let content = match shown {
            Img::Ready(picture) => {
                let shown = picture.shown(self.animating());
                // Its own width in window pixels, for the double-click's full size.
                let natural = shown.size(0).width.0 as f32 / self.scale.max(0.5);
                let fitted = layout.get().fitted;
                // The id lets gpui step through an animation's frames; without one a GIF
                // stays on its first frame.
                let picture =
                    img(shown)
                        .object_fit(gpui_kit::ObjectFit::Contain)
                        .id(SharedString::from(format!(
                            "viewer-pic-{}",
                            viewer.attachment.id
                        )));
                let picture = if zoomed {
                    picture
                        .flex_shrink_0()
                        .w(px(fitted.0 * zoom))
                        .h(px(fitted.1 * zoom))
                } else {
                    picture.max_w_full().max_h_full()
                };
                let record = layout.clone();
                div()
                    .id("viewer-picture")
                    .relative()
                    .flex()
                    .flex_shrink_0()
                    .left(px(viewer.pan.0))
                    .top(px(viewer.pan.1))
                    .when(zoomed, |d| d.cursor_grab())
                    .when(!zoomed, |d| d.max_w_full().max_h_full())
                    .child(picture)
                    // Fitted, it notes the size it fits at: zooming starts from there.
                    .when(!zoomed, |d| {
                        gpui_kit::base::ElementExt::on_prepaint(d, move |b, _, _| {
                            let mut l = record.get();
                            l.fitted = (b.size.width.as_f32(), b.size.height.as_f32());
                            record.set(l);
                        })
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if let Some(v) = &mut this.viewer
                                && v.zoom > 1.
                            {
                                let at = event.position;
                                v.drag = Some(((at.x.as_f32(), at.y.as_f32()), v.pan));
                            }
                        }),
                    )
                    // A click on the picture leaves the viewer open; a double-click zooms.
                    .on_click(
                        cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
                            cx.stop_propagation();
                            if event.click_count() >= 2
                                && let Some(v) = &mut this.viewer
                            {
                                let at = event.position();
                                v.toggle_zoom((at.x.as_f32(), at.y.as_f32()), natural);
                                cx.notify();
                            }
                        }),
                    )
                    .into_any_element()
            }
            Img::Failed => {
                let (inst, attachment) = (viewer.inst.clone(), viewer.attachment.clone());
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .child("The picture could not be loaded")
                    .child(
                        gpui_kit::component::button::Button::new("viewer-retry")
                            .cursor_pointer()
                            .label("Try again")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_image(inst.clone(), attachment.clone(), cx);
                            })),
                    )
                    .into_any_element()
            }
            Img::Loading => "Loading…".into_any_element(),
        };
        Some(
            gpui_kit::deferred(
                div()
                    .id("viewer")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .flex_col()
                    .bg(theme::scrim_strong())
                    .occlude()
                    .font_family(theme::font())
                    .text_color(theme::text())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.viewer = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_end()
                            .gap(px(8.))
                            .p(px(12.))
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .child(viewer.attachment.filename.clone()),
                            )
                            .child(
                                crate::ui::tip_button(
                                    self,
                                    "viewer-download",
                                    assets::DOWNLOAD,
                                    "Download",
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        cx.stop_propagation();
                                        if let Some(viewer) = &this.viewer {
                                            this.download(
                                                viewer.inst.clone(),
                                                viewer.attachment.clone(),
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            )
                            .child(
                                crate::ui::tip_button(
                                    self,
                                    "viewer-close",
                                    assets::CLOSE,
                                    "Close",
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.viewer = None;
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child({
                        let record = layout.clone();
                        div()
                            .id("viewer-area")
                            .relative()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .p(px(16.))
                            .child(
                                gpui_kit::canvas(
                                    move |bounds, _, _| {
                                        let mut l = record.get();
                                        let c = bounds.center();
                                        l.middle = (c.x.as_f32(), c.y.as_f32());
                                        l.area = (
                                            bounds.size.width.as_f32() - 32.,
                                            bounds.size.height.as_f32() - 32.,
                                        );
                                        record.set(l);
                                    },
                                    |_, (), _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .child(content)
                            // The wheel zooms toward the pointer.
                            .on_scroll(cx.listener(|this, event: &gpui_kit::ScrollEvent, _, cx| {
                                let Some(v) = &mut this.viewer else { return };
                                // The picture, or its preview while the picture loads.
                                let preview =
                                    this.images.thumbs.get(&(v.inst.clone(), v.attachment.id));
                                if !matches!(v.image, Img::Ready(_))
                                    && !matches!(
                                        (&v.image, preview),
                                        (Img::Loading, Some(Img::Ready(_)))
                                    )
                                {
                                    return;
                                }
                                cx.stop_propagation();
                                let dy = event.delta.pixel_delta(px(20.)).y.as_f32();
                                let at = event.position;
                                v.zoom_to(
                                    v.zoom * (dy * 0.004).exp(),
                                    (at.x.as_f32(), at.y.as_f32()),
                                );
                                cx.notify();
                            }))
                            // Dragging moves a zoomed picture.
                            .on_mouse_move(cx.listener(
                                |this, event: &gpui_kit::MouseMoveEvent, _, cx| {
                                    let Some(v) = &mut this.viewer else { return };
                                    let Some((start, pan)) = v.drag else { return };
                                    if event.pressed_button != Some(MouseButton::Left) {
                                        v.drag = None;
                                        return;
                                    }
                                    let at = event.position;
                                    v.pan = (
                                        pan.0 + at.x.as_f32() - start.0,
                                        pan.1 + at.y.as_f32() - start.1,
                                    );
                                    v.clamp_pan();
                                    cx.notify();
                                },
                            ))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, _| {
                                    if let Some(v) = &mut this.viewer {
                                        v.drag = None;
                                    }
                                }),
                            )
                    }),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }

    /// Sends message `message` of `from` again in `to`, quoted under a "Forwarded from" line.
    /// Files stay where they are (an upload belongs to one message); a link leads to them.
    pub fn forward_message(
        &mut self,
        inst: InstanceId,
        from: Id,
        message: Id,
        to: Id,
        cx: &mut Context<Self>,
    ) {
        let content = {
            let store = self.core.store();
            let Some(s) = store.instance(&inst) else {
                return;
            };
            let Some(m) = s.window(from).and_then(|w| w.get(message)) else {
                return;
            };
            let place = s
                .channel(from)
                .map(|c| view::channel_title(s, c))
                .unwrap_or_default();
            let author = s.display_name(s.space_of(from), m.author.id);
            forwarded_text(
                &m.content,
                &author,
                &place,
                m.attachments.len(),
                &format!("hermes://{inst}/channels/{from}/{message}"),
            )
        };
        let draft = Draft {
            content,
            reply_to: None,
            attachments: Vec::new(),
        };
        match self.core.send_message(&inst, to, draft) {
            Ok(_) => self.info("Forwarded", cx),
            Err(err) => self.error(format!("could not forward: {err}"), cx),
        }
        if self.chats.contains_key(&(inst.clone(), to)) {
            self.sync_rows(&(inst, to));
        }
    }

    // ---- message actions (from the menu) ------------------------------------------------------
}
