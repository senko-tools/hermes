//! Fenced code blocks in messages. Each has a copy button in its top-right corner (#63). A long
//! one stops growing at about 20 lines and a wide one keeps its lines unwrapped; both then
//! scroll inside themselves (#65). The plain wheel over a block keeps scrolling the chat: the
//! block's own content moves only by its scrollbars (dragging, clicking the track, the wheel
//! over a bar) and by sideways scrolling, which the chat never uses.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{
    Bounds, DispatchPhase, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollDelta, ScrollEvent, Size, StyledText,
};

use super::*;
use crate::ui;

/// A block shows about this many lines before it scrolls...
const MAX_LINES: f32 = 20.;
/// ...or this share of the message list's height, whichever is smaller.
const MAX_SHARE: f32 = 0.4;
/// A capped block still shows at least this many lines.
const MIN_LINES: f32 = 3.;
/// The space around the code.
const PAD: f32 = 8.;
/// A scrollbar's width, which takes the mouse; the thumb drawn in it is thinner.
const BAR: f32 = 10.;
const THUMB: f32 = 4.;
/// The shortest a thumb gets.
const MIN_THUMB: f32 = 24.;
/// The pixels of one wheel line over a scrollbar, as the chat counts it.
const LINE: f32 = 20.;
/// How tall the buttons in the corner are, with their inset from the edges.
const HEADER: f32 = 32.;
/// The fade over the bottom edge of a block cut short.
const FADE: f32 = 24.;
/// How visible the corner buttons are while the block is not under the pointer.
const FAINT: f32 = 0.35;

/// A block: its message and its place among the message's blocks.
pub type CodeKey = (Id, usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

/// How a block was laid out when it was painted last.
#[derive(Debug, Clone, Copy, Default)]
struct Laid {
    /// The visible part.
    viewport: Bounds<Pixels>,
    /// The code itself, without the space around it.
    text: Size<Pixels>,
    line_height: f32,
    /// The first line reaches under the corner buttons, so they get a row of their own.
    header_row: bool,
}

/// What a block's scrollbars work from, noted while drawing it.
#[derive(Debug, Clone, Copy, Default)]
struct Geometry {
    view: Size<f32>,
    content: Size<f32>,
    capped: bool,
    wide: bool,
}

/// A press on a scrollbar: on its thumb (dragging it, `grab` pixels from the thumb's start), or
/// on its track (it jumps there when let go).
#[derive(Debug, Clone, Copy)]
struct Press {
    key: CodeKey,
    axis: Axis,
    grab: Option<f32>,
}

#[derive(Default)]
pub struct CodeBlocks {
    /// How far each block is scrolled, sideways and down.
    offsets: HashMap<CodeKey, (f32, f32)>,
    /// Blocks shown whole by their "Show all" button, until they leave the screen.
    expanded: HashSet<CodeKey>,
    laid: Rc<RefCell<HashMap<CodeKey, Laid>>>,
    geometry: RefCell<HashMap<CodeKey, Geometry>>,
    /// The blocks drawn since the last frame started.
    drawn: RefCell<HashSet<CodeKey>>,
    press: Option<Press>,
    /// The line height of the last block painted: a new block's height before it is painted.
    line_height: Rc<std::cell::Cell<f32>>,
}

/// A scrollbar's thumb on a `track` long track, for a `view` long window into `content`
/// scrolled `offset` along: where the thumb starts along the track, and how long it is.
fn thumb(track: f32, view: f32, content: f32, offset: f32) -> (f32, f32) {
    if content <= view || track <= 0. {
        return (0., track.max(0.));
    }
    let len = (track * view / content).clamp(MIN_THUMB.min(track), track);
    let at = (track - len) * (offset / (content - view)).clamp(0., 1.);
    (at, len)
}

/// The offset that puts a thumb held `grab` pixels from its start under the pointer, `pointer`
/// pixels along the track.
fn offset_for(track: f32, view: f32, content: f32, pointer: f32, grab: f32) -> f32 {
    let (_, len) = thumb(track, view, content, 0.);
    let room = track - len;
    if room <= 0. || content <= view {
        return 0.;
    }
    ((pointer - grab) / room).clamp(0., 1.) * (content - view)
}

/// A wheel turn's pixels, sideways and down (gpui's signs: positive moves the content right
/// and down, so it scrolls back).
fn wheel_pixels(event: &ScrollEvent) -> (f32, f32) {
    match event.delta {
        ScrollDelta::Pixels(p) => (p.x.as_f32(), p.y.as_f32()),
        ScrollDelta::Lines(l) => (l.x * LINE * theme::scale(), l.y * LINE * theme::scale()),
    }
}

impl CodeBlocks {
    /// At the start of each frame: a block that was not drawn last frame left the screen, so it
    /// forgets being shown whole and how it was laid out.
    pub fn advance(&mut self) {
        let drawn = std::mem::take(&mut *self.drawn.borrow_mut());
        self.expanded.retain(|key| drawn.contains(key));
        self.laid.borrow_mut().retain(|key, _| drawn.contains(key));
        self.geometry
            .borrow_mut()
            .retain(|key, _| drawn.contains(key));
    }

    /// A scrollbar's track: where it starts in the window and how long it is, the visible
    /// length and the content's length along it.
    fn track(&self, key: CodeKey, axis: Axis) -> Option<(f32, f32, f32, f32)> {
        let viewport = self.laid.borrow().get(&key)?.viewport;
        let g = *self.geometry.borrow().get(&key)?;
        let bar = BAR * theme::scale();
        Some(match axis {
            Axis::Vertical => (
                viewport.top().as_f32(),
                g.view.height - if g.wide { bar } else { 0. },
                g.view.height,
                g.content.height,
            ),
            Axis::Horizontal => (
                viewport.left().as_f32(),
                g.view.width - if g.capped { bar } else { 0. },
                g.view.width,
                g.content.width,
            ),
        })
    }

    /// Scrolls a block to `offset` along `axis`, kept within its content.
    fn set(&mut self, key: CodeKey, axis: Axis, offset: f32) -> bool {
        let Some(g) = self.geometry.borrow().get(&key).copied() else {
            return false;
        };
        let at = self.offsets.entry(key).or_default();
        let old = *at;
        match axis {
            Axis::Horizontal => at.0 = offset.clamp(0., (g.content.width - g.view.width).max(0.)),
            Axis::Vertical => at.1 = offset.clamp(0., (g.content.height - g.view.height).max(0.)),
        }
        *at != old
    }

    fn offset(&self, key: CodeKey, axis: Axis) -> f32 {
        let at = self.offsets.get(&key).copied().unwrap_or_default();
        match axis {
            Axis::Horizontal => at.0,
            Axis::Vertical => at.1,
        }
    }

    /// The offset a thumb dragged or a track clicked at `at` (in the window) asks for.
    fn offset_at(&self, press: Press, at: Point<Pixels>) -> Option<f32> {
        let (start, track, view, content) = self.track(press.key, press.axis)?;
        let along = match press.axis {
            Axis::Vertical => at.y.as_f32(),
            Axis::Horizontal => at.x.as_f32(),
        } - start;
        let grab = match press.grab {
            Some(grab) => grab,
            None => thumb(track, view, content, 0.).1 / 2.,
        };
        Some(offset_for(track, view, content, along, grab))
    }
}

impl Shell {
    /// A fenced code block of a message: `text` is its selectable code, `record` what notes
    /// where that is drawn.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_code_block(
        &self,
        code: &str,
        language: Option<String>,
        text: StyledText,
        record: AnyElement,
        message: Id,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key: CodeKey = (message, index);
        let blocks = &self.code_blocks;
        blocks.drawn.borrow_mut().insert(key);
        let laid = blocks.laid.borrow().get(&key).copied();
        let scale = theme::scale();
        let pad = PAD * scale;
        let bar = BAR * scale;
        let lines = code.split('\n').count().max(1) as f32;
        // Until the block has been painted once, the line height of the last block painted, or
        // gpui's usual one.
        let line_height = laid
            .map(|l| l.line_height)
            .filter(|h| *h > 0.)
            .or_else(|| Some(blocks.line_height.get()).filter(|h| *h > 0.))
            .unwrap_or(theme::secondary_size().as_f32() * 1.618);
        let text_size = laid.map(|l| l.text).filter(|s| s.height.as_f32() > 0.);
        let text_height = text_size.map_or(lines * line_height, |s| s.height.as_f32());
        let header_row = laid.is_some_and(|l| l.header_row);
        let top = if header_row { HEADER * scale } else { pad };
        let content = Size {
            width: text_size.map_or(0., |s| s.width.as_f32()) + 2. * pad,
            height: top + text_height + pad,
        };
        let mut cap = MAX_LINES * line_height;
        if let Some(list) = self.selection.list.get() {
            cap = cap.min(list.size.height.as_f32() * MAX_SHARE);
        }
        let cap = top + cap.max(MIN_LINES * line_height) + pad;
        let long = content.height > cap + 0.5;
        let expanded = blocks.expanded.contains(&key);
        let capped = long && !expanded;
        let view_width = laid.map_or(0., |l| l.viewport.size.width.as_f32());
        let view = Size {
            width: view_width,
            height: if capped { cap } else { content.height },
        };
        let wide = view_width > 0. && content.width > view_width + 0.5;
        blocks.geometry.borrow_mut().insert(
            key,
            Geometry {
                view,
                content,
                capped,
                wide,
            },
        );
        let max_x = if wide { content.width - view.width } else { 0. };
        let max_y = if capped {
            content.height - view.height
        } else {
            0.
        };
        let x = blocks.offset(key, Axis::Horizontal).clamp(0., max_x);
        let y = blocks.offset(key, Axis::Vertical).clamp(0., max_y);

        let id = format!("code-{message}-{index}");
        let hover = SharedString::from(format!("{id}-hover"));
        let copy_id = SharedString::from(format!("{id}-copy"));
        let expand_id = SharedString::from(format!("{id}-expand"));
        let button_hover =
            |id: &SharedString| format!("{:?}-hover", gpui_kit::ElementId::from(id.clone()));
        let copied = self.copied.as_deref() == Some(copy_id.as_ref());
        let shown = self.is_hovered(&hover)
            || self.is_hovered(&button_hover(&copy_id))
            || self.is_hovered(&button_hover(&expand_id))
            || copied
            || blocks.press.is_some_and(|p| p.key == key);

        // The corner: the language, "Show all" for a long block, and the copy button.
        let caption = language.filter(|l| !l.is_empty());
        let header_width = (4.
            + 28.
            + if long { 30. } else { 0. }
            + caption
                .as_ref()
                .map_or(0., |c| c.chars().count() as f32 * 7. + 8.))
            * scale;
        let copy_text = code.to_owned();
        let copy_button = copy_id.to_string();
        let corner = div()
            .absolute()
            .top(px(2.))
            .right(px(2.))
            .flex()
            .items_center()
            .gap(px(2.))
            .with_spring(
                SharedString::from(format!("{id}-corner")),
                gpui_kit::SpringAnimation::new(theme::paced(crate::hover::HOVER_SPRING))
                    .to(if shown { 1. } else { FAINT }),
                |d, t| d.opacity(t.clamp(0., 1.)),
            )
            .children(caption.map(|language| {
                div()
                    .px(px(4.))
                    .font_family(theme::font())
                    .text_size(theme::caption_size())
                    .text_color(theme::text_tertiary())
                    .child(language)
            }))
            .when(long, |d| {
                d.child(
                    ui::icon_button_with(
                        self,
                        expand_id.clone(),
                        move |color| {
                            icon(assets::CHEVRON_DOWN, 18., color)
                                .when(expanded, |i| {
                                    i.with_transformation(gpui_kit::Transformation::rotate(
                                        gpui_kit::radians(std::f32::consts::PI),
                                    ))
                                })
                                .into_any_element()
                        },
                        cx,
                    )
                    .occlude()
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(if expanded {
                            "Show less"
                        } else {
                            "Show all"
                        })
                        .build(window, cx)
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let blocks = &mut this.code_blocks;
                        if !blocks.expanded.remove(&key) {
                            blocks.expanded.insert(key);
                        }
                        cx.notify();
                    })),
                )
            })
            .child(
                ui::tip_button(
                    self,
                    copy_id.clone(),
                    if copied { assets::CHECK } else { assets::COPY },
                    if copied { "Copied" } else { "Copy" },
                    cx,
                )
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.copy_quietly(copy_button.clone(), copy_text.clone(), cx);
                })),
            );

        // Paints note the block's size, and ask for one more frame when it changed, so the cap
        // and the scrollbars follow at once.
        let laid_view = blocks.laid.clone();
        let viewport_recorder = canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                let mut laid = laid_view.borrow_mut();
                let entry = laid.entry(key).or_default();
                let changed = entry.viewport.size != bounds.size;
                entry.viewport = bounds;
                if changed {
                    window.request_animation_frame();
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let layout = text.layout().clone();
        let laid_text = blocks.laid.clone();
        let last_line_height = blocks.line_height.clone();
        let text_recorder = canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                // Every line on one row: the widest line's width, a line height per line. (The
                // text's own box may be narrower than its lines.)
                let lines = layout.line_layouts();
                let first = lines
                    .first()
                    .map_or(0., |l| l.unwrapped_layout.width.as_f32());
                let widest = lines
                    .iter()
                    .map(|l| l.unwrapped_layout.width)
                    .max()
                    .unwrap_or_default();
                let height = layout.line_height().as_f32();
                let size = Size {
                    width: widest,
                    height: gpui_kit::px(height * lines.len().max(1) as f32),
                };
                let mut laid = laid_text.borrow_mut();
                let entry = laid.entry(key).or_default();
                let row = first + 2. * pad + header_width > entry.viewport.size.width.as_f32();
                last_line_height.set(height);
                let changed =
                    entry.text != size || entry.line_height != height || entry.header_row != row;
                entry.text = size;
                entry.line_height = height;
                entry.header_row = row;
                if changed {
                    window.request_animation_frame();
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let viewport = div()
            .relative()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .h(gpui_kit::px(view.height))
            .child(viewport_recorder)
            .child(
                // Out of the flow, so its unwrapped lines never widen the chat: the block's
                // height is worked out from its lines instead.
                div()
                    .absolute()
                    .top(gpui_kit::px(-y))
                    .left(gpui_kit::px(-x))
                    .pt(gpui_kit::px(top))
                    .pb(gpui_kit::px(pad))
                    .px(gpui_kit::px(pad))
                    .whitespace_nowrap()
                    .child(div().relative().child(text).child(text_recorder)),
            )
            .child(record);

        let thumb_color = theme::alpha(theme::text_tertiary(), if shown { 0.9 } else { 0.5 });
        let scrollbar = |axis: Axis| {
            let (track, view_len, content_len, offset) = match axis {
                Axis::Vertical => (
                    view.height - if wide { bar } else { 0. },
                    view.height,
                    content.height,
                    y,
                ),
                Axis::Horizontal => (
                    view.width - if capped { bar } else { 0. },
                    view.width,
                    content.width,
                    x,
                ),
            };
            let (at, len) = thumb(track, view_len, content_len, offset);
            let thumb_el = div()
                .absolute()
                .rounded_full()
                .bg(thumb_color)
                .map(|d| match axis {
                    Axis::Vertical => d
                        .top(gpui_kit::px(at))
                        .h(gpui_kit::px(len))
                        .right(px((BAR - THUMB) / 2.))
                        .w(px(THUMB)),
                    Axis::Horizontal => d
                        .left(gpui_kit::px(at))
                        .w(gpui_kit::px(len))
                        .bottom(px((BAR - THUMB) / 2.))
                        .h(px(THUMB)),
                });
            let name = match axis {
                Axis::Vertical => "v",
                Axis::Horizontal => "h",
            };
            div()
                .id(SharedString::from(format!("{id}-{name}bar")))
                .absolute()
                .map(|d| match axis {
                    Axis::Vertical => d.top_0().right_0().w(px(BAR)).h(gpui_kit::px(track)),
                    Axis::Horizontal => d.bottom_0().left_0().h(px(BAR)).w(gpui_kit::px(track)),
                })
                .occlude()
                .cursor_default()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        let blocks = &mut this.code_blocks;
                        let Some((start, track, view_len, content_len)) = blocks.track(key, axis)
                        else {
                            return;
                        };
                        let along = match axis {
                            Axis::Vertical => event.position.y.as_f32(),
                            Axis::Horizontal => event.position.x.as_f32(),
                        } - start;
                        let offset = blocks.offset(key, axis);
                        let (at, len) = thumb(track, view_len, content_len, offset);
                        let on_thumb = (at..=at + len).contains(&along);
                        blocks.press = Some(Press {
                            key,
                            axis,
                            grab: on_thumb.then_some(along - at),
                        });
                        cx.notify();
                    }),
                )
                .on_scroll(cx.listener(move |this, event: &ScrollEvent, _, cx| {
                    cx.stop_propagation();
                    let (dx, dy) = wheel_pixels(event);
                    let delta = match axis {
                        Axis::Vertical => dy,
                        Axis::Horizontal if dx != 0. => dx,
                        Axis::Horizontal => dy,
                    };
                    let blocks = &mut this.code_blocks;
                    let offset = blocks.offset(key, axis) - delta;
                    if blocks.set(key, axis, offset) {
                        cx.notify();
                    }
                }))
                .child(thumb_el)
        };

        let block = div()
            .id(SharedString::from(id.clone()))
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .rounded(theme::radius_small())
            .bg(theme::code_background())
            .border_1()
            .border_color(theme::border())
            .font_family(theme::mono_font())
            .text_size(theme::secondary_size())
            .on_hover(self.track_hover(hover, cx))
            // Sideways scrolling (Shift+wheel, a touchpad swipe) moves a wide block; the chat
            // only scrolls up and down, so it loses nothing. The event goes on to the chat.
            .when(wide, |d| {
                d.on_scroll(cx.listener(move |this, event: &ScrollEvent, _, cx| {
                    let (dx, _) = wheel_pixels(event);
                    if dx == 0. {
                        return;
                    }
                    let blocks = &mut this.code_blocks;
                    let offset = blocks.offset(key, Axis::Horizontal) - dx;
                    if blocks.set(key, Axis::Horizontal, offset) {
                        cx.notify();
                    }
                }))
            })
            .child(viewport)
            .when(capped && y < max_y - 0.5, |d| {
                let background = theme::code_background();
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(FADE))
                        .bg(gpui_kit::linear_gradient(
                            180.,
                            gpui_kit::linear_color_stop(theme::alpha(background, 0.), 0.),
                            gpui_kit::linear_color_stop(background, 1.),
                        )),
                )
            })
            .when(capped, |d| d.child(scrollbar(Axis::Vertical)))
            .when(wide, |d| d.child(scrollbar(Axis::Horizontal)))
            .child(corner);
        block.into_any_element()
    }

    /// While a code block's scrollbar is held: follows the mouse in the whole window. It goes
    /// in the root.
    pub fn code_scroll_catcher(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.code_blocks.press?;
        let this = cx.entity().downgrade();
        Some(
            canvas(
                |_, _, _| (),
                move |_, (), window, _| {
                    let moved = this.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        let _ = moved.update(cx, |this, cx| {
                            let blocks = &mut this.code_blocks;
                            let Some(press) = blocks.press else { return };
                            if press.grab.is_none() {
                                return;
                            }
                            if event.pressed_button != Some(MouseButton::Left) {
                                blocks.press = None;
                                cx.notify();
                                return;
                            }
                            if let Some(offset) = blocks.offset_at(press, event.position)
                                && blocks.set(press.key, press.axis, offset)
                            {
                                cx.notify();
                            }
                        });
                    });
                    let released = this.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                            return;
                        }
                        cx.stop_propagation();
                        let _ = released.update(cx, |this, cx| {
                            let blocks = &mut this.code_blocks;
                            let Some(press) = blocks.press.take() else {
                                return;
                            };
                            // A click on the track brings the thumb there, when it is let go.
                            if press.grab.is_none()
                                && let Some(offset) = blocks.offset_at(press, event.position)
                            {
                                blocks.set(press.key, press.axis, offset);
                            }
                            cx.notify();
                        });
                    });
                },
            )
            .absolute()
            .size_0()
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thumb_shows_how_much_is_visible_and_where() {
        // A quarter visible: a quarter of the track.
        assert_eq!(thumb(200., 100., 400., 0.), (0., 50.));
        // Scrolled all the way: at the end of the track.
        assert_eq!(thumb(200., 100., 400., 300.), (150., 50.));
        // Halfway.
        assert_eq!(thumb(200., 100., 400., 150.), (75., 50.));
        // Past the ends, it stays on the track.
        assert_eq!(thumb(200., 100., 400., 900.), (150., 50.));
        assert_eq!(thumb(200., 100., 400., -5.), (0., 50.));
        // Everything visible: the whole track.
        assert_eq!(thumb(200., 100., 80., 0.), (0., 200.));
    }

    #[test]
    fn a_thumb_never_gets_too_short_to_grab() {
        let (_, len) = thumb(200., 100., 100_000., 0.);
        assert_eq!(len, MIN_THUMB);
        let (at, _) = thumb(200., 100., 100_000., 99_900.);
        assert_eq!(at, 200. - MIN_THUMB);
    }

    #[test]
    fn dragging_a_thumb_scrolls_in_step_with_it() {
        // Held 10 px into the thumb: the thumb's start follows the pointer.
        assert_eq!(offset_for(200., 100., 400., 10., 10.), 0.);
        assert_eq!(offset_for(200., 100., 400., 85., 10.), 150.);
        assert_eq!(offset_for(200., 100., 400., 160., 10.), 300.);
        // Past the ends it stops there.
        assert_eq!(offset_for(200., 100., 400., 500., 10.), 300.);
        assert_eq!(offset_for(200., 100., 400., -50., 10.), 0.);
        // Nothing to scroll.
        assert_eq!(offset_for(200., 100., 80., 50., 10.), 0.);
    }

    #[test]
    fn the_thumb_lands_where_the_offset_puts_it() {
        for offset in [0., 37., 150., 300.] {
            let (at, _) = thumb(200., 100., 400., offset);
            let back = offset_for(200., 100., 400., at + 5., 5.);
            assert!((back - offset).abs() < 0.01, "{offset} -> {back}");
        }
    }
}
