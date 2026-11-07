//! The root view: the icon rail, the sidebar next to it and the main panel, whose page names the
//! window (the native title bar is the only header).

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Entity, Focusable as _, ListAlignment, ListState, MouseButton, Subscription, Window,
    div, px,
};
use hermes_audio::{AudioSettings, Engine, Io, Sound};

use crate::menu::{MenuTarget, OpenMenu};
use crate::sample::{ChatId, Data, VoiceId};
use crate::theme;

/// What the main panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Friends,
    Chat(ChatId),
    Voice(VoiceId),
}

/// The rail's selected icon, which decides what the sidebar lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selected {
    /// The app icon: Friends, with the DMs in the sidebar.
    Home,
    /// A space, by its position: its channels.
    Space(usize),
}

pub struct Shell {
    pub data: Data,
    pub page: Page,
    pub selected: Selected,
    /// The voice channel I am in.
    pub in_voice: Option<VoiceId>,
    pub composer: Entity<TextareaState>,
    /// The message list of the open chat.
    pub messages: ListState,
    /// Where the right button went down, until it comes up.
    pub right_pressed: Option<MenuTarget>,
    pub menu: Option<OpenMenu>,
    /// The title last given to the window.
    title: String,
    /// Plays the UI sounds. It opens the speakers only while a sound plays, and no microphone.
    audio: Engine,
    _composer_events: Subscription,
}

impl Shell {
    pub fn new(data: Data, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .submit_on_enter(true)
                .placeholder("Message")
        });
        let events = cx.subscribe_in(&composer, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = event {
                this.send(window, cx);
            }
        });
        Self {
            data,
            page: Page::Friends,
            selected: Selected::Home,
            in_voice: None,
            composer,
            messages: ListState::new(0, ListAlignment::Bottom, px(400.)),
            right_pressed: None,
            menu: None,
            title: String::new(),
            audio: Engine::start(Io::Devices, AudioSettings::default(), |_| {}),
            _composer_events: events,
        }
    }

    pub fn open_friends(&mut self, cx: &mut Context<Self>) {
        self.selected = Selected::Home;
        self.page = Page::Friends;
        cx.notify();
    }

    /// Selects a space on the rail and opens its first text channel.
    pub fn select_space(&mut self, space: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Selected::Space(space);
        if let Some(id) = self.data.spaces[space].first_chat() {
            self.open_chat(id, window, cx);
        }
        cx.notify();
    }

    pub fn open_voice(&mut self, id: VoiceId, cx: &mut Context<Self>) {
        self.page = Page::Voice(id);
        cx.notify();
    }

    pub fn open_chat(&mut self, id: ChatId, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Chat(id) {
            self.page = Page::Chat(id);
            self.data.mark_read(id);
            self.messages.reset(self.data.chat(id).messages.len());
            self.messages.scroll_to_end();
        }
        self.composer.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Joins the room, or leaves it when already in it.
    pub fn toggle_voice(&mut self, id: VoiceId, cx: &mut Context<Self>) {
        if self.in_voice == Some(id) {
            self.in_voice = None;
            self.audio.play_sound(Sound::Leave);
        } else {
            self.in_voice = Some(id);
            self.audio.play_sound(Sound::Join);
        }
        cx.notify();
    }

    /// Sends what is in the composer to the open chat.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Page::Chat(id) = self.page else { return };
        let text = self.composer.read(cx).value().to_string();
        if !self.data.send(id, &text) {
            return;
        }
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        let added = self.data.chat(id).messages.len() - 1;
        self.messages.splice(added..added, 1);
        self.messages.scroll_to_end();
        cx.notify();
    }

    /// The window's title: the current page.
    fn page_title(&self) -> String {
        match self.page {
            Page::Friends => "Friends".into(),
            Page::Chat(id) => self.data.chat(id).title(),
            Page::Voice(id) => self.data.voice[id.0].name.clone(),
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.page_title();
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }

        let rail = self.render_rail(cx);
        let sidebar = self.render_sidebar(cx);
        let main = match self.page {
            Page::Friends => self.render_friends(cx).into_any_element(),
            Page::Chat(id) => self.render_chat(id, cx).into_any_element(),
            Page::Voice(id) => self.render_voice(id, cx).into_any_element(),
        };
        div()
            .flex()
            .size_full()
            .font_family(theme::FONT)
            .bg(theme::background())
            .text_color(theme::text())
            // After any element has seen the release: it bubbles from them up to here.
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, _, _, _| this.right_pressed = None),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(|this, _, _, _| this.right_pressed = None),
            )
            .child(rail)
            .child(
                div().flex_1().min_w_0().h_full().child(
                    h_resizable("shell")
                        .child(
                            resizable_panel()
                                .size(px(theme::SIDEBAR_WIDTH))
                                .size_range(px(180.)..px(480.))
                                .child(sidebar),
                        )
                        .child(resizable_panel().child(main)),
                ),
            )
            .children(self.render_menu())
    }
}
