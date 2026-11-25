//! A watched screen share in a window of its own, which can go fullscreen: double-click or F11
//! toggles it, Esc leaves fullscreen and then closes the window.

use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyWindowHandle, Bounds, Context, FocusHandle, KeyDownEvent, RenderImage, TitlebarOptions,
    WeakEntity, Window, WindowBounds, WindowOptions, div, img, px, size,
};

use crate::shell::{ChannelKey, Shell};
use crate::theme;

pub struct PopOut {
    shell: WeakEntity<Shell>,
    key: ChannelKey,
    /// The shell lists this window as its pop-out (only once it has opened): from then on, not
    /// being listed closes it.
    listed: bool,
    /// The picture drawn last: freed from this window's GPU memory once a newer one shows.
    shown: Option<Arc<RenderImage>>,
    focus: FocusHandle,
    /// Shown in Hermes' own title bar where the desktop draws none (`csd`).
    title: String,
}

impl PopOut {
    fn toggle_fullscreen(window: &mut Window) {
        window.toggle_fullscreen();
    }
}

impl Render for PopOut {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(shell) = self.shell.upgrade() else {
            window.remove_window();
            return div().into_any_element();
        };
        let (picture, latest, gone) = {
            let shell = shell.read(cx);
            (
                shell.screens.get(&self.key).map(|s| s.image.clone()),
                shell.screen_latest.clone(),
                shell.popped_out.as_ref().map(|(k, _)| *k == self.key),
            )
        };
        let gone = match gone {
            Some(listed) => {
                self.listed |= listed;
                !listed
            }
            None => self.listed,
        };
        // The share ended, or another one took this window's place.
        if gone {
            window.remove_window();
            return div().into_any_element();
        }
        if let Some(picture) = &picture
            && self
                .shown
                .as_ref()
                .is_some_and(|old| !Arc::ptr_eq(old, picture))
            && let Some(old) = self.shown.replace(picture.clone())
        {
            let _ = window.drop_image(old);
        } else if self.shown.is_none() {
            self.shown.clone_from(&picture);
        }
        // The decoder makes pictures as large as this window shows them.
        if let Some(latest) = latest {
            let area = window.viewport_size();
            let scale = window.scale_factor();
            latest.set_shown_size(
                (area.width.as_f32() * scale) as u32,
                (area.height.as_f32() * scale) as u32,
            );
        }
        let content = div()
            .id("popout")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui_kit::rgb(0))
            .text_color(theme::text_secondary())
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, _| {
                match event.keystroke.key.as_str() {
                    "f11" => Self::toggle_fullscreen(window),
                    "escape" if window.is_fullscreen() => Self::toggle_fullscreen(window),
                    "escape" => window.remove_window(),
                    _ => {}
                }
            }))
            .on_click(cx.listener(|_, event: &gpui_kit::ClickEvent, window, _| {
                if event.click_count() >= 2 {
                    Self::toggle_fullscreen(window);
                }
            }))
            .child(match picture {
                Some(picture) => img(picture)
                    .size_full()
                    .object_fit(gpui_kit::ObjectFit::Contain)
                    .into_any_element(),
                None => "Waiting for a picture…".into_any_element(),
            })
            .into_any_element();
        if window.is_fullscreen() {
            return content;
        }
        crate::csd::frame(
            content,
            &self.title,
            |window, _| window.remove_window(),
            window,
        )
    }
}

impl Shell {
    /// Opens `key`'s share in a window of its own (or brings it there instead of another one).
    /// The window opens once this update is over: Windows draws a new window while opening it,
    /// and its drawing reads this shell, which cannot be read while it is being updated.
    pub fn pop_out_share(&mut self, key: ChannelKey, title: String, cx: &mut Context<Self>) {
        if let Some((_, handle)) = self.popped_out.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        self.watch_share(key.clone(), true);
        let shell = cx.entity().downgrade();
        cx.defer(move |cx| open_pop_out(shell, key, title, cx));
        cx.notify();
    }
}

/// Opens the pop-out window, outside any update of the shell.
fn open_pop_out(shell: WeakEntity<Shell>, key: ChannelKey, title: String, cx: &mut gpui_kit::App) {
    let bar_title = title.clone();
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1280.), px(760.)),
            cx,
        ))),
        window_min_size: Some(size(px(320.), px(200.))),
        // Draws at the display's rate also while another window has focus, so scrolling and
        // animations beside it stay smooth (#66). Frames are still drawn only when something
        // changed; a window hidden behind another one is slowed down in vendor/gpui-pre-windows.
        inactive_frame_interval: None,
        titlebar: Some(TitlebarOptions {
            title: Some(title.into()),
            appears_transparent: false,
            ..Default::default()
        }),
        app_id: Some(crate::desktop::APP_ID.into()),
        ..Default::default()
    };
    let (popped, main) = (key.clone(), shell.clone());
    let opened = gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            // Drawn again when the main window's state changes: a new picture arrived, or the
            // share ended. Not on every screen refresh (#60).
            if let Some(main) = main.upgrade() {
                cx.observe(&main, |_, _, cx| cx.notify()).detach();
            }
            // Closed by the user: the share shows in the main window again. Later, as the
            // window can go while the shell is being updated (bringing the share back).
            let released = main.clone();
            cx.on_release(move |this: &mut PopOut, cx| {
                let key = this.key.clone();
                let released = released.clone();
                cx.defer(move |cx| {
                    let _ = released.update(cx, |shell, cx| {
                        if shell.popped_out.as_ref().is_some_and(|(k, _)| *k == key) {
                            shell.popped_out = None;
                            cx.notify();
                        }
                    });
                });
            })
            .detach();
            PopOut {
                shell: main,
                key: popped,
                listed: false,
                shown: None,
                focus,
                title: bar_title,
            }
        })
    });
    let _ = shell.update(cx, |shell, cx| {
        match opened {
            Ok((handle, _)) => shell.popped_out = Some((key, handle)),
            Err(err) => shell.error(format!("could not open a window: {err}"), cx),
        }
        cx.notify();
    });
}

impl Shell {
    /// Closes the pop-out window: its share shows in this window again, still watched.
    pub fn pop_in_share(&mut self, cx: &mut Context<Self>) {
        if let Some((_, handle)) = self.popped_out.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        cx.notify();
    }
}

/// The pop-out window of a share, if open.
pub type PoppedOut = Option<(ChannelKey, AnyWindowHandle)>;
