//! Smooth scrolling for the mouse wheel. A wheel notch jumps a whole step at once; this catches
//! the notches before any list sees them and plays each back as small pixel scrolls, one per
//! frame, easing out. Touchpads already scroll in pixels and are left alone.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::{
    App, DispatchPhase, Modifiers, PlatformInput, Point, ScrollDelta, ScrollEvent, ScrollSource,
    TouchPhase, Window, canvas, point, px,
};

/// How much further a notch goes than it did before, until the setting is read.
const SPEED: f32 = 1.25;
/// The pixels of one wheel line, as gpui's lists count it (the chat is one).
const LINE: f32 = 20.;
/// How quickly the rest of a scroll is played back: after this many seconds about 63 % of it.
const TAU: f32 = 0.045;
/// Less than this many pixels left: the rest goes at once.
const DONE: f32 = 0.5;

#[derive(Default)]
struct State {
    /// The pixels still to scroll.
    left: Point<f32>,
    /// When the last frame scrolled; `None` while nothing plays.
    last: Option<Instant>,
    modifiers: Modifiers,
    /// The wheel is left to the lists themselves.
    off: bool,
    /// How much further a notch goes; 0 until configured.
    speed: f32,
}

#[derive(Clone, Default)]
pub struct SmoothScroll(Rc<RefCell<State>>);

/// The part of `left` scrolled in a frame `dt` seconds long.
fn step(left: f32, dt: f32) -> f32 {
    if left.abs() < DONE {
        return left;
    }
    left * (1. - (-dt / TAU).exp())
}

/// A new notch `add` on top of what is still left: the same way adds up, the other way starts
/// over.
fn merge(left: f32, add: f32) -> f32 {
    if add == 0. {
        left
    } else if left.signum() == add.signum() {
        left + add
    } else {
        add
    }
}

impl SmoothScroll {
    /// Follows the settings.
    pub fn configure(&self, on: bool, speed: f32) {
        let mut s = self.0.borrow_mut();
        s.off = !on;
        s.speed = speed;
    }

    /// An empty element that catches the wheel for the whole window. It goes in the root.
    pub fn catcher(&self) -> impl IntoElement + use<> {
        let state = self.0.clone();
        canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture
                        || event.source != ScrollSource::Pointer
                        || event.modifiers.control
                        || cx.reduce_motion()
                        || state.borrow().off
                    {
                        return;
                    }
                    let ScrollDelta::Lines(lines) = event.delta else {
                        return;
                    };
                    cx.stop_propagation();
                    let speed = state.borrow().speed;
                    let line = LINE * if speed > 0. { speed } else { SPEED };
                    let start = {
                        let mut s = state.borrow_mut();
                        s.left.x = merge(s.left.x, lines.x * line);
                        s.left.y = merge(s.left.y, lines.y * line);
                        s.modifiers = event.modifiers;
                        let start = s.last.is_none();
                        if start {
                            s.last = Some(Instant::now());
                        }
                        start
                    };
                    if start {
                        play(state.clone(), window);
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }
}

/// Scrolls one frame's part after the next frame.
fn play(state: Rc<RefCell<State>>, window: &mut Window) {
    window.on_next_frame(move |window, cx| tick(state, window, cx));
}

fn tick(state: Rc<RefCell<State>>, window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    let (moved, modifiers, more) = {
        let mut s = state.borrow_mut();
        let dt = s
            .last
            .map_or(0.016, |last| now.duration_since(last).as_secs_f32())
            .clamp(0.001, 0.05);
        let moved = point(step(s.left.x, dt), step(s.left.y, dt));
        s.left.x -= moved.x;
        s.left.y -= moved.y;
        let more = s.left.x != 0. || s.left.y != 0.;
        s.last = more.then_some(now);
        (moved, s.modifiers, more)
    };
    if moved.x != 0. || moved.y != 0. {
        window.dispatch_event(
            PlatformInput::Scroll(ScrollEvent {
                position: window.mouse_position(),
                delta: ScrollDelta::Pixels(point(px(moved.x), px(moved.y))),
                modifiers,
                touch_phase: TouchPhase::Moved,
                source: ScrollSource::Pointer,
            }),
            cx,
        );
    }
    if more {
        play(state, window);
    }
}

/// How quickly a glide to the end plays out: after this many seconds about 63 % of the way.
const GLIDE_TAU: f32 = 0.07;

/// Glides `list` down to its end instead of jumping there (sending, "Jump to present"). A long
/// way first jumps to a screen and a half above the end, so the glide stays short.
pub fn glide_to_end(list: gpui_kit::ListState, window: &mut Window, cx: &App) {
    let left = remaining(&list);
    let screen = list.viewport_bounds().size.height.as_f32();
    if cx.reduce_motion() || left <= DONE {
        list.scroll_to_end();
        return;
    }
    if screen > 0. && left > screen * 3. {
        list.scroll_by(px(left - screen * 1.5));
    }
    let last = Instant::now();
    window.on_next_frame(move |window, cx| glide(list, last, window, cx));
}

/// The pixels between where `list` is and its end.
fn remaining(list: &gpui_kit::ListState) -> f32 {
    let max = list.max_offset_for_scrollbar().y.as_f32();
    let at = -list.scroll_px_offset_for_scrollbar().y.as_f32();
    (max - at).max(0.)
}

fn glide(list: gpui_kit::ListState, last: Instant, window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    let dt = now.duration_since(last).as_secs_f32().clamp(0.001, 0.05);
    let left = remaining(&list);
    if left <= DONE {
        list.scroll_to_end();
        window.refresh();
        return;
    }
    let moved = (left * (1. - (-dt / GLIDE_TAU).exp())).max(1.).min(left);
    list.scroll_by(px(moved));
    window.refresh();
    let _ = cx;
    window.on_next_frame(move |window, cx| glide(list, now, window, cx));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scroll_mostly_plays_out_in_a_tenth_of_a_second() {
        let mut left: f32 = 75.;
        let mut frames = 0;
        while left != 0. {
            left -= step(left, 1. / 60.);
            frames += 1;
            if frames == 7 {
                assert!(left.abs() < 7.5, "{left} px left after 7 frames");
            }
        }
        assert!(frames <= 20, "{frames} frames");
    }

    #[test]
    fn notches_the_same_way_add_up() {
        assert_eq!(merge(30., 75.), 105.);
        assert_eq!(merge(-30., -75.), -105.);
    }

    #[test]
    fn a_notch_the_other_way_starts_over() {
        assert_eq!(merge(30., -75.), -75.);
        assert_eq!(merge(30., 0.), 30.);
    }
}
