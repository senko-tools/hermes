//! Dialogs over the whole window. The kit's dialogs rebuild their content from a closure that holds
//! no state of ours, so these draw from the shell's state like everything else.

use gpui_kit::AnimationExt as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    Anchor, AnyElement, Bounds, Context, Div, Pixels, Point, SharedString, Size, anchored,
    deferred, div,
};

use crate::menu_entries::{Action, Target};
use crate::shell::Shell;
use crate::theme::{self, px};

/// A heading for a dialog.
pub fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(theme::heading_size())
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .child(text.into())
}

/// A paragraph for a dialog, in the secondary colour.
pub fn paragraph(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(theme::secondary_size())
        .text_color(theme::text_secondary())
        .child(text.into())
}

/// An error line for a dialog.
pub fn error_line(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(theme::secondary_size())
        .text_color(theme::danger())
        .child(text.into())
}

/// The dimmed backdrop with `card` in the middle. `id` names the dialog.
pub fn dialog(id: &'static str, width: f32, card: impl IntoElement) -> AnyElement {
    deferred(
        div()
            .id(id)
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::scrim())
            // The dialog is modal: nothing behind it reacts.
            .occlude()
            .child(
                div()
                    .id("dialog-card")
                    .occlude()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .w(px(width))
                    .max_h_full()
                    .overflow_y_scroll()
                    .p(px(20.))
                    .rounded(theme::radius_large())
                    .bg(theme::background())
                    .border_1()
                    .border_color(theme::border())
                    .text_color(theme::text())
                    .font_family(theme::font())
                    .child(div().child(card).with_animation(
                        id,
                        gpui_kit::Animation::new(std::time::Duration::from_millis(120)),
                        |d, t| d.opacity(t),
                    )),
            ),
    )
    .with_priority(gpui_kit::base::POPUP_PRIORITY)
    .into_any_element()
}

/// Where a popup was asked for: the click, and what was clicked (the row or button it hugs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopupFrom {
    pub at: Point<Pixels>,
    pub trigger: Bounds<Pixels>,
}

impl PopupFrom {
    /// Nothing known around the click: the popup hugs the point itself.
    pub fn point(at: Point<Pixels>) -> PopupFrom {
        PopupFrom {
            at,
            trigger: Bounds::new(at, gpui_kit::size(gpui_kit::px(1.), gpui_kit::px(1.))),
        }
    }

    /// The smallest of `triggers` under `at` (the most specific thing clicked), else the point.
    pub fn find(at: Point<Pixels>, triggers: &[Bounds<Pixels>]) -> PopupFrom {
        let area = |b: &Bounds<Pixels>| b.size.width.as_f32() * b.size.height.as_f32();
        triggers
            .iter()
            .filter(|b| b.contains(&at))
            .min_by(|a, b| area(a).total_cmp(&area(b)))
            .map_or_else(
                || PopupFrom::point(at),
                |trigger| PopupFrom {
                    at,
                    trigger: *trigger,
                },
            )
    }
}

/// Where a popup goes and which way it slides in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Hug {
    /// The popup's corner that sits at `position`.
    pub anchor: Anchor,
    pub position: Point<Pixels>,
    /// The way it slides as it opens: (x, y), each -1, 0 or 1.
    pub slide: (f32, f32),
}

/// The space between a popup and what was clicked.
const HUG_GAP: f32 = 12.;
/// How close a popup comes to the window's edge.
const HUG_MARGIN: f32 = 8.;
/// How far the click may be from the popup's near corner along the clicked thing.
const HUG_LEAD: f32 = 24.;

/// Places a popup `width` wide beside what was clicked, on the side towards the window's middle
/// (or the other side, if only that one has room), with a fixed gap. Along that edge it follows
/// the click, but only within the clicked thing's ends. With no room beside it, it goes above or
/// below instead. The window's edge is kept by gpui's snapping afterwards. All in window pixels.
pub(crate) fn hug(from: PopupFrom, width: Pixels, viewport: Size<Pixels>, scale: f32) -> Hug {
    let (gap, margin, lead) = (HUG_GAP * scale, HUG_MARGIN * scale, HUG_LEAD * scale);
    let t = from.trigger;
    let (left, right) = (t.origin.x.as_f32(), (t.origin.x + t.size.width).as_f32());
    let (top, bottom) = (t.origin.y.as_f32(), (t.origin.y + t.size.height).as_f32());
    let (vw, vh) = (viewport.width.as_f32(), viewport.height.as_f32());
    let (ax, ay) = (from.at.x.as_f32(), from.at.y.as_f32());
    let need = width.as_f32() + gap + margin;
    let room_left = left;
    let room_right = vw - right;
    let towards_right = (left + right) / 2. < vw / 2.;
    let side = match (room_left >= need, room_right >= need) {
        (true, true) => Some(towards_right),
        (false, true) => Some(true),
        (true, false) => Some(false),
        (false, false) => None,
    };
    let point = |x: f32, y: f32| gpui_kit::point(gpui_kit::px(x), gpui_kit::px(y));
    match side {
        Some(to_right) => {
            // Opening downwards in the window's upper half, upwards in the lower one.
            let down = ay <= vh / 2.;
            let y = if down {
                (ay - lead).clamp(top, bottom.max(top))
            } else {
                (ay + lead).clamp(top, bottom.max(top))
            };
            let anchor = match (to_right, down) {
                (true, true) => Anchor::TopLeft,
                (true, false) => Anchor::BottomLeft,
                (false, true) => Anchor::TopRight,
                (false, false) => Anchor::BottomRight,
            };
            let x = if to_right { right + gap } else { left - gap };
            Hug {
                anchor,
                position: point(x, y),
                slide: (if to_right { 1. } else { -1. }, 0.),
            }
        }
        None => {
            let below = (top + bottom) / 2. <= vh / 2.;
            let rightwards = ax <= vw / 2.;
            let x = if rightwards {
                (ax - lead).clamp(left, right.max(left))
            } else {
                (ax + lead).clamp(left, right.max(left))
            };
            let anchor = match (below, rightwards) {
                (true, true) => Anchor::TopLeft,
                (true, false) => Anchor::TopRight,
                (false, true) => Anchor::BottomLeft,
                (false, false) => Anchor::BottomRight,
            };
            let y = if below { bottom + gap } else { top - gap };
            Hug {
                anchor,
                position: point(x, y),
                slide: (0., if below { 1. } else { -1. }),
            }
        }
    }
}

/// A popup opened by a click (the share menu, the network details, a profile, a volume menu):
/// `card`, `width` wide, sits beside what was clicked (see [`hug`]) and fades in as it slides
/// out from it a little.
pub fn popup(
    id: &'static str,
    from: PopupFrom,
    width: f32,
    viewport: Size<Pixels>,
    card: impl IntoElement + 'static,
) -> AnyElement {
    use gpui_kit::{Animation, AnimationExt as _, ease_out_quint};
    let place = hug(from, px(width), viewport, theme::scale());
    let (sx, sy) = place.slide;
    deferred(
        anchored()
            .anchor(place.anchor)
            .position(place.position)
            .snap_to_window_with_margin(px(HUG_MARGIN))
            .child(div().relative().child(card).with_animation(
                id,
                Animation::new(std::time::Duration::from_millis(150)).with_easing(ease_out_quint()),
                move |d, t| {
                    let back = 6. * (1. - t);
                    d.opacity(t).left(px(-sx * back)).top(px(-sy * back))
                },
            )),
    )
    .with_priority(gpui_kit::base::POPUP_PRIORITY)
    .into_any_element()
}

impl Shell {
    pub fn render_modals(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        out.extend(self.render_add_instance(cx));
        out.extend(self.render_form(cx));
        out.extend(self.render_new_dm(cx));
        out.extend(self.render_keys_dialog(cx));
        out.extend(self.render_viewer(cx));
        out.extend(self.render_network(cx));
        out.extend(self.render_volume_menu(cx));
        out.extend(self.render_profile(cx));
        out.extend(self.render_share_menu(cx));
        out.extend(self.render_command_bar(cx));
        out.extend(self.render_crop(cx));
        out.extend(self.render_video_full(cx));
        out.extend(self.render_confirm(cx));
        out
    }

    fn render_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (target, action) = self.confirm.as_ref()?;
        let text = self.confirm_text(target, *action);
        let cancel = Button::new("confirm-cancel")
            .cursor_pointer()
            .label("Cancel")
            .on_click(cx.listener(|this, _, _, cx| {
                this.confirm = None;
                cx.notify();
            }));
        let ok = Button::new("confirm-ok")
            .cursor_pointer()
            .label(action.label())
            .danger()
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some((target, action)) = this.confirm.take() {
                    this.run_action(target, action, window, cx);
                }
                cx.notify();
            }));
        Some(dialog(
            "confirm",
            420.,
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(heading("Are you sure?"))
                .child(paragraph(text))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(cancel.large())
                        .child(ok.large()),
                ),
        ))
    }

    /// What a destructive action does, in words.
    fn confirm_text(&self, target: &Target, action: Action) -> String {
        let store = self.core.store();
        let s = target.inst().and_then(|i| store.instance(i));
        match (target, action) {
            (Target::Instance(inst), _) => {
                format!("{inst} is removed from this client, with its saved sign-in.")
            }
            (Target::Space(_, space), Action::DeleteSpace) => format!(
                "{} and everything in it are deleted for everyone. This cannot be undone.",
                s.and_then(|s| s.spaces.get(space))
                    .map_or("This server", |st| st.space.name.as_str())
            ),
            (Target::Space(_, space), _) => format!(
                "You leave {} and need an invite to come back.",
                s.and_then(|s| s.spaces.get(space))
                    .map_or("this server", |st| st.space.name.as_str())
            ),
            (Target::Channel(_, channel), Action::LeaveGroup) => format!(
                "You leave {} and stop receiving its messages.",
                s.and_then(|s| s
                    .channel(*channel)
                    .map(|c| hermes_client_core::view::dm_name(s, c)))
                    .unwrap_or_else(|| "this group".into())
            ),
            (Target::Channel(_, channel), _) => format!(
                "{} and its messages are deleted for everyone. This cannot be undone.",
                s.and_then(|s| s
                    .channel(*channel)
                    .map(|c| hermes_client_core::view::channel_title(s, c)))
                    .unwrap_or_else(|| "This channel".into())
            ),
            (Target::Message(..), _) => "This message is deleted for everyone.".into(),
            (Target::Messages(_, _, messages), _) => {
                format!("{} messages are deleted for everyone.", messages.len())
            }
            (Target::Member(_, space, user), Action::Kick) => format!(
                "{} is removed from the server. They can come back with an invite.",
                s.map(|s| s.display_name(*space, *user)).unwrap_or_default()
            ),
            (Target::Member(_, space, user), _) => format!(
                "{} becomes the owner of this server, and you no longer are.",
                s.map(|s| s.display_name(*space, *user)).unwrap_or_default()
            ),
            (Target::Role(..), _) => "This role is deleted and taken from everyone.".into(),
            (Target::Invite(..), _) => "This invite link stops working.".into(),
            _ => action.label().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Hug, PopupFrom, hug};
    use gpui_kit::{Anchor, Bounds, point, px, size};

    fn from(at: (f32, f32), origin: (f32, f32), extent: (f32, f32)) -> PopupFrom {
        PopupFrom {
            at: point(px(at.0), px(at.1)),
            trigger: Bounds::new(
                point(px(origin.0), px(origin.1)),
                size(px(extent.0), px(extent.1)),
            ),
        }
    }

    fn place(from: PopupFrom, width: f32) -> Hug {
        hug(from, px(width), size(px(1200.), px(800.)), 1.)
    }

    #[test]
    fn a_member_row_on_the_right_opens_to_its_left() {
        // A 240 wide row at the window's right edge, clicked in its middle.
        let h = place(from((1100., 217.), (960., 200.), (240., 34.)), 300.);
        assert_eq!(h.anchor, Anchor::TopRight);
        assert_eq!(h.position, point(px(948.), px(200.)));
        assert_eq!(h.slide, (-1., 0.));
    }

    #[test]
    fn the_top_follows_the_click_within_the_trigger() {
        // A tall trigger: the top is the click less the lead, not the trigger's top.
        let h = place(from((1100., 300.), (960., 100.), (240., 300.)), 300.);
        assert_eq!(h.position.y, px(276.));
        // Low in the window, it opens upwards with its bottom near the click.
        let h = place(from((1100., 600.), (960., 590.), (240., 34.)), 300.);
        assert_eq!(h.anchor, Anchor::BottomRight);
        assert_eq!(h.position.y, px(624.));
    }

    #[test]
    fn a_button_bottom_left_opens_to_its_right() {
        let h = place(from((200., 770.), (184., 754.), (32., 32.)), 400.);
        assert_eq!(h.anchor, Anchor::BottomLeft);
        assert_eq!(h.position, point(px(228.), px(786.)));
        assert_eq!(h.slide, (1., 0.));
    }

    #[test]
    fn with_no_room_beside_it_goes_above_or_below() {
        // A message across the whole chat.
        let h = place(from((300., 120.), (0., 100.), (1200., 60.)), 300.);
        assert_eq!(h.anchor, Anchor::TopLeft);
        assert_eq!(h.position, point(px(276.), px(172.)));
        assert_eq!(h.slide, (0., 1.));
        let h = place(from((900., 700.), (0., 680.), (1200., 60.)), 300.);
        assert_eq!(h.anchor, Anchor::BottomRight);
        assert_eq!(h.position, point(px(924.), px(668.)));
    }

    #[test]
    fn a_tiny_window_still_places_it() {
        let h = hug(
            from((50., 50.), (40., 40.), (20., 20.)),
            px(300.),
            size(px(200.), px(150.)),
            1.,
        );
        assert_eq!(h.anchor, Anchor::TopLeft);
        assert_eq!(h.position.y, px(72.));
    }

    #[test]
    fn the_smallest_trigger_under_the_click_wins() {
        let row = Bounds::new(point(px(0.), px(0.)), size(px(500.), px(40.)));
        let avatar = Bounds::new(point(px(8.), px(4.)), size(px(32.), px(32.)));
        let at = point(px(20.), px(20.));
        assert_eq!(PopupFrom::find(at, &[row, avatar]).trigger, avatar);
        assert_eq!(
            PopupFrom::find(point(px(200.), px(20.)), &[row, avatar]).trigger,
            row
        );
        assert_eq!(
            PopupFrom::find(point(px(900.), px(900.)), &[row]),
            PopupFrom::point(point(px(900.), px(900.)))
        );
    }
}
