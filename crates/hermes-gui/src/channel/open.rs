//! Opening a chat: its view, the composer's focus, drafts and read markers.

use super::*;

impl Shell {
    /// The state of a channel's chat, created the first time it opens.
    pub(super) fn chat_view(
        &mut self,
        key: &ChannelKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chats.contains_key(key) {
            return;
        }
        let marker = self
            .core
            .store()
            .instance(&key.0)
            .and_then(|s| s.read_states.get(&key.1).map(|r| r.last_read_id));
        let list = ListState::new(0, ListAlignment::Bottom, px(600.));
        list.set_follow_mode(gpui_kit::FollowMode::Tail);
        let scroll_key = key.clone();
        list.set_scroll_handler(
            cx.listener(move |this, event: &ListScrollEvent, window, cx| {
                // A turn of the wheel going on over the chat passes code blocks by.
                this.code_blocks.chat_scrolled();
                // gpui keeps the list's state mutably borrowed while it calls this handler, so reading
                // the list here panics. Deferred work runs once the event has been dispatched.
                let near_top = event.visible_range.start < LOAD_OLDER_WITHIN;
                let at_end = event.visible_range.end >= event.count;
                let shown_end = event.visible_range.end;
                let key = scroll_key.clone();
                cx.defer_in(window, move |this, _, cx| {
                    if let Some(view) = this.chats.get_mut(&key) {
                        view.shown_end = shown_end;
                    }
                    this.chat_scrolled(&key, near_top, at_end, cx);
                });
            }),
        );
        let placeholder = {
            let store = self.core.store();
            let s = store.instance(&key.0);
            let channel = s.and_then(|s| s.channel(key.1));
            match (s, channel) {
                (Some(s), Some(c)) => {
                    let title = view::channel_title(s, c);
                    if c.kind == hermes_client_core::proto::models::ChannelKind::Dm {
                        format!("Message @{title}")
                    } else {
                        format!("Message {title}")
                    }
                }
                _ => "Message".to_owned(),
            }
        };
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .submit_on_enter(true)
                .placeholder(placeholder)
        });
        let event_key = key.clone();
        let events = cx.subscribe_in(
            &input,
            window,
            move |this, _, event, window, cx| match event {
                InputEvent::Change => this.composer_changed(&event_key, cx),
                InputEvent::PressEnter { shift, secondary } => {
                    this.composer_enter(&event_key, *shift, *secondary, window, cx);
                }
                _ => {}
            },
        );
        self.chats.insert(
            key.clone(),
            ChatView {
                list,
                rows: Vec::new(),
                signatures: Vec::new(),
                read_marker: marker,
                divider_seen: std::rc::Rc::default(),
                loading_older: false,
                loading_newer: false,
                palette: None,
                revealed: HashSet::new(),
                jump: None,
                highlight: None,
                load_failed: false,
                unread_jump_done: false,
                shown_end: 0,
                selected: BTreeSet::new(),
                anchor: None,
                input,
                reply_to: None,
                editing: None,
                uploads: Vec::new(),
                suggestions: Vec::new(),
                suggestion: 0,
                gone: HashMap::new(),
                _input_events: events,
            },
        );
        if let Some(text) = self.core.draft(&key.0, key.1)
            && let Some(view) = self.chats.get(key)
        {
            view.input
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
    }

    /// Shows a channel's chat and loads its latest messages.
    pub fn open_chat(
        &mut self,
        inst: InstanceId,
        channel: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = (inst.clone(), channel);
        if let Some(left) = self.open_channel().filter(|open| *open != key) {
            self.remember_draft(&left, cx);
        }
        // Pictures that failed get another chance.
        self.images
            .thumbs
            .retain(|_, picture| !matches!(picture, Img::Failed));
        self.chat_view(&key, window, cx);
        // Coming back moves the "new messages" line to what arrived since the last visit.
        if self.open_channel().as_ref() != Some(&key) {
            let marker = self
                .core
                .store()
                .instance(&inst)
                .and_then(|s| s.read_states.get(&channel).map(|r| r.last_read_id));
            if let Some(view) = self.chats.get_mut(&key) {
                view.read_marker = marker;
                view.unread_jump_done = false;
                view.divider_seen.set(false);
            }
        }
        self.page = Page::Chat(inst.clone(), channel);
        self.on_chat_opened(&inst, channel, cx);
        self.update_visible_channels();
        self.sync_rows(&key);
        self.focus_composer = true;
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.open_channel(&inst, channel).await },
            move |this, result, _, cx| {
                let failed = result.is_err();
                if let Some(view) = this.chats.get_mut(&key) {
                    view.load_failed = failed;
                }
                if let Err(err) = result {
                    this.error(format!("could not open the channel: {err}"), cx);
                }
                this.sync_rows(&key);
                this.mark_open_read(cx);
                cx.notify();
            },
        );
        cx.notify();
    }

    pub fn focus_open_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.open_channel().and_then(|key| self.chats.get(&key)) {
            view.input.update(cx, |input, cx| input.focus(window, cx));
        }
    }

    // ---- the list -----------------------------------------------------------------------------

    /// The store changed for a channel: brings its rows and list up to date.
    pub fn messages_changed(&mut self, inst: &str, channel: Id, cx: &mut Context<Self>) {
        let key = (inst.to_owned(), channel);
        if self.open_channel().as_ref() == Some(&key) {
            self.follow_read_marker(&key);
        }
        self.sync_rows(&key);
        if self.open_channel().as_ref() == Some(&key) {
            self.mark_open_read(cx);
        }
    }

    /// Moves the open channel's read marker along while I watch (`rows::followed_marker`).
    pub(super) fn follow_read_marker(&mut self, key: &ChannelKey) {
        let newest = {
            let store = self.core.store();
            store
                .instance(&key.0)
                .and_then(|s| s.window(key.1))
                .filter(|w| w.loaded && w.reached_end)
                .and_then(|w| w.messages.last().map(|m| m.id))
        };
        let focused = self.focused;
        if let Some(view) = self.chats.get_mut(key) {
            let watching = focused && view.list.is_scrolled_to_end() != Some(false);
            view.read_marker =
                rows::followed_marker(view.read_marker, &view.rows, watching, newest);
        }
    }

    /// What `content` looks like parsed, from the cache when it was parsed before.
    pub(super) fn cached_blocks(
        &self,
        content: &str,
        s: &hermes_client_core::store::InstanceStore,
        space: Option<Id>,
    ) -> std::rc::Rc<Vec<markup::Block>> {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let formatted = self.config.buffer.markdown;
        (content, space, s.me_id(), formatted).hash(&mut hasher);
        let key = hasher.finish();
        let mut cache = self.markup_cache.borrow_mut();
        if let Some(found) = cache.get(&key) {
            return found.clone();
        }
        if cache.len() >= MARKUP_CACHE_SIZE {
            cache.clear();
        }
        let blocks = std::rc::Rc::new(if formatted {
            markup::blocks(content, s, space)
        } else {
            markup::plain(content)
        });
        cache.insert(key, blocks.clone());
        blocks
    }

    /// Keeps what is written in the composer of `key` for when Hermes comes back.
    pub fn remember_draft(&self, key: &ChannelKey, cx: &Context<Self>) {
        if let Some(view) = self.chats.get(key) {
            let text = view.input.read(cx).value().to_string();
            self.core.set_draft(&key.0, key.1, &text);
        }
    }
}
