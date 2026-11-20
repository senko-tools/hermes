//! Drawing a message: its row, author, reactions and the reaction palette.

use super::*;

impl Shell {
    /// Before a message row is drawn: starts loading its pictures that are not loaded yet.
    pub(super) fn prepare_message(&mut self, key: &ChannelKey, id: Id, cx: &mut Context<Self>) {
        if self.config.buffer.link_previews {
            let text = {
                let store = self.core.store();
                store.instance(&key.0).and_then(|s| {
                    let m = s.window(key.1)?.get(id)?;
                    (!m.deleted && !s.blocked.contains(&m.author.id)).then(|| m.content.clone())
                })
            };
            if let Some(text) = text {
                self.prepare_link_card(&text, cx);
            }
        }
        if !self.config.buffer.image_previews {
            return;
        }
        let pictures: Vec<Attachment> = {
            let store = self.core.store();
            let Some(message) = store
                .instance(&key.0)
                .and_then(|s| s.window(key.1)?.get(id))
            else {
                return;
            };
            message
                .attachments
                .iter()
                .filter(|a| {
                    has_poster(a) && !self.images.thumbs.contains_key(&(key.0.clone(), a.id))
                })
                .cloned()
                .collect()
        };
        for attachment in &pictures {
            self.load_thumbnail(&key.0, attachment, cx);
        }
    }

    pub(super) fn render_row(
        &mut self,
        inst: &InstanceId,
        channel: Id,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = (inst.clone(), channel);
        let message = match self.chats.get(&key).and_then(|v| v.rows.get(ix)) {
            Some(Row::Message(m)) => Some(m.id),
            Some(_) => None,
            None => return div().into_any_element(),
        };
        if let Some(id) = message {
            self.prepare_message(&key, id, cx);
        }
        let store = self.core.store();
        let Some(s) = store.instance(inst) else {
            return div().into_any_element();
        };
        let Some(row) = self.chats.get(&key).and_then(|v| v.rows.get(ix)) else {
            return div().into_any_element();
        };
        // Rows that just came grow in, so the list glides instead of jumping: someone else's
        // fresh message, and mine while it sends (it then stays as it is).
        let grow = (!cx.reduce_motion())
            .then(|| match row {
                Row::Message(m) => s.window(channel).and_then(|w| w.get(m.id)).and_then(|msg| {
                    let age = now_ms() - msg.created_at() as i64;
                    ((-500..1500).contains(&age) && s.me_id() != Some(msg.author.id))
                        .then(|| SharedString::from(format!("grow-{}", m.id)))
                }),
                Row::Pending(p) => s
                    .window(channel)
                    .and_then(|w| w.pending(p.local_id))
                    .filter(|p| matches!(p.state, PendingState::Sending | PendingState::Queued))
                    .filter(|p| now_ms() - p.created_at < 1500)
                    .map(|_| SharedString::from(format!("grow-pending-{:?}", p.local_id))),
                _ => None,
            })
            .flatten();
        let element = match row {
            Row::Older => div()
                .w_full()
                .flex()
                .justify_center()
                .py(px(8.))
                .text_size(theme::caption_size())
                .text_color(theme::text_tertiary())
                .child("Loading older messages…")
                .into_any_element(),
            Row::Day { day, .. } => divider(
                day_label(*day, &self.config.buffer.date_format),
                theme::text_tertiary(),
                theme::border(),
            ),
            Row::NewMessages => {
                // Notes when the divider is on screen, so the button leading to it goes.
                let seen = self.chats.get(&key).map(|v| v.divider_seen.clone());
                let viewport = self.selection.list.clone();
                let shell = cx.entity_id();
                div()
                    .w_full()
                    .relative()
                    .child(new_messages_divider())
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                let on_screen = viewport.get().is_some_and(|v| {
                                    bounds.bottom() > v.top() && bounds.top() < v.bottom()
                                });
                                if on_screen
                                    && let Some(seen) = &seen
                                    && !seen.replace(true)
                                {
                                    cx.notify(shell);
                                }
                            },
                            |_, (), _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .into_any_element()
            }
            Row::Message(m) => match s.window(channel).and_then(|w| w.get(m.id)) {
                Some(message) => self.render_message(s, inst, channel, m, message, cx),
                None => div().into_any_element(),
            },
            Row::Pending(p) => self.render_pending(s, inst, channel, p, cx),
            Row::Gone(id) => {
                let height = self
                    .chats
                    .get(&key)
                    .and_then(|v| v.gone.get(id))
                    .map_or(px(0.), |(h, _)| *h);
                closing_gap(*id, height)
            }
        };
        match grow {
            Some(grow_key) => grow_in(&self.grow_heights, grow_key, element),
            None => element,
        }
    }

    pub(super) fn render_message(
        &self,
        s: &InstanceStore,
        inst: &InstanceId,
        channel: Id,
        m: &MessageRow,
        msg: &ChatMessage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = (inst.clone(), channel);
        let view = self.chats.get(&key);
        let space = s.space_of(channel);
        let id = msg.id;
        let time_text = time::format(
            msg.created_at() as i64,
            &self.config.buffer.timestamp_format,
            self.utc_offset,
        );
        let compact = self.config.buffer.compact;
        let mut body: Vec<AnyElement> = Vec::new();
        // Compact: what goes before the text on its first line, and the time in front of it.
        let mut head_line: Vec<AnyElement> = Vec::new();
        let mut compact_time: Option<AnyElement> = None;
        if let Some(reply) = &m.reply {
            let loaded = s.window(channel).and_then(|w| w.get(reply.id));
            let excerpt = reply_text(loaded, &reply.excerpt);
            let (jump_inst, reply_id) = (inst.clone(), reply.id);
            body.push(
                div()
                    .id(SharedString::from(format!("reply-{id}")))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .text_size(theme::caption_size())
                    .text_color(theme::text_tertiary())
                    // A click jumps to it: its text brightens under the pointer.
                    .hover(|d| d.text_color(theme::text_secondary()))
                    .child(icon(assets::REPLY, 12., theme::text_tertiary()))
                    .child(
                        div()
                            .text_color(theme::text_secondary())
                            .font_weight(FontWeight::MEDIUM)
                            .child(reply.author.clone()),
                    )
                    .child({
                        let part = Part {
                            message: id,
                            index: selection::REPLY,
                        };
                        let (text, record) = self.selectable(inst, channel, part, excerpt, vec![]);
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(text)
                            .child(record)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        if !this.selection.dragged {
                            this.jump_to(jump_inst.clone(), channel, reply_id, window, cx);
                        }
                    }))
                    .into_any_element(),
            );
        }
        if !m.grouped {
            let name = s.display_name(space, msg.author.id);
            let color = space
                .and_then(|sp| view::role_color(s, sp, msg.author.id))
                .map_or(theme::role_default(), theme::rgb_u32);
            let part = |index| Part { message: id, index };
            let (name, name_record) =
                self.selectable(inst, channel, part(selection::AUTHOR), name, vec![]);
            let (time, time_record) = self.selectable(
                inst,
                channel,
                part(selection::TIME),
                time_text.clone(),
                vec![],
            );
            let author = self.author_menu(
                div()
                    .id(SharedString::from(format!("author-{id}")))
                    .relative()
                    .cursor_pointer()
                    .text_size(theme::body_size())
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color)
                    .hover(|d| d.underline())
                    .child(name)
                    .child(name_record),
                inst,
                space,
                msg.author.id,
                cx,
            );
            // Under the pointer its time says the whole date, and when it was edited.
            let time_tip = time_tip(msg, self.utc_offset);
            let time = div()
                .id(SharedString::from(format!("time-{id}")))
                .relative()
                .text_size(theme::caption_size())
                .text_color(theme::text_tertiary())
                .child(time)
                .child(time_record)
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(time_tip.clone()).build(window, cx)
                });
            let lock = crate::encryption::lock_mark(
                msg.lock,
                s.channel_encrypted(channel)
                    && msg.kind == hermes_client_core::proto::models::MessageKind::Default,
            );
            if compact {
                // Time, small picture, name and text on one line.
                if self.config.buffer.show_avatars {
                    let name = s.display_name(space, msg.author.id);
                    let user = s.user(msg.author.id).unwrap_or(&msg.author);
                    let avatar = self.avatar(inst, Some(user), &name, COMPACT_AVATAR);
                    head_line.push(
                        self.author_menu(
                            div()
                                .id(SharedString::from(format!("avatar-{id}")))
                                .cursor_pointer()
                                .child(avatar),
                            inst,
                            space,
                            msg.author.id,
                            cx,
                        )
                        .into_any_element(),
                    );
                }
                head_line.push(author.into_any_element());
                head_line.extend(lock.map(IntoElement::into_any_element));
                compact_time = Some(time.into_any_element());
            } else {
                body.push(
                    div()
                        .flex()
                        .gap(px(8.))
                        .items_baseline()
                        .child(author)
                        .child(time)
                        .children(lock)
                        .into_any_element(),
                );
            }
        }
        let revealed = view.is_some_and(|v| v.revealed.contains(&id));
        let blocks = if s.blocked.contains(&msg.author.id) && !revealed {
            std::rc::Rc::new(vec![markup::Block::Text(vec![markup::Piece {
                text: "Message from someone you blocked. Click to show it.".to_owned(),
                link: Some(Link::Reveal),
                ..markup::Piece::default()
            }])])
        } else {
            self.cached_blocks(&msg.content, s, space)
        };
        let last = blocks.len().saturating_sub(1);
        let system = m.system;
        let mut texts: Vec<AnyElement> = Vec::new();
        for (i, block) in blocks.iter().cloned().enumerate() {
            let edited = i == last && msg.edited_at.is_some();
            texts
                .push(self.render_block(block, inst, channel, id, i, system, revealed, edited, cx));
        }
        if msg.content.is_empty() && msg.edited_at.is_some() {
            texts.push(
                div()
                    .text_size(theme::caption_size())
                    .text_color(theme::text_tertiary())
                    .child("(edited)")
                    .into_any_element(),
            );
        }
        if compact && !(head_line.is_empty() && texts.is_empty()) {
            body.push(compact_line(head_line, texts));
        } else {
            body.extend(texts);
        }
        for (index, attachment) in msg.attachments.iter().enumerate() {
            let place = FilePlace {
                channel,
                message: id,
                index,
            };
            // A file keeps a little more room from what is above it. When nothing is, the row's
            // own space holds it (`rows::padding`), so the highlight stays even.
            let after_something = !body.is_empty();
            // A row, so the file is only as wide as it looks: its hover and menu end with it.
            body.push(
                div()
                    .flex()
                    .min_w_0()
                    .when(after_something, |d| d.mt(px(crate::rows::FILE_SPACE)))
                    .child(self.render_attachment(inst, place, attachment, cx))
                    .into_any_element(),
            );
        }
        if !s.blocked.contains(&msg.author.id)
            && let Some(card) = self.render_link_card(&msg.content, cx)
        {
            body.push(
                div()
                    .flex()
                    .min_w_0()
                    .mt(px(crate::rows::FILE_SPACE))
                    .child(card)
                    .into_any_element(),
            );
        }
        // Messages and chips drawn before: a chip new on a message already shown pops in, one
        // only shown again (scrolled back into view) does not.
        let shown_before = {
            let mut seen = self.reactions_seen.borrow_mut();
            if seen.messages.len() > REACTIONS_SEEN_MAX {
                seen.messages.clear();
                seen.chips.clear();
            }
            !seen.messages.insert(id)
        };
        if !msg.reactions.is_empty() {
            let chips = msg.reactions.iter().enumerate().map(|(n, r)| {
                let fresh_chip = self
                    .reactions_seen
                    .borrow_mut()
                    .chips
                    .insert((id, r.emoji.clone()))
                    && shown_before;
                let (mine, emoji) = (r.me, r.emoji.clone());
                let (inst, emoji_id) =
                    (inst.clone(), SharedString::from(format!("react-{id}-{n}")));
                let chip = div()
                    .id(emoji_id)
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(10.))
                    .border_1()
                    .cursor_pointer()
                    .text_size(theme::secondary_size())
                    .when(!mine, |d| d.hover(|d| d.bg(theme::hover())))
                    .child(r.emoji.clone())
                    .child(reaction_count(&format!("{id}-{}", r.emoji), r.count))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.react(inst.clone(), channel, id, emoji.clone(), !mine, cx);
                    }))
                    // Reacting or taking it back: the accent flows in or out.
                    .with_spring(
                        SharedString::from(format!("react-mine-{id}-{}", r.emoji)),
                        gpui_kit::SpringAnimation::new(crate::theme::paced(
                            crate::hover::HOVER_SPRING,
                        ))
                        .to(mine),
                        |d, phase| {
                            let t = phase.0.clamp(0., 1.);
                            d.bg(theme::mix(theme::sidebar(), theme::accent_soft(), t))
                                .border_color(theme::mix(theme::border(), theme::accent(), t))
                        },
                    );
                if fresh_chip {
                    // New on a message already shown: it pops up into its place.
                    div()
                        .relative()
                        .child(chip)
                        .with_spring(
                            SharedString::from(format!("react-new-{id}-{}", r.emoji)),
                            gpui_kit::SpringAnimation::new(crate::theme::paced(
                                gpui_kit::SpringConfig::new(520., 22., 1.),
                            ))
                            .to(true)
                            .from(false),
                            |d, phase| {
                                d.opacity(phase.0.clamp(0., 1.))
                                    .top(px(6. * (1. - phase.0)))
                            },
                        )
                        .into_any_element()
                } else {
                    chip.into_any_element()
                }
            });
            body.push(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .pt(px(4.))
                    .children(chips)
                    .into_any_element(),
            );
        }

        let gutter: AnyElement = if compact {
            compact_time_slot(&time_text, compact_time)
        } else if !m.grouped && self.config.buffer.show_avatars {
            // The user's current picture (the message keeps the one from when it was sent).
            let name = s.display_name(space, msg.author.id);
            let user = s.user(msg.author.id).unwrap_or(&msg.author);
            let avatar = self.avatar(inst, Some(user), &name, theme::AVATAR);
            self.author_menu(
                div()
                    .id(SharedString::from(format!("avatar-{id}")))
                    .cursor_pointer()
                    .child(avatar),
                inst,
                space,
                msg.author.id,
                cx,
            )
            .into_any_element()
        } else if m.grouped {
            // Always laid out and only made visible on hover, so hovering never resizes the row.
            let tip = time_tip(msg, self.utc_offset);
            div()
                .id(SharedString::from(format!("gutter-time-{id}")))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .pt(px(3.))
                .text_size(theme::caption_size())
                .text_color(rgba(0))
                .group_hover(ROW_GROUP, |d| d.text_color(theme::text_tertiary()))
                .child(time_text)
                .into_any_element()
        } else {
            div().into_any_element()
        };

        let selected = view.is_some_and(|v| v.selected.contains(&id));
        let jumped = view.is_some_and(|v| v.highlight == Some(id));
        let palette_open = view.is_some_and(|v| v.palette == Some(id));
        let highlight = m.mentions_me;
        let (top, bottom) = crate::rows::padding(m, self.config.buffer.compact);
        let (click_inst, press_inst) = (inst.clone(), inst.clone());
        let target = Target::Message(inst.clone(), channel, id);
        let element = div()
            .id(SharedString::from(format!("message-{id}")))
            .group(ROW_GROUP)
            .relative()
            .flex()
            .w_full()
            .pt(px(top))
            .pb(px(bottom))
            .pl(px(8.))
            .pr(px(8.))
            .rounded(theme::radius_small())
            .when(selected, |d| d.bg(theme::accent_soft()))
            // Just edited while it shows: a soft flash says it changed.
            .when(
                msg.edited_at
                    .is_some_and(|at| (-500..1500).contains(&(hermes_client_core::now_ms() - at))),
                |d| {
                    d.child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded(theme::radius_small())
                            .with_animation(
                                SharedString::from(format!(
                                    "edited-flash-{id}-{}",
                                    msg.edited_at.unwrap_or_default()
                                )),
                                gpui_kit::Animation::new(std::time::Duration::from_millis(900))
                                    .with_easing(gpui_kit::ease_out_quint()),
                                |d, t| d.bg(theme::alpha(theme::accent_soft(), 1. - t)),
                            ),
                    )
                },
            )
            // The message jumped to glows, then fades back over the time it stays marked.
            .when(jumped && !selected, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(theme::radius_small())
                        .with_animation(
                            SharedString::from(format!("jumped-{id}")),
                            gpui_kit::Animation::new(HIGHLIGHT_FOR),
                            |d, t| {
                                // Full for the first half, then easing away.
                                let fade = ((t - 0.5) * 2.).clamp(0., 1.);
                                d.bg(theme::alpha(theme::selected(), 1. - fade * fade))
                            },
                        ),
                )
            })
            .when(highlight && !selected && !jumped, |d| {
                d.bg(theme::mention_soft())
                    .border_l_2()
                    .border_color(theme::accent())
            })
            // Under the pointer its background eases in and out instead of switching.
            .when(!selected && !jumped && !highlight, |d| {
                let key = format!("message-hover-{id}");
                let hovered = self.is_hovered(&key) && self.hovered_file != Some(id);
                d.on_hover(self.track_hover(key.clone(), cx)).child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(theme::radius_small())
                        .with_spring(
                            SharedString::from(key),
                            gpui_kit::SpringAnimation::new(crate::theme::paced(
                                crate::hover::HOVER_SPRING,
                            ))
                            .to(hovered),
                            |d, phase| {
                                d.bg(theme::alpha(theme::hover_soft(), phase.0.clamp(0., 1.)))
                            },
                        ),
                )
            })
            // The text cursor shows over text only (the root follows the pointer, #2); a press
            // in the row's empty space still starts a selection from the nearest text.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui_kit::MouseDownEvent, _, cx| {
                    this.text_press(&press_inst, channel, id, event, cx);
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.selection.dragged {
                    return;
                }
                let modifiers = window.modifiers();
                let selectable = selectable_messages(this, &click_inst, channel);
                if let Some(view) = this.chats.get_mut(&(click_inst.clone(), channel)) {
                    view.click(id, modifiers.control, modifiers.shift, &selectable);
                    if view.highlight == Some(id) {
                        view.highlight = None;
                    }
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .when(!compact, |d| d.w(px(GUTTER)))
                    .child(gutter),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .flex_1()
                    .min_w_0()
                    .children(body),
            )
            .children(palette_open.then(|| self.render_palette(inst, channel, id, cx)));
        // The row fills the list's width; the margin keeps its highlight off the edges.
        let row = div()
            .w_full()
            .px(px(8.))
            .child(self.with_menu(element, target, cx));
        // Someone else's message that just arrived fades in, rising into place. Only fresh ones:
        // the list drops rows scrolled away, and an old one coming back must not replay it.
        let age = hermes_client_core::now_ms() - msg.created_at() as i64;
        let fresh = (-500..1500).contains(&age);
        let mine = s.me_id() == Some(msg.author.id);
        if fresh && !mine {
            row.relative()
                .with_animation(
                    SharedString::from(format!("arrived-{id}")),
                    gpui_kit::Animation::new(theme::fade() * 2)
                        .with_easing(gpui_kit::ease_out_quint()),
                    |d, t| d.opacity(t).top(px(10. * (1. - t))),
                )
                .into_any_element()
        } else if fresh {
            // Mine, just confirmed: it brightens from the dimmed row it was while sending.
            row.with_animation(
                SharedString::from(format!("confirmed-{id}")),
                gpui_kit::Animation::new(theme::fade() * 2).with_easing(gpui_kit::ease_out_quint()),
                |d, t| d.opacity(0.6 + 0.4 * t),
            )
            .into_any_element()
        } else {
            row.into_any_element()
        }
    }

    /// `element` opens the person's profile on a left click and their menu on a right click.
    pub(super) fn author_menu(
        &self,
        element: gpui_kit::Stateful<gpui_kit::Div>,
        inst: &InstanceId,
        space: Option<Id>,
        user: Id,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let target = Target::Member(inst.clone(), space, user);
        let left = target.clone();
        self.with_own_menu(element, target, cx)
            .on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
                    cx.stop_propagation();
                    if !this.selection.dragged {
                        this.open_profile(&left, event.position(), cx);
                    }
                }),
            )
    }

    pub fn react(
        &mut self,
        inst: InstanceId,
        channel: Id,
        message: Id,
        emoji: String,
        add: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self.chats.get_mut(&(inst.clone(), channel)) {
            view.palette = None;
        }
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                if add {
                    core.react(&inst, channel, message, &emoji).await
                } else {
                    core.unreact(&inst, channel, message, &emoji).await
                }
            },
            |this, result, _, cx| {
                if let Err(err) = result {
                    this.error(format!("reaction failed: {err}"), cx);
                }
            },
        );
    }

    pub(super) fn render_palette(
        &self,
        inst: &InstanceId,
        channel: Id,
        message: Id,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let quick: Vec<String> = self
            .config
            .buffer
            .quick_reactions
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let more_inst = inst.clone();
        let more = div()
            .id(("quick-reaction", usize::MAX))
            .flex()
            .items_center()
            .justify_center()
            .size(px(30.))
            .rounded(theme::radius_small())
            .cursor_pointer()
            .hover(|d| d.bg(theme::hover()))
            .child("…")
            .on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_emoji_picker(
                        crate::emoji_picker::PickFor::React(more_inst.clone(), channel, message),
                        event.position(),
                        window,
                        cx,
                    );
                }),
            );
        let buttons = quick.into_iter().enumerate().map(|(n, emoji)| {
            let inst = inst.clone();
            div()
                .id(("quick-reaction", n))
                .flex()
                .items_center()
                .justify_center()
                .size(px(30.))
                .rounded(theme::radius_small())
                .cursor_pointer()
                .hover(|d| d.bg(theme::hover()))
                .child(emoji.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.react(inst.clone(), channel, message, emoji.clone(), true, cx);
                }))
        });
        let buttons = buttons.chain(std::iter::once(more));
        let close_inst = inst.clone();
        div()
            .id("palette")
            .absolute()
            .right(px(16.))
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(3.))
            .rounded(theme::row_radius())
            .bg(theme::sidebar())
            .border_1()
            .border_color(theme::border())
            .occlude()
            .children(buttons)
            .child(
                crate::ui::tip_button(self, "palette-close", assets::CLOSE, "Close", cx)
                    // Like the reactions beside it.
                    .size(px(30.))
                    .hover(|d| d.bg(theme::hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(view) = this.chats.get_mut(&(close_inst.clone(), channel)) {
                            view.palette = None;
                        }
                        cx.notify();
                    })),
            )
            // It pops up over the message's corner as it fades in.
            .with_animation(
                SharedString::from(format!("palette-in-{message}")),
                gpui_kit::Animation::new(theme::fade()).with_easing(gpui_kit::ease_out_quint()),
                |d, t| d.opacity(t).top(px(-16. + 6. * (1. - t))),
            )
            .into_any_element()
    }
}
