//! Window and startup, and screen sharing.

use super::*;

impl Shell {
    pub(super) fn tab_window(&self, cx: &mut Context<Self>) -> AnyElement {
        let g = &self.config.general;
        page_body(
            "Window & startup",
            None,
            vec![group(
                "Window",
                vec![
                    flag!(
                        self,
                        cx,
                        "Close to the tray",
                        "Closing the window keeps Hermes running in the tray.",
                        general.close_to_tray
                    ),
                    setting(
                        "Start when I sign in",
                        "Hermes opens by itself when you sign in to this computer.",
                        Switch::new("start-on-login")
                            .cursor_pointer()
                            .checked(g.start_on_login)
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                let on = *on;
                                match crate::desktop::set_start_on_login(on) {
                                    Ok(()) => {
                                        this.set_config(|c| c.general.start_on_login = on, cx)
                                    }
                                    Err(err) => this.error(err, cx),
                                }
                            })),
                    ),
                    setting(
                        "Go idle after",
                        "With nothing touched for this long.",
                        crate::ui::segments().children(
                            [(0, "Never"), (5, "5 min"), (10, "10 min"), (30, "30 min")]
                                .into_iter()
                                .enumerate()
                                .map(|(n, (minutes, name))| {
                                    crate::ui::segment(
                                        self,
                                        ("idle-after", n),
                                        name,
                                        g.auto_idle_minutes == minutes,
                                        cx,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.set_config(
                                                |c| c.general.auto_idle_minutes = minutes,
                                                cx,
                                            );
                                        },
                                    ))
                                }),
                        ),
                    ),
                    flag!(
                        self,
                        cx,
                        "Start minimized to the tray",
                        "",
                        general.start_minimized
                    ),
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
                    setting(
                        "Check for updates now",
                        "",
                        Button::new("update-now")
                            .cursor_pointer()
                            .label("Check")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.check_for_update_now(cx);
                            })),
                    ),
                ],
            )],
        )
    }

    pub(super) fn tab_screen_share(&self, cx: &mut Context<Self>) -> AnyElement {
        let v = &self.config.voice;
        // Before the list has loaded (or where capture is unavailable): the primary screen only.
        let screens: Vec<(String, String)> = if self.screen_list.is_empty() {
            vec![(String::new(), "Screen 1 (primary)".to_owned())]
        } else {
            self.screen_list
                .iter()
                .map(|s| (s.value.clone(), s.label()))
                .collect()
        };
        let screen_chips = screens.into_iter().enumerate().map(|(n, (screen, name))| {
            crate::ui::segment(
                self,
                ("settings-screen", n),
                name,
                screen == v.share_screen,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let screen = screen.clone();
                this.set_config(|c| c.voice.share_screen = screen, cx);
            }))
        });
        let res_chips = ShareResolution::ALL
            .iter()
            .copied()
            .enumerate()
            .map(|(n, value)| {
                let name = match value {
                    ShareResolution::Native => "Native",
                    _ => value.as_str(),
                };
                crate::ui::segment(
                    self,
                    ("settings-res", n),
                    name,
                    v.share_resolution == value,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_config(|c| c.voice.share_resolution = value, cx);
                }))
            });
        let fps_chips = FRAME_RATES.into_iter().enumerate().map(|(n, fps)| {
            crate::ui::segment(
                self,
                ("settings-fps", n),
                format!("{fps} fps"),
                v.share_fps == fps,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_config(|c| c.voice.share_fps = fps, cx);
            }))
        });
        let current = Preference::from_config(v.share_preference);
        let pref_chips = PREFERENCES.into_iter().enumerate().map(|(n, preference)| {
            crate::ui::segment(
                self,
                ("settings-pref", n),
                preference.label(),
                preference == current,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_config(|c| c.voice.share_preference = preference.config_value(), cx);
                if let Some(capture) = &this.capture {
                    capture.set_preference(preference);
                }
            }))
        });
        let sharing = group(
            "What you share",
            vec![
                setting(
                    "Screen to share",
                    "",
                    crate::ui::segments().children(screen_chips),
                ),
                setting(
                    "Resolution",
                    "From your next share",
                    crate::ui::segments().children(res_chips),
                ),
                setting(
                    "Frame rate",
                    "From your next share",
                    crate::ui::segments().children(fps_chips),
                ),
                setting(
                    "Share sound",
                    "Others hear what your computer plays, but not this call. From your next \
                    share; needs Windows 10 version 2004 or later.",
                    Switch::new("share-sound")
                        .cursor_pointer()
                        .checked(v.share_sound)
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.set_config(|c| c.voice.share_sound = *on, cx);
                        })),
                ),
                setting(
                    "When it cannot keep up",
                    "What your share keeps when it falls behind",
                    crate::ui::segments().children(pref_chips),
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
