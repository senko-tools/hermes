//! Desktop integration of the shell (PLAN.md section 15.7): closing to the tray, the tray's menu,
//! toasts, the taskbar flash, push-to-talk and the global mute key.

use futures_util::StreamExt as _;
use gpui_kit::{AppContext as _, Context, Window};
use hermes_client_core::InstanceId;
use hermes_client_core::notify::Notification;

use crate::desktop::tray::TrayAction;
use crate::desktop::{self, CloseAction, window as native};
use crate::ptt;
use crate::shell::Shell;

impl Shell {
    pub fn start_desktop(
        &mut self,
        start_hidden: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Closing asks first: it may only hide the window.
        let shell = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            shell
                .update(cx, |this, cx| this.close_requested(window, cx))
                .unwrap_or(true)
        });
        if start_hidden {
            native::set_visible(window, false);
        }
        if self.tray.is_some() {
            let handle = self.window;
            cx.spawn(async move |this, cx| {
                let mut events = std::pin::pin!(crate::desktop::tray::events());
                while let Some(action) = events.next().await {
                    let result = cx.update_window(handle, |_, window, cx| {
                        this.update(cx, |this, cx| this.tray_action(action, window, cx))
                    });
                    if !matches!(result, Ok(Ok(()))) {
                        return;
                    }
                }
            })
            .detach();
        }

        // Push-to-talk polls on a thread of its own.
        self.update_ptt_keys();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        ptt::poll(self.ptt_keys.clone(), tx);
        cx.spawn(async move |this, cx| {
            while let Some(down) = rx.recv().await {
                let result = this.update(cx, |this, cx| {
                    this.ptt_down = down;
                    this.core.set_ptt(down);
                    cx.notify();
                });
                if result.is_err() {
                    return;
                }
            }
        })
        .detach();

        // The global mute key.
        self.register_global_keys(cx);
        if self.global_keys.is_some() {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            ptt::global_presses(tx);
            cx.spawn(async move |this, cx| {
                while rx.recv().await.is_some() {
                    let result = this.update(cx, |this, cx| {
                        this.toggle_mute(cx);
                        cx.notify();
                    });
                    if result.is_err() {
                        return;
                    }
                }
            })
            .detach();
        }
    }

    /// What the push-to-talk thread watches, from the config.
    pub fn update_ptt_keys(&self) {
        let keys = ptt::virtual_keys(&self.config.voice.ptt_key).unwrap_or_default();
        *self.ptt_keys.lock().expect("ptt keys") = keys;
    }

    pub fn register_global_keys(&mut self, cx: &mut Context<Self>) {
        let combo = self
            .config
            .keys
            .get(crate::keybinds::GLOBAL_MUTE)
            .cloned()
            .unwrap_or_default();
        let Some(keys) = &mut self.global_keys else {
            return;
        };
        if keys.combo == combo {
            return;
        }
        if let Err(err) = keys.set_mute(&combo) {
            self.warn(
                format!("[keys] {}: {err}", crate::keybinds::GLOBAL_MUTE),
                cx,
            );
        }
    }

    /// `true` closes the window, which ends the app.
    fn close_requested(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match desktop::on_close(self.config.general.close_to_tray, self.tray.is_some()) {
            CloseAction::Hide => {
                self.hide_window(window, cx);
                false
            }
            CloseAction::Quit => true,
        }
    }

    fn hide_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.window_visible = false;
        native::set_visible(window, false);
        self.sync_screens(window);
        cx.notify();
    }

    pub fn show_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.window_visible = true;
        native::set_visible(window, true);
        window.activate_window();
        self.sync_screens(window);
        cx.notify();
    }

    fn tray_action(&mut self, action: TrayAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            TrayAction::ToggleMute => self.toggle_mute(cx),
            TrayAction::ToggleDeafen => self.toggle_deafen(cx),
            TrayAction::ToggleWindow => {
                if self.window_visible {
                    self.hide_window(window, cx);
                } else {
                    self.show_window(window, cx);
                }
            }
            TrayAction::Quit => cx.quit(),
        }
        cx.notify();
    }

    /// The tray's Mute / Deafen check marks follow the core.
    pub fn update_tray_voice(&mut self) {
        if let Some(tray) = &mut self.tray {
            tray.set_voice(self.core.self_mute(), self.core.self_deaf());
        }
    }

    /// The tray's badge dot follows unread mentions and DMs.
    pub fn update_badge(&mut self) {
        if let Some(tray) = &mut self.tray {
            let badge = desktop::has_badge(&self.core.store(), &self.core.instance_ids());
            tray.set_badge(badge);
        }
    }

    /// A toast (if enabled) and a taskbar flash for DMs and mentions while unfocused.
    pub fn notify(
        &mut self,
        inst: InstanceId,
        notification: Notification,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = self.config.notifications.clone();
        if !settings.enabled {
            return;
        }
        if !self.focused && settings.flash_taskbar && desktop::flashes(&notification) {
            native::flash(window);
        }
        self.run(
            cx,
            desktop::show_toast(inst, notification, settings),
            |this, clicked, window, cx| {
                if let Some((inst, channel)) = clicked {
                    this.show_window(window, cx);
                    this.select(inst, channel, window, cx);
                }
            },
        );
    }
}
