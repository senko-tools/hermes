//! Controls used all over the app, built once so they look and move the same everywhere.
//!
//! Rule: a control that changes colour under the pointer notes its hover with
//! [`Shell::track_hover`] and springs to it. gpui's own text-colour `.hover` keeps a stale
//! "hovered" when the control's hover style comes and goes (a chip while it is chosen), so the
//! text stayed the hover colour once it was unchosen. A plain `.hover`
//! that is always attached is fine; one inside `.when(..)` is not, and a test keeps it out.

use gpui_kit::prelude::*;
use gpui_kit::{AnimationExt as _, Context, SharedString, div};

use crate::assets::icon;
use crate::shell::Shell;
use crate::theme::{self, px};

/// The spring every hover and choice follows.
fn spring(to: f32) -> gpui_kit::SpringAnimation<f32> {
    gpui_kit::SpringAnimation::new(theme::paced(crate::hover::HOVER_SPRING)).to(to)
}

/// Where a control rests: 0 at rest, 1 under the pointer, 2 chosen. One spring between them
/// moves its colours together, so choosing one under the pointer flows on from its hover.
fn state(hovered: bool, chosen: bool) -> f32 {
    if chosen {
        2.
    } else if hovered {
        1.
    } else {
        0.
    }
}

/// A tag or filter that is on or off, where many can be on: outlined when off, filled with the
/// accent when on. Never in a joined track (one of a few is [`segmented`]). The caller adds what
/// a click does.
pub fn chip(
    shell: &Shell,
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    chosen: bool,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let id = id.into();
    let hover = SharedString::from(format!("{id:?}-hover"));
    let to = state(shell.is_hovered(&hover), chosen);
    let face = div()
        .id("face")
        .px(px(10.))
        .py(px(4.))
        .rounded_full()
        .border_1()
        .truncate()
        .child(label.into())
        .with_spring(
            SharedString::from(format!("{id:?}-chosen")),
            spring(to),
            |d, t| {
                let t = t.clamp(0., 2.);
                // Rest to hover: the outline and text turn accent. Hover to on: it fills in.
                let hover = t.min(1.);
                let fill = (t - 1.).max(0.);
                let text = theme::mix(theme::text_secondary(), theme::accent(), hover);
                let outline = theme::mix(theme::border(), theme::accent(), hover);
                d.bg(theme::alpha(theme::accent(), fill))
                    .border_color(theme::mix(outline, theme::accent(), fill))
                    .text_color(theme::mix(text, theme::on_accent(), fill))
            },
        );
    div()
        .id(id)
        .rounded_full()
        .cursor_pointer()
        .max_w_full()
        .overflow_hidden()
        .text_size(theme::secondary_size())
        .on_hover(shell.track_hover(hover, cx))
        .child(face)
}

/// What a [`Segment`] shows before its label, drawn in its text colour of the moment (it
/// turns `on_accent` as the thumb covers it).
pub type Preview = std::rc::Rc<dyn Fn(gpui_kit::Rgba) -> gpui_kit::AnyElement>;

/// An icon as a [`Segment`]'s picture.
pub fn icon_preview(glyph: &'static str) -> Preview {
    std::rc::Rc::new(move |color| icon(glyph, 14., color).into_any_element())
}

// ---------------------------------------------------------------------------------------------
// The control family (#62): switch, segmented choice, radio list, dropdown, chip and buttons.
// They share a height, corners, text size, the hover spring, a focus ring (keyboard only) and
// one disabled look, and each one's shape shows how it works.

/// How tall a control is: buttons, dropdowns, segmented tracks.
pub fn control_h() -> gpui_kit::Pixels {
    px(32.)
}

/// What a control does when clicked, or pressed with Space or Enter while focused.
type Act = std::rc::Rc<dyn Fn(&mut gpui_kit::Window, &mut gpui_kit::App)>;

/// A `Shell::track_hover` listener shared by a control drawn again each frame.
type HoverListener = std::rc::Rc<dyn Fn(&bool, &mut gpui_kit::Window, &mut gpui_kit::App)>;

fn act(
    cx: &Context<Shell>,
    action: impl Fn(&mut Shell, &mut gpui_kit::Window, &mut Context<Shell>) + 'static,
) -> Act {
    let listener = cx.listener(move |this, _: &(), window, cx| action(this, window, cx));
    std::rc::Rc::new(move |window, cx| listener(&(), window, cx))
}

impl Shell {
    /// The keyboard focus of the control `key`, made the first time it is drawn.
    fn control_focus_handle(
        &self,
        key: &SharedString,
        cx: &gpui_kit::App,
    ) -> gpui_kit::FocusHandle {
        self.control_focus
            .borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }
}

/// Makes `el` a control: its pointer, hover, focus ring and what a click or Space/Enter does.
/// A disabled one is drawn at half strength and does nothing.
fn pressable(
    el: gpui_kit::Stateful<gpui_kit::Div>,
    shell: &Shell,
    key: &SharedString,
    enabled: bool,
    action: Option<Act>,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let el = el
        .border_1()
        .border_color(gpui_kit::transparent_black())
        .on_hover(shell.track_hover(SharedString::from(format!("{key}-hover")), cx));
    if !enabled {
        return el.opacity(0.5).cursor_not_allowed();
    }
    let focus = shell.control_focus_handle(key, cx);
    let el = el
        .cursor_pointer()
        .track_focus(&focus)
        .focus_visible(|s| s.border_color(theme::focus_ring()));
    match action {
        Some(action) => {
            let on_key = action.clone();
            el.on_click(move |_, window, cx| action(window, cx))
                .on_key_down(move |event: &gpui_kit::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "space" | "enter") {
                        cx.stop_propagation();
                        on_key(window, cx);
                    }
                })
        }
        None => el,
    }
}

fn hovered(shell: &Shell, key: &SharedString) -> bool {
    shell.is_hovered(&format!("{key}-hover"))
}

/// An on/off setting: a pill track whose round knob slides across and whose track fills with
/// the accent when on. The track lightens under the pointer.
pub fn switch(
    shell: &Shell,
    id: impl Into<SharedString>,
    on: bool,
    enabled: bool,
    action: impl Fn(&mut Shell, &mut gpui_kit::Window, &mut Context<Shell>) + 'static,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let id: SharedString = id.into();
    const W: f32 = 34.;
    const H: f32 = 20.;
    const KNOB: f32 = 14.;
    let hover = hovered(shell, &id) && enabled;
    let lift = div().absolute().inset_0().rounded_full().with_spring(
        SharedString::from(format!("{id}-lift")),
        spring(if hover { 1. } else { 0. }),
        |d, t| d.bg(theme::alpha(theme::hover(), 0.6 * t.clamp(0., 1.))),
    );
    let track = div()
        .relative()
        .w(px(W))
        .h(px(H))
        .rounded_full()
        .overflow_hidden()
        .with_spring(
            SharedString::from(format!("{id}-on")),
            spring(if on { 1. } else { 0. }),
            move |d, t| {
                let fill = t.clamp(0., 1.);
                d.bg(theme::mix(theme::control_track(), theme::accent(), fill))
                    .child(
                        div()
                            .absolute()
                            .top(px((H - KNOB) / 2.))
                            .left(px(3. + t * (W - KNOB - 6.)))
                            .size(px(KNOB))
                            .rounded_full()
                            .bg(theme::mix(theme::text(), theme::on_accent(), fill)),
                    )
            },
        );
    let track = track.child(lift);
    let el = div()
        .id(id.clone())
        .flex_shrink_0()
        .rounded_full()
        .child(track);
    pressable(el, shell, &id, enabled, Some(act(cx, action)), cx)
}

/// One option of a [`segmented`] choice.
pub struct Segment {
    pub label: SharedString,
    pub preview: Option<Preview>,
    /// Off: shown at half strength and cannot be picked, while the others can.
    pub enabled: bool,
}

impl Segment {
    pub fn new(label: impl Into<SharedString>) -> Segment {
        Segment {
            label: label.into(),
            preview: None,
            enabled: true,
        }
    }

    /// With a small picture before the label (an icon, a shape, a layout).
    pub fn with(preview: Preview, label: impl Into<SharedString>) -> Segment {
        Segment {
            label: label.into(),
            preview: Some(preview),
            enabled: true,
        }
    }

    /// Whether this one can be picked.
    pub fn enabled(mut self, enabled: bool) -> Segment {
        self.enabled = enabled;
        self
    }
}

/// Where each option of a segmented choice sat (left edge and width inside its track), by
/// control id. Recorded after layout and read on the next frame, the way `channel::grow_in`
/// measures heights, so the thumb knows where to go without an extra redraw.
pub type Spans =
    std::rc::Rc<std::cell::RefCell<std::collections::HashMap<SharedString, Vec<(f32, f32)>>>>;

/// The thumb's left edge and width at `t`, the chosen index as it springs along: whole numbers
/// sit exactly on an option, anything between blends the two neighbours. Overshoot past either
/// end is held at that end.
pub fn thumb_span(spans: &[(f32, f32)], t: f32) -> Option<(f32, f32)> {
    let last = spans.len().checked_sub(1)?;
    let t = t.clamp(0., last as f32);
    let i = (t.floor() as usize).min(last);
    let j = (i + 1).min(last);
    let f = t - i as f32;
    let (a, b) = (spans[i], spans[j]);
    Some((a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f))
}

/// How much of `span` the thumb covers, 0 to 1: a label turns the on-accent colour as the thumb
/// passes over it.
pub fn coverage(span: (f32, f32), thumb: (f32, f32)) -> f32 {
    if span.1 <= 0. {
        return 0.;
    }
    let overlap = (span.0 + span.1).min(thumb.0 + thumb.1) - span.0.max(thumb.0);
    (overlap / span.1).clamp(0., 1.)
}

/// One of a few short options, in one joined track. The accent thumb slides from the old choice
/// to the new one on a spring, and each label turns to the on-accent colour as it passes. With
/// `chosen` `None` nothing is filled.
#[allow(clippy::too_many_arguments)]
pub fn segmented(
    shell: &Shell,
    id: impl Into<SharedString>,
    options: Vec<Segment>,
    chosen: Option<usize>,
    enabled: bool,
    on_pick: impl Fn(&mut Shell, usize, &mut gpui_kit::Window, &mut Context<Shell>) + 'static,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    segmented_in(shell, id, options, chosen, enabled, false, on_pick, cx)
}

/// [`segmented`] across the whole width it is given, each option an equal share with its label
/// centered: for options with long labels, under the setting's name.
#[allow(clippy::too_many_arguments)]
pub fn segmented_wide(
    shell: &Shell,
    id: impl Into<SharedString>,
    options: Vec<Segment>,
    chosen: Option<usize>,
    enabled: bool,
    on_pick: impl Fn(&mut Shell, usize, &mut gpui_kit::Window, &mut Context<Shell>) + 'static,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    segmented_in(shell, id, options, chosen, enabled, true, on_pick, cx)
}

#[allow(clippy::too_many_arguments)]
fn segmented_in(
    shell: &Shell,
    id: impl Into<SharedString>,
    options: Vec<Segment>,
    chosen: Option<usize>,
    enabled: bool,
    stretch: bool,
    on_pick: impl Fn(&mut Shell, usize, &mut gpui_kit::Window, &mut Context<Shell>) + 'static,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    use std::rc::Rc;
    const PAD: f32 = 3.;
    let id: SharedString = id.into();
    let on_pick = Rc::new(on_pick);
    let spans = shell.segment_spans.borrow().get(&id).cloned();
    // Measured spans of another option count (the list changed) are stale.
    let spans = spans.filter(|s| s.len() == options.len());
    let measured = spans.is_some();
    let spans = spans.unwrap_or_default();
    let options: Vec<_> = options
        .into_iter()
        .enumerate()
        .map(|(n, option)| {
            let key = SharedString::from(format!("{id}-{n}"));
            let enabled = enabled && option.enabled;
            let hover = enabled && hovered(shell, &key);
            let action = enabled.then(|| {
                let on_pick = on_pick.clone();
                act(cx, move |this, window, cx| on_pick(this, n, window, cx))
            });
            let hover_listener: HoverListener =
                Rc::new(shell.track_hover(SharedString::from(format!("{key}-hover")), cx));
            let focus = enabled.then(|| shell.control_focus_handle(&key, cx));
            (key, option, hover, action, hover_listener, focus)
        })
        .collect();
    let options = Rc::new(options);
    let record = shell.segment_spans.clone();
    let record_id = id.clone();
    let row = {
        let options = options.clone();
        let spans = spans.clone();
        div()
            .flex()
            .h_full()
            .when(stretch, |d| d.w_full())
            .with_spring(
                SharedString::from(format!("{id}-thumb")),
                spring(chosen.unwrap_or(0) as f32),
                move |d, t| {
                    let record = record.clone();
                    let record_id = record_id.clone();
                    let d = d.on_children_prepainted(move |bounds, _, _| {
                        let Some(first) = bounds.first() else { return };
                        let origin = f32::from(first.origin.x);
                        let now: Vec<(f32, f32)> = bounds
                            .iter()
                            .map(|b| (f32::from(b.origin.x) - origin, f32::from(b.size.width)))
                            .collect();
                        let mut all = record.borrow_mut();
                        if all.get(&record_id) != Some(&now) {
                            if all.len() > 256 {
                                all.clear();
                            }
                            all.insert(record_id.clone(), now);
                        }
                    });
                    // Before the options were measured the chosen one fills itself in.
                    let thumb = if measured {
                        chosen.and_then(|_| thumb_span(&spans, t))
                    } else {
                        None
                    };
                    d.children(options.iter().enumerate().map(
                        |(n, (key, option, hover, action, hover_listener, focus))| {
                            let fill = match (thumb, spans.get(n)) {
                                (Some(thumb), Some(span)) => coverage(*span, thumb),
                                _ if !measured && chosen == Some(n) => 1.,
                                _ => 0.,
                            };
                            let label = option.label.clone();
                            let preview = option.preview.clone();
                            let face = div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .h_full()
                                .px(px(14.))
                                .when(stretch, |d| d.justify_center())
                                .rounded(theme::radius_small())
                                .when(!measured && fill > 0., |d| d.bg(theme::accent()))
                                .whitespace_nowrap()
                                .with_spring(
                                    SharedString::from(format!("{key}-face")),
                                    spring(if *hover { 1. } else { 0. }),
                                    move |d, h| {
                                        let h = h.clamp(0., 1.);
                                        let text =
                                            theme::mix(theme::text_secondary(), theme::accent(), h);
                                        let text = theme::mix(text, theme::on_accent(), fill);
                                        d.text_color(text)
                                            .children(preview.as_ref().map(|p| p(text)))
                                            .child(label.clone())
                                    },
                                );
                            let el = div()
                                .id(key.clone())
                                .h_full()
                                .when(stretch, |d| d.flex_1().min_w_0())
                                .rounded(theme::radius_small())
                                .border_1()
                                .border_color(gpui_kit::transparent_black())
                                .on_hover({
                                    let l = hover_listener.clone();
                                    move |h, w, cx| l(h, w, cx)
                                })
                                .child(face);
                            match (action, focus) {
                                (Some(action), Some(focus)) => {
                                    let click = action.clone();
                                    let key_act = action.clone();
                                    el.cursor_pointer()
                                        .track_focus(focus)
                                        .focus_visible(|s| s.border_color(theme::focus_ring()))
                                        .on_click(move |_, window, cx| click(window, cx))
                                        .on_key_down(
                                            move |event: &gpui_kit::KeyDownEvent, window, cx| {
                                                if matches!(
                                                    event.keystroke.key.as_str(),
                                                    "space" | "enter"
                                                ) {
                                                    cx.stop_propagation();
                                                    key_act(window, cx);
                                                }
                                            },
                                        )
                                }
                                _ if !option.enabled => el.opacity(0.5).cursor_not_allowed(),
                                _ => el,
                            }
                        },
                    ))
                },
            )
    };
    // The thumb, under the labels: it springs on the same key as the row, so the two agree.
    let thumb = div()
        .absolute()
        .top(gpui_kit::px(PAD))
        .bottom(gpui_kit::px(PAD))
        .rounded(theme::radius_small())
        .with_spring(
            SharedString::from(format!("{id}-thumb-fill")),
            spring(chosen.unwrap_or(0) as f32),
            move |d, t| match (measured, chosen, thumb_span(&spans, t)) {
                (true, Some(_), Some((x, w))) => d
                    .left(gpui_kit::px(PAD + x))
                    .w(gpui_kit::px(w))
                    .bg(theme::accent()),
                _ => d,
            },
        );
    let el = div()
        .id(id)
        .relative()
        .flex_shrink_0()
        .when(stretch, |d| d.w_full())
        .max_w_full()
        .h(control_h())
        .p(gpui_kit::px(PAD))
        .rounded(theme::radius_small())
        .bg(theme::control_track())
        .text_size(theme::body_size())
        .child(thumb)
        .child(row);
    if enabled {
        el
    } else {
        el.opacity(0.5).cursor_not_allowed()
    }
}

/// One of several options that each want a line of explanation: rows with a radio dot, a title
/// and an optional hint. The dot fills in when chosen.
pub fn radio_list(
    shell: &Shell,
    id: impl Into<SharedString>,
    options: Vec<(SharedString, Option<SharedString>)>,
    chosen: Option<usize>,
    on_pick: impl Fn(&mut Shell, usize, &mut gpui_kit::Window, &mut Context<Shell>) + 'static,
    cx: &Context<Shell>,
) -> gpui_kit::Div {
    let id: SharedString = id.into();
    let on_pick = std::rc::Rc::new(on_pick);
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .children(options.into_iter().enumerate().map(|(n, (title, hint))| {
            let key = SharedString::from(format!("{id}-{n}"));
            let hover = hovered(shell, &key);
            let on = chosen == Some(n);
            let dot = div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .size(px(16.))
                .rounded_full()
                .border_2()
                .with_spring(
                    SharedString::from(format!("{key}-dot")),
                    spring(state(hover, on)),
                    |d, t| {
                        let t = t.clamp(0., 2.);
                        let h = t.min(1.);
                        let fill = (t - 1.).max(0.);
                        let ring = theme::mix(theme::text_tertiary(), theme::accent(), h.max(fill));
                        d.border_color(ring)
                            .child(div().size(px(8. * fill)).rounded_full().bg(theme::accent()))
                    },
                );
            let text = div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .text_size(theme::body_size())
                        .text_color(if on {
                            theme::text()
                        } else {
                            theme::text_secondary()
                        })
                        .child(title),
                )
                .children(hint.map(|hint| {
                    div()
                        .text_size(theme::secondary_size())
                        .text_color(theme::text_tertiary())
                        .child(hint)
                }));
            let face = div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(8.))
                .py(px(4.))
                .rounded(theme::radius_small())
                .with_spring(
                    SharedString::from(format!("{key}-row")),
                    spring(if hover { 1. } else { 0. }),
                    |d, t| d.bg(theme::alpha(theme::hover(), t.clamp(0., 1.))),
                )
                .child(dot)
                .child(text);
            let on_pick = on_pick.clone();
            let action = act(cx, move |this, window, cx| on_pick(this, n, window, cx));
            pressable(
                div()
                    .id(key.clone())
                    .rounded(theme::radius_small())
                    .child(face),
                shell,
                &key,
                true,
                Some(action),
                cx,
            )
        }))
}

/// How wide a [`dropdown`] is, by what it holds.
#[derive(Debug, Clone, Copy)]
pub enum DropWidth {
    /// Fonts, themes, account names.
    Medium,
    /// Device and screen names.
    Wide,
}

/// A field showing the current value with a chevron, opening the list of options: for many
/// options or long names. Wraps the kit's `Select` at the family's height.
pub fn dropdown<D>(
    state: &gpui_kit::Entity<gpui_kit::component::select::SelectState<D>>,
    width: DropWidth,
) -> gpui_kit::Div
where
    D: gpui_kit::component::select::SelectDelegate + 'static,
{
    let w = match width {
        DropWidth::Medium => 220.,
        DropWidth::Wide => 300.,
    };
    div().w(px(w)).max_w_full().flex_shrink_0().child(
        gpui_kit::component::select::Select::new(state)
            .h(control_h())
            .text_size(theme::secondary_size())
            .cursor_pointer(),
    )
}

/// What a [`button`] is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// The main action of a form or dialog: filled accent.
    Primary,
    /// "Change", "Test", "Reset": a subtle filled surface, a shade lighter under the pointer.
    Secondary,
    /// Delete, leave, log out: red text, filling red under the pointer.
    Danger,
}

/// A button with a label. Its colours spring under the pointer like every other control's.
#[allow(clippy::too_many_arguments)]
pub fn button(
    shell: &Shell,
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    enabled: bool,
    action: impl Fn(&mut Shell, &mut gpui_kit::Window, &mut Context<Shell>) + 'static,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let id: SharedString = id.into();
    let label: SharedString = label.into();
    let hover = enabled && hovered(shell, &id);
    let face = div()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h_full()
        .px(px(12.))
        .rounded(theme::radius_small())
        .whitespace_nowrap()
        // A light line along the top edge, inside.
        .shadow(vec![shadow(0.07, true, 1., 0., true)])
        .with_spring(
            SharedString::from(format!("{id}-face")),
            spring(if hover { 1. } else { 0. }),
            move |d, t| {
                let t = t.clamp(0., 1.);
                let (bg, text) = button_colors(kind, t);
                // An edge a shade lighter than the face, so it reads as a raised plate.
                d.bg(bg)
                    .border_1()
                    .border_color(theme::mix(bg, text, 0.14))
                    .text_color(text)
                    .child(label.clone())
            },
        );
    let el = div()
        .id(id.clone())
        .flex_shrink_0()
        .h(control_h())
        .rounded(theme::radius_small())
        .text_size(theme::secondary_size())
        .when(enabled, |d| {
            d.shadow(vec![shadow(0.28, false, 1., 2., false)])
                // Pressed, it sinks: the shadow under it goes and the face moves down a pixel.
                .active(|s| s.shadow_none().pt(px(1.)))
        })
        .child(face);
    pressable(el, shell, &id, enabled, Some(act(cx, action)), cx)
}

/// A black or white shadow `y` px down: outside a raised control, or inside it (`inset`) for
/// the light line along its top edge.
fn shadow(alpha: f32, white: bool, y: f32, blur: f32, inset: bool) -> gpui_kit::BoxShadow {
    gpui_kit::BoxShadow {
        color: gpui_kit::hsla(0., 0., if white { 1. } else { 0. }, alpha),
        offset: gpui_kit::point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset,
    }
}

/// A button's fill and text at hover `t` (0 at rest, 1 under the pointer).
fn button_colors(kind: ButtonKind, t: f32) -> (gpui_kit::Rgba, gpui_kit::Rgba) {
    match kind {
        ButtonKind::Primary => (
            theme::mix(theme::button_primary(), theme::button_primary_hover(), t),
            theme::button_primary_text(),
        ),
        ButtonKind::Secondary => (
            theme::mix(
                theme::button_secondary(),
                theme::button_secondary_hover(),
                t,
            ),
            theme::button_secondary_text(),
        ),
        ButtonKind::Danger => (
            theme::mix(
                theme::alpha(theme::button_danger(), 0.12),
                theme::button_danger(),
                t,
            ),
            theme::mix(theme::danger(), theme::button_danger_text(), t),
        ),
    }
}

/// An entry of a page's navigation column (settings tabs, server settings tabs). The caller
/// adds what a click does.
pub fn nav_item(
    shell: &Shell,
    glyph: &str,
    name: &str,
    selected: bool,
    id: String,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let hover = SharedString::from(format!("{id}-hover"));
    let hovered = shell.is_hovered(&hover);
    // The chosen tab's background flows out of the last one and into the new one.
    let pill = div()
        .absolute()
        .inset_0()
        .rounded(theme::row_radius())
        .with_spring(
            SharedString::from(format!("{id}-pill")),
            spring(if selected { 1. } else { 0. }),
            |d, t| {
                let t = t.clamp(0., 1.);
                // It grows from the left edge as it fills in.
                d.bg(theme::alpha(theme::selected(), t))
                    .right(gpui_kit::relative(0.3 * (1. - t)))
            },
        );
    // Under the pointer the text and icon turn accent; the chosen one keeps its own colours.
    let glyph = glyph.to_owned();
    let name = SharedString::from(name.to_owned());
    let label = div().flex().items_center().gap(px(8.)).with_spring(
        SharedString::from(format!("{id}-text")),
        spring(state(hovered, selected)),
        move |d, t| {
            let t = t.clamp(0., 2.);
            let hover = t.min(1.);
            let chosen = (t - 1.).max(0.);
            let text = theme::mix(theme::text_secondary(), theme::accent(), hover);
            let glyph_color = theme::mix(theme::text_tertiary(), theme::accent(), hover);
            d.text_color(theme::mix(text, theme::text(), chosen))
                .child(icon(
                    &glyph,
                    18.,
                    theme::mix(glyph_color, theme::text_tertiary(), chosen),
                ))
                .child(name.clone())
        },
    );
    div()
        .id(SharedString::from(id))
        .relative()
        .child(pill)
        .flex()
        .flex_shrink_0()
        .items_center()
        .h(px(34.))
        .px(px(8.))
        .rounded(theme::row_radius())
        .cursor_pointer()
        .text_size(theme::body_size())
        .on_hover(shell.track_hover(hover, cx))
        .child(label)
}

/// A row of a list (DMs, requests, server settings' roles, channels and members): its background
/// fades in under the pointer and fills in when selected, on one spring. The caller adds the
/// contents and what a click does; it may change the corner radius and text size.
pub fn list_row(
    shell: &Shell,
    id: impl Into<SharedString>,
    selected: bool,
    height: f32,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let id = id.into();
    let hover = SharedString::from(format!("{id}-hover"));
    let to = state(shell.is_hovered(&hover), selected);
    // Under the contents; the row's own corners clip it.
    let fill = div().absolute().inset_0().with_spring(
        SharedString::from(format!("{id}-fill")),
        spring(to),
        |d, t| {
            let t = t.clamp(0., 2.);
            let hover = t.min(1.);
            let chosen = (t - 1.).max(0.);
            d.bg(theme::mix(
                theme::alpha(theme::hover(), hover),
                theme::selected(),
                chosen,
            ))
        },
    );
    div()
        .id(id)
        .relative()
        .overflow_hidden()
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
        .on_hover(shell.track_hover(hover, cx))
        .child(fill)
}

/// An error's text, selectable like any text, with a copy button that shows a check mark for
/// a moment once used. The caller sets the colour and size around it.
pub fn error_text(
    shell: &Shell,
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    cx: &Context<Shell>,
) -> gpui_kit::Div {
    let id: SharedString = id.into();
    let text: SharedString = text.into();
    let button = format!("{id}-copy");
    let copied = shell.copied.as_deref() == Some(button.as_str());
    let glyph = if copied {
        crate::assets::CHECK
    } else {
        crate::assets::COPY
    };
    let tip = if copied { "Copied" } else { "Copy" };
    let copy_text = text.to_string();
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .min_w_0()
        .child(
            div().flex_1().min_w_0().child(
                gpui_kit::component::text::TextView::markdown(
                    SharedString::from(format!("{id}-text")),
                    text,
                )
                .selectable(true),
            ),
        )
        .child(
            tip_button(shell, SharedString::from(button.clone()), glyph, tip, cx).on_click(
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.copy_quietly(button.clone(), copy_text.clone(), cx);
                }),
            ),
        )
}

/// A fold's chevron (`glyph`, a down-pointing icon the caller styled): it points down while
/// `open` and turns to point right as it folds, on a spring. Server categories and settings
/// groups share it.
pub fn chevron(glyph: gpui_kit::Svg, id: String, open: bool) -> impl IntoElement + use<> {
    glyph.with_spring(
        SharedString::from(id),
        gpui_kit::SpringAnimation::new(theme::paced(gpui_kit::SpringConfig::new(420., 32., 1.)))
            .to(!open),
        |d, phase| {
            d.with_transformation(gpui_kit::Transformation::rotate(gpui_kit::radians(
                -std::f32::consts::FRAC_PI_2 * phase.0,
            )))
        },
    )
}

/// A small square button holding an icon. Under the pointer a soft background fades in behind
/// the icon and the icon turns accent. The caller adds what a click does.
pub fn icon_button(
    shell: &Shell,
    id: impl Into<gpui_kit::ElementId>,
    glyph: &'static str,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    icon_button_with(
        shell,
        id,
        move |color| icon(glyph, 18., color).into_any_element(),
        cx,
    )
}

/// [`icon_button`] whose icon the caller draws (turned, say), in the colour it is given.
pub fn icon_button_with(
    shell: &Shell,
    id: impl Into<gpui_kit::ElementId>,
    glyph: impl Fn(gpui_kit::Rgba) -> gpui_kit::AnyElement + 'static,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let id = id.into();
    let hover = SharedString::from(format!("{id:?}-hover"));
    let to = if shell.is_hovered(&hover) { 1. } else { 0. };
    // Its own layer, so a background the caller sets (a scrim over video) stays.
    let face = div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::radius_small())
        .with_spring(
            SharedString::from(format!("{id:?}-face")),
            spring(to),
            move |d, t| {
                let t = t.clamp(0., 1.);
                d.bg(theme::alpha(theme::hover(), t))
                    .child(glyph(theme::mix(
                        theme::text_secondary(),
                        theme::accent(),
                        t,
                    )))
            },
        );
    div()
        .id(id)
        .relative()
        .flex_shrink_0()
        .size(px(28.))
        .rounded(theme::radius_small())
        .cursor_pointer()
        .on_hover(shell.track_hover(hover, cx))
        .child(face)
}

/// [`icon_button`] that says what it does when hovered.
pub fn tip_button(
    shell: &Shell,
    id: impl Into<gpui_kit::ElementId>,
    glyph: &'static str,
    tip: &'static str,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    icon_button(shell, id, glyph, cx).tooltip(move |window, cx| {
        gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
    })
}

#[cfg(test)]
mod tests {
    use super::{coverage, thumb_span};

    const SPANS: [(f32, f32); 3] = [(0., 40.), (40., 60.), (100., 30.)];

    #[test]
    fn thumb_sits_on_whole_choices() {
        for (n, span) in SPANS.iter().enumerate() {
            assert_eq!(thumb_span(&SPANS, n as f32), Some(*span));
        }
    }

    #[test]
    fn thumb_blends_neighbours_on_the_way() {
        // Halfway from the first to the second: left edge and width both halfway.
        assert_eq!(thumb_span(&SPANS, 0.5), Some((20., 50.)));
        // A quarter of the way from the second to the third.
        assert_eq!(thumb_span(&SPANS, 1.25), Some((55., 52.5)));
    }

    #[test]
    fn thumb_overshoot_is_held_at_the_ends() {
        assert_eq!(thumb_span(&SPANS, -0.3), Some(SPANS[0]));
        assert_eq!(thumb_span(&SPANS, 2.4), Some(SPANS[2]));
        assert_eq!(thumb_span(&[], 0.), None);
        assert_eq!(thumb_span(&[(5., 10.)], 0.7), Some((5., 10.)));
    }

    #[test]
    fn labels_turn_as_the_thumb_covers_them() {
        let thumb = thumb_span(&SPANS, 0.5).unwrap(); // 20..70
        assert_eq!(coverage(SPANS[0], thumb), 0.5);
        assert_eq!(coverage(SPANS[1], thumb), 0.5);
        assert_eq!(coverage(SPANS[2], thumb), 0.);
        assert_eq!(coverage(SPANS[1], SPANS[1]), 1.);
        assert_eq!(coverage((0., 0.), SPANS[0]), 0.);
    }

    /// gpui's `.hover` keeps a stale state for a text colour whose hover style comes and goes
    /// (see the module docs): no text-colour `.hover` may sit inside a `.when(..)`.
    #[test]
    fn no_text_colour_hovers_that_come_and_go() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let lines: Vec<&str> = text.lines().collect();
                    let indent = |l: &str| l.len() - l.trim_start().len();
                    for (n, line) in lines.iter().enumerate() {
                        if !(line.contains(".hover(") && line.contains("text_color")) {
                            continue;
                        }
                        // On the same line as a `.when(`, or under one opened just above it.
                        let inside_when = line.contains(".when(")
                            || lines[n.saturating_sub(4)..n].iter().any(|above| {
                                above.contains(".when(")
                                    && above.trim_end().ends_with('{')
                                    && indent(above) < indent(line)
                            });
                        if inside_when {
                            found.push(format!("{}:{}", path.display(), n + 1));
                        }
                    }
                }
            }
        }
        assert!(
            found.is_empty(),
            "use Shell::track_hover instead: {found:#?}"
        );
    }
}
