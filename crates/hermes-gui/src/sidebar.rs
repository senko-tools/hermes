//! The icon rail and the sidebar next to it. The rail's top icon is the app icon and opens
//! Friends, with the DMs in the sidebar; the other icons are spaces, each showing its channels.
//! Instances stay out of it unless none is saved or one needs attention; the rest is in the menu
//! of the empty space and in settings.

use gpui_kit::prelude::*;
use gpui_kit::{
    AnimationExt as _, AnyElement, Context, Div, FontWeight, SharedString, Stateful, div,
};
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{ChannelKind, User};
use hermes_client_core::{InstanceId, view};

use crate::assets::{self, icon};
use crate::channel::icon_button;
use crate::collapsed::Node;
use crate::drag::{Ghost, Item, Kind};
use crate::menu_entries::Target;
use crate::shell::{Page, Selected, Shell};
use crate::theme::{self, px};
use crate::tree::{self, Badge, Row};

/// The hover groups of channel and category rows.
const CHANNEL_ROW: &str = "channel-row";
const CATEGORY_ROW: &str = "category-row";

/// A row without a hover look: the caller gives it one (gpui allows one per element).
fn bare_row(id: impl Into<SharedString>, selected: bool, height: f32) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(8.))
        .h(px(height))
        .px(px(8.))
        .rounded(theme::row_radius())
        .text_size(theme::body_size())
        .cursor_pointer()
        .text_color(if selected {
            theme::text()
        } else {
            theme::text_secondary()
        })
}

/// A row with hover and selected looks. The caller adds what a click does.
fn row(id: impl Into<SharedString>, selected: bool, height: f32) -> Stateful<Div> {
    bare_row(id, selected, height)
        .when(selected, |d| d.bg(theme::selected()))
        .when(!selected, |d| d.hover(|d| d.bg(theme::hover())))
}

fn count_badge(count: u32) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .min_w(px(18.))
        .h(px(18.))
        .px(px(4.))
        .rounded(px(9.))
        .bg(theme::danger())
        .text_color(theme::on_accent())
        .text_size(theme::caption_size())
        .child(count.to_string())
}

fn unread_dot() -> Div {
    div()
        .flex_shrink_0()
        .size(px(8.))
        .rounded(px(4.))
        .bg(theme::text())
}

/// A row's unread dot or mention count.
fn badge_element(badge: Badge) -> Option<Div> {
    if badge.mentions > 0 {
        Some(count_badge(badge.mentions))
    } else if badge.unread {
        Some(unread_dot())
    } else {
        None
    }
}

/// How a sidebar row looks in a state: the bar on its left edge and its backgrounds.
#[derive(Clone, Copy)]
struct RowState {
    bar: gpui_kit::Rgba,
    background: gpui_kit::Rgba,
    hover: gpui_kit::Rgba,
}

impl RowState {
    /// `None`: a plain row. The states are in order of what matters most.
    fn of(selected: bool, connected: bool, badge: Badge) -> Option<RowState> {
        if selected {
            Some(RowState {
                bar: theme::row_selected_bar(),
                background: theme::row_selected_bg(),
                hover: theme::row_selected_hover(),
            })
        } else if connected {
            Some(RowState {
                bar: theme::row_connected_bar(),
                background: theme::row_connected_bg(),
                hover: theme::row_connected_hover(),
            })
        } else if badge.mentions > 0 {
            Some(RowState {
                bar: theme::row_mention_bar(),
                background: theme::row_mention_bg(),
                hover: theme::row_mention_hover(),
            })
        } else if badge.unread {
            Some(RowState {
                bar: theme::row_unread_bar(),
                background: theme::row_unread_bg(),
                hover: theme::row_unread_hover(),
            })
        } else {
            None
        }
    }
}

/// An icon of the rail; [`rail_morph`] gives it its shape.
fn rail_icon(id: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id.into())
        .relative()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(theme::RAIL_ICON))
        .cursor_pointer()
        .text_size(theme::body_size())
}

/// Morphs a rail icon from round to a rounded square as it gets selected, and back, while its
/// background fades to the accent. A server's `picture` fills it, in the same shape.
fn rail_morph(
    id: impl Into<gpui_kit::ElementId>,
    element: Stateful<Div>,
    selected: bool,
    picture: Option<std::sync::Arc<gpui_kit::RenderImage>>,
) -> AnyElement {
    use gpui_kit::{AnimationExt as _, SpringAnimation, SpringConfig};
    element
        .with_spring(
            id,
            SpringAnimation::new(SpringConfig::new(380., 30., 1.)).to(selected),
            move |d, phase| {
                let shape = phase.0.clamp(0., 1.2);
                let radius = px(theme::RAIL_ICON / 2. + (14. - theme::RAIL_ICON / 2.) * shape);
                let fade = phase.0.clamp(0., 1.);
                d.rounded(radius)
                    // Under a picture it would show at the rounded edge.
                    .when(picture.is_none(), |d| {
                        d.bg(theme::mix(theme::selected(), theme::accent(), fade))
                    })
                    .children(picture.map(|picture| {
                        let id = SharedString::from(format!(
                            "pic-{:p}",
                            std::sync::Arc::as_ptr(&picture)
                        ));
                        gpui_kit::img(picture)
                            .absolute()
                            .inset_0()
                            .size_full()
                            .rounded(radius)
                            .object_fit(gpui_kit::ObjectFit::Cover)
                            .id(id)
                    }))
            },
        )
        .into_any_element()
}

/// The rail's top padding, and the gap between its icons.
const RAIL_TOP: f32 = 12.;
const RAIL_GAP: f32 = 8.;
/// The separator's height between the home icon and the servers.
const RAIL_SEPARATOR: f32 = 2.;

/// The top of the icon in rail slot `slot` (0 is home, 1 the first server), from the rail's top.
/// `scroll` is how far the servers are scrolled. Between two slots it is in between them.
fn rail_slot_top(slot: f32, scroll: f32) -> f32 {
    let servers = RAIL_TOP + theme::RAIL_ICON + RAIL_GAP + RAIL_SEPARATOR + RAIL_GAP;
    let top = |slot: f32| {
        if slot < 1. {
            RAIL_TOP
        } else {
            servers + (slot - 1.) * (theme::RAIL_ICON + RAIL_GAP) - scroll
        }
    };
    let below = slot.floor();
    top(below) + (top(below + 1.) - top(below)) * (slot - below)
}

/// The bar at the rail's edge beside the selected icon. One bar for the whole rail: it springs
/// from the icon selected before to the new one.
fn rail_pill(slot: Option<f32>, last: f32, scroll: f32) -> AnyElement {
    use gpui_kit::{AnimationExt as _, SpringAnimation, SpringConfig};
    let servers_top = RAIL_TOP + theme::RAIL_ICON + RAIL_GAP + RAIL_SEPARATOR;
    div()
        .absolute()
        .left_0()
        .w(px(4.))
        .h(px(theme::RAIL_ICON - 16.))
        .rounded_r(px(2.))
        .bg(theme::accent())
        .with_spring(
            "rail-pill",
            SpringAnimation::new(SpringConfig::new(300., 28., 1.)).to(slot.unwrap_or(last)),
            move |d, at| {
                let top = rail_slot_top(at, scroll) + 8.;
                // Hidden while nothing is selected, or while its server is scrolled out of view.
                let hidden = slot.is_none() || (at >= 1. && top < servers_top);
                d.top(px(top)).when(hidden, |d| d.opacity(0.))
            },
        )
        .into_any_element()
}

/// A running time as `1:01:42`, or `2:05` under an hour.
fn elapsed_text(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The two letters on a space's rail icon.
fn initials(name: &str) -> String {
    name.chars().take(2).collect()
}

impl Shell {
    /// Every space of every instance, in config order: `(instance, space, name, badge)`.
    fn rail_spaces(&self) -> Vec<(InstanceId, Id, String, Badge)> {
        let store = self.core.store();
        let mut spaces = Vec::new();
        for inst in self.core.instance_ids() {
            let Some(s) = store.instance(&inst) else {
                continue;
            };
            for (id, state) in &s.spaces {
                let mut badge = Badge::default();
                for c in view::channel_tree(s, *id) {
                    badge.add(Badge {
                        unread: c.unread,
                        mentions: c.mentions,
                    });
                }
                spaces.push((inst.clone(), *id, state.space.name.clone(), badge));
            }
        }
        spaces
    }

    /// What the Friends icon's badge shows: mentions in DMs, and a dot for anything else unread.
    fn home_badge(&self) -> Badge {
        let store = self.core.store();
        let mut badge = Badge::default();
        for inst in self.core.instance_ids() {
            let Some(s) = store.instance(&inst) else {
                continue;
            };
            for dm in view::dm_list(s) {
                badge.add(Badge {
                    unread: dm.unread,
                    mentions: dm.mentions,
                });
            }
            badge.unread |= !view::dm_requests(s).is_empty();
            let incoming = s
                .relationships
                .values()
                .filter(|r| r.kind == hermes_client_core::proto::models::RelationshipKind::Incoming)
                .count();
            badge.mentions += incoming as u32;
        }
        badge
    }

    /// The rail stays beside the settings pages: a click on it leaves them for what it opens.
    fn close_pages(&mut self, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            self.close_settings(cx);
        }
        if self.open_space_page.is_some() {
            self.close_space_settings(cx);
        }
    }

    pub fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let on_right = self.config.sidebar.position.eq_ignore_ascii_case("right");
        let home_badge = self.home_badge();
        let home_selected = self.selected == Selected::Home;
        let home = rail_icon("home")
            .when(!home_selected, |d| d.hover(|d| d.bg(theme::hover())))
            .child(icon(
                assets::FEATHER,
                28.,
                if home_selected {
                    theme::on_accent()
                } else {
                    theme::accent()
                },
            ))
            .children(
                badge_element(home_badge)
                    .map(|badge| div().absolute().bottom_0().right_0().child(badge)),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_pages(cx);
                this.open_friends(cx);
            }));
        let home = rail_morph("home-morph", home, home_selected, None);

        let rail_spaces = self.rail_spaces();
        let slot = if home_selected {
            Some(0.)
        } else {
            rail_spaces
                .iter()
                .position(|(inst, id, ..)| self.selected == Selected::Space(inst.clone(), *id))
                .map(|n| (n + 1) as f32)
        };
        if let Some(slot) = slot {
            self.rail_pill_slot.set(slot);
        }
        let scroll = -self.rail_scroll.offset().y.as_f32() / theme::scale();
        let pill = rail_pill(slot, self.rail_pill_slot.get(), scroll);

        let spaces: Vec<AnyElement> = rail_spaces
            .into_iter()
            .map(|(inst, id, name, badge)| {
                let selected = self.selected == Selected::Space(inst.clone(), id);
                let target = Target::Space(inst.clone(), id);
                let inst_key = inst.clone();
                let picture = self.space_picture(&inst, id);
                let element = rail_icon(format!("rail-space-{inst}-{id}"))
                    .text_color(if selected {
                        theme::on_accent()
                    } else {
                        theme::text()
                    })
                    .when(!selected && picture.is_none(), |d| {
                        d.hover(|d| d.bg(theme::hover()))
                    })
                    .when(picture.is_none(), |d| d.child(initials(&name)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_pages(cx);
                        this.select_space(inst.clone(), id, window, cx);
                    }));
                let element = self.with_menu(element, target, cx);
                // The badge goes over the picture, so it comes after it.
                div()
                    .relative()
                    .flex_shrink_0()
                    .child(rail_morph(
                        SharedString::from(format!("rail-morph-{inst_key}-{id}")),
                        element,
                        selected,
                        picture,
                    ))
                    .children(
                        badge_element(badge)
                            .map(|badge| div().absolute().bottom_0().right_0().child(badge)),
                    )
                    .into_any_element()
            })
            .collect();

        let add = rail_icon("rail-add")
            .rounded(px(theme::RAIL_ICON / 2.))
            .bg(theme::selected())
            .hover(|d| d.bg(theme::hover()))
            .child(icon(assets::PLUS, 24., theme::accent()))
            .on_click(
                cx.listener(|this, event: &gpui_kit::ClickEvent, window, cx| {
                    this.open_menu(Target::Instances, event.position(), window, cx);
                }),
            );

        div()
            .relative()
            .overflow_hidden()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .items_center()
            .gap(px(RAIL_GAP))
            .w(px(theme::RAIL_WIDTH))
            .h_full()
            .pt(px(RAIL_TOP))
            .bg(theme::sidebar())
            .when(on_right, |d| d.border_l_1())
            .when(!on_right, |d| d.border_r_1())
            .border_color(theme::border())
            .child(home)
            .child(div().w(px(32.)).h(px(RAIL_SEPARATOR)).bg(theme::border()))
            .child(
                div()
                    .id("rail-spaces")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .items_center()
                    .gap(px(RAIL_GAP))
                    .overflow_y_scroll()
                    .track_scroll(&self.rail_scroll)
                    .children(spaces)
                    .child(add)
                    .child(self.with_instances_menu(
                        div().id("rail-empty").flex_1().min_h(px(24.)).w_full(),
                        cx,
                    )),
            )
            .child(pill)
    }

    pub fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let header = match &self.selected {
            Selected::Space(inst, space) => {
                let store = self.core.store();
                store
                    .instance(inst)
                    .and_then(|s| s.spaces.get(space))
                    .map(|st| (inst.clone(), *space, st.space.name.clone()))
            }
            Selected::Home => None,
        };
        let rows = match self.selected.clone() {
            Selected::Home => self.home_rows(cx),
            Selected::Space(inst, space) => self.space_rows(inst, space, cx),
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::sidebar())
            .children(header.map(|(inst, space, name)| {
                let target = Target::Space(inst, space);
                let open = target.clone();
                let element = div()
                    .id("space-header")
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .h(px(theme::SIDEBAR_HEADER))
                    .px(px(16.))
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(theme::border())
                    .cursor_pointer()
                    .group("space-header")
                    .font_weight(FontWeight::BOLD)
                    .text_size(theme::body_size())
                    .text_color(theme::accent_highlight())
                    .hover(|d| d.text_color(theme::accent()))
                    .child(div().min_w_0().truncate().child(name))
                    .child(
                        icon(assets::CHEVRON_DOWN, 18., theme::accent_highlight())
                            .group_hover("space-header", |s| s.text_color(theme::accent())),
                    )
                    .child(div().flex_1())
                    .on_click(cx.listener(
                        move |this, event: &gpui_kit::ClickEvent, window, cx| {
                            this.open_menu(open.clone(), event.position(), window, cx);
                        },
                    ));
                self.with_menu(element, target, cx)
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
                    .child({
                        // Under a server's channels: making more of them. Under the DMs: the
                        // instances.
                        let empty = div().id("sidebar-empty").flex_1().min_h(px(48.));
                        match &self.selected {
                            Selected::Space(inst, space) => {
                                self.with_menu(empty, Target::ChannelArea(inst.clone(), *space), cx)
                            }
                            Selected::Home => self.with_instances_menu(empty, cx),
                        }
                    })
                    // Another server's channels fade in.
                    .relative()
                    .with_animation(
                        SharedString::from(format!("sidebar-{:?}", self.selected)),
                        gpui_kit::Animation::new(std::time::Duration::from_millis(180))
                            .with_easing(gpui_kit::ease_out_quint()),
                        |d, t| d.opacity(t).top(px(6. * (1. - t))),
                    ),
            )
    }

    /// Under the rail and the sidebar, across both: the voice panel and the user panel.
    pub fn render_bottom_panels(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut panels: Vec<AnyElement> = self.render_voice_panel(cx).into_iter().collect();
        panels.push(self.render_me(cx).into_any_element());
        panels
    }

    /// Instances that need attention: signing in again, or a lost connection.
    fn instance_notices(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        for inst in self.core.instance_ids() {
            let status = self.core.status(&inst);
            let text = match &status {
                ConnStatus::Connected | ConnStatus::Offline => continue,
                ConnStatus::Connecting => format!("{inst}: connecting…"),
                ConnStatus::Reconnecting { attempt } => {
                    format!("{inst}: reconnecting (try {attempt})…")
                }
                ConnStatus::Failed { reason } => format!("{inst}: {reason}"),
                ConnStatus::LoggedOut => format!("{inst}: sign in"),
            };
            let logged_out = status == ConnStatus::LoggedOut;
            let id = SharedString::from(format!("notice-{inst}"));
            let target = Target::Instance(inst.clone());
            let element = row(id, false, theme::ROW_HEIGHT)
                .text_size(theme::secondary_size())
                .text_color(if logged_out {
                    theme::accent()
                } else {
                    theme::text_tertiary()
                })
                .child(icon(assets::SERVER, 16., theme::text_tertiary()))
                .child(div().flex_1().min_w_0().truncate().child(text))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if logged_out {
                        this.sign_in(&inst, window, cx);
                    }
                }));
            rows.push(self.with_menu(element, target, cx).into_any_element());
        }
        rows
    }

    /// Friends is open: the DMs, or the way in when no instance is saved.
    fn home_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let instances = self.core.instance_ids();
        if instances.is_empty() {
            return vec![
                row("add-instance", false, theme::ROW_HEIGHT)
                    .child(icon(assets::PLUS, theme::ROW_ICON, theme::text_secondary()))
                    .child("Add an instance")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_add_instance(String::new(), window, cx);
                    }))
                    .into_any_element(),
            ];
        }
        let mut rows = self.instance_notices(cx);
        rows.push(section_heading("DIRECT MESSAGES"));
        let selected = self.open_page_channel();
        let tree_rows: Vec<Row> = {
            let store = self.core.store();
            let core = &self.core;
            let mut out = Vec::new();
            for inst in &instances {
                if let Some(s) = store.instance(inst) {
                    let here = selected
                        .as_ref()
                        .filter(|(i, _)| i == inst)
                        .map(|(_, c)| *c);
                    tree::dm_rows(s, inst, here, &|i, u| core.is_speaking(i, u), &mut out);
                }
            }
            out
        };
        let many = instances.len() > 1;
        rows.extend(
            tree_rows
                .into_iter()
                .map(|r| self.render_tree_row(r, many, cx)),
        );
        rows
    }

    fn open_page_channel(&self) -> Option<(InstanceId, Id)> {
        match &self.page {
            Page::Chat(inst, channel) | Page::Voice(inst, channel) => {
                Some((inst.clone(), *channel))
            }
            Page::Friends | Page::Results(_) => None,
        }
    }

    fn space_rows(&self, inst: InstanceId, space: Id, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let selected = self
            .open_page_channel()
            .filter(|(i, _)| *i == inst)
            .map(|(_, c)| c);
        let tree_rows: Vec<Row> = {
            let store = self.core.store();
            let core = &self.core;
            let mut out = Vec::new();
            if let Some(s) = store.instance(&inst) {
                tree::space_rows(
                    s,
                    &inst,
                    space,
                    &self.collapsed,
                    selected,
                    &|i, u| core.is_speaking(i, u),
                    &mut out,
                );
            }
            out
        };
        let mut rows = self.instance_notices(cx);
        let mut tree_rows = tree_rows.into_iter().peekable();
        while let Some(tree_row) = tree_rows.next() {
            // The space itself is the header above the list.
            if matches!(tree_row, Row::Space { .. }) {
                continue;
            }
            let block = match &tree_row {
                Row::Channel {
                    kind: ChannelKind::Voice,
                    selected,
                    connected,
                    badge,
                    nested,
                    id,
                    ..
                } => RowState::of(*selected, *connected, *badge).map(|s| (s, *nested, *id)),
                _ => None,
            };
            let Some((state, nested, id)) = block else {
                rows.push(self.render_tree_row(tree_row, false, cx));
                continue;
            };
            // Highlighted: the channel's highlight goes around the people in it too.
            let head = self.render_channel_row(tree_row, true, cx);
            let mut people = Vec::new();
            while tree_rows
                .peek()
                .is_some_and(|r| matches!(r, Row::VoiceUser { .. }))
            {
                let person = tree_rows.next().expect("just looked at it");
                people.push(self.render_tree_row(person, false, cx));
            }
            rows.push(
                div()
                    .id(SharedString::from(format!("voice-block-{id}")))
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .pb(px(if people.is_empty() { 0. } else { 4. }))
                    .when(nested, |d| d.ml(px(10.)))
                    .rounded(theme::row_radius())
                    .overflow_hidden()
                    .bg(state.background)
                    .hover(move |d| d.bg(state.hover))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(theme::indicator_width())
                            .bg(state.bar),
                    )
                    .child(head)
                    .children(people)
                    .into_any_element(),
            );
        }
        rows
    }

    fn render_tree_row(&self, tree_row: Row, many: bool, cx: &mut Context<Self>) -> AnyElement {
        match tree_row {
            // Never reaches here: the header stands for it.
            Row::Space { .. } => div().into_any_element(),
            Row::Category {
                inst,
                id,
                name,
                collapsed,
                badge,
                drag,
            } => {
                let node = Node::Category(inst.clone(), id);
                let target = Target::Channel(inst.clone(), id);
                let add = drag.is_some().then(|| {
                    let inst = inst.clone();
                    div()
                        .id(SharedString::from(format!("category-add-{id}")))
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(20.))
                        .rounded(px(4.))
                        .group("category-add")
                        .child(
                            icon(assets::PLUS, 16., theme::category_text())
                                .group_hover("category-add", |s| s.text_color(theme::accent())),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.run_action(
                                Target::Channel(inst.clone(), id),
                                crate::menu_entries::Action::CreateChannel,
                                window,
                                cx,
                            );
                        }))
                });
                let element = div()
                    .id(SharedString::from(format!("category-{id}")))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(4.))
                    .h(px(28.))
                    .mt(px(10.))
                    .pl(px(4.))
                    .cursor_pointer()
                    .text_size(theme::secondary_size())
                    .text_color(theme::category_text())
                    .group(CATEGORY_ROW)
                    .hover(|d| d.text_color(theme::accent()))
                    .child(div().min_w_0().truncate().child(name.clone()))
                    .child(
                        icon(
                            if collapsed {
                                assets::CHEVRON_RIGHT
                            } else {
                                assets::CHEVRON_DOWN
                            },
                            14.,
                            theme::category_text(),
                        )
                        .group_hover(CATEGORY_ROW, |s| s.text_color(theme::accent())),
                    )
                    .child(div().flex_1())
                    .children(badge_element(badge))
                    .children(add)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Err(err) = this.collapsed.toggle(&node) {
                            this.error(format!("could not save the sidebar state: {err}"), cx);
                        }
                        cx.notify();
                    }));
                let element = self.with_drag(element, drag, name, cx);
                self.with_menu(element, target, cx).into_any_element()
            }
            channel @ Row::Channel { .. } => self.render_channel_row(channel, false, cx),
            Row::VoiceUser {
                inst,
                channel,
                user,
                name,
                muted,
                deafened,
                speaking,
                nested,
                dm,
                ..
            } => self.render_voice_user(
                inst, channel, user, name, muted, deafened, speaking, nested, dm, cx,
            ),
            Row::NewDm { inst } => row(format!("new-dm-{inst}"), false, theme::ROW_HEIGHT)
                .text_size(theme::body_size())
                .child(icon(assets::PLUS, 18., theme::text_tertiary()))
                .child(if many {
                    format!("New message on {inst}")
                } else {
                    "New message".to_owned()
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_new_dm(inst.clone(), window, cx);
                }))
                .into_any_element(),
            Row::Requests { count, .. } => section_heading_owned(format!("REQUESTS ({count})")),
            Row::Request {
                inst,
                id,
                name,
                peer,
                selected,
                ..
            } => {
                let target = Target::Channel(inst.clone(), id);
                let avatar = self.dm_avatar(&inst, peer, &name);
                let (open_inst, accept_inst) = (inst.clone(), inst);
                let element = row(format!("request-{id}"), selected, 44.)
                    .child(avatar)
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .child(
                        icon_button(
                            SharedString::from(format!("request-accept-{id}")),
                            assets::CHECK,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.run_action(
                                    Target::Channel(accept_inst.clone(), id),
                                    crate::menu_entries::Action::AcceptRequest,
                                    window,
                                    cx,
                                );
                            },
                        )),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select(open_inst.clone(), id, window, cx);
                    }));
                self.with_menu(element, target, cx).into_any_element()
            }
            Row::Dm {
                inst,
                id,
                name,
                peer,
                badge,
                selected,
                connected,
                ..
            } => {
                let target = Target::Channel(inst.clone(), id);
                let avatar = self.dm_avatar(&inst, peer, &name);
                let element = row(format!("dm-{id}"), selected, 44.)
                    .when(badge.unread || badge.mentions > 0, |d| {
                        d.text_color(theme::text())
                    })
                    .child(avatar)
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .when(connected, |d| {
                        d.child(icon(assets::PHONE, 16., theme::speaking()))
                    })
                    .children(badge_element(badge))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select(inst.clone(), id, window, cx);
                    }));
                self.with_menu(element, target, cx).into_any_element()
            }
        }
    }

    /// A channel's row. `in_block`: the block around it draws its state.
    fn render_channel_row(
        &self,
        tree_row: Row,
        in_block: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Row::Channel {
            inst,
            id,
            kind,
            name,
            nested,
            badge,
            selected,
            drag,
            muted,
            connected,
        } = tree_row
        else {
            return div().into_any_element();
        };
        let glyph = if kind == ChannelKind::Voice {
            assets::VOLUME
        } else {
            assets::POUND
        };
        let target = Target::Channel(inst.clone(), id);
        let strong = selected || connected || badge.unread || badge.mentions > 0;
        let state = RowState::of(selected, connected, badge).filter(|_| !in_block);
        let timer = connected.then(|| self.time_in_voice(&inst)).flatten();
        let element = bare_row(format!("channel-{id}"), selected, theme::ROW_HEIGHT)
            .relative()
            .overflow_hidden()
            .text_color(theme::channel_text())
            .when(nested && !in_block, |d| d.ml(px(10.)))
            .group(CHANNEL_ROW)
            .when_some(state, |d, state| {
                d.bg(state.background)
                    .hover(move |d| d.bg(state.hover))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(theme::indicator_width())
                            .bg(state.bar),
                    )
            })
            .when(muted, |d| d.text_color(theme::text_tertiary()))
            .when(strong && !muted, |d| {
                d.text_color(theme::text()).font_weight(FontWeight::MEDIUM)
            })
            // Plain rows brighten their text and icon under the pointer.
            .when(state.is_none() && !in_block, |d| {
                d.hover(|d| d.text_color(theme::accent()))
            })
            .child(
                icon(
                    glyph,
                    theme::ROW_ICON,
                    if connected {
                        theme::success()
                    } else {
                        theme::icon_color()
                    },
                )
                .when(!connected, |i| {
                    i.group_hover(CHANNEL_ROW, |s| s.text_color(theme::accent()))
                }),
            )
            .child(div().flex_1().min_w_0().truncate().child(name.clone()))
            .children(timer.map(|text| {
                div()
                    .flex_shrink_0()
                    .text_size(theme::caption_size())
                    .font_weight(FontWeight::NORMAL)
                    .text_color(theme::success())
                    .child(text)
            }))
            .children(badge_element(badge))
            .on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                    this.select(inst.clone(), id, window, cx);
                    let join_on = crate::voice::JoinOn::from_config(&this.config.voice.join_on);
                    let joins =
                        join_on == crate::voice::JoinOn::SingleClick || event.click_count() >= 2;
                    if kind == ChannelKind::Voice && joins {
                        this.join_voice(inst.clone(), id, cx);
                    }
                }),
            );
        let element = self.with_drag(element, drag, name, cx);
        self.with_menu(element, target, cx).into_any_element()
    }

    /// How long I have been in this instance's voice channel.
    fn time_in_voice(&self, inst: &str) -> Option<String> {
        let store = self.core.store();
        let s = store.instance(inst)?;
        let joined = s.voice_states.get(&s.me_id()?)?.joined_at;
        Some(elapsed_text(hermes_client_core::now_ms() - joined))
    }

    fn dm_avatar(&self, inst: &str, peer: Option<Id>, name: &str) -> AnyElement {
        let store = self.core.store();
        let s = store.instance(inst);
        let user = peer.and_then(|p| s.and_then(|s| s.user(p)));
        match (peer, s) {
            (Some(peer), Some(s)) => {
                self.avatar_with_status(inst, user, name, theme::AVATAR_SMALL, s.status(peer))
            }
            _ => self.avatar(inst, None, name, theme::AVATAR_SMALL),
        }
    }

    /// Lets a row be dragged onto another of the same list, when I may move it.
    fn with_drag(
        &self,
        element: Stateful<Div>,
        drag: Option<Item>,
        name: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let Some(item) = drag else {
            return element;
        };
        let target = item.clone();
        let over = item.clone();
        element
            .on_drag(item, move |_, _, _, cx| cx.new(|_| Ghost(name.clone())))
            .drag_over::<Item>(move |style, dragged, _, _| {
                if dragged.can_drop_on(&over) {
                    style.bg(theme::selected())
                } else {
                    style
                }
            })
            .on_drop(cx.listener(move |this, dragged: &Item, _, cx| {
                if dragged.can_drop_on(&target) {
                    this.drop_item(dragged.clone(), target.clone(), cx);
                }
            }))
    }

    /// A row was dropped on another: reorders the channels (the roles' list has its own).
    pub fn drop_item(&mut self, dragged: Item, target: Item, cx: &mut Context<Self>) {
        if dragged.kind != Kind::Channel {
            self.drop_role(dragged, target, cx);
            return;
        }
        let moves = {
            let store = self.core.store();
            store
                .instance(&dragged.inst)
                .map(|s| view::move_channel(s, dragged.space, dragged.id, target.id))
                .unwrap_or_default()
        };
        if moves.is_empty() {
            return;
        }
        let space = dragged.space;
        self.call_status(
            &dragged.inst,
            "Channels reordered",
            move |r| async move { r.reorder_channels(space, &moves).await },
            cx,
        );
    }

    /// My name at the bottom: the status menu, the settings and the voice buttons.
    fn render_me(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let inst = match &self.selected {
            Selected::Space(inst, _) => Some(inst.clone()),
            Selected::Home => self.core.instance_ids().first().cloned(),
        };
        let (name, handle, avatar) = {
            let store = self.core.store();
            let me = inst
                .as_ref()
                .and_then(|i| store.instance(i))
                .and_then(|s| s.me.as_ref().map(|m| m.user.clone()));
            let name = me
                .as_ref()
                .map_or_else(|| "Not signed in".to_owned(), |u| u.name().to_owned());
            let handle = me.as_ref().map(User::handle);
            let presence = crate::shell::presence_status(self.core.presence());
            let avatar = self.avatar_with_status(
                inst.as_deref().unwrap_or(""),
                me.as_ref(),
                &name,
                theme::AVATAR_SMALL,
                presence,
            );
            (name, handle, avatar)
        };
        let target = Target::Me(inst);
        let open = target.clone();
        let me = div()
            .id("me")
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(8.))
            .h(px(40.))
            .px(px(4.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|d| d.bg(theme::hover()))
            .child(avatar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().child(name))
                    .children(handle.map(|handle| {
                        div()
                            .truncate()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(handle)
                    })),
            )
            .on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                    this.open_menu(open.clone(), event.position(), window, cx);
                }),
            );
        let me = self.with_menu(me, target, cx);
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(2.))
            .h(px(theme::FOOTER_HEIGHT))
            .px(px(8.))
            .bg(theme::footer())
            .border_t_1()
            .border_color(theme::border())
            .text_size(theme::body_size())
            .child(me)
            .children(self.render_me_buttons(cx))
    }
}

fn section_heading(text: &'static str) -> AnyElement {
    section_heading_owned(text.to_owned())
}

fn section_heading_owned(text: String) -> AnyElement {
    div()
        .flex_shrink_0()
        .px(px(8.))
        .py(px(6.))
        .text_size(theme::caption_size())
        .text_color(theme::text_tertiary())
        .child(text)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::rail_slot_top;

    #[test]
    fn rail_slots_sit_under_each_other() {
        assert_eq!(rail_slot_top(0., 0.), 12.);
        assert_eq!(rail_slot_top(1., 0.), 78.);
        assert_eq!(rail_slot_top(2., 0.), 134.);
        assert_eq!(rail_slot_top(2., 30.), 104.);
    }

    #[test]
    fn between_two_slots_is_in_between() {
        assert_eq!(rail_slot_top(0.5, 0.), 45.);
        assert_eq!(rail_slot_top(1.5, 0.), 106.);
    }
}
