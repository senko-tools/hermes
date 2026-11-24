//! Client-side window decorations, for desktops that leave them to the app: GNOME on Wayland,
//! Weston, and other compositors without `xdg-decoration`. There gpui opens a bare window, and
//! Hermes draws what Windows and KDE draw for it: a title bar showing the page (as the native
//! one does), minimize / maximize / close, a thin border with a shadow, and edges to resize by.
//!
//! Everywhere else (`Decorations::Server`) this adds nothing.

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, BoxShadow, CursorStyle, Decorations, HitboxBehavior, MouseButton, Pixels,
    Point, ResizeEdge, Tiling, Window, canvas, div, point, px, size,
};

use crate::assets;
use crate::theme;

/// The invisible margin around the window that holds the shadow and the resize edges.
const SHADOW: Pixels = px(10.);
const RADIUS: Pixels = px(8.);
/// The title bar's height (at UI scale 1).
pub const BAR_HEIGHT: f32 = 32.;

/// `content` in a frame with a title bar, when the desktop leaves decorations to the app.
/// `on_close` runs for the close button, as the native one would ask the window.
pub fn frame(
    content: AnyElement,
    title: &str,
    on_close: impl Fn(&mut Window, &mut gpui_kit::App) + 'static,
    window: &mut Window,
) -> AnyElement {
    let Decorations::Client { tiling } = window.window_decorations() else {
        window.set_client_inset(px(0.));
        return content;
    };
    window.set_client_inset(SHADOW);
    let maximized = window.is_maximized();
    let inner = div()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .cursor(CursorStyle::Arrow)
        .bg(theme::background())
        .border_color(theme::border())
        .when(!tiling.top, |d| d.border_t_1())
        .when(!tiling.bottom, |d| d.border_b_1())
        .when(!tiling.left, |d| d.border_l_1())
        .when(!tiling.right, |d| d.border_r_1())
        .when(!(tiling.top || tiling.left), |d| d.rounded_tl(RADIUS))
        .when(!(tiling.top || tiling.right), |d| d.rounded_tr(RADIUS))
        .when(!(tiling.bottom || tiling.left), |d| d.rounded_bl(RADIUS))
        .when(!(tiling.bottom || tiling.right), |d| d.rounded_br(RADIUS))
        .when(!tiling.is_tiled(), |d| {
            d.shadow(vec![BoxShadow {
                color: gpui_kit::black().opacity(0.35),
                offset: point(px(0.), px(2.)),
                blur_radius: SHADOW / 2.,
                spread_radius: px(0.),
                inset: false,
            }])
        })
        // The content's own moves never reach the resize edges' cursor below.
        .on_mouse_move(|_, _, cx| cx.stop_propagation())
        .child(title_bar(title, maximized, on_close, window))
        .child(div().flex_1().min_h_0().child(content));
    div()
        .id("window-frame")
        .size_full()
        .bg(gpui_kit::transparent_black())
        .when(!tiling.top, |d| d.pt(SHADOW))
        .when(!tiling.bottom, |d| d.pb(SHADOW))
        .when(!tiling.left, |d| d.pl(SHADOW))
        .when(!tiling.right, |d| d.pr(SHADOW))
        .on_mouse_move(|_, window, _| window.refresh())
        .on_mouse_down(MouseButton::Left, move |event, window, _| {
            let size = window.window_bounds().get_bounds().size;
            if let Some(edge) = resize_edge(event.position, size, tiling) {
                window.start_window_resize(edge);
            }
        })
        .child(inner)
        // The resize cursors over the margin.
        .child(
            canvas(
                |_, window, _| {
                    let size = window.window_bounds().get_bounds().size;
                    window.insert_hitbox(
                        Bounds::new(point(px(0.), px(0.)), size),
                        HitboxBehavior::Normal,
                    )
                },
                move |_, hitbox, window, _| {
                    let size = window.window_bounds().get_bounds().size;
                    let Some(edge) = resize_edge(window.mouse_position(), size, tiling) else {
                        return;
                    };
                    let cursor = match edge {
                        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
                        ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                            CursorStyle::ResizeUpLeftDownRight
                        }
                        ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                            CursorStyle::ResizeUpRightDownLeft
                        }
                    };
                    window.set_cursor_style(cursor, &hitbox);
                },
            )
            .size_full()
            .absolute()
            .top_0()
            .left_0(),
        )
        .into_any_element()
}

/// The bar: the page's name, and the window's buttons. Dragging it moves the window, a double
/// click maximizes, and a right click opens the desktop's window menu.
fn title_bar(
    title: &str,
    maximized: bool,
    on_close: impl Fn(&mut Window, &mut gpui_kit::App) + 'static,
    window: &Window,
) -> impl IntoElement {
    let controls = window.window_controls();
    let button = |id: &'static str, icon: &'static str, close: bool| {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(theme::px(40.))
            .h_full()
            .cursor_pointer()
            .text_color(theme::text_secondary())
            .hover(move |d| {
                if close {
                    d.bg(theme::danger()).text_color(gpui_kit::white())
                } else {
                    d.bg(theme::hover()).text_color(theme::text())
                }
            })
            // Pressing a button must not start a window move.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(assets::icon(icon, 14., theme::text_secondary()))
    };
    div()
        .id("title-bar")
        .flex()
        .items_center()
        .flex_shrink_0()
        .h(theme::px(BAR_HEIGHT))
        .w_full()
        .bg(theme::sidebar())
        .border_b_1()
        .border_color(theme::border())
        .on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count == 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
        .on_mouse_down(MouseButton::Right, |event, window, _| {
            window.show_window_menu(event.position);
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px(theme::px(12.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(theme::secondary_size())
                .text_color(theme::text_secondary())
                .child(title.to_owned()),
        )
        .when(controls.minimize, |d| {
            d.child(
                button("window-minimize", assets::WINDOW_MINIMIZE, false)
                    .on_click(|_, window, _| window.minimize_window()),
            )
        })
        .when(controls.maximize, |d| {
            d.child(
                button(
                    "window-maximize",
                    if maximized {
                        assets::WINDOW_RESTORE
                    } else {
                        assets::WINDOW_MAXIMIZE
                    },
                    false,
                )
                .on_click(|_, window, _| window.zoom_window()),
            )
        })
        .child(
            button("window-close", assets::WINDOW_CLOSE, true)
                // Like the native button: the window decides (it may go to the tray).
                .on_click(move |_, window, cx| on_close(window, cx)),
        )
}

/// Which edge of the window `position` is on, inside the shadow's margin.
fn resize_edge(
    position: Point<Pixels>,
    window: gpui_kit::Size<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let inner = Bounds::new(point(px(0.), px(0.)), window).inset(SHADOW * 1.5);
    if inner.contains(&position) {
        return None;
    }
    let corner = size(SHADOW * 1.5, SHADOW * 1.5);
    let at = |x: Pixels, y: Pixels| Bounds::new(point(x, y), corner).contains(&position);
    let (right, bottom) = (window.width - corner.width, window.height - corner.height);
    if !tiling.top && !tiling.left && at(px(0.), px(0.)) {
        return Some(ResizeEdge::TopLeft);
    }
    if !tiling.top && !tiling.right && at(right, px(0.)) {
        return Some(ResizeEdge::TopRight);
    }
    if !tiling.bottom && !tiling.left && at(px(0.), bottom) {
        return Some(ResizeEdge::BottomLeft);
    }
    if !tiling.bottom && !tiling.right && at(right, bottom) {
        return Some(ResizeEdge::BottomRight);
    }
    if !tiling.top && position.y < SHADOW {
        Some(ResizeEdge::Top)
    } else if !tiling.bottom && position.y > window.height - SHADOW {
        Some(ResizeEdge::Bottom)
    } else if !tiling.left && position.x < SHADOW {
        Some(ResizeEdge::Left)
    } else if !tiling.right && position.x > window.width - SHADOW {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_and_corners() {
        let window = size(px(800.), px(600.));
        let free = Tiling::default();
        assert_eq!(resize_edge(point(px(400.), px(300.)), window, free), None);
        assert_eq!(
            resize_edge(point(px(2.), px(2.)), window, free),
            Some(ResizeEdge::TopLeft)
        );
        assert_eq!(
            resize_edge(point(px(400.), px(3.)), window, free),
            Some(ResizeEdge::Top)
        );
        assert_eq!(
            resize_edge(point(px(797.), px(300.)), window, free),
            Some(ResizeEdge::Right)
        );
        assert_eq!(
            resize_edge(point(px(798.), px(598.)), window, free),
            Some(ResizeEdge::BottomRight)
        );
        let tiled = Tiling {
            top: true,
            ..Tiling::default()
        };
        assert_eq!(resize_edge(point(px(400.), px(3.)), window, tiled), None);
    }
}
