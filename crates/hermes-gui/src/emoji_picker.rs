//! The emoji picker: a search field, a row of group tabs and a grid of emoji, with the ones used
//! last first. It writes an emoji into the message box, or reacts with it.

use std::sync::OnceLock;

use gpui_kit::component::RopeExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, Pixels, Point, SharedString, Subscription, Window, div,
};
use hermes_client_core::proto::id::Id;
use hermes_client_core::{InstanceId, view};

use crate::shell::{ChannelKey, Shell};
use crate::theme::{self, px};

/// Emoji in a row of the grid.
const PER_ROW: usize = 9;
/// One cell of the grid.
const CELL: f32 = 36.;
/// The most emoji kept as "recently used".
const RECENT: usize = 18;
const WIDTH: f32 = PER_ROW as f32 * CELL + 24.;

/// What a chosen emoji is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickFor {
    /// Written into this channel's message box at the cursor.
    Compose(ChannelKey),
    /// A reaction to this message.
    React(InstanceId, Id, Id),
}

pub struct EmojiPicker {
    pub target: PickFor,
    pub from: crate::modal::PopupFrom,
    input: Entity<InputState>,
    /// The tab shown while nothing is searched: 0 = recently used, then the groups.
    tab: usize,
    /// What the grid shows now.
    shown: Vec<SharedString>,
    _events: Subscription,
}

/// Every group's emoji, built once.
fn groups() -> &'static [(&'static str, Vec<&'static str>)] {
    static GROUPS: OnceLock<Vec<(&'static str, Vec<&'static str>)>> = OnceLock::new();
    GROUPS.get_or_init(view::emoji_groups)
}

impl EmojiPicker {
    fn refill(&mut self, query: &str, recent: &[String]) {
        self.shown = if !query.trim().is_empty() {
            view::search_emoji(query, PER_ROW * 12)
                .into_iter()
                .map(|(emoji, _)| emoji.into())
                .collect()
        } else if self.tab == 0 {
            recent.iter().map(|e| e.clone().into()).collect()
        } else {
            groups()
                .get(self.tab - 1)
                .map(|(_, list)| list.iter().map(|e| SharedString::from(*e)).collect())
                .unwrap_or_default()
        };
    }
}

impl Shell {
    /// The emoji used last, newest first.
    fn recent_emoji(&self) -> Vec<String> {
        self.config
            .buffer
            .recent_emoji
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    }

    pub fn open_emoji_picker(
        &mut self,
        target: PickFor,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dismissed_by_this_press(&crate::modal::PopupKey::Emoji(target.clone())) {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find an emoji"));
        let events = cx.subscribe_in(
            &input,
            window,
            |this, input, event, window, cx| match event {
                InputEvent::Change => {
                    let query = input.read(cx).value().to_string();
                    let recent = this.recent_emoji();
                    if let Some(picker) = &mut this.emoji_picker {
                        picker.refill(&query, &recent);
                    }
                    cx.notify();
                }
                // Enter takes the first match.
                InputEvent::PressEnter { .. } => {
                    let first = this
                        .emoji_picker
                        .as_ref()
                        .and_then(|p| p.shown.first().cloned());
                    if let Some(emoji) = first {
                        this.pick_emoji(emoji.to_string(), window, cx);
                    }
                }
                _ => {}
            },
        );
        input.update(cx, |input, cx| input.focus(window, cx));
        let recent = self.recent_emoji();
        let mut picker = EmojiPicker {
            target,
            from: self.popup_from(at),
            input,
            // Nothing used yet: the smileys.
            tab: usize::from(recent.is_empty()),
            shown: Vec::new(),
            _events: events,
        };
        picker.refill("", &recent);
        self.cancel_closing("emoji-picker-in");
        self.emoji_picker = Some(picker);
        cx.notify();
    }

    fn close_emoji_picker(&mut self, cx: &mut Context<Self>) {
        self.close_later("emoji-picker-in", |s| s.emoji_picker = None, cx);
        cx.notify();
    }

    /// Uses `emoji` for what the picker was opened for, notes it as used, and closes.
    fn pick_emoji(&mut self, emoji: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.emoji_picker.as_ref().map(|p| p.target.clone()) else {
            return;
        };
        match target {
            PickFor::Compose(key) => {
                if let Some(view) = self.chats.get(&key) {
                    let input = view.input.clone();
                    input.update(cx, |input, cx| {
                        let text = input.value().to_string();
                        let at = input.cursor().min(text.len());
                        let new = format!("{}{emoji}{}", &text[..at], &text[at..]);
                        input.set_value(new, window, cx);
                        let position = input.text().offset_to_position(at + emoji.len());
                        input.set_cursor_position(position, window, cx);
                        input.focus(window, cx);
                    });
                }
            }
            PickFor::React(inst, channel, message) => {
                if let Some(view) = self.chats.get_mut(&(inst.clone(), channel)) {
                    view.palette = None;
                }
                self.react(inst, channel, message, emoji.clone(), true, cx);
            }
        }
        let mut recent = self.recent_emoji();
        recent.retain(|e| *e != emoji);
        recent.insert(0, emoji);
        recent.truncate(RECENT);
        self.set_config(|c| c.buffer.recent_emoji = recent.join(" "), cx);
        self.close_emoji_picker(cx);
    }

    pub fn render_emoji_picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.emoji_picker.as_ref()?;
        let searching = !picker.input.read(cx).value().trim().is_empty();
        // Tabs: recently used, then each group by its first emoji.
        let tabs = std::iter::once(("Recently used", "🕘"))
            .chain(groups().iter().map(|(name, list)| (*name, list[0])))
            .enumerate()
            .map(|(n, (name, face))| {
                let on = !searching && picker.tab == n;
                div()
                    .id(("emoji-tab", n))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(30.))
                    .rounded(theme::radius_small())
                    .cursor_pointer()
                    .when(on, |d| d.bg(theme::selected()))
                    .when(!on, |d| {
                        d.opacity(0.7).hover(|d| d.bg(theme::hover()).opacity(1.))
                    })
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(name).build(window, cx)
                    })
                    .child(face)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let recent = this.recent_emoji();
                        if let Some(p) = &mut this.emoji_picker {
                            p.tab = n;
                            p.input
                                .update(cx, |input, cx| input.set_value("", window, cx));
                            p.refill("", &recent);
                        }
                        cx.notify();
                    }))
            });
        let shown = picker.shown.clone();
        let rows = shown.len().div_ceil(PER_ROW);
        let grid = if shown.is_empty() {
            div()
                .flex()
                .items_center()
                .justify_center()
                .h(px(CELL * 3.))
                .text_color(theme::text_tertiary())
                .child(if searching {
                    "No emoji by that name"
                } else {
                    "Emoji you use show up here"
                })
                .into_any_element()
        } else {
            gpui_kit::uniform_list(
                "emoji-grid",
                rows,
                cx.processor(
                    move |_this: &mut Shell, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|row| {
                                let cells = shown
                                    .iter()
                                    .enumerate()
                                    .skip(row * PER_ROW)
                                    .take(PER_ROW)
                                    .map(|(n, emoji)| {
                                        let pick = emoji.to_string();
                                        let name = view::emoji_name(emoji).unwrap_or_default();
                                        div()
                                            .id(("emoji", n))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .size(px(CELL))
                                            .rounded(theme::radius_small())
                                            .cursor_pointer()
                                            .text_size(px(22.))
                                            .hover(|d| d.bg(theme::hover()))
                                            .tooltip(move |window, cx| {
                                                gpui_kit::component::tooltip::Tooltip::new(format!(
                                                    ":{name}:"
                                                ))
                                                .build(window, cx)
                                            })
                                            .child(emoji.clone())
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.pick_emoji(pick.clone(), window, cx);
                                            }))
                                    });
                                div().flex().h(px(CELL)).children(cells)
                            })
                            .collect()
                    },
                ),
            )
            .h(px(CELL * (rows.min(8) as f32)))
            .into_any_element()
        };
        Some(crate::modal::popup(
            "emoji-picker-in",
            picker.from,
            WIDTH,
            self.viewport,
            div()
                .id("emoji-picker")
                .occlude()
                .flex()
                .flex_col()
                .gap(px(8.))
                .w(px(WIDTH))
                .p(px(12.))
                .rounded(theme::radius_large())
                .bg(theme::sidebar())
                .border_1()
                .border_color(theme::border())
                .shadow_lg()
                .font_family(theme::font())
                .text_color(theme::text())
                .text_size(theme::secondary_size())
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if let Some(picker) = &this.emoji_picker
                        && !crate::modal::closing("emoji-picker-in")
                    {
                        this.note_dismissed(crate::modal::PopupKey::Emoji(picker.target.clone()));
                    }
                    this.close_emoji_picker(cx);
                }))
                .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.close_emoji_picker(cx);
                    }
                }))
                .child(Input::new(&picker.input))
                .child(div().flex().gap(px(2.)).children(tabs))
                .child(grid),
        ))
    }
}
