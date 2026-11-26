//! A setting's description too long for its line (#73): it shows its start with "..." at the
//! end, and while the pointer rests on it, it scrolls to its end after a delay. It waits there,
//! fades back to the start, and goes round again until the pointer leaves.
//!
//! The rows are drawn by free functions without the shell, so the hovered description, the
//! measured overflows and the delay live here. Only a moving or fading description asks for
//! frames; the waits ask for one redraw when they end, so a resting page draws nothing.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, SharedString, canvas, div};

use crate::theme::{self, px};

/// How fast the text moves, in pixels a second: slow enough to read along.
const SPEED: f32 = 60.;
/// A very long description moves faster, so it reaches its end within this many seconds.
const MOST_MOVING_SECS: f32 = 8.;
/// How far past "the end just shows" it goes, so the last word does not touch the edge.
const PAST_END: f32 = 24.;
/// How long it stays at the end.
const HOLD: Duration = Duration::from_millis(3000);
/// How long the fade out at the end takes, and again the fade in at the start.
const FADE: Duration = Duration::from_millis(200);
/// Room the "..." over the right edge takes; the text is only "all shown" past it.
const DOTS_W: f32 = 14.;

thread_local! {
    /// The description under the pointer and since when.
    static HOVERED: RefCell<Option<(SharedString, Instant)>> = const { RefCell::new(None) };
    /// How far each description runs past its line, measured as it was last drawn.
    static OVERFLOW: RefCell<HashMap<SharedString, f32>> = RefCell::new(HashMap::new());
    /// How long the pointer rests before it moves (the setting).
    static DELAY: Cell<Duration> = const { Cell::new(Duration::from_millis(500)) };
    /// The redraw already asked for at the end of a wait.
    static WAKE: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Sets how long the pointer rests on a description before it moves.
pub fn set_delay(ms: u32) {
    DELAY.with(|d| d.set(Duration::from_millis(u64::from(ms))));
}

/// Where a scrolling description is at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Frame {
    /// How far the text has moved left.
    pub x: f32,
    pub opacity: f32,
    /// The "..." still shows over the right edge.
    pub dots: bool,
    /// How long until it next looks different: a moving one at its next whole pixel (not every
    /// refresh of the screen, #60); `None` while it fades, which is short.
    pub still_for: Option<Duration>,
}

/// Where a description that runs `overflow` px past its line is, `elapsed` after the pointer
/// came onto it: wait `delay`, move, hold, fade out and in at the start, again and again.
pub(super) fn frame(elapsed: Duration, delay: Duration, overflow: f32) -> Frame {
    let end = overflow + PAST_END;
    let speed = SPEED.max(end / MOST_MOVING_SECS);
    let moving = Duration::from_secs_f32(end / speed);
    let round = delay + moving + HOLD + FADE * 2;
    let t = Duration::from_nanos((elapsed.as_nanos() % round.as_nanos().max(1)) as u64);
    let at_start = |still_for| Frame {
        x: 0.,
        opacity: 1.,
        dots: true,
        still_for,
    };
    if t < delay {
        return at_start(Some(delay - t));
    }
    let t = t - delay;
    if t < moving {
        let x = (t.as_secs_f32() * speed).min(end);
        return Frame {
            x,
            opacity: 1.,
            dots: x < overflow + DOTS_W,
            still_for: Some(Duration::from_secs_f32(1. / speed).min(moving - t)),
        };
    }
    let t = t - moving;
    if t < HOLD {
        return Frame {
            x: end,
            opacity: 1.,
            dots: false,
            still_for: Some(HOLD - t),
        };
    }
    let t = t - HOLD;
    let fade = FADE.as_secs_f32();
    if t < FADE {
        return Frame {
            x: end,
            opacity: 1. - t.as_secs_f32() / fade,
            dots: false,
            still_for: None,
        };
    }
    let t = t - FADE;
    Frame {
        x: 0.,
        opacity: (t.as_secs_f32() / fade).min(1.),
        dots: true,
        still_for: None,
    }
}

/// The description line of the setting `name`.
pub(super) fn hint_line(name: &str, hint: SharedString) -> AnyElement {
    let key = SharedString::from(name.to_owned());
    let overflow = OVERFLOW.with(|o| o.borrow().get(&key).copied().unwrap_or(0.));
    let since = HOVERED.with(|h| {
        h.borrow()
            .as_ref()
            .and_then(|(k, at)| (*k == key).then_some(*at))
    });
    let delay = DELAY.with(Cell::get);
    let now = (since.is_some() && overflow > 0.5).then(|| {
        frame(
            since.map_or(Duration::ZERO, |at| at.elapsed()),
            delay,
            overflow,
        )
    });
    // Measures how far the text runs past the line, and keeps a moving one going.
    let measured = {
        let (key, text) = (key.clone(), hint.clone());
        canvas(
            move |bounds, window, cx| {
                let style = window.text_style();
                let size = style.font_size.to_pixels(window.rem_size());
                let run = style.to_run(text.len());
                let width = window
                    .text_system()
                    .shape_line(text.clone(), size, &[run], None)
                    .width;
                let over = f32::from(width - bounds.size.width).max(0.);
                let changed = OVERFLOW.with(|o| {
                    let mut o = o.borrow_mut();
                    let before = o.insert(key.clone(), over);
                    before.is_none_or(|b| (b - over).abs() > 0.5)
                });
                // The pointer came on before it was measured: start once it is.
                if changed && since.is_some() {
                    window.refresh();
                }
                match now.map(|f| f.still_for) {
                    Some(None) => window.request_animation_frame(),
                    Some(Some(wait)) => {
                        let at = Instant::now() + wait;
                        let asked = WAKE.with(|w| {
                            w.get().is_some_and(|w| {
                                w <= at + Duration::from_millis(5) && w > Instant::now()
                            })
                        });
                        if !asked {
                            WAKE.with(|w| w.set(Some(at)));
                            window
                                .spawn(cx, async move |cx| {
                                    cx.background_executor().timer(wait).await;
                                    let _ = cx.update(|window, _| window.refresh());
                                })
                                .detach();
                        }
                    }
                    None => {}
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    };
    let hover_key = key.clone();
    let line = div()
        .id(SharedString::from(format!("hint-{name}")))
        .relative()
        .w_full()
        .overflow_hidden()
        .text_size(theme::secondary_size())
        .text_color(theme::text_tertiary())
        .on_hover(move |hovered, window, _| {
            let key = hover_key.clone();
            HOVERED.with(|h| {
                let mut h = h.borrow_mut();
                if *hovered {
                    *h = Some((key, Instant::now()));
                } else if h.as_ref().is_some_and(|(k, _)| *k == key) {
                    *h = None;
                }
            });
            window.refresh();
        })
        .child(measured);
    match now {
        // Moving, at the end, or fading.
        Some(f) if f.x > 0. || f.opacity < 1. => line
            .flex()
            .child(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .relative()
                    .left(px(-f.x))
                    .opacity(f.opacity)
                    .child(hint),
            )
            .when(f.dots, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .pl(px(4.))
                        .bg(theme::sidebar())
                        .opacity(f.opacity)
                        .child("\u{2026}"),
                )
            }),
        // At rest or waiting: the start with the line's own "...".
        _ => line.child(div().truncate().child(hint)),
    }
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELAY: Duration = Duration::from_millis(500);

    #[test]
    fn waits_then_moves_then_holds_at_the_end() {
        let start = frame(Duration::from_millis(100), DELAY, 100.);
        assert_eq!(start.x, 0.);
        assert!(start.dots);
        assert_eq!(start.still_for, Some(Duration::from_millis(400)));

        let moving = frame(Duration::from_millis(1500), DELAY, 100.);
        assert!((moving.x - SPEED).abs() < 0.01);
        assert!(moving.dots);
        // Drawn again a pixel later, not on every refresh.
        let step = moving.still_for.unwrap();
        assert!(step <= Duration::from_secs_f32(1. / SPEED) && step > Duration::ZERO);

        // (100 + 24) / 45 s of moving after the delay: then it holds past the end, no dots.
        let held = frame(DELAY + Duration::from_secs(3), DELAY, 100.);
        assert_eq!(held.x, 100. + PAST_END);
        assert!(!held.dots);
        assert!(held.still_for.is_some());
    }

    #[test]
    fn dots_stay_until_the_end_shows() {
        let overflow = 100.;
        let just_before = Duration::from_secs_f32((overflow - 1.) / SPEED);
        assert!(frame(DELAY + just_before, DELAY, overflow).dots);
        let past = Duration::from_secs_f32((overflow + DOTS_W + 1.) / SPEED);
        assert!(!frame(DELAY + past, DELAY, overflow).dots);
    }

    #[test]
    fn fades_back_and_goes_round_again() {
        let moving = Duration::from_secs_f32((100. + PAST_END) / SPEED);
        let fading = DELAY + moving + HOLD + FADE / 2;
        let f = frame(fading, DELAY, 100.);
        assert!(f.opacity < 1. && f.opacity > 0.);
        let back = frame(DELAY + moving + HOLD + FADE * 2, DELAY, 100.);
        assert_eq!((back.x, back.opacity), (0., 1.));
        assert!(back.still_for.is_some(), "waits the delay again");
    }

    #[test]
    fn a_very_long_one_moves_faster() {
        // 1000 px at the plain speed would take over 16 s.
        let f = frame(
            DELAY + Duration::from_secs_f32(MOST_MOVING_SECS),
            DELAY,
            1000.,
        );
        assert_eq!(f.x, 1000. + PAST_END);
        assert!(!f.dots);
    }

    #[test]
    fn no_delay_moves_at_once() {
        let f = frame(Duration::from_millis(100), Duration::ZERO, 100.);
        assert!(f.x > 0.);
    }
}
