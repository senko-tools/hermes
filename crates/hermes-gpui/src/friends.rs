//! The Friends page: friends, friend requests, message requests and the people I share a space
//! with, under tabs. Opening any of them opens the private messages in the main panel. The DMs
//! themselves are in the sidebar while Friends is selected.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, SharedString, div};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{RelationshipKind, Status};
use hermes_client_core::{InstanceId, view};

use crate::assets;
use crate::channel::icon_button;
use crate::menu_entries::{Action, Target};
use crate::shell::Shell;
use crate::theme::{self, px};
use crate::voice::chip;

/// What the Friends page lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FriendsTab {
    #[default]
    Online,
    All,
    Pending,
    /// People from my spaces, friends or not.
    People,
}

impl FriendsTab {
    const ALL: [FriendsTab; 4] = [
        FriendsTab::Online,
        FriendsTab::All,
        FriendsTab::Pending,
        FriendsTab::People,
    ];

    fn label(self) -> &'static str {
        match self {
            FriendsTab::Online => "Online",
            FriendsTab::All => "All",
            FriendsTab::Pending => "Pending",
            FriendsTab::People => "People",
        }
    }
}

fn heading(text: String) -> AnyElement {
    div()
        .mt(px(20.))
        .mb(px(6.))
        .text_size(theme::caption_size())
        .text_color(theme::text_tertiary())
        .child(text)
        .into_any_element()
}

/// What a line of the page stands for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    /// A message request: its channel opens.
    MessageRequest(Id),
    Friend,
    Incoming,
    Outgoing,
    /// Someone from a space.
    Person,
}

/// One line of the page.
struct Line {
    inst: InstanceId,
    id: Id,
    kind: LineKind,
    name: String,
    handle: String,
    detail: String,
    status: Status,
}

fn status_text(status: Status) -> &'static str {
    match status {
        Status::Online => "Online",
        Status::Idle => "Idle",
        Status::Dnd => "Do not disturb",
        Status::Offline => "Offline",
    }
}

impl Shell {
    fn friends_lines(&self) -> Vec<Line> {
        let store = self.core.store();
        let mut lines = Vec::new();
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
                lines.push(Line {
                    inst: inst.clone(),
                    id: peer.unwrap_or(request.id),
                    kind: LineKind::MessageRequest(request.id),
                    name: request.name,
                    handle: String::new(),
                    detail: "Wants to message you".into(),
                    status: peer.map_or(Status::Offline, |p| s.status(p)),
                });
            }
            for r in s.relationships.values() {
                let (kind, pending) = match r.kind {
                    RelationshipKind::Friend => (LineKind::Friend, None),
                    RelationshipKind::Incoming => {
                        (LineKind::Incoming, Some("Wants to be your friend"))
                    }
                    RelationshipKind::Outgoing => (LineKind::Outgoing, Some("Request sent")),
                };
                let status = s.status(r.user.id);
                let detail = pending.map_or_else(
                    || {
                        r.user
                            .status_text
                            .clone()
                            .unwrap_or_else(|| status_text(status).to_owned())
                    },
                    str::to_owned,
                );
                lines.push(Line {
                    inst: inst.clone(),
                    id: r.user.id,
                    kind,
                    name: r.user.name().to_owned(),
                    handle: r.user.handle(),
                    detail,
                    status,
                });
            }
            let me = s.me_id();
            let known: std::collections::BTreeSet<Id> = s
                .spaces
                .values()
                .flat_map(|sp| sp.members.keys())
                .copied()
                .collect();
            for id in known {
                if Some(id) == me {
                    continue;
                }
                let Some(user) = s.user(id) else {
                    continue;
                };
                let status = s.status(id);
                lines.push(Line {
                    inst: inst.clone(),
                    id,
                    kind: LineKind::Person,
                    name: user.name().to_owned(),
                    handle: user.handle(),
                    detail: user
                        .status_text
                        .clone()
                        .unwrap_or_else(|| status_text(status).to_owned()),
                    status,
                });
            }
        }
        lines.sort_by_key(|l| (l.status == Status::Offline, l.name.to_lowercase()));
        lines
    }

    pub fn render_friends(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let lines = self.friends_lines();
        let pending_count = lines
            .iter()
            .filter(|l| {
                matches!(
                    l.kind,
                    LineKind::Incoming | LineKind::Outgoing | LineKind::MessageRequest(_)
                )
            })
            .count();
        let tab = self.friends_tab;
        let tabs: Vec<_> = FriendsTab::ALL
            .into_iter()
            .enumerate()
            .map(|(n, t)| {
                let label = if t == FriendsTab::Pending && pending_count > 0 {
                    format!("{} ({pending_count})", t.label())
                } else {
                    t.label().to_owned()
                };
                chip(("friends-tab", n), label, t == tab).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.friends_tab = t;
                        cx.notify();
                    },
                ))
            })
            .collect();
        let shown: Vec<Line> = lines
            .into_iter()
            .filter(|l| match tab {
                FriendsTab::Online => l.kind == LineKind::Friend && l.status != Status::Offline,
                FriendsTab::All => l.kind == LineKind::Friend,
                FriendsTab::Pending => matches!(
                    l.kind,
                    LineKind::Incoming | LineKind::Outgoing | LineKind::MessageRequest(_)
                ),
                FriendsTab::People => l.kind == LineKind::Person,
            })
            .collect();
        let mut content: Vec<AnyElement> = Vec::new();
        if shown.is_empty() {
            content.push(
                div()
                    .mt(px(20.))
                    .text_size(theme::body_size())
                    .text_color(theme::text_tertiary())
                    .child(match tab {
                        FriendsTab::Online => "No friends online right now.",
                        FriendsTab::All => {
                            "No friends yet. Add someone with the button above: you need their name and # number."
                        }
                        FriendsTab::Pending => "No requests waiting.",
                        FriendsTab::People => "Nobody here yet. Add an instance to get started.",
                    })
                    .into_any_element(),
            );
        } else {
            content.push(heading(format!(
                "{} — {}",
                tab.label().to_uppercase(),
                shown.len()
            )));
            for line in shown {
                content.push(self.person_row(line, cx));
            }
        }
        div()
            .id("friends")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .px(px(20.))
            .pb(px(20.))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .mt(px(14.))
                    .child(
                        div()
                            .mr(px(8.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Friends"),
                    )
                    .children(tabs)
                    .child(div().flex_1())
                    .child(
                        Button::new("add-friend")
                            .cursor_pointer()
                            .primary()
                            .label("Add friend")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_form_add_friend(window, cx);
                            })),
                    ),
            )
            .children(content)
    }

    fn person_row(&self, line: Line, cx: &mut Context<Self>) -> AnyElement {
        let Line {
            inst,
            id,
            kind,
            name,
            handle,
            detail,
            status,
        } = line;
        let avatar = {
            let store = self.core.store();
            let user = store.instance(&inst).and_then(|s| s.user(id));
            self.avatar_with_status(&inst, user, &name, theme::AVATAR, status)
        };
        let element_id = SharedString::from(format!("person-{inst}-{id}-{}", kind_key(kind)));
        let target = Target::Member(inst.clone(), None, id);
        let mut buttons: Vec<AnyElement> = Vec::new();
        let action_button =
            |key: &str, glyph: &'static str, action: Action, cx: &mut Context<Self>| {
                let inst = inst.clone();
                icon_button(SharedString::from(format!("friend-{key}-{id}")), glyph)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.run_action(Target::Member(inst.clone(), None, id), action, window, cx);
                    }))
                    .into_any_element()
            };
        match kind {
            LineKind::Incoming => {
                buttons.push(action_button(
                    "accept",
                    assets::CHECK,
                    Action::AcceptFriend,
                    cx,
                ));
                buttons.push(action_button(
                    "decline",
                    assets::CLOSE,
                    Action::DeclineFriend,
                    cx,
                ));
            }
            LineKind::Outgoing => {
                buttons.push(action_button(
                    "cancel",
                    assets::CLOSE,
                    Action::CancelFriendRequest,
                    cx,
                ));
            }
            _ => {}
        }
        let open_inst = inst.clone();
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
            .on_click(cx.listener(move |this, _, window, cx| match kind {
                LineKind::MessageRequest(channel) => {
                    this.select(open_inst.clone(), channel, window, cx);
                }
                _ => this.message_user(open_inst.clone(), id, window, cx),
            }))
            .child(avatar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .items_baseline()
                            .child(div().text_size(theme::body_size()).child(name))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(theme::caption_size())
                                    .text_color(theme::text_tertiary())
                                    .child(handle),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::secondary_size())
                            .text_color(theme::text_tertiary())
                            .child(detail),
                    ),
            )
            .children(buttons);
        self.with_menu(element, target, cx).into_any_element()
    }
}

fn kind_key(kind: LineKind) -> &'static str {
    match kind {
        LineKind::MessageRequest(_) => "request",
        LineKind::Friend => "friend",
        LineKind::Incoming => "incoming",
        LineKind::Outgoing => "outgoing",
        LineKind::Person => "person",
    }
}
