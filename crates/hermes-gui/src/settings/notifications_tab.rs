//! Notifications and keybinds.

use super::*;

impl Shell {
    pub(super) fn tab_notifications(&self, cx: &mut Context<Self>) -> AnyElement {
        let n = &self.config.notifications;
        page_body(
            "Notifications",
            Some("Per server and channel: right-click it, Notifications."),
            vec![group(
                "Desktop notifications",
                vec![
                    flag!(self, cx, "Show notifications", "", notifications.enabled),
                    flag!(
                        self,
                        cx,
                        "Show the message text",
                        "Off: only who wrote, and where.",
                        notifications.show_preview
                    ),
                    flag!(self, cx, "Play a sound", "", notifications.sound),
                    setting(
                        "Test sounds",
                        "Messages and DMs play the softer one, mentions the ping.",
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(
                                Button::new("test-sound-message")
                                    .cursor_pointer()
                                    .label("Message")
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.core.play_sound(Sound::Message);
                                    })),
                            )
                            .child(
                                Button::new("test-sound-mention")
                                    .cursor_pointer()
                                    .label("Mention")
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.core.play_sound(Sound::Ping);
                                    })),
                            ),
                    ),
                    setting(
                        "Quiet hours",
                        &if n.quiet_from.is_empty() {
                            "None".to_owned()
                        } else {
                            format!("{} to {}", n.quiet_from, n.quiet_to)
                        },
                        Button::new("quiet-hours")
                            .cursor_pointer()
                            .label("Change")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_form_quiet_hours(window, cx);
                            })),
                    ),
                    setting(
                        "Words that notify me",
                        &if n.keywords.is_empty() {
                            "None".to_owned()
                        } else {
                            n.keywords.clone()
                        },
                        Button::new("keywords")
                            .cursor_pointer()
                            .label("Change")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_form_keywords(window, cx);
                            })),
                    ),
                    flag!(self, cx, "Pop up for DMs", "", notifications.toast_dms),
                    flag!(
                        self,
                        cx,
                        "Pop up for mentions",
                        "",
                        notifications.toast_mentions
                    ),
                    flag!(
                        self,
                        cx,
                        "Pop up for other messages",
                        "",
                        notifications.toast_other
                    ),
                    flag!(self, cx, "Sound for DMs", "", notifications.sound_dms),
                    flag!(
                        self,
                        cx,
                        "Sound for mentions",
                        "",
                        notifications.sound_mentions
                    ),
                    flag!(
                        self,
                        cx,
                        "Sound for other messages",
                        "",
                        notifications.sound_other
                    ),
                    flag!(
                        self,
                        cx,
                        "Flash the taskbar button",
                        "",
                        notifications.flash_taskbar
                    ),
                ],
            )],
        )
    }

    /// A key pressed while a keybind waits for one. Whether it was taken.
    pub fn capture_key(&mut self, keystroke: &gpui_kit::Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(page) = self.settings.as_mut() else {
            return false;
        };
        let Some(name) = page.capturing_key else {
            return false;
        };
        page.capturing_key = None;
        if keystroke.key == "escape" {
            cx.notify();
            return true;
        }
        let m = keystroke.modifiers;
        let mut combo = String::new();
        for (on, part) in [(m.control, "Ctrl+"), (m.shift, "Shift+"), (m.alt, "Alt+")] {
            if on {
                combo.push_str(part);
            }
        }
        combo.push_str(&keystroke.key.to_uppercase());
        let taken = ACTIONS
            .iter()
            .filter(|(_, other, ..)| *other != name)
            .find(|(action, ..)| {
                Keybinds::combo_text(&self.config.keys, *action).is_some_and(|text| text == combo)
            })
            .map(|(.., title)| *title);
        if let Some(title) = taken {
            self.warn(format!("{combo} was already used for {title}"), cx);
        }
        self.set_config(|c| _ = c.keys.insert(name.to_owned(), combo), cx);
        true
    }

    pub(super) fn tab_keybinds(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let mut groups: Vec<AnyElement> = KEY_GROUPS
            .iter()
            .map(|(title, actions)| {
                let rows = actions
                    .iter()
                    .filter_map(|action| ACTIONS.iter().find(|(a, ..)| a == action))
                    .map(|(action, name, default, title)| {
                        let combo =
                            Keybinds::combo_text(&self.config.keys, *action).unwrap_or_default();
                        let (name, default): (&'static str, &'static str) = (name, default);
                        let waiting = page.capturing_key == Some(name);
                        setting(
                            title,
                            if waiting {
                                "Press the keys; Escape cancels."
                            } else {
                                ""
                            },
                            wrap()
                                .child(key_caps(&combo))
                                .child(
                                    Button::new(SharedString::from(format!("bind-{name}")))
                                        .cursor_pointer()
                                        .label(if waiting { "Waiting…" } else { "Change" })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(page) = &mut this.settings {
                                                page.capturing_key = Some(name);
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("unbind-{name}")))
                                        .cursor_pointer()
                                        .label("Reset")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.set_config(
                                                |c| {
                                                    c.keys.insert(
                                                        name.to_owned(),
                                                        default.to_owned(),
                                                    );
                                                },
                                                cx,
                                            );
                                        })),
                                ),
                        )
                    })
                    .collect();
                group(title, rows)
            })
            .collect();
        groups.push(group(
            "Everywhere",
            vec![muted(
                "Right-click anything for more. Drag channels and roles to reorder them.",
            )],
        ));
        groups.push(group(
            "These do not change",
            vec![
                setting("Close what is on top", "", key_caps("Escape")),
                setting("Copy what is selected", "", key_caps("Ctrl+C")),
                setting("Open the menu", "", key_caps("Shift+F10")),
                setting("Edit your last message", "", key_caps("Up")),
            ],
        ));
        page_body(
            "Keybinds",
            Some("Click Change, then press the keys."),
            groups,
        )
    }
}
