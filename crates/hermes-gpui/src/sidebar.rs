//! The icon rail and the sidebar next to it. The rail's top icon is the app icon and opens
//! Friends, with the DMs in the sidebar; the other icons are spaces, each showing its channels.
//! Instances stay out of it unless none is saved; more are added from the menu of the empty space
//! or from settings.

use gpui_kit::component::WindowExt as _;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Context, Div, SharedString, Stateful, Window, div, px};

use crate::assets::{self, icon};
use crate::menu::MenuTarget;
use crate::sample::SpaceItem;
use crate::shell::{Page, Selected, Shell};
use crate::theme;

pub fn open_add_instance(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, _, _| {
        dialog
            .title("Add an instance")
            .child("Placeholder: the real dialog comes with the login work.")
    });
}

fn open_settings(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, _, _| {
        dialog
            .title("Settings")
            .child("Placeholder: instances will be managed here too.")
    });
}

/// A row with hover and selected looks. The caller adds what a click does.
fn row(id: impl Into<SharedString>, selected: bool, height: f32) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(8.))
        .h(px(height))
        .px(px(8.))
        .rounded(px(4.))
        .text_size(px(theme::TEXT))
        .cursor_pointer()
        .text_color(if selected {
            theme::text()
        } else {
            theme::text_secondary()
        })
        .when(selected, |d| d.bg(theme::selected()))
        .when(!selected, |d| d.hover(|d| d.bg(theme::hover())))
}

fn badge(count: u32) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(18.))
        .h(px(18.))
        .px(px(4.))
        .rounded(px(9.))
        .bg(theme::accent())
        .text_color(theme::on_accent())
        .text_size(px(theme::TEXT_TINY))
        .child(count.to_string())
}

/// An icon of the rail: round, and a rounded square while selected, like Discord's.
fn rail_icon(id: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
    div()
        .id(id.into())
        .relative()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(theme::RAIL_ICON))
        .rounded(px(if selected { 14. } else { theme::RAIL_ICON / 2. }))
        .cursor_pointer()
        .text_size(px(theme::TEXT))
}

impl Shell {
    pub fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let unread = self.data.dm_unread();
        let home = rail_icon("home", self.selected == Selected::Home)
            .bg(theme::app_icon_background())
            .child(icon(assets::FEATHER, 28., theme::app_icon_feather()))
            .when(unread > 0, |d| {
                d.child(div().absolute().bottom_0().right_0().child(badge(unread)))
            })
            .on_click(cx.listener(|this, _, _, cx| this.open_friends(cx)));

        let spaces: Vec<AnyElement> = self
            .data
            .spaces
            .iter()
            .enumerate()
            .map(|(s, space)| {
                let selected = self.selected == Selected::Space(s);
                rail_icon(format!("rail-space-{s}"), selected)
                    .bg(if selected {
                        theme::accent()
                    } else {
                        theme::selected()
                    })
                    .text_color(if selected {
                        theme::on_accent()
                    } else {
                        theme::text()
                    })
                    .child(space.initials())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_space(s, window, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .items_center()
            .gap(px(8.))
            .w(px(theme::RAIL_WIDTH))
            .h_full()
            .pt(px(12.))
            .bg(theme::rail())
            .child(home)
            .child(div().w(px(32.)).h(px(2.)).bg(theme::border()))
            .children(spaces)
            .child(self.with_menu(
                div().id("rail-empty").flex_1().w_full(),
                MenuTarget::EmptySpace,
                cx,
            ))
    }

    pub fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let header = match self.selected {
            Selected::Space(s) => Some(self.data.spaces[s].name.clone()),
            Selected::Home => None,
        };
        let rows = match self.selected {
            Selected::Home => self.home_rows(cx),
            Selected::Space(s) => self.space_rows(s, cx),
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::sidebar())
            .children(header.map(|name| {
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .h(px(theme::SIDEBAR_HEADER))
                    .px(px(16.))
                    .border_b_1()
                    .border_color(theme::border())
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_size(px(theme::TEXT))
                    .child(name)
            }))
            .child(
                div()
                    .id("sidebar-rows")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(8.))
                    .pt(px(8.))
                    .gap(px(2.))
                    .children(rows)
                    .child(self.with_menu(
                        div().id("sidebar-empty").flex_1().min_h(px(48.)),
                        MenuTarget::EmptySpace,
                        cx,
                    )),
            )
            .children(self.render_voice_panel(cx))
            .child(self.render_me())
    }

    /// Friends is open: the DMs, or the way in when no instance is saved.
    fn home_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.data.spaces.is_empty() {
            return vec![
                row("add-instance", false, theme::ROW_HEIGHT)
                    .child(icon(assets::PLUS, theme::ROW_ICON, theme::text_secondary()))
                    .child("Add an instance")
                    .on_click(|_, window, cx| open_add_instance(window, cx))
                    .into_any_element(),
            ];
        }
        let mut rows = vec![
            div()
                .flex_shrink_0()
                .px(px(8.))
                .py(px(6.))
                .text_size(px(theme::TEXT_TINY))
                .text_color(theme::text_tertiary())
                .child("DIRECT MESSAGES")
                .into_any_element(),
        ];
        for id in self.data.recent.iter().copied() {
            let chat = self.data.chat(id);
            rows.push(
                row(format!("dm-{}", id.0), self.page == Page::Chat(id), 44.)
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(px(theme::AVATAR_SMALL))
                            .rounded(px(theme::AVATAR_SMALL / 2.))
                            .bg(theme::selected()),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(chat.name.clone()))
                    .when(chat.unread > 0, |d| d.child(badge(chat.unread)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_chat(id, window, cx);
                    }))
                    .into_any_element(),
            );
        }
        rows
    }

    fn space_rows(&self, s: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let items = self.data.spaces[s].items.clone();
        items
            .into_iter()
            .flat_map(|item| self.item_rows(item, cx))
            .collect()
    }

    fn item_rows(&self, item: SpaceItem, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let lead = theme::text_tertiary();
        match item {
            SpaceItem::Text(id) => vec![
                row(
                    format!("chat-{}", id.0),
                    self.page == Page::Chat(id),
                    theme::ROW_HEIGHT,
                )
                .child(icon(assets::POUND, theme::ROW_ICON, lead))
                .child(self.data.chat(id).name.clone())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_chat(id, window, cx);
                }))
                .into_any_element(),
            ],
            SpaceItem::Voice(id) => {
                let room = &self.data.voice[id.0];
                let mut rows = vec![
                    row(
                        format!("voice-{}", id.0),
                        self.page == Page::Voice(id),
                        theme::ROW_HEIGHT,
                    )
                    .child(icon(assets::VOLUME, theme::ROW_ICON, lead))
                    .child(room.name.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.open_voice(id, cx)))
                    .into_any_element(),
                ];
                let me = (self.in_voice == Some(id)).then_some(&self.data.me);
                for person in room.people.iter().chain(me) {
                    rows.push(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(8.))
                            .h(px(30.))
                            .pl(px(36.))
                            .text_size(px(theme::TEXT_SMALL))
                            .text_color(theme::text_secondary())
                            .child(div().size(px(22.)).rounded(px(11.)).bg(theme::selected()))
                            .child(person.clone())
                            .into_any_element(),
                    );
                }
                rows
            }
        }
    }

    fn render_voice_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.in_voice?;
        Some(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(8.))
                .p(px(10.))
                .border_t_1()
                .border_color(theme::border())
                .text_size(px(theme::TEXT_SMALL))
                .child(div().size(px(10.)).rounded(px(5.)).bg(theme::speaking()))
                .child(div().flex_1().child(self.data.voice[id.0].name.clone()))
                .child(
                    div()
                        .id("voice-leave")
                        .cursor_pointer()
                        .child(icon(assets::HANG_UP, theme::ROW_ICON, theme::text()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_voice(id, cx);
                        })),
                )
                .into_any_element(),
        )
    }

    fn render_me(&self) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(8.))
            .h(px(52.))
            .px(px(10.))
            .bg(theme::rail())
            .text_size(px(theme::TEXT_SMALL))
            .child(
                div()
                    .size(px(theme::AVATAR_SMALL))
                    .rounded(px(theme::AVATAR_SMALL / 2.))
                    .bg(theme::selected()),
            )
            .child(div().flex_1().child(self.data.me.clone()))
            .child(
                div()
                    .id("settings")
                    .cursor_pointer()
                    .child(icon(assets::COG, theme::ROW_ICON, theme::text_secondary()))
                    .on_click(|_, window, cx| open_settings(window, cx)),
            )
    }
}
