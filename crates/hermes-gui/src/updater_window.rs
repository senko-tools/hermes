//! The updater's window: what it is doing, a progress bar, and how much of it is done
//! (`0.33 / 2.10 MB`). It downloads only what changed (see `hermes_client_core::updater`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyWindowHandle, Bounds, Context, SharedString, TitlebarOptions, WeakEntity, Window,
    WindowBounds, WindowOptions, div, px, relative, size,
};
use hermes_client_core::updater::{self, Stage};

use crate::shell::Shell;
use crate::theme;

pub struct UpdaterView {
    shell: WeakEntity<Shell>,
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
}

impl UpdaterView {
    fn line(&self) -> String {
        match &self.stage {
            Stage::Checking => "Checking what changed".into(),
            Stage::Downloading { version, .. } => format!("Downloading Hermes {version}"),
            Stage::Patching { file } => format!("Applying the update to {file}"),
            Stage::Verifying => "Verifying".into(),
            Stage::Installing => "Installing".into(),
            Stage::Restarting => "Restarting".into(),
            Stage::UpToDate => "Hermes is up to date".into(),
            Stage::Failed(err) => format!("The update failed: {err}"),
        }
    }

    /// How far along, 0–1.
    fn fraction(&self) -> f32 {
        match &self.stage {
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

    fn bytes(&self) -> String {
        match &self.stage {
            Stage::Downloading { done, total, .. } => updater::human_progress(*done, *total),
            _ => String::new(),
        }
    }
}

impl Render for UpdaterView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::{AnimationExt as _, SpringAnimation, SpringConfig};
        let failed = matches!(self.stage, Stage::Failed(_));
        let cancellable = matches!(
            self.stage,
            Stage::Checking | Stage::Downloading { .. } | Stage::Patching { .. } | Stage::Verifying
        );
        let done = matches!(self.stage, Stage::UpToDate);
        let fraction = self.fraction();
        let (version, url) = (self.version.clone(), self.url.clone());
        let shell = self.shell.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(20.))
            .bg(theme::background())
            .font_family(theme::font())
            .text_color(theme::text())
            .text_size(theme::body_size())
            // 1. What it is doing.
            .child(
                div()
                    .min_h(px(20.))
                    .when(failed, |d| d.text_color(theme::danger()))
                    .child(self.line()),
            )
            // 2. The bar.
            .child(
                div()
                    .w_full()
                    .h(px(8.))
                    .rounded(px(4.))
                    .bg(theme::raised())
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .rounded(px(4.))
                            .bg(if failed {
                                theme::danger()
                            } else {
                                theme::accent()
                            })
                            .with_spring(
                                "update-bar",
                                SpringAnimation::new(crate::theme::paced(SpringConfig::new(
                                    300., 34., 1.,
                                )))
                                .to(fraction),
                                |d, phase| d.w(relative(phase.clamp(0., 1.))),
                            ),
                    ),
            )
            // 3. How much.
            .child(
                div()
                    .min_h(px(18.))
                    .text_size(theme::secondary_size())
                    .text_color(theme::text_secondary())
                    .child(self.bytes()),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .mt_auto()
                    .when(cancellable, |d| {
                        let shell = shell.clone();
                        d.child(
                            Button::new("update-cancel")
                                .cursor_pointer()
                                .label("Cancel")
                                .on_click(move |_, _, cx| {
                                    let _ = shell.update(cx, |s, cx| s.close_updater(cx));
                                }),
                        )
                    })
                    .when(failed, |d| {
                        let (retry, page) = (shell.clone(), url.clone());
                        let (version, url) = (version.clone(), url.clone());
                        d.child(
                            Button::new("update-page")
                                .cursor_pointer()
                                .label("Open the release page")
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
                                    let _ =
                                        retry.update(cx, |s, cx| s.start_update(version, url, cx));
                                }),
                        )
                    })
                    .when(done, |d| {
                        d.child(
                            Button::new("update-close")
                                .cursor_pointer()
                                .primary()
                                .label("Close")
                                .on_click(move |_, _, cx| {
                                    let _ = shell.update(cx, |s, cx| s.close_updater(cx));
                                }),
                        )
                    }),
            )
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
    /// Opens the updater's window (or uses the open one) and updates to `version`.
    pub fn start_update(&mut self, version: String, url: String, cx: &mut Context<Self>) {
        if let Some(old) = self.updater.take() {
            old.cancel.store(true, Ordering::Relaxed);
            if let Some(task) = old.task {
                task.abort();
            }
            let _ = old.window.update(cx, |_, window, _| window.remove_window());
        }
        let shell = cx.entity().downgrade();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(460.), px(190.)),
                cx,
            ))),
            is_resizable: false,
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from(format!("Updating Hermes to {version}"))),
                appears_transparent: false,
                ..Default::default()
            }),
            ..Default::default()
        };
        let view_version = version.clone();
        let view_url = url.clone();
        let opened = gpui_kit::open_window(options, cx, move |_, cx| {
            cx.new(|_| UpdaterView {
                shell,
                version: view_version,
                url: view_url,
                stage: Stage::Checking,
            })
        });
        let (window, view) = match opened {
            Ok(opened) => opened,
            Err(err) => {
                self.error(format!("could not open the updater: {err}"), cx);
                return;
            }
        };
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
                view_handle.update(cx, |v, cx| {
                    v.stage = stage;
                    cx.notify();
                });
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
                        }
                    }
                    return;
                }
            }
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        self.updater = Some(Updater {
            window,
            cancel,
            task: Some(task),
        });
    }

    /// Stops the update (until it installs) and closes its window.
    pub fn close_updater(&mut self, cx: &mut Context<Self>) {
        if let Some(updater) = self.updater.take() {
            updater.cancel.store(true, Ordering::Relaxed);
            if let Some(task) = updater.task {
                task.abort();
            }
            let _ = updater
                .window
                .update(cx, |_, window, _| window.remove_window());
        }
        cx.notify();
    }
}
