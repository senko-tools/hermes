//! The main panel's chat view (channels and DMs) and the voice room placeholder.

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, div, list, px};

use crate::sample::{ChatId, Message, VoiceId};
use crate::shell::Shell;
use crate::theme;

/// The width of the avatar column, which grouped messages leave empty.
const GUTTER: f32 = theme::AVATAR + 16.0;

fn message_row(message: &Message, grouped: bool) -> AnyElement {
    let text = div().text_size(px(theme::TEXT)).child(message.text.clone());
    if grouped {
        return div()
            .pl(px(16. + GUTTER))
            .pr(px(16.))
            .py(px(1.))
            .child(text)
            .into_any_element();
    }
    // The avatar is out of the flow, so this text wraps at the same width as grouped lines.
    div()
        .relative()
        .pt(px(16.))
        .pb(px(1.))
        .pl(px(16. + GUTTER))
        .pr(px(16.))
        .child(
            div()
                .absolute()
                .top(px(16.))
                .left(px(16.))
                .size(px(theme::AVATAR))
                .rounded(px(theme::AVATAR / 2.))
                .bg(theme::selected()),
        )
        .child(
            div()
                .flex()
                .gap(px(8.))
                .items_baseline()
                .child(
                    div()
                        .text_size(px(theme::TEXT))
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .text_color(theme::accent())
                        .child(message.author.clone()),
                )
                .child(
                    div()
                        .text_size(px(theme::TEXT_TINY))
                        .text_color(theme::text_tertiary())
                        .child(message.time()),
                ),
        )
        .child(text)
        .into_any_element()
}

impl Shell {
    pub fn render_chat(&self, id: ChatId, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let messages = list(
            self.messages.clone(),
            cx.processor(move |this, ix: usize, _, _| {
                let chat = this.data.chat(id);
                message_row(&chat.messages[ix], chat.is_grouped(ix))
            }),
        )
        .flex_1()
        .min_h_0();

        div().flex().flex_col().size_full().child(messages).child(
            div()
                .flex_shrink_0()
                .px(px(16.))
                .pt(px(8.))
                .pb(px(20.))
                .child(Textarea::new(&self.composer).large()),
        )
    }

    pub fn render_voice(&self, id: VoiceId, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let room = &self.data.voice[id.0];
        let joined = self.in_voice == Some(id);
        let me = joined.then_some(&self.data.me);
        let people: Vec<String> = room.people.iter().chain(me).cloned().collect();

        let join = Button::new("join-voice")
            .large()
            .label(if joined { "Leave voice" } else { "Join voice" })
            .when(!joined, |b| b.primary())
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_voice(id, cx)));

        div()
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .gap(px(20.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap(px(12.))
                    .children(people.into_iter().map(|person| {
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(220.))
                            .h(px(140.))
                            .rounded(px(8.))
                            .bg(theme::selected())
                            .text_size(px(theme::TEXT))
                            .child(person)
                    })),
            )
            .child(join)
    }
}
