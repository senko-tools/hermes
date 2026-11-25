//! Window and startup, and screen sharing.

use super::*;
use crate::ui::{DropWidth, Segment};

const IDLE_AFTER: [(u32, &str); 4] = [(0, "Never"), (5, "5 min"), (10, "10 min"), (30, "30 min")];

impl Shell {
    pub(super) fn tab_window(&self, cx: &mut Context<Self>) -> AnyElement {
        let g = &self.config.general;
        let portable = self.core.paths().is_some_and(|p| p.portable);
        let startup = group(
            "Startup",
            vec![
                setting_fit(
                    "Start when I sign in",
                    if portable {
                        "Hermes opens by itself when you sign in to this computer. This \
                         computer then remembers where this portable copy is."
                    } else {
                        "Hermes opens by itself when you sign in to this computer."
                    },
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
                    if crate::desktop::window::can_hide() {
                        ""
                    } else {
                        "Not on this system: here, Hermes opens its window."
                    },
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
                    if crate::desktop::window::can_hide() {
                        "Closing the window keeps Hermes running in the tray."
                    } else {
                        "Not on this system: here, closing the window quits Hermes."
                    },
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
        let mut groups = vec![startup, window];
        if portable {
            groups.push(group(
                "Portable",
                vec![flag!(
                    self,
                    cx,
                    "Keep sign-ins in this folder",
                    if cfg!(windows) {
                        "Sign-ins and encryption keys go in tokens.toml next to Hermes instead of \
                         Windows Credential Manager, so this folder works on another computer. \
                         Anyone who copies the folder is signed in as you, and on a USB stick the \
                         file has no protection at all. From the next start."
                    } else {
                        "Sign-ins and encryption keys go in tokens.toml next to Hermes instead of \
                         the system keyring, so this folder works on another computer. Anyone who \
                         copies the folder is signed in as you, and on a USB stick the file has no \
                         protection at all. From the next start."
                    },
                    general.sign_ins_in_folder
                )],
            ));
        }
        page_body("Window & startup", None, groups)
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
        let limit = crate::ui::segmented(
            self,
            "settings-share-limit",
            UPLOAD_LIMITS
                .iter()
                .map(|&mbps| Segment::new(upload_limit_label(mbps)))
                .collect(),
            UPLOAD_LIMITS.iter().position(|m| *m == v.share_max_mbps),
            true,
            |this, n, _, cx| {
                let mbps = UPLOAD_LIMITS[n];
                this.set_config(|c| c.voice.share_max_mbps = mbps, cx);
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
                    if cfg!(windows) {
                        "Others hear what your computer plays, but not this call. From your next \
                         share; needs Windows 10 version 2004 or later."
                    } else {
                        "Others hear what your computer plays, but not this call. From your next \
                         share; needs PipeWire."
                    },
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
                setting_fit(
                    "Upload limit",
                    "The most your share sends; everyone watching downloads as much. Lower \
                     leaves room for games. From your next share.",
                    limit,
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

/// An upload limit's segment: `8 Mbit/s`, or `No limit` for 0.
fn upload_limit_label(mbps: u32) -> String {
    if mbps == 0 {
        "No limit".to_owned()
    } else {
        format!("{mbps} Mbit/s")
    }
}
