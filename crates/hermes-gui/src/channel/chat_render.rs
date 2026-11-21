//! Drawing the chat: the list, the typing line and the selection bar.

use super::*;

impl Shell {
    pub fn render_chat(
        &self,
        inst: InstanceId,
        channel: Id,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let key = (inst.clone(), channel);
        let store = self.core.store();
        let s = store.instance(&inst);
        let window_state = s.and_then(|s| s.window(channel));
        let loaded = window_state.is_some_and(|w| w.loaded);
        let live = window_state.is_none_or(|w| w.reached_end);
        let Some(view) = self.chats.get(&key) else {
            return div().into_any_element();
        };
        if loaded && !self.first_chat.replace(true) {
            tracing::info!(
                elapsed_ms = self.launched.elapsed().as_millis(),
                "first usable chat"
            );
        }
        let empty = view.rows.is_empty();
        let title = s
            .and_then(|s| s.channel(channel).map(|c| view::channel_title(s, c)))
            .unwrap_or_default();
        let typing = s.and_then(|s| self.typing_line(s, channel));
        let at_end = view.list.is_scrolled_to_end() != Some(false);
        // How many messages are below what shows, when all of them are loaded.
        let below = view
            .rows
            .iter()
            .skip(view.shown_end)
            .filter(|r| matches!(r, Row::Message(_)))
            .count();
        let jump_label = if live && below > 0 {
            SharedString::from(format!("{below} new"))
        } else {
            SharedString::from("Jump to present")
        };
        let jump_news = live && below > 0;
        // The first new message and how many there are, while the divider is off screen and was
        // not seen yet. My own messages never make a divider (`rows::build`).
        let first_unread = rows::first_new(&view.rows)
            .filter(|_| !view.unread_jump_done && !view.divider_seen.get());
        let selected_count = view.selected.len();
        let can_send = s.is_some_and(|s| view::can(s, channel, Permissions::SEND_MESSAGES));
        let reply_name = view.reply_to.map(|id| {
            s.and_then(|s| {
                s.window(channel)?
                    .get(id)
                    .map(|m| s.display_name(s.space_of(channel), m.author.id))
            })
            .unwrap_or_else(|| "a message".into())
        });
        drop(store);

        let content: AnyElement = if !loaded && view.load_failed {
            let (retry_inst, retry_channel) = key.clone();
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .text_color(theme::text_secondary())
                .child("This channel could not be loaded.")
                .child(
                    Button::new("chat-retry")
                        .cursor_pointer()
                        .label("Try again")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_chat(retry_inst.clone(), retry_channel, window, cx);
                        })),
                )
                .into_any_element()
        } else if !loaded {
            // Nothing for a moment (most channels arrive by then), then lines standing in for
            // the messages, instead of a "Loading…" that flashes by (#4).
            {
                use gpui_kit::AnimationExt as _;
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .p(px(16.))
                    .child(theme::skeleton(&format!("chat-wait-{channel}"), 6))
                    .with_animation(
                        SharedString::from(format!("chat-wait-in-{channel}")),
                        gpui_kit::Animation::new(
                            std::time::Duration::from_millis(150) + theme::fade(),
                        ),
                        |d, t| {
                            let wait = 150. / (150. + theme::fade().as_millis() as f32);
                            d.opacity(((t - wait) / (1. - wait)).clamp(0., 1.))
                        },
                    )
                    .into_any_element()
            }
        } else if empty {
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .flex_1()
                .min_h_0()
                .child(icon(assets::CHAT, 40., theme::accent()))
                .child(
                    div()
                        .text_size(theme::heading_size())
                        .child(format!("Welcome to {title}")),
                )
                .child(
                    div()
                        .text_size(theme::secondary_size())
                        .text_color(theme::text_secondary())
                        .child("This is the start of the conversation. Say hello!"),
                )
                .into_any_element()
        } else {
            let row_inst = inst.clone();
            let messages = list(
                view.list.clone(),
                cx.processor(move |this: &mut Shell, ix: usize, _, cx| {
                    this.render_row(&row_inst, channel, ix, cx)
                }),
            )
            .size_full();
            let jump_key = key.clone();
            // The notices over the chat's top keep the "new messages" button below them.
            let below_notices = self.notices_bottom();
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(self.selection_list_marker())
                .child(messages)
                .children(first_unread.map(|(first, count)| {
                    let (jump_inst, jump_channel) = key.clone();
                    div()
                        .absolute()
                        .top(px(8. + below_notices))
                        .right(px(16.))
                        .child(
                            Button::new("jump-unread")
                                .cursor_pointer()
                                .label(format!(
                                    "{count} new message{} · Jump",
                                    if count == 1 { "" } else { "s" }
                                ))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(view) =
                                        this.chats.get_mut(&(jump_inst.clone(), jump_channel))
                                    {
                                        view.unread_jump_done = true;
                                    }
                                    this.jump_to(
                                        jump_inst.clone(),
                                        jump_channel,
                                        first,
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        // It drops in from the top edge.
                        .with_animation(
                            "jump-unread-in",
                            gpui_kit::Animation::new(theme::fade() * 2)
                                .with_easing(gpui_kit::ease_out_quint()),
                            move |d, t| d.opacity(t).top(px(8. * t + below_notices)),
                        )
                }))
                .when(!at_end || !live, |d| {
                    // A small tab on the message box's top edge, out of the way of the messages.
                    d.child(
                        div()
                            .id("jump-present")
                            .absolute()
                            .bottom_0()
                            .right(px(16.))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .px(px(10.))
                            .py(px(3.))
                            .rounded_t(theme::radius_small())
                            .bg(theme::raised())
                            .border_t_1()
                            .border_l_1()
                            .border_r_1()
                            .border_color(theme::border())
                            .cursor_pointer()
                            .text_size(theme::caption_size())
                            .text_color(if jump_news {
                                theme::accent()
                            } else {
                                theme::text_secondary()
                            })
                            .hover(|d| d.bg(theme::hover()))
                            .child(icon(
                                assets::CHEVRON_DOWN,
                                12.,
                                if jump_news {
                                    theme::accent()
                                } else {
                                    theme::text_secondary()
                                },
                            ))
                            .child(jump_label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.jump_to_present(&jump_key, window, cx);
                            }))
                            // It rises out of the message box.
                            .with_animation(
                                "jump-present-in",
                                gpui_kit::Animation::new(theme::fade() * 2)
                                    .with_easing(gpui_kit::ease_out_quint()),
                                |d, t| d.opacity(t).bottom(px(-6. * (1. - t))),
                            ),
                    )
                })
                .children(typing.map(|text| {
                    div()
                        .absolute()
                        .bottom(px(2.))
                        .left(px(16.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(theme::radius_small())
                        .bg(theme::background())
                        .text_size(theme::caption_size())
                        .text_color(theme::text_secondary())
                        .child(typing_dots("typing-dots", theme::text_secondary()))
                        .child(text)
                        // It fades in when someone starts.
                        .with_animation(
                            "typing-in",
                            gpui_kit::Animation::new(theme::fade())
                                .with_easing(gpui_kit::ease_out_quint()),
                            |d, t| d.opacity(t),
                        )
                }))
                .into_any_element()
        };

        let bottom = if selected_count > 0 {
            self.render_selection_bar(&key, selected_count, cx)
        } else if !can_send {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(20.))
                .py(px(16.))
                .text_size(theme::secondary_size())
                .text_color(theme::text_tertiary())
                .child(icon(assets::LOCK, 14., theme::text_tertiary()))
                .child("You cannot send messages in this channel.")
                .into_any_element()
        } else {
            self.render_composer(&key, reply_name, cx)
        };

        let drop_key = key.clone();
        div()
            .flex()
            .flex_col()
            .size_full()
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                for path in paths.paths().iter().filter(|p| p.is_file()) {
                    this.start_upload(&drop_key, path.clone(), None, cx);
                }
            }))
            .child(content)
            .children(self.render_new_devices(&key.0, key.1, cx))
            .children(self.render_unencrypted_bar(&key.0, key.1))
            .child(bottom)
            .into_any_element()
    }

    pub(super) fn typing_line(&self, s: &InstanceStore, channel: Id) -> Option<String> {
        if !self.config.buffer.show_typing {
            return None;
        }
        let space = s.space_of(channel);
        let names: Vec<String> = s
            .typing(channel, now_ms())
            .into_iter()
            .map(|u| s.display_name(space, u))
            .collect();
        view::typing_text(&names)
    }

    pub(super) fn render_selection_bar(
        &self,
        key: &ChannelKey,
        count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let what = if count == 1 {
            "1 message selected".to_owned()
        } else {
            format!("{count} messages selected")
        };
        let cancel_key = key.clone();
        let delete_key = key.clone();
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(12.))
            .mx(px(16.))
            .mb(px(20.))
            .mt(px(8.))
            .px(px(12.))
            .py(px(8.))
            .rounded(theme::row_radius())
            .bg(theme::selected())
            .child(div().text_size(theme::body_size()).child(what))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme::caption_size())
                    .text_color(theme::text_tertiary())
                    .child("Ctrl+click to add · Shift+click for a range"),
            )
            .child(
                Button::new("selection-cancel")
                    .cursor_pointer()
                    .label("Cancel")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(view) = this.chats.get_mut(&cancel_key) {
                            view.clear_selection();
                        }
                        cx.notify();
                    })),
            )
            .child(
                Button::new("selection-delete")
                    .cursor_pointer()
                    .danger()
                    .label("Delete")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Only what still exists and I may still delete.
                        let selectable = selectable_messages(this, &delete_key.0, delete_key.1);
                        if let Some(view) = this.chats.get_mut(&delete_key) {
                            view.selected.retain(|id| selectable.contains(id));
                            let ids: Vec<Id> = view.selected.iter().copied().collect();
                            if !ids.is_empty() {
                                this.cancel_closing("confirm");
                                this.confirm = Some((
                                    Target::Messages(delete_key.0.clone(), delete_key.1, ids),
                                    Action::DeleteMessage,
                                ));
                            }
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}
