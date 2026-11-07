//! The Friends page: message requests and people. Opening any of them opens its private messages
//! in the main panel. The DMs themselves are in the sidebar while Friends is selected.

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, div, px};

use crate::sample::ChatId;
use crate::shell::Shell;
use crate::theme;

fn heading(text: String) -> AnyElement {
    div()
        .mt(px(20.))
        .mb(px(6.))
        .text_size(px(theme::TEXT_TINY))
        .text_color(theme::text_tertiary())
        .child(text)
        .into_any_element()
}

impl Shell {
    pub fn render_friends(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut content: Vec<AnyElement> = Vec::new();

        if !self.data.requests.is_empty() {
            content.push(heading(format!("REQUESTS ({})", self.data.requests.len())));
            for id in &self.data.requests {
                let name = self.data.chat(*id).name.clone();
                let detail = "Wants to message you".to_string();
                content.push(self.person_row("request", *id, name, detail, cx));
            }
        }
        if !self.data.people.is_empty() {
            content.push(heading("PEOPLE".into()));
            for person in &self.data.people {
                let (name, status) = (person.name.clone(), person.status.clone());
                content.push(self.person_row("person", person.dm, name, status, cx));
            }
        }
        if content.is_empty() {
            content.push(
                div()
                    .mt(px(20.))
                    .text_size(px(theme::TEXT))
                    .text_color(theme::text_tertiary())
                    .child("Nobody here yet. Add an instance to get started.")
                    .into_any_element(),
            );
        }

        div()
            .id("friends")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .px(px(20.))
            .pb(px(20.))
            .children(content)
    }

    fn person_row(
        &self,
        section: &str,
        id: ChatId,
        name: String,
        detail: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let unread = self.data.chat(id).unread;
        div()
            .id(SharedString::from(format!("{section}-{}", id.0)))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(12.))
            .h(px(62.))
            .px(px(10.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|d| d.bg(theme::hover()))
            .on_click(cx.listener(move |this, _, window, cx| this.open_chat(id, window, cx)))
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(theme::AVATAR))
                    .rounded(px(theme::AVATAR / 2.))
                    .bg(theme::selected()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_size(px(theme::TEXT)).child(name))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(theme::TEXT_SMALL))
                            .text_color(theme::text_tertiary())
                            .child(detail),
                    ),
            )
            .when(unread > 0, |d| {
                d.child(
                    div()
                        .px(px(7.))
                        .rounded(px(9.))
                        .bg(theme::accent())
                        .text_color(theme::on_accent())
                        .text_size(px(theme::TEXT_TINY))
                        .child(unread.to_string()),
                )
            })
            .into_any_element()
    }
}
