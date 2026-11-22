//! Window and startup, and screen sharing.

use super::*;
use crate::ui::{ButtonKind, DropWidth, Segment};

const IDLE_AFTER: [(u32, &str); 4] = [(0, "Never"), (5, "5 min"), (10, "10 min"), (30, "30 min")];

impl Shell {
    pub(super) fn tab_window(&self, cx: &mut Context<Self>) -> AnyElement {
        let g = &self.config.general;
        let startup = group(
            "Startup",
            vec![
                setting_fit(
                    "Start when I sign in",
                    "Hermes opens by itself when you sign in to this computer.",
                    {
                        let on = !g.start_on_login;
                        crate::ui::switch(
                            self,
                            "start-on-login",
                            g.start_on_login,
                            true,
                            move |this, _, cx| match crate::desktop::set_start_on_login(on) {
                                Ok(()) => this.set_config(|c| c.general.start_on_login = on, cx),
                                Err(err) => this.error(err, cx),
                            },
                            cx,
                        )
                    },
                ),
                flag!(
                    self,
                    cx,
                    "Start minimized to the tray",
                    "",
                    general.start_minimized
                ),
            ],
        );
        let window = group(
            "Window",
            vec![
                flag!(
                    self,
                    cx,
                    "Close to the tray",
                    "Closing the window keeps Hermes running in the tray.",
                    general.close_to_tray
                ),
                setting_fit(
                    "Go idle after",
                    "With nothing touched for this long.",
                    crate::ui::segmented(
                        self,
                        "idle-after",
                        IDLE_AFTER
                            .iter()
                            .map(|(_, name)| Segment::new(*name))
                            .collect(),
                        IDLE_AFTER
                            .iter()
                            .position(|(m, _)| *m == g.auto_idle_minutes),
                        true,
                        |this, n, _, cx| {
                            let minutes = IDLE_AFTER[n].0;
                            this.set_config(|c| c.general.auto_idle_minutes = minutes, cx);
                        },
                        cx,
                    ),
                ),
            ],
        );
        note_part("Check for updates now", "", "Check for updates on start");
        let updates = group_with(
            "Updates",
            crate::ui::button(
                self,
                "update-now",
                "Check now",
                ButtonKind::Secondary,
                true,
                |this, _, cx| this.check_for_update_now(cx),
                cx,
            )
            .into_any_element(),
            vec![
                flag!(
                    self,
                    cx,
                    "Check for updates on start",
                    "",
                    general.check_for_updates
                ),
                flag!(
                    self,
                    cx,
                    "Install updates by themselves",
                    "A newer version found on start is downloaded (only what changed) \
                     and Hermes restarts into it.",
                    general.auto_update
                ),
            ],
        );
        page_body("Window & startup", None, vec![startup, window, updates])
    }

    pub(super) fn tab_screen_share(
        &self,
        page: &SettingsPage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let v = &self.config.voice;
        // Before the screens are listed (or where capture is unavailable): the primary screen.
        let screen: AnyElement = match &page.screen_select {
            Some((state, _)) => crate::ui::dropdown(state, DropWidth::Wide).into_any_element(),
            None => muted("Screen 1 (primary)"),
        };
        let resolution = crate::ui::segmented(
            self,
            "settings-res",
            ShareResolution::ALL
                .iter()
                .map(|value| {
                    Segment::new(match value {
                        ShareResolution::Native => "Native",
                        _ => value.as_str(),
                    })
                })
                .collect(),
            ShareResolution::ALL
                .iter()
                .position(|r| *r == v.share_resolution),
            true,
            |this, n, _, cx| {
                let value = ShareResolution::ALL[n];
                this.set_config(|c| c.voice.share_resolution = value, cx);
            },
            cx,
        );
        let fps = crate::ui::segmented(
            self,
            "settings-fps",
            FRAME_RATES
                .iter()
                .map(|fps| Segment::new(format!("{fps} fps")))
                .collect(),
            FRAME_RATES.iter().position(|f| *f == v.share_fps),
            true,
            |this, n, _, cx| {
                let fps = FRAME_RATES[n];
                this.set_config(|c| c.voice.share_fps = fps, cx);
            },
            cx,
        );
        let current = Preference::from_config(v.share_preference);
        let preference = crate::ui::segmented(
            self,
            "settings-pref",
            PREFERENCES
                .iter()
                .map(|p| Segment::new(p.label()))
                .collect(),
            PREFERENCES.iter().position(|p| *p == current),
            true,
            |this, n, _, cx| {
                let preference = PREFERENCES[n];
                this.set_config(|c| c.voice.share_preference = preference.config_value(), cx);
                if let Some(capture) = &this.capture {
                    capture.set_preference(preference);
                }
            },
            cx,
        );
        let sharing = group(
            "What you share",
            vec![
                setting_fit("Screen to share", "", screen),
                // Both apply from the next share; side by side, as they are chosen together.
                setting_pair(
                    setting_below("Resolution", "From your next share", resolution),
                    setting_below("Frame rate", "From your next share", fps),
                ),
                setting_fit(
                    "Share sound",
                    "Others hear what your computer plays, but not this call. From your next \
                    share; needs Windows 10 version 2004 or later.",
                    {
                        let on = !v.share_sound;
                        crate::ui::switch(
                            self,
                            "share-sound",
                            v.share_sound,
                            true,
                            move |this, _, cx| this.set_config(|c| c.voice.share_sound = on, cx),
                            cx,
                        )
                    },
                ),
                setting_fit(
                    "When it cannot keep up",
                    "What your share keeps when it falls behind.",
                    preference,
                ),
            ],
        );
        page_body(
            "Screen sharing",
            Some("These are also chosen in the share picker each time you share."),
            vec![sharing],
        )
    }
}
