//! The member list on the right of a space's chat (PLAN.md section 16.1): online members under
//! their highest hoisted role, then other online members, then everyone offline. The channel's
//! topic sits at its top, as the header has no room for text.

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, div};
use hermes_client_core::proto::id::Id;
use hermes_client_core::{InstanceId, view};

use crate::menu_entries::Target;
use crate::shell::Shell;
use crate::theme::{self, px};

impl Shell {
    /// The member list's edge: dragging starts on press, a double-click goes back to the default.
    fn render_member_edge(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("members-edge")
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .w(px(6.))
            .cursor_col_resize()
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
                    if event.click_count >= 2 {
                        this.set_config(|c| c.sidebar.member_list_width = 232, cx);
                        return;
                    }
                    let width = f32::from(this.config.sidebar.member_list_width);
                    this.resizing = Some((event.position.x.as_f32(), width));
                    this.resizing_members = true;
                    cx.notify();
                }),
            )
    }

    pub fn render_members(
        &self,
        inst: &InstanceId,
        channel: Id,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.show_members {
            return None;
        }
        let store = self.core.store();
        let s = store.instance(inst)?;
        let topic = s.channel(channel).and_then(|c| c.topic.clone());
        let mut content: Vec<AnyElement> = Vec::new();
        if let Some(topic) = topic.filter(|t| !t.is_empty()) {
            content.push(
                div()
                    .pb(px(8.))
                    .mb(px(4.))
                    .border_b_1()
                    .border_color(theme::border())
                    .text_size(theme::secondary_size())
                    .text_color(theme::text_secondary())
                    .child(topic)
                    .into_any_element(),
            );
        }
        match s.space_of(channel) {
            Some(space) => {
                let groups = self
                    .member_groups_cache
                    .borrow_mut()
                    .entry((inst.clone(), space))
                    .or_insert_with(|| std::rc::Rc::new(view::member_groups(s, space)))
                    .clone();
                for group in groups.iter().cloned() {
                    content.push(heading(format!(
                        "{} — {}",
                        group.title.to_uppercase(),
                        group.members.len()
                    )));
                    for member in group.members {
                        let user = s.user(member.user_id);
                        let avatar = self.avatar_with_status(
                            inst,
                            user,
                            &member.name,
                            theme::AVATAR_SMALL,
                            member.status,
                        );
                        let color = member.color.map_or(theme::text_secondary(), theme::rgb_u32);
                        content.push(self.member_row(
                            inst,
                            Some(space),
                            member.user_id,
                            member.name,
                            color,
                            avatar,
                            cx,
                        ));
                    }
                }
            }
            None => {
                let recipients = s.channel(channel).map(|c| c.recipients.clone());
                content.push(heading("IN THIS CONVERSATION".to_owned()));
                for user in recipients.into_iter().flatten() {
                    let avatar = self.avatar_with_status(
                        inst,
                        Some(&user),
                        user.name(),
                        theme::AVATAR_SMALL,
                        s.status(user.id),
                    );
                    content.push(self.member_row(
                        inst,
                        None,
                        user.id,
                        user.name().to_owned(),
                        theme::text_secondary(),
                        avatar,
                        cx,
                    ));
                }
            }
        }
        Some(
            div()
                .relative()
                .flex_shrink_0()
                .h_full()
                .child(
                    div()
                        .id("members")
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .h_full()
                        .w(px(f32::from(self.config.sidebar.member_list_width)))
                        .p(px(8.))
                        .gap(px(2.))
                        .overflow_y_scroll()
                        .bg(theme::sidebar())
                        .border_l_1()
                        .border_color(theme::border())
                        .children(content),
                )
                .child(self.render_member_edge(cx))
                .into_any_element(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn member_row(
        &self,
        inst: &InstanceId,
        space: Option<Id>,
        user: Id,
        name: String,
        color: gpui_kit::Rgba,
        avatar: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = Target::Member(inst.clone(), space, user);
        let open = target.clone();
        let element = div()
            .id(SharedString::from(format!("member-{user}")))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(8.))
            .h(px(44.))
            .px(px(8.))
            .rounded(theme::row_radius())
            .cursor_pointer()
            .hover(|d| d.bg(theme::hover()))
            .text_size(theme::body_size())
            .text_color(color)
            .child(avatar)
            .child(div().flex_1().min_w_0().truncate().child(name))
            .on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
                    this.open_profile(&open, event.position(), cx);
                }),
            );
        self.with_menu(element, target, cx).into_any_element()
    }
}

fn heading(text: String) -> AnyElement {
    div()
        .flex_shrink_0()
        .pt(px(12.))
        .pb(px(4.))
        .px(px(8.))
        .text_size(theme::caption_size())
        .text_color(theme::text_tertiary())
        .child(text)
        .into_any_element()
}
