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
use crate::collapsed::Node;
use crate::drag::{Ghost, Item, Kind};
use crate::menu_entries::Target;
use crate::shell::{Page, Selected, Shell};
use crate::theme::{self, px};
use crate::tree::{self, Badge, Row};

/// A lost connection shows as reconnecting only after this long: resuming takes a moment.
pub const RECONNECTING_AFTER: std::time::Duration = std::time::Duration::from_secs(3);
/// The chat's "reconnecting" banner waits this long.
pub const BANNER_AFTER: std::time::Duration = std::time::Duration::from_secs(15);

/// What the connection icon in the user panel shows, over all instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnLook {
    Connected,
    /// Lost for at least [`RECONNECTING_AFTER`] and trying again.
    Reconnecting,
    /// Needs the person (sign in again, update, check the address).
    Disconnected,
    /// Every instance is offline on request.
    Offline,
}

impl ConnLook {
    /// From each instance's status and how long it has been down.
    pub fn of(states: &[(ConnStatus, Option<std::time::Duration>)]) -> ConnLook {
        if states.is_empty() {
            return ConnLook::Offline;
        }
        let mut look = ConnLook::Connected;
        for (status, down) in states {
            match status {
                ConnStatus::Failed { .. } | ConnStatus::LoggedOut => {
                    return ConnLook::Disconnected;
                }
                ConnStatus::Connecting | ConnStatus::Reconnecting { .. }
                    if down.is_some_and(|d| d >= RECONNECTING_AFTER) =>
                {
                    look = ConnLook::Reconnecting;
                }
                _ => {}
            }
        }
        if states.iter().all(|(s, _)| *s == ConnStatus::Offline) {
            return ConnLook::Offline;
        }
        look
    }

    fn icon(self) -> &'static str {
        match self {
            ConnLook::Connected => assets::LAN_CONNECT,
            ConnLook::Reconnecting => assets::LAN_PENDING,
            ConnLook::Disconnected | ConnLook::Offline => assets::LAN_DISCONNECT,
        }
    }

    fn color(self) -> gpui_kit::Rgba {
        match self {
            // Quiet: it is the normal case.
            ConnLook::Connected => gpui_kit::Rgba {
                a: 0.55,
                ..theme::success()
            },
            ConnLook::Reconnecting => theme::warning(),
            ConnLook::Disconnected => theme::danger(),
            ConnLook::Offline => theme::text_tertiary(),
        }
    }
}

/// One instance's line in the connection icon's tooltip.
fn status_line(inst: &str, status: &ConnStatus) -> String {
    match status {
        ConnStatus::Connected => format!("{inst}: connected"),
        ConnStatus::Connecting => format!("{inst}: connecting…"),
        ConnStatus::Reconnecting { attempt } => format!("{inst}: reconnecting (try {attempt})…"),
        ConnStatus::Failed { reason } => format!("{inst}: {reason}"),
        ConnStatus::LoggedOut => format!("{inst}: signed out"),
        ConnStatus::Offline => format!("{inst}: offline"),
    }
}

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

/// A server icon's tooltip: its name, and how many are in voice there.
fn rail_tooltip(name: &str, in_voice: usize) -> String {
    match in_voice {
        0 => name.to_owned(),
        n => format!("{name} - {n} in voice"),
    }
}

/// The voice chip's size.
const VOICE_CHIP: f32 = 20.;
/// It pops in with a little overshoot when someone joins an empty voice channel.
const CHIP_SPRING: gpui_kit::SpringConfig = gpui_kit::SpringConfig::new(520., 22., 1.);

/// A round chip with a speaker in a rail icon's top-right corner while people are in voice there
/// (the tooltip says how many). Accent when I am one of them; on a selected icon, which is accent
/// itself, only the speaker is. A ring in the rail's colour sets it off from the picture. It sits
/// inside the icon, so the rail's edge never cuts it.
fn voice_chip(id: SharedString, here: view::VoiceHere, selected: bool) -> impl IntoElement {
    use gpui_kit::{AnimationExt as _, SpringAnimation};
    let (bg, fg) = match (here.me, selected) {
        (true, false) => (theme::accent(), theme::on_accent()),
        (true, true) => (theme::raised(), theme::accent()),
        (false, _) => (theme::raised(), theme::text_secondary()),
    };
    // A fixed box, so the chip grows from its middle.
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(VOICE_CHIP))
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .border_2()
                .border_color(theme::sidebar())
                .bg(bg)
                .child(icon(assets::VOLUME, 11., fg))
                .with_spring(
                    id,
                    SpringAnimation::new(CHIP_SPRING).to(true).from(false),
                    |d, phase| {
                        let size = VOICE_CHIP * (0.6 + 0.4 * phase.0.max(0.));
                        d.size(px(size))
                            .rounded(px(size / 2.))
                            .opacity(phase.0.clamp(0., 1.))
                    },
                ),
        )
}

fn unread_dot() -> Div {
    div()
        .flex_shrink_0()
        .size(px(8.))
        .rounded(px(4.))
        .bg(theme::text())
}

/// A row's unread dot or mention count.
fn badge_element(badge: Badge, counts: bool, dots: bool) -> Option<Div> {
    if badge.mentions > 0 && counts {
        Some(count_badge(badge.mentions))
    } else if (badge.unread || badge.mentions > 0) && dots {
        Some(unread_dot())
    } else {
        None
    }
}

/// The key a badge's pop plays under: a new one when it appears or its count changes.
fn badge_key(id: &str, badge: Badge, counts: bool) -> String {
    if badge.mentions > 0 && counts {
        format!("badge-{id}-{}", badge.mentions)
    } else {
        format!("badge-{id}-dot")
    }
}

impl Shell {
    /// A row's badge, as the settings want it shown. It pops in on a spring when it appears and
    /// again when its count changes; `id` names the row.
    fn badge(&self, id: &str, badge: Badge) -> Option<AnyElement> {
        use gpui_kit::{AnimationExt as _, SpringAnimation};
        let sidebar = &self.config.sidebar;
        let counts = sidebar.show_mention_counts;
        badge_element(badge, counts, sidebar.show_unread_dots)?;
        let key = badge_key(id, badge, counts);
        let mentions = (badge.mentions > 0 && counts).then_some(badge.mentions);
        // A box as tall as the largest badge, so the row never moves while one grows in it.
        Some(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .h(px(18.))
                .with_spring(
                    SharedString::from(key),
                    SpringAnimation::new(CHIP_SPRING).to(true).from(false),
                    move |d, phase| {
                        let pop = 0.5 + 0.5 * phase.0.max(0.);
                        let badge = match mentions {
                            Some(count) => count_badge(count)
                                .h(px(18. * pop))
                                .min_w(px(18. * pop))
                                .rounded(px(9. * pop)),
                            None => unread_dot().size(px(8. * pop)).rounded(px(4. * pop)),
                        };
                        d.min_w(px(if mentions.is_some() { 18. } else { 8. }))
                            .child(badge.opacity(phase.0.clamp(0., 1.)))
                    },
                )
                .into_any_element(),
        )
    }
}

/// How a sidebar row looks in a state: the bar on its left edge and its backgrounds.
#[derive(Clone, Copy, Debug)]
pub struct RowState {
    /// Which state: the colours crossfade when it changes.
    name: &'static str,
    bar: gpui_kit::Rgba,
    background: gpui_kit::Rgba,
    hover: gpui_kit::Rgba,
}

impl RowState {
    /// `None`: a plain row. The states are in order of what matters most.
    fn of(selected: bool, connected: bool, badge: Badge) -> Option<RowState> {
        if selected {
            Some(RowState {
                name: "selected",
                bar: theme::row_selected_bar(),
                background: theme::row_selected_bg(),
                hover: theme::row_selected_hover(),
            })
        } else if connected {
            Some(RowState {
                name: "connected",
                bar: theme::row_connected_bar(),
                background: theme::row_connected_bg(),
                hover: theme::row_connected_hover(),
            })
        } else if badge.mentions > 0 {
            Some(RowState {
                name: "mention",
                bar: theme::row_mention_bar(),
                background: theme::row_mention_bg(),
                hover: theme::row_mention_hover(),
            })
        } else if badge.unread {
            Some(RowState {
                name: "unread",
                bar: theme::row_unread_bar(),
                background: theme::row_unread_bg(),
                hover: theme::row_unread_hover(),
            })
        } else {
            None
        }
    }
}

impl Shell {
    /// The state a highlighted row's colours crossfade from (itself when nothing changed), and
    /// notes the one it shows now. A row that isn't highlighted forgets it.
    fn tint_from(&self, row: &str, state: Option<RowState>) -> Option<RowState> {
        let mut tints = self.row_tints.borrow_mut();
        let Some(state) = state else {
            tints.remove(row);
            return None;
        };
        let entry = tints.entry(row.to_owned()).or_insert((state, state));
        if entry.1.name != state.name {
            *entry = (entry.1, state);
        }
        Some(entry.0)
    }

    /// How far highlighted rows reach left past the list's padding: to the rail's border line
    /// when the rail is on their left.
    fn row_reach(&self) -> f32 {
        if self.config.sidebar.position.eq_ignore_ascii_case("right") {
            0.
        } else {
            LIST_PAD
        }
    }
}

/// Key of the crossfade from `from` to `to`: a new one each time the state changes.
fn tint_key(row: &str, part: &str, from: RowState, to: RowState) -> SharedString {
    SharedString::from(format!("{row}-{part}-{}-{}", from.name, to.name))
}

/// The bar on a highlighted row's left edge. It grows out from its middle when the row lights
/// up; going from one highlighted state to another only crossfades its colour from `from`.
fn state_bar(row: &str, state: RowState, from: RowState) -> impl IntoElement {
    use gpui_kit::{Animation, AnimationExt as _, ease_out_quint, relative};
    div()
        .absolute()
        .left_0()
        .w(theme::indicator_width())
        .child(div().size_full().with_animation(
            tint_key(row, "bar-tint", from, state),
            Animation::new(theme::FADE * 2).with_easing(ease_out_quint()),
            move |d, t| d.bg(theme::mix(from.bar, state.bar, t)),
        ))
        .with_animation(
            SharedString::from(format!("{row}-bar")),
            Animation::new(theme::FADE * 2).with_easing(ease_out_quint()),
            |d, t| {
                let inset = relative(0.5 * (1. - t));
                d.top(inset).bottom(inset)
            },
        )
}

/// A highlighted row's background, under its content. It fades in when the row lights up and
/// crossfades from `from` when the state changes; `hover`: it shows the state's hover colour
/// under the pointer.
fn state_fill(row: &str, state: RowState, from: RowState, hover: bool) -> impl IntoElement {
    use gpui_kit::{Animation, AnimationExt as _, ease_out_quint};
    div()
        .absolute()
        .inset_0()
        .child(
            div()
                .size_full()
                .when(hover, |d| d.hover(move |d| d.bg(state.hover)))
                .with_animation(
                    tint_key(row, "fill-tint", from, state),
                    Animation::new(theme::FADE * 2).with_easing(ease_out_quint()),
                    move |d, t| d.bg(theme::mix(from.background, state.background, t)),
                ),
        )
        .with_animation(
            SharedString::from(format!("{row}-fill")),
            Animation::new(theme::FADE * 2).with_easing(ease_out_quint()),
            |d, t| d.opacity(t),
        )
}

/// How far a highlighted row reaches left, into the list's padding, so its bar sits on the
/// rail's border line. `0` with the sidebar on the right, where the rail is on the other side.
const LIST_PAD: f32 = 8.;

/// A highlighted row or block reaching `reach` left past the list's padding: its left corners
/// go square against the border, and `indent` plus `reach` move into its padding so its content
/// stays where it was.
fn reach_left<E: Styled>(element: E, reach: f32, indent: f32, pad: f32) -> E {
    if reach <= 0. {
        return if indent > 0. {
            element.ml(px(indent))
        } else {
            element
        };
    }
    element
        .ml(px(-reach))
        .pl(px(pad + reach + indent))
        .rounded_tl(px(0.))
        .rounded_bl(px(0.))
}

/// Between two rows of a category that was just opened.
const STAGGER: std::time::Duration = std::time::Duration::from_millis(18);

/// The `n`th row of a category that was just opened: it fades in and drops into place a moment
/// after the one above it.
fn stagger_in(element: AnyElement, key: &str, n: u32) -> AnyElement {
    use gpui_kit::{Animation, AnimationExt as _};
    let delay = STAGGER * n;
    let total = theme::FADE * 2 + delay;
    let ease = gpui_kit::ease_out_quint();
    div()
        .relative()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .child(element)
        .with_animation(
            SharedString::from(format!("{key}-{n}")),
            Animation::new(total),
            move |d, t| {
                let elapsed = total.as_secs_f32() * t - delay.as_secs_f32();
                let local = ease((elapsed / (theme::FADE * 2).as_secs_f32()).clamp(0., 1.));
                d.opacity(local).top(px(-4. * (1. - local)))
            },
        )
        .into_any_element()
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

/// Morphs a rail icon from round to a rounded square as it gets selected or hovered, and back,
/// while its background fades to the accent as it gets selected. A server's `picture` fills it,
/// in the same shape.
fn rail_morph(
    id: &str,
    element: Stateful<Div>,
    selected: bool,
    hovered: bool,
    picture: Option<std::sync::Arc<gpui_kit::RenderImage>>,
) -> AnyElement {
    use gpui_kit::{AnimationExt as _, SpringAnimation, SpringConfig};
    let plain = picture.is_none();
    element
        .with_spring(
            SharedString::from(format!("{id}-shape")),
            SpringAnimation::new(SpringConfig::new(380., 30., 1.)).to(selected || hovered),
            move |d, phase| {
                let shape = phase.0.clamp(0., 1.2);
                let radius = px(theme::RAIL_ICON / 2. + (14. - theme::RAIL_ICON / 2.) * shape);
                d.rounded(radius).children(picture.map(|picture| {
                    let id =
                        SharedString::from(format!("pic-{:p}", std::sync::Arc::as_ptr(&picture)));
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
        // Under a picture the background would show at the rounded edge.
        .with_spring(
            SharedString::from(format!("{id}-fade")),
            SpringAnimation::new(SpringConfig::new(380., 30., 1.)).to(selected),
            move |d, phase| {
                let fade = phase.0.clamp(0., 1.);
                d.map_element(|e| {
                    e.when(plain, |e| {
                        e.bg(theme::mix(theme::selected(), theme::accent(), fade))
                    })
                })
            },
        )
        .into_any_element()
}

/// How tall an icon's own pill at the rail's edge is: a dot while unread, longer under the
/// pointer, none while selected (the rail's one pill is there).
fn rail_mark_height(selected: bool, hovered: bool, unread: bool) -> f32 {
    if selected {
        0.
    } else if hovered {
        20.
    } else if unread {
        8.
    } else {
        0.
    }
}

/// An icon's own pill at the rail's edge, springing between the heights above.
fn rail_mark(id: &str, height: f32) -> impl IntoElement {
    use gpui_kit::{AnimationExt as _, SpringAnimation, SpringConfig};
    div()
        .absolute()
        .left(px(-(theme::RAIL_WIDTH - theme::RAIL_ICON) / 2.))
        .w(px(4.))
        .rounded_r(px(2.))
        .bg(theme::text())
        .with_spring(
            SharedString::from(format!("{id}-mark")),
            SpringAnimation::new(SpringConfig::new(420., 30., 1.)).to(height),
            |d, h| {
                let h = h.max(0.);
                d.h(px(h))
                    .top(px((theme::RAIL_ICON - h) / 2.))
                    .opacity((h / 4.).min(1.))
            },
        )
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
        // Those without a place of their own keep the server's order, after the others.
        let order = &self.config.sidebar.server_order;
        spaces.sort_by_key(|(inst, id, ..)| {
            let key = format!("{inst}/{id}");
            order.iter().position(|e| *e == key).unwrap_or(usize::MAX)
        });
        spaces
    }

    /// Moves a server one place up or down the rail. The order is this device's own.
    pub fn move_server(&mut self, inst: &InstanceId, space: Id, up: bool, cx: &mut Context<Self>) {
        let mut keys: Vec<String> = self
            .rail_spaces()
            .into_iter()
            .map(|(inst, id, ..)| format!("{inst}/{id}"))
            .collect();
        let key = format!("{inst}/{space}");
        let Some(at) = keys.iter().position(|k| *k == key) else {
            return;
        };
        let other = if up { at.checked_sub(1) } else { Some(at + 1) };
        if let Some(other) = other.filter(|o| *o < keys.len()) {
            keys.swap(at, other);
            let list = toml::Value::Array(keys.into_iter().map(toml::Value::String).collect());
            self.persist("sidebar.server_order", &list.to_string(), cx);
        }
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
    pub(crate) fn close_pages(&mut self, cx: &mut Context<Self>) {
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
                self.badge("home", home_badge)
                    .map(|badge| div().absolute().bottom_0().right_0().child(badge)),
            )
            .on_hover(self.track_hover("rail-home", cx))
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_pages(cx);
                this.open_friends(cx);
            }));
        let home_hovered = self.is_hovered("rail-home");
        let home_unread = home_badge.unread || home_badge.mentions > 0;
        let home = div()
            .relative()
            .flex_shrink_0()
            .child(rail_mark(
                "rail-home",
                rail_mark_height(home_selected, home_hovered, home_unread),
            ))
            .child(rail_morph(
                "home-morph",
                home,
                home_selected,
                home_hovered,
                None,
            ));

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
                let voice = self
                    .core
                    .store()
                    .instance(&inst)
                    .map(|s| view::voice_in_space(s, id))
                    .unwrap_or_default();
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
                    .tooltip({
                        let name = SharedString::from(rail_tooltip(&name, voice.count));
                        move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(name.clone())
                                .build(window, cx)
                        }
                    })
                    .on_hover(self.track_hover(format!("rail-{inst_key}-{id}"), cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_pages(cx);
                        this.select_space(inst.clone(), id, window, cx);
                    }));
                let element = self.with_menu(element, target, cx);
                let hovered = self.is_hovered(&format!("rail-{inst_key}-{id}"));
                let unread = badge.unread || badge.mentions > 0;
                // The badge goes over the picture, so it comes after it.
                div()
                    .relative()
                    .flex_shrink_0()
                    .child(rail_mark(
                        &format!("rail-{inst_key}-{id}"),
                        rail_mark_height(selected, hovered, unread),
                    ))
                    .child(rail_morph(
                        &format!("rail-morph-{inst_key}-{id}"),
                        element,
                        selected,
                        hovered,
                        picture,
                    ))
                    .children(
                        self.badge(&format!("rail-{inst_key}-{id}"), badge)
                            .map(|badge| div().absolute().bottom_0().right_0().child(badge)),
                    )
                    .when(voice.count > 0, |d| {
                        d.child(div().absolute().top_0().right(px(-4.)).child(voice_chip(
                            SharedString::from(format!("rail-voice-{inst_key}-{id}")),
                            voice,
                            selected,
                        )))
                    })
                    .into_any_element()
            })
            .collect();

        let add = rail_icon("rail-add")
            .hover(|d| d.bg(theme::hover()))
            .on_hover(self.track_hover("rail-add", cx))
            .child(icon(assets::PLUS, 24., theme::accent()))
            .on_click(
                cx.listener(|this, event: &gpui_kit::ClickEvent, window, cx| {
                    this.open_menu(Target::Instances, event.position(), window, cx);
                }),
            );
        let add = rail_morph("rail-add", add, false, self.is_hovered("rail-add"), None);

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
                let key = SharedString::from(format!("sidebar-header-{inst}-{space}"));
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
                // Another server's name fades in with its channels.
                self.with_menu(element, target, cx).with_animation(
                    key,
                    gpui_kit::Animation::new(theme::FADE).with_easing(gpui_kit::ease_out_quint()),
                    |d, t| d.opacity(t),
                )
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
                        gpui_kit::Animation::new(theme::FADE)
                            .with_easing(gpui_kit::ease_out_quint()),
                        |d, t| d.opacity(t),
                    ),
            )
    }

    /// Under the rail and the sidebar, across both: the voice panel and the user panel.
    pub fn render_bottom_panels(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut panels: Vec<AnyElement> = self.render_voice_panel(cx).into_iter().collect();
        // Just left: the panel's place closes instead of vanishing.
        if panels.is_empty() && crate::modal::closing("voice-panel") {
            use gpui_kit::AnimationExt as _;
            let height = self.voice_panel_height.get();
            panels.push(
                div()
                    .flex_shrink_0()
                    .bg(theme::footer())
                    .border_t_1()
                    .border_color(theme::border())
                    .with_animation(
                        "voice-panel-out",
                        gpui_kit::Animation::new(crate::modal::POP)
                            .with_easing(gpui_kit::ease_out_quint()),
                        move |d, t| d.h(gpui_kit::px(height * (1. - t))),
                    )
                    .into_any_element(),
            );
        }
        panels.push(self.render_me(cx).into_any_element());
        panels
    }

    /// Instances that need the person: signing in again, or a connection that gave up. A
    /// connection that is only reconnecting shows in the user panel's connection icon instead.
    fn instance_notices(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        for inst in self.core.instance_ids() {
            let status = self.core.status(&inst);
            let text = match &status {
                ConnStatus::Connected
                | ConnStatus::Offline
                | ConnStatus::Connecting
                | ConnStatus::Reconnecting { .. } => continue,
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
        let show_people = self.config.sidebar.show_voice_users;
        let mut tree_rows = tree_rows
            .into_iter()
            .filter(|row| show_people || !matches!(row, Row::VoiceUser { .. }))
            .peekable();
        // The category last opened by a click, and how many of its rows came so far: they fade
        // in one after another.
        let mut opening: Option<(Id, u32)> = None;
        // The order rows are shown in, for where a drop lands.
        {
            let mut order = self.sidebar_order.borrow_mut();
            order.clear();
            for (n, row) in tree_rows.clone().enumerate() {
                match row {
                    Row::Category { id, .. } => {
                        order.insert(id, (n, true));
                    }
                    Row::Channel { id, .. } => {
                        order.insert(id, (n, false));
                    }
                    _ => {}
                }
            }
        }
        let mut index = 0;
        while let Some(tree_row) = tree_rows.next() {
            // The space itself is the header above the list.
            if matches!(tree_row, Row::Space { .. }) {
                continue;
            }
            match &tree_row {
                Row::Category { id, .. } => {
                    opening = self
                        .category_opened
                        .get(&(inst.clone(), *id))
                        .map(|generation| (*id, *generation));
                    index = 0;
                }
                Row::Channel { nested: false, .. } => opening = None,
                _ => {}
            }
            let stagger = opening.map(|(category, generation)| {
                index += 1;
                (format!("open-{category}-{generation}"), index)
            });
            let stagger = move |element: AnyElement| match &stagger {
                Some((key, n)) => stagger_in(element, key, *n),
                None => element,
            };
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
                rows.push(stagger(self.render_tree_row(tree_row, false, cx)));
                continue;
            };
            // Highlighted: the channel's highlight goes around the people in it too.
            let block_key = format!("voice-block-{id}");
            let from = self.tint_from(&block_key, Some(state)).unwrap_or(state);
            let reach = self.row_reach();
            let head = self.render_channel_row(tree_row, true, cx);
            let mut people = Vec::new();
            while let Some(Row::VoiceUser {
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
            }) = tree_rows.next_if(|r| matches!(r, Row::VoiceUser { .. }))
            {
                people.push(self.render_voice_user(
                    inst, channel, user, name, muted, deafened, speaking, nested, dm, true, cx,
                ));
            }
            rows.push(stagger(
                div()
                    .id(SharedString::from(format!("voice-block-{id}")))
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .pb(px(if people.is_empty() { 0. } else { 4. }))
                    .rounded(theme::row_radius())
                    .map(|d| reach_left(d, reach, if nested { 10. } else { 0. }, 0.))
                    .overflow_hidden()
                    .child(state_fill(&block_key, state, from, false))
                    // The block itself stays still: its rows light up one at a time, clear of
                    // the bar on the left.
                    .child(state_bar(&block_key, state, from))
                    .child(head)
                    .children(people)
                    .into_any_element(),
            ));
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
                let open_inst = inst.clone();
                let add = drag.is_some().then(|| {
                    let inst = inst.clone();
                    div()
                        .id(SharedString::from(format!("category-add-{id}")))
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(20.))
                        .rounded(theme::radius_small())
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
                    .pl(px(8.))
                    .cursor_pointer()
                    .text_size(theme::secondary_size())
                    .text_color(theme::category_text())
                    .group(CATEGORY_ROW)
                    .hover(|d| d.text_color(theme::accent()))
                    .child(div().min_w_0().truncate().child(name.clone()))
                    // The chevron turns to point right as the category folds, and back.
                    .child(
                        icon(assets::CHEVRON_DOWN, 14., theme::category_text())
                            .group_hover(CATEGORY_ROW, |s| s.text_color(theme::accent()))
                            .with_spring(
                                SharedString::from(format!("category-chevron-{id}")),
                                gpui_kit::SpringAnimation::new(gpui_kit::SpringConfig::new(
                                    420., 32., 1.,
                                ))
                                .to(collapsed),
                                |d, phase| {
                                    d.with_transformation(gpui_kit::Transformation::rotate(
                                        gpui_kit::radians(-std::f32::consts::FRAC_PI_2 * phase.0),
                                    ))
                                },
                            ),
                    )
                    .child(div().flex_1())
                    .children(self.badge(&format!("category-{id}"), badge))
                    .children(add)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Err(err) = this.collapsed.toggle(&node) {
                            this.error(format!("could not save the sidebar state: {err}"), cx);
                        }
                        // Opening it: its rows fade in, one after another.
                        if collapsed {
                            *this
                                .category_opened
                                .entry((open_inst.clone(), id))
                                .or_default() += 1;
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
                inst, channel, user, name, muted, deafened, speaking, nested, dm, false, cx,
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
                        crate::channel::tip_button(
                            SharedString::from(format!("request-accept-{id}")),
                            assets::CHECK,
                            "Accept",
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
                let typing =
                    self.config.buffer.show_typing
                        && self.core.store().instance(&inst).is_some_and(|s| {
                            !s.typing(id, hermes_client_core::now_ms()).is_empty()
                        });
                let element = row(format!("dm-{id}"), selected, 44.)
                    .when(badge.unread || badge.mentions > 0, |d| {
                        d.text_color(theme::text())
                    })
                    .child(avatar)
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    // They are writing something: dots beside their name.
                    .when(typing, |d| {
                        d.child(crate::channel::typing_dots(
                            SharedString::from(format!("dm-typing-{id}")),
                            theme::text_secondary(),
                        ))
                    })
                    .when(connected, |d| {
                        d.child(icon(assets::PHONE, 16., theme::speaking()))
                    })
                    .children(self.badge(&format!("dm-{id}"), badge))
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
        let row_key = format!("channel-{id}");
        let from = self.tint_from(&row_key, state);
        let reach = self.row_reach();
        let timer = connected.then(|| self.time_in_voice(&inst)).flatten();
        let indent = if nested && !in_block { 10. } else { 0. };
        let element = bare_row(row_key.clone(), selected, theme::ROW_HEIGHT)
            .relative()
            .overflow_hidden()
            .text_color(theme::channel_text())
            .map(|d| match state {
                // Lit: it reaches the rail's border line.
                Some(_) => reach_left(d, reach, indent, 8.),
                None => d.when(indent > 0., |d| d.ml(px(indent))),
            })
            .group(CHANNEL_ROW)
            .when_some(state.zip(from), |d, (state, from)| {
                d.child(state_fill(&row_key, state, from, true))
                    .child(state_bar(&row_key, state, from))
            })
            .when(muted, |d| d.text_color(theme::text_tertiary()))
            .when(strong && !muted, |d| {
                d.text_color(theme::text()).font_weight(FontWeight::MEDIUM)
            })
            // Plain rows brighten their text and icon under the pointer.
            .when(state.is_none() && !in_block, |d| {
                d.hover(|d| d.text_color(theme::accent()))
            })
            // Inside a highlighted block: an inset pill beside the bar, the text where it was.
            .when(in_block, |d| {
                d.ml(theme::indicator_width() + px(2.))
                    .mr(px(2.))
                    .pl(px(6.) - theme::indicator_width())
                    .hover(|d| d.bg(theme::hover()).text_color(theme::accent()))
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
            .children(self.badge(&format!("channel-{id}"), badge))
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
        let order = self.sidebar_order.clone();
        element
            .on_drag(item, move |_, _, _, cx| cx.new(|_| Ghost(name.clone())))
            .drag_over::<Item>(move |style, dragged, _, _| {
                if !dragged.can_drop_on(&over) {
                    return style;
                }
                // Where it would land: a line on that edge, over a light accent tint. Dragged
                // down it goes after the row, up before it, onto a category into it.
                let order = order.borrow();
                let after = match (order.get(&dragged.id), order.get(&over.id)) {
                    (_, Some((_, true))) => true,
                    (Some((from, _)), Some((to, _))) => from < to,
                    _ => true,
                };
                let style = style
                    .bg(theme::alpha(theme::accent(), 0.1))
                    .border_color(theme::accent());
                if after {
                    style.border_b_2()
                } else {
                    style.border_t_2()
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
            let avatar = self.avatar_with_status_on(
                inst.as_deref().unwrap_or(""),
                me.as_ref(),
                &name,
                theme::AVATAR_SMALL,
                presence,
                theme::footer(),
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
            .rounded(theme::radius_small())
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
            .child(self.render_connection_icon(cx))
            .children(self.render_me_buttons(cx))
    }

    /// Between my name and the buttons: how the connection to the instances is. Its slot never
    /// changes size, so nothing moves when it does.
    fn render_connection_icon(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let instances = self.core.instance_ids();
        let states: Vec<(ConnStatus, Option<std::time::Duration>)> = instances
            .iter()
            .map(|inst| (self.core.status(inst), self.core.down_for(inst)))
            .collect();
        let look = ConnLook::of(&states);
        let tip: SharedString = if instances.is_empty() {
            "No instance".into()
        } else {
            instances
                .iter()
                .zip(&states)
                .map(|(inst, (status, _))| status_line(inst, status))
                .collect::<Vec<_>>()
                .join("\n")
                .into()
        };
        let glyph = icon(look.icon(), 16., look.color());
        let glyph = if look == ConnLook::Reconnecting {
            glyph
                .with_animation(
                    "connection-pulse",
                    gpui_kit::Animation::new(std::time::Duration::from_millis(1600)).repeat(),
                    |svg, t| {
                        // Slowly between full and faint and back.
                        let wave = (t * std::f32::consts::TAU).cos() * 0.5 + 0.5;
                        svg.opacity(0.35 + 0.65 * wave)
                    },
                )
                .into_any_element()
        } else {
            glyph.into_any_element()
        };
        div()
            .id("connection")
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(px(20.))
            .rounded(theme::radius_small())
            .when(look != ConnLook::Connected, |d| {
                d.cursor_pointer().hover(|d| d.bg(theme::hover()))
            })
            .child(glyph)
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                for inst in this.core.instance_ids() {
                    let status = this.core.status(&inst);
                    if matches!(
                        status,
                        ConnStatus::Connected | ConnStatus::LoggedOut | ConnStatus::Failed { .. }
                    ) {
                        continue;
                    }
                    if let Err(err) = this.core.set_online(&inst, true) {
                        this.error(format!("could not reconnect: {err}"), cx);
                    }
                }
                cx.notify();
            }))
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
    use super::{ConnLook, rail_slot_top};
    use hermes_client_core::gateway::ConnStatus;
    use std::time::Duration;

    #[test]
    fn the_tooltip_says_how_many_are_in_voice() {
        assert_eq!(super::rail_tooltip("Home", 0), "Home");
        assert_eq!(super::rail_tooltip("Home", 3), "Home - 3 in voice");
    }

    #[test]
    fn connection_look_ignores_blips_and_shows_the_worst() {
        let secs = |s| Some(Duration::from_secs(s));
        let up = (ConnStatus::Connected, None);
        let blip = (ConnStatus::Reconnecting { attempt: 1 }, secs(1));
        let lost = (ConnStatus::Reconnecting { attempt: 3 }, secs(5));
        let failed = (ConnStatus::Failed { reason: "x".into() }, secs(60));
        let offline = (ConnStatus::Offline, secs(60));
        assert_eq!(ConnLook::of(std::slice::from_ref(&up)), ConnLook::Connected);
        assert_eq!(ConnLook::of(&[up.clone(), blip]), ConnLook::Connected);
        assert_eq!(
            ConnLook::of(&[up.clone(), lost.clone()]),
            ConnLook::Reconnecting
        );
        assert_eq!(ConnLook::of(&[lost, failed]), ConnLook::Disconnected);
        assert_eq!(
            ConnLook::of(std::slice::from_ref(&offline)),
            ConnLook::Offline
        );
        assert_eq!(ConnLook::of(&[offline, up]), ConnLook::Connected);
        assert_eq!(
            ConnLook::of(&[(ConnStatus::LoggedOut, None)]),
            ConnLook::Disconnected
        );
        assert_eq!(ConnLook::of(&[]), ConnLook::Offline);
    }

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
