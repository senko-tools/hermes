//! The message list: rows from the store, loading older and newer, read marks and jumps.

use super::*;

impl Shell {
    /// Says so above the chat when messages are not getting through: at once when the
    /// connection gave up, and after [`crate::sidebar::BANNER_AFTER`] while it is reconnecting
    /// (a short blip only shows in the connection icon).
    pub(super) fn render_connection_banner(
        &self,
        inst: &InstanceId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use hermes_client_core::gateway::ConnStatus;
        let text = match self.core.status(inst) {
            ConnStatus::Reconnecting { attempt }
                if self
                    .core
                    .down_for(inst)
                    .is_some_and(|d| d >= crate::sidebar::BANNER_AFTER) =>
            {
                format!("Connection lost. Reconnecting (try {attempt})…")
            }
            ConnStatus::Failed { reason } => format!("Not connected: {reason}"),
            // Back after the banner showed: it says so in green for a moment, then goes.
            status => {
                let mut banners = self.reconnect_banner.borrow_mut();
                match banners.get(inst) {
                    Some(None) if status == ConnStatus::Connected => {
                        let now = std::time::Instant::now();
                        banners.insert(inst.clone(), Some(now));
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(RECONNECTED_FOR).await;
                            let _ = this.update(cx, |_, cx| cx.notify());
                        })
                        .detach();
                    }
                    Some(Some(at)) if at.elapsed() < RECONNECTED_FOR => {}
                    _ => {
                        banners.remove(inst);
                        return None;
                    }
                }
                return Some(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(8.))
                        .mx(px(16.))
                        .mb(px(6.))
                        .p(px(8.))
                        .rounded(theme::row_radius())
                        .bg(theme::raised())
                        .border_1()
                        .border_color(theme::success())
                        .text_size(theme::secondary_size())
                        .text_color(theme::success())
                        .child(icon(assets::LAN_CONNECT, 16., theme::success()))
                        .child(div().flex_1().child("Reconnected"))
                        .into_any_element(),
                );
            }
        };
        self.reconnect_banner
            .borrow_mut()
            .insert(inst.clone(), None);
        let inst = inst.clone();
        Some(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(8.))
                .mx(px(16.))
                .mb(px(6.))
                .p(px(8.))
                .rounded(theme::row_radius())
                .bg(theme::raised())
                .border_1()
                .border_color(theme::warning())
                .text_size(theme::secondary_size())
                .child(icon(assets::LAN_DISCONNECT, 16., theme::warning()))
                .child(div().flex_1().min_w_0().truncate().child(text))
                .child(
                    Button::new("connection-retry")
                        .cursor_pointer()
                        .label("Reconnect")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Err(err) = this.core.set_online(&inst, true) {
                                this.error(format!("could not reconnect: {err}"), cx);
                            }
                        })),
                )
                // It rises over the message box as it fades in.
                .relative()
                .with_animation(
                    "connection-banner-in",
                    gpui_kit::Animation::new(theme::fade() * 2)
                        .with_easing(gpui_kit::ease_out_quint()),
                    |d, t| d.opacity(t).top(px(6. * (1. - t))),
                )
                .into_any_element(),
        )
    }

    /// A setting that changes how messages look moved: every open chat follows it at once.
    pub fn refresh_chats(&mut self) {
        let keys: Vec<ChannelKey> = self.chats.keys().cloned().collect();
        for key in &keys {
            self.sync_rows(key);
        }
        self.remeasure_chats();
    }

    /// Rebuilds a channel's rows and tells the list only what differs, which keeps its position.
    pub(super) fn sync_rows(&mut self, key: &ChannelKey) {
        let store = self.core.store();
        let Some(view) = self.chats.get_mut(key) else {
            return;
        };
        let Some(s) = store.instance(&key.0) else {
            return;
        };
        // A message deleted since: its gap closes from the height it had. Gaps closed by now go.
        let window = s.window(key.1);
        let closing = theme::fade() * 2 + std::time::Duration::from_millis(40);
        view.gone.retain(|_, (_, since)| since.elapsed() < closing);
        for (ix, row) in view.rows.iter().enumerate() {
            if let Row::Message(m) = row
                && window.and_then(|w| w.get(m.id)).is_some_and(|m| m.deleted)
                && let Some(bounds) = view.list.bounds_for_item(ix)
                && bounds.size.height > px(0.)
            {
                view.gone
                    .insert(m.id, (bounds.size.height, std::time::Instant::now()));
            }
        }
        let new = match window {
            Some(window) => rows::build(
                s,
                window,
                &rows::Options {
                    group_minutes: self.config.buffer.group_minutes,
                    utc_offset: self.utc_offset,
                    read_marker: view.read_marker,
                    gone: view.gone.keys().copied().collect(),
                },
            ),
            None => Vec::new(),
        };
        let old_keys: Vec<RowKey> = view.rows.iter().map(Row::key).collect();
        let new_keys: Vec<RowKey> = new.iter().map(Row::key).collect();
        let signatures: Vec<u64> = new.iter().map(|r| signature(r, s, key.1)).collect();
        let (front, back) = common_ends(&old_keys, &new_keys);
        if front + back != old_keys.len() || front + back != new_keys.len() {
            view.list
                .splice(front..old_keys.len() - back, new_keys.len() - front - back);
        }
        for (i, signature) in signatures.iter().enumerate().take(front) {
            if view.signatures[i] != *signature {
                view.list.remeasure_items(i..i + 1);
            }
        }
        for i in 0..back {
            let (old, new) = (old_keys.len() - 1 - i, new_keys.len() - 1 - i);
            if view.signatures[old] != signatures[new] {
                view.list.remeasure_items(new..new + 1);
            }
        }
        view.rows = new;
        view.signatures = signatures;
        if let Some(target) = view.jump
            && let Some(ix) = view
                .rows
                .iter()
                .position(|r| r.key() == RowKey::Message(target))
        {
            view.list.scroll_to_reveal_item(ix);
            view.jump = None;
        }
    }

    pub(super) fn chat_scrolled(
        &mut self,
        key: &ChannelKey,
        near_top: bool,
        at_end: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.chats.get(key) else {
            return;
        };
        if near_top && !view.loading_older {
            self.load_older(key, cx);
        }
        let jumping = self.chats.get(key).is_some_and(|v| v.jump.is_some());
        if at_end && !jumping {
            self.load_newer(key, cx);
            self.mark_open_read(cx);
        }
        // The "jump to present" button follows the position.
        cx.notify();
    }

    /// Loads the page before the oldest loaded message.
    pub(super) fn load_older(&mut self, key: &ChannelKey, cx: &mut Context<Self>) {
        let oldest = {
            let store = self.core.store();
            let Some(window) = store.instance(&key.0).and_then(|s| s.window(key.1)) else {
                return;
            };
            if !window.loaded || window.reached_start {
                return;
            }
            match window.messages.first() {
                Some(m) => m.id,
                None => return,
            }
        };
        if let Some(view) = self.chats.get_mut(key) {
            view.loading_older = true;
        }
        let core = self.core.clone();
        let (inst, channel) = key.clone();
        let done = key.clone();
        self.run(
            cx,
            async move {
                core.load_history(&inst, channel, HistoryAnchor::Before(oldest))
                    .await
            },
            move |this, result, _, cx| {
                if let Err(err) = result {
                    tracing::warn!("older messages: {err}");
                    this.error(format!("could not load older messages: {err}"), cx);
                }
                if let Some(view) = this.chats.get_mut(&done) {
                    view.loading_older = false;
                }
            },
        );
    }

    /// At the bottom of a window that does not reach the present (after a jump): the next page.
    pub(super) fn load_newer(&mut self, key: &ChannelKey, cx: &mut Context<Self>) {
        if self.chats.get(key).is_none_or(|v| v.loading_newer) {
            return;
        }
        let newest = {
            let store = self.core.store();
            let Some(window) = store.instance(&key.0).and_then(|s| s.window(key.1)) else {
                return;
            };
            if !window.loaded || window.reached_end {
                return;
            }
            match window.messages.last() {
                Some(m) => m.id,
                None => return,
            }
        };
        if let Some(view) = self.chats.get_mut(key) {
            view.loading_newer = true;
        }
        let core = self.core.clone();
        let (inst, channel) = key.clone();
        let done = key.clone();
        self.run(
            cx,
            async move {
                core.load_history(&inst, channel, HistoryAnchor::After(newest))
                    .await
            },
            move |this, result, _, cx| {
                if let Err(err) = result {
                    tracing::warn!("newer messages: {err}");
                    this.error(format!("could not load newer messages: {err}"), cx);
                }
                if let Some(view) = this.chats.get_mut(&done) {
                    view.loading_newer = false;
                }
            },
        );
    }

    /// Marks the open channel read when its newest message is on screen and the window has the
    /// focus (section 15.2).
    pub fn mark_open_read(&self, _cx: &mut Context<Self>) {
        if !self.focused {
            return;
        }
        let Some((inst, channel)) = self.open_channel() else {
            return;
        };
        let Some(view) = self.chats.get(&(inst.clone(), channel)) else {
            return;
        };
        if view.list.is_scrolled_to_end() == Some(false) {
            return;
        }
        let last = {
            let store = self.core.store();
            let Some(s) = store.instance(&inst) else {
                return;
            };
            let Some(window) = s.window(channel) else {
                return;
            };
            if !window.reached_end {
                return;
            }
            let Some(last) = window
                .messages
                .iter()
                .rev()
                .find(|m| !m.deleted)
                .map(|m| m.id)
            else {
                return;
            };
            if s.read_states
                .get(&channel)
                .is_some_and(|r| r.last_read_id >= last)
            {
                return;
            }
            last
        };
        self.core.mark_read(&inst, channel, last);
    }

    /// Scrolls the open channel to `message`, loading its page if needed.
    pub fn jump_to(
        &mut self,
        inst: InstanceId,
        channel: Id,
        message: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = (inst.clone(), channel);
        if self.open_channel().as_ref() != Some(&key) {
            self.select(inst.clone(), channel, window, cx);
        }
        self.chat_view(&key, window, cx);
        if let Some(view) = self.chats.get_mut(&key) {
            view.jump = Some(message);
            view.highlight = Some(message);
        }
        let clear = key.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HIGHLIGHT_FOR).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(view) = this.chats.get_mut(&clear)
                    && view.highlight == Some(message)
                {
                    view.highlight = None;
                    cx.notify();
                }
            });
        })
        .detach();
        let loaded = self
            .core
            .store()
            .instance(&inst)
            .and_then(|s| s.window(channel))
            .is_some_and(|w| w.loaded && w.get(message).is_some());
        if loaded {
            self.sync_rows(&key);
            return;
        }
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                core.load_history(&inst, channel, HistoryAnchor::Around(message))
                    .await
            },
            move |this, result, _, cx| {
                if let Err(err) = result {
                    this.error(format!("could not load the message: {err}"), cx);
                }
                this.sync_rows(&key);
            },
        );
    }

    /// Shows a channel from the first message at or after `unix_ms` (the last one before it when
    /// nothing came after).
    pub fn jump_to_time(
        &mut self,
        inst: InstanceId,
        channel: Id,
        unix_ms: i64,
        cx: &mut Context<Self>,
    ) {
        let anchor = Id::min_at_unix_ms(u64::try_from(unix_ms).unwrap_or(0));
        let core = self.core.clone();
        let load_inst = inst.clone();
        self.run(
            cx,
            async move {
                core.load_history(&load_inst, channel, HistoryAnchor::Around(anchor))
                    .await
            },
            move |this, result, window, cx| {
                if let Err(err) = result {
                    this.error(format!("could not load that day: {err}"), cx);
                    return;
                }
                let found = {
                    let store = this.core.store();
                    store
                        .instance(&inst)
                        .and_then(|s| s.window(channel))
                        .and_then(|w| {
                            w.messages
                                .iter()
                                .find(|m| m.id >= anchor)
                                .or_else(|| w.messages.last())
                                .map(|m| m.id)
                        })
                };
                match found {
                    Some(message) => this.jump_to(inst.clone(), channel, message, window, cx),
                    None => this.info("No messages here yet.", cx),
                }
            },
        );
    }

    /// Back to the newest messages.
    pub(super) fn jump_to_present(
        &mut self,
        key: &ChannelKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let live = self
            .core
            .store()
            .instance(&key.0)
            .and_then(|s| s.window(key.1).map(|w| w.reached_end))
            .unwrap_or(false);
        if live {
            if let Some(view) = self.chats.get(key) {
                crate::smooth_scroll::glide_to_end(view.list.clone(), window, cx);
            }
            cx.notify();
            return;
        }
        if let Some(view) = self.chats.get(key) {
            view.list.scroll_to_end();
        }
        let core = self.core.clone();
        let (inst, channel) = key.clone();
        let key = key.clone();
        self.run(
            cx,
            async move {
                core.load_history(&inst, channel, HistoryAnchor::Latest)
                    .await
            },
            move |this, result, _, cx| {
                if let Err(err) = result {
                    this.error(format!("could not load messages: {err}"), cx);
                }
                this.sync_rows(&key);
                if let Some(view) = this.chats.get(&key) {
                    view.list.scroll_to_end();
                }
            },
        );
    }

    // ---- the message box ----------------------------------------------------------------------
}
