//! The updater's window, much like Steam's: a small borderless window, dragged by any part of it,
//! that is the only window while Hermes updates. It says what it is doing ("Downloading update
//! (1,234 of 5,678 KB)…") over a thin bar, and a × in the corner cancels until it installs. The
//! main window hides while it runs and comes back when it is cancelled or fails. It downloads only
//! what changed (see `hermes_client_core::updater`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyWindowHandle, App, Bounds, Context, SharedString, TitlebarOptions, WeakEntity, Window,
    WindowBounds, WindowControlArea, WindowOptions, div, relative, size,
};
use hermes_client_core::updater::{self, Stage};

use crate::assets::{self, icon};
use crate::shell::Shell;
use crate::theme::{self, px};

/// The window's size, before the UI scale.
const WIDTH: f32 = 420.;
const HEIGHT: f32 = 150.;

pub struct UpdaterView {
    shell: WeakEntity<Shell>,
    /// The main window, shown again from here.
    main: AnyWindowHandle,
    version: String,
    url: String,
    stage: Stage,
}

/// The updater's window and the work it shows.
pub struct Updater {
    pub window: AnyWindowHandle,
    /// Set by Cancel: the work stops at its next step.
    cancel: Arc<AtomicBool>,
    task: Option<tokio::task::JoinHandle<()>>,
    /// Whether the main window showed when the update started: it shows again only then.
    main_was_visible: bool,
}

/// Bytes as whole kilobytes with thousands separators, as Steam writes them: `45,678`.
fn kilobytes(bytes: u64) -> String {
    let digits = bytes.div_ceil(1024).to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What the updater is doing, in Steam's words.
fn status_line(stage: &Stage) -> String {
    match stage {
        Stage::Checking => "Checking for available updates…".into(),
        Stage::Downloading { done, total, .. } => format!(
            "Downloading update ({} of {} KB)…",
            kilobytes(*done),
            kilobytes(*total)
        ),
        Stage::Patching { .. } => "Extracting package…".into(),
        Stage::Verifying => "Verifying installation…".into(),
        Stage::Installing => "Installing update…".into(),
        Stage::Restarting => "Starting Hermes…".into(),
        Stage::UpToDate => "Hermes is up to date".into(),
        Stage::Failed(err) => format!("The update failed: {err}"),
    }
}

/// How far along, 0–1.
fn fraction(stage: &Stage) -> f32 {
    match stage {
        Stage::Checking | Stage::Failed(_) => 0.,
        Stage::Downloading { done, total, .. } => {
            (*done as f64 / (*total).max(1) as f64).clamp(0., 1.) as f32 * 0.9
        }
        Stage::Patching { .. } => 0.9,
        Stage::Verifying => 0.94,
        Stage::Installing => 0.97,
        Stage::Restarting | Stage::UpToDate => 1.,
    }
}

/// Until it installs, the update can still be stopped.
fn cancellable(stage: &Stage) -> bool {
    matches!(
        stage,
        Stage::Checking | Stage::Downloading { .. } | Stage::Patching { .. } | Stage::Verifying
    )
}

/// Runs `f` on the shell with the main window, once the updater's window is done handling its
/// event: until then that window can't be closed or replaced.
fn with_shell(
    shell: &WeakEntity<Shell>,
    main: AnyWindowHandle,
    cx: &mut App,
    f: impl FnOnce(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
) {
    let shell = shell.clone();
    cx.defer(move |cx| {
        let _ = cx.update_window(main, move |_, window, cx| {
            let _ = shell.update(cx, |s, cx| f(s, window, cx));
        });
    });
}

impl Render for UpdaterView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::{AnimationExt as _, SpringAnimation, SpringConfig};
        let failed = matches!(self.stage, Stage::Failed(_));
        let done = matches!(self.stage, Stage::UpToDate);
        let (shell, main) = (self.shell.clone(), self.main);
        let close = cancellable(&self.stage).then(|| {
            let shell = shell.clone();
            div()
                .id("update-cancel")
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .size(px(24.))
                .rounded(theme::radius_small())
                .cursor_pointer()
                .hover(|d| d.bg(theme::hover()))
                .child(icon(assets::CLOSE, 14., theme::text_secondary()))
                .on_click(move |_, _, cx| {
                    with_shell(&shell, main, cx, |s, window, cx| {
                        s.close_updater(window, cx)
                    });
                })
        });
        let buttons = (failed || done).then(|| {
            let row = div().id("update-buttons").occlude().flex().gap(px(8.));
            if done {
                return row.child(
                    Button::new("update-close")
                        .cursor_pointer()
                        .primary()
                        .label("Close")
                        .on_click(move |_, _, cx| {
                            with_shell(&shell, main, cx, |s, window, cx| {
                                s.close_updater(window, cx)
                            });
                        }),
                );
            }
            let page = self.url.clone();
            let (version, url) = (self.version.clone(), self.url.clone());
            row.child(
                Button::new("update-page")
                    .cursor_pointer()
                    .label("Release page")
                    .on_click(move |_, _, _| {
                        let _ = crate::open::url(&page);
                    }),
            )
            .child(
                Button::new("update-retry")
                    .cursor_pointer()
                    .primary()
                    .label("Try again")
                    .on_click(move |_, _, cx| {
                        let (version, url) = (version.clone(), url.clone());
                        with_shell(&shell, main, cx, move |s, window, cx| {
                            s.start_update(version, url, window, cx)
                        });
                    }),
            )
        });
        div()
            .id("updater")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(18.))
            .pt(px(12.))
            .pb(px(16.))
            .bg(theme::sidebar())
            .border_1()
            .border_color(theme::border())
            .font_family(theme::font())
            .text_color(theme::text())
            .text_size(theme::body_size())
            // Dragged by any part that is not a button, like Steam's.
            .window_control_area(WindowControlArea::Drag)
            // 1. The feather, the title and the ×.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .min_h(px(24.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(24.))
                            .rounded(theme::radius_small())
                            .bg(theme::accent())
                            .child(icon(assets::FEATHER, 16., theme::rgb_u32(0xffffff))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(format!("Updating Hermes to {}", self.version)),
                    )
                    .children(close),
            )
            // 2. What it is doing.
            .child(
                div()
                    .mt(px(6.))
                    .min_h(px(18.))
                    .text_size(theme::secondary_size())
                    .text_color(if failed {
                        theme::danger()
                    } else {
                        theme::text_secondary()
                    })
                    .line_clamp(2)
                    .child(status_line(&self.stage)),
            )
            // 3. The bar.
            .child(
                div()
                    .w_full()
                    .h(px(6.))
                    .rounded(px(3.))
                    .bg(theme::raised())
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .rounded(px(3.))
                            .bg(if failed {
                                theme::danger()
                            } else {
                                theme::accent()
                            })
                            .with_spring(
                                "update-bar",
                                SpringAnimation::new(theme::paced(SpringConfig::new(
                                    300., 34., 1.,
                                )))
                                .to(fraction(&self.stage)),
                                |d, phase| d.w(relative(phase.clamp(0., 1.))),
                            ),
                    ),
            )
            .children(buttons.map(|row| div().mt_auto().flex().justify_end().child(row)))
    }
}

/// The command line the new version starts with: this one's, without a link (already handled)
/// or the updater's own flags.
fn restart_args() -> Vec<std::ffi::OsString> {
    std::env::args_os()
        .skip(1)
        .filter(|a| {
            let a = a.to_string_lossy();
            a != updater::AFTER_FLAG && !a.to_ascii_lowercase().starts_with("hermes:")
        })
        .collect()
}

impl Shell {
    /// Hides the main window and updates to `version` in the updater's window (a new one, or in
    /// place of the open one). `window` is the main window.
    pub fn start_update(
        &mut self,
        version: String,
        url: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let main_was_visible = match self.updater.take() {
            Some(old) => {
                old.cancel.store(true, Ordering::Relaxed);
                if let Some(task) = old.task {
                    task.abort();
                }
                let _ = old.window.update(cx, |_, window, _| window.remove_window());
                old.main_was_visible
            }
            None => self.window_visible,
        };
        let shell = cx.entity().downgrade();
        let main = self.window;
        let scale = theme::scale();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(gpui_kit::px(WIDTH * scale), gpui_kit::px(HEIGHT * scale)),
                cx,
            ))),
            is_resizable: false,
            is_minimizable: false,
            // No title bar (the taskbar still shows the title): the window is dragged by its body.
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from(format!("Updating Hermes to {version}"))),
                appears_transparent: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let view_version = version.clone();
        let view_url = url.clone();
        let opened = gpui_kit::open_window(options, cx, move |window, cx| {
            let view = cx.new(|_| UpdaterView {
                shell: shell.clone(),
                main,
                version: view_version,
                url: view_url,
                stage: Stage::Checking,
            });
            // Alt+F4 is the × (refused once it installs).
            let closing = view.downgrade();
            window.on_window_should_close(cx, move |_, cx| {
                let Some(stage) = closing.upgrade().map(|v| v.read(cx).stage.clone()) else {
                    return true;
                };
                if cancellable(&stage) || matches!(stage, Stage::Failed(_) | Stage::UpToDate) {
                    with_shell(&shell, main, cx, |s, window, cx| {
                        s.close_updater(window, cx)
                    });
                    return true;
                }
                false
            });
            view
        });
        let (updater_window, view) = match opened {
            Ok(opened) => opened,
            Err(err) => {
                self.error(format!("could not open the updater: {err}"), cx);
                return;
            }
        };
        if main_was_visible {
            self.hide_window(window, cx);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Stage>();
        let stopped = cancel.clone();
        let task = self.runtime.spawn(async move {
            let send = |stage: Stage| {
                if !stopped.load(Ordering::Relaxed) {
                    let _ = tx.send(stage);
                }
            };
            let work = async {
                send(Stage::Checking);
                let plan = updater::prepare(&version).await?;
                if plan.is_empty() {
                    send(Stage::UpToDate);
                    return Ok(());
                }
                let staging = updater::download(&plan, &send).await?;
                if stopped.load(Ordering::Relaxed) {
                    return Ok(());
                }
                send(Stage::Installing);
                let installed = {
                    let plan = plan.clone();
                    tokio::task::spawn_blocking(move || updater::install(&plan, &staging))
                        .await
                        .map_err(|e| e.to_string())?
                };
                installed?;
                send(Stage::Restarting);
                Ok::<(), String>(())
            };
            if let Err(err) = work.await {
                tracing::warn!("update failed: {err}");
                send(Stage::Failed(err));
            }
        });
        let view_handle = view.clone();
        cx.spawn(async move |this, cx| {
            while let Some(stage) = rx.recv().await {
                let restarting = stage == Stage::Restarting;
                let failed = matches!(stage, Stage::Failed(_));
                view_handle.update(cx, |v, cx| {
                    v.stage = stage;
                    cx.notify();
                });
                if failed {
                    // The main window comes back beside the error.
                    let _ = cx.update_window(main, |_, window, cx| {
                        let _ = this.update(cx, |s, cx| s.show_main_after_update(window, cx));
                    });
                }
                if restarting {
                    match updater::restart(&restart_args()) {
                        Ok(()) => {
                            cx.update(|cx| cx.quit());
                        }
                        Err(err) => {
                            updater::roll_back();
                            view_handle.update(cx, |v, cx| {
                                v.stage = Stage::Failed(err);
                                cx.notify();
                            });
                            let _ = cx.update_window(main, |_, window, cx| {
                                let _ =
                                    this.update(cx, |s, cx| s.show_main_after_update(window, cx));
                            });
                        }
                    }
                    return;
                }
            }
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        self.updater = Some(Updater {
            window: updater_window,
            cancel,
            task: Some(task),
            main_was_visible,
        });
    }

    /// Stops the update (until it installs), closes its window and shows the main window again.
    /// `window` is the main window.
    pub fn close_updater(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_main_after_update(window, cx);
        if let Some(updater) = self.updater.take() {
            updater.cancel.store(true, Ordering::Relaxed);
            if let Some(task) = updater.task {
                task.abort();
            }
            // Fails when the window is closing itself (Alt+F4); it goes anyway.
            let _ = updater
                .window
                .update(cx, |_, window, _| window.remove_window());
        }
        cx.notify();
    }

    /// The main window again, if it showed when the update started.
    fn show_main_after_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .updater
            .as_ref()
            .is_some_and(|updater| updater.main_was_visible)
            && !self.window_visible
        {
            self.show_window(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kilobytes_are_grouped_like_steam() {
        assert_eq!(kilobytes(0), "0");
        assert_eq!(kilobytes(1), "1");
        assert_eq!(kilobytes(1024 * 999), "999");
        assert_eq!(kilobytes(1024 * 1000), "1,000");
        assert_eq!(kilobytes(1024 * 45_678), "45,678");
        assert_eq!(kilobytes(1024 * 1_234_567), "1,234,567");
    }

    #[test]
    fn the_status_line_reads_like_steam() {
        let stage = Stage::Downloading {
            version: "1.5.0".into(),
            done: 1024 * 1234,
            total: 1024 * 45_678,
        };
        assert_eq!(
            status_line(&stage),
            "Downloading update (1,234 of 45,678 KB)…"
        );
        assert_eq!(status_line(&Stage::Verifying), "Verifying installation…");
    }
}
