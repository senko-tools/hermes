//! The Friends page: message requests and the people I share a space with (the friend system of
//! its own comes later). Opening any of them opens the private messages in the main panel. The
//! DMs themselves are in the sidebar while Friends is selected.

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, div};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Status;
use hermes_client_core::{InstanceId, view};

use crate::shell::Shell;
use crate::theme::{self, px};

fn heading(text: String) -> AnyElement {
    div()
        .mt(px(20.))
        .mb(px(6.))
        .text_size(theme::caption_size())
        .text_color(theme::text_tertiary())
        .child(text)
        .into_any_element()
}

/// One line of the page: a message request or a person.
struct Line {
    inst: InstanceId,
    id: Id,
    /// `Some`: a request's channel. `None`: a person, whose DM opens or is created.
    channel: Option<Id>,
    name: String,
    detail: String,
    status: Status,
}

impl Shell {
    fn friends_lines(&self) -> (Vec<Line>, Vec<Line>) {
        let store = self.core.store();
        let mut requests = Vec::new();
        let mut people = Vec::new();
        for inst in self.core.instance_ids() {
            let Some(s) = store.instance(&inst) else {
                continue;
            };
            for request in view::dm_requests(s) {
                let peer = s
                    .dms
                    .get(&request.id)
                    .and_then(|c| s.dm_partner(c))
                    .map(|u| u.id);
                requests.push(Line {
                    inst: inst.clone(),
                    id: peer.unwrap_or(request.id),
                    channel: Some(request.id),
                    name: request.name,
                    detail: "Wants to message you".into(),
                    status: peer.map_or(Status::Offline, |p| s.status(p)),
                });
            }
            let me = s.me_id();
            let mut known: Vec<Line> = s
                .spaces
                .values()
                .flat_map(|sp| sp.members.keys())
                .copied()
                .collect::<std::collections::BTreeSet<Id>>()
                .into_iter()
                .filter(|id| Some(*id) != me)
                .filter_map(|id| {
                    let user = s.user(id)?;
                    let status = s.status(id);
                    let detail = user.status_text.clone().unwrap_or_else(|| match status {
                        Status::Online => "Online".to_owned(),
                        Status::Idle => "Idle".to_owned(),
                        Status::Dnd => "Do not disturb".to_owned(),
                        Status::Offline => "Offline".to_owned(),
                    });
                    Some(Line {
                        inst: inst.clone(),
                        id,
                        channel: None,
                        name: user.name().to_owned(),
                        detail,
                        status,
                    })
                })
                .collect();
            people.append(&mut known);
        }
        people.sort_by_key(|p| (p.status == Status::Offline, p.name.to_lowercase()));
        (requests, people)
    }

    pub fn render_friends(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (requests, people) = self.friends_lines();
        let mut content: Vec<AnyElement> = Vec::new();
        if !requests.is_empty() {
            content.push(heading(format!("REQUESTS ({})", requests.len())));
            for line in requests {
                content.push(self.person_row("request", line, cx));
            }
        }
        if !people.is_empty() {
            content.push(heading(format!("PEOPLE ({})", people.len())));
            for line in people {
                content.push(self.person_row("person", line, cx));
            }
        }
        if content.is_empty() {
            content.push(
                div()
                    .mt(px(20.))
                    .text_size(theme::body_size())
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

    fn person_row(&self, section: &str, line: Line, cx: &mut Context<Self>) -> AnyElement {
        let Line {
            inst,
            id,
            channel,
            name,
            detail,
            status,
        } = line;
        let avatar = {
            let store = self.core.store();
            let user = store.instance(&inst).and_then(|s| s.user(id));
            self.avatar_with_status(&inst, user, &name, theme::AVATAR, status)
        };
        let element_id = SharedString::from(format!("{section}-{inst}-{id}"));
        let target = crate::menu_entries::Target::Member(inst.clone(), None, id);
        let element = div()
            .id(element_id)
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(12.))
            .h(px(62.))
            .px(px(10.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|d| d.bg(theme::hover()))
            .on_click(cx.listener(move |this, _, window, cx| match channel {
                Some(channel) => this.select(inst.clone(), channel, window, cx),
                None => this.message_user(inst.clone(), id, window, cx),
            }))
            .child(avatar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_size(theme::body_size()).child(name))
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::secondary_size())
                            .text_color(theme::text_tertiary())
                            .child(detail),
                    ),
            );
        self.with_menu(element, target, cx).into_any_element()
    }
}
