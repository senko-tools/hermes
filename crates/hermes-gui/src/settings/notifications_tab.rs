//! Notifications and keybinds.

use super::*;
use crate::ui::ButtonKind;

impl Shell {
    pub(super) fn tab_notifications(&self, cx: &mut Context<Self>) -> AnyElement {
        let n = &self.config.notifications;
        let desktop = group(
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
                flag!(
                    self,
                    cx,
                    "Flash the taskbar button",
                    "",
                    notifications.flash_taskbar
                ),
            ],
        );
        let test_sounds = |cx: &mut Context<Self>| {
            vec![setting_fit(
                "Test sounds",
                "Messages and DMs play the softer one, mentions the ping.",
                div()
                    .flex()
                    .gap(px(8.))
                    .child(crate::ui::button(
                        self,
                        "test-sound-message",
                        "Message",
                        ButtonKind::Secondary,
                        true,
                        |this, _, _| {
                            this.core.play_sound(Sound::Message);
                        },
                        cx,
                    ))
                    .child(crate::ui::button(
                        self,
                        "test-sound-mention",
                        "Mention",
                        ButtonKind::Secondary,
                        true,
                        |this, _, _| {
                            this.core.play_sound(Sound::Ping);
                        },
                        cx,
                    )),
            )]
        };
        let messages = group(
            "Messages",
            vec![
                flag!(
                    self,
                    cx,
                    "Play a sound",
                    "Off: no message makes a sound.",
                    notifications.sound
                ),
                self.pop_ups_and_sounds(cx),
                self.more("notification-sounds", "Try the sounds", test_sounds, cx),
            ],
        );
        let when = group(
            "When and what",
            vec![
                setting_fit(
                    "Quiet hours",
                    &if n.quiet_from.is_empty() {
                        "None".to_owned()
                    } else {
                        format!("{} to {}", n.quiet_from, n.quiet_to)
                    },
                    crate::ui::button(
                        self,
                        "quiet-hours",
                        "Change",
                        ButtonKind::Secondary,
                        true,
                        |this, window, cx| {
                            this.open_form_quiet_hours(window, cx);
                        },
                        cx,
                    ),
                ),
                setting_fit(
                    "Words that notify me",
                    &if n.keywords.is_empty() {
                        "None".to_owned()
                    } else {
                        n.keywords.clone()
                    },
                    crate::ui::button(
                        self,
                        "keywords",
                        "Change",
                        ButtonKind::Secondary,
                        true,
                        |this, window, cx| {
                            this.open_form_keywords(window, cx);
                        },
                        cx,
                    ),
                ),
            ],
        );
        page_body(
            "Notifications",
            Some("Per server and channel: right-click it, Notifications."),
            vec![desktop, messages, when],
        )
    }

    /// Which messages pop up and which play a sound: one table, a column for each kind of
    /// message (#64), instead of six rows.
    fn pop_ups_and_sounds(&self, cx: &mut Context<Self>) -> AnyElement {
        const ROW: &str = "Pop-ups and sounds";
        type Field = fn(&mut config::Notifications) -> &mut bool;
        type Line = (&'static str, &'static str, [(&'static str, Field); 3]);
        let kinds = ["DMs", "Mentions", "Other messages"];
        let lines: [Line; 2] = [
            (
                "Pop up",
                "Pop up for",
                [
                    ("toast-dms", |n| &mut n.toast_dms),
                    ("toast-mentions", |n| &mut n.toast_mentions),
                    ("toast-other", |n| &mut n.toast_other),
                ],
            ),
            (
                "Sound",
                "Sound for",
                [
                    ("sound-dms", |n| &mut n.sound_dms),
                    ("sound-mentions", |n| &mut n.sound_mentions),
                    ("sound-other", |n| &mut n.sound_other),
                ],
            ),
        ];
        let label_width = 90.;
        let cell_width = 120.;
        let caption = |text: &'static str| {
            div()
                .w(px(cell_width))
                .flex_shrink_0()
                .flex()
                .justify_center()
                .text_size(theme::secondary_size())
                .text_color(theme::text_secondary())
                .child(text)
        };
        let header = div()
            .flex()
            .items_center()
            .child(div().w(px(label_width)).flex_shrink_0())
            .children(kinds.map(caption));
        let mut table = div().flex().flex_col().gap(px(8.)).child(header);
        let mut current = self.config.notifications.clone();
        for (label, search_name, cells) in lines {
            let mut line = div().flex().items_center().child(
                div()
                    .w(px(label_width))
                    .flex_shrink_0()
                    .text_size(theme::body_size())
                    .child(label),
            );
            for ((id, field), kind) in cells.into_iter().zip(kinds) {
                note_part(&format!("{search_name} {}", kind.to_lowercase()), "", ROW);
                let on = *field(&mut current);
                line = line.child(
                    div()
                        .w(px(cell_width))
                        .flex_shrink_0()
                        .flex()
                        .justify_center()
                        .child(crate::ui::switch(
                            self,
                            id,
                            on,
                            true,
                            move |this, _, cx| {
                                this.set_config(|c| *field(&mut c.notifications) = !on, cx);
                            },
                            cx,
                        )),
                );
            }
            table = table.child(line);
        }
        setting_below(
            ROW,
            "Which messages pop up, and which play a sound.",
            div().overflow_x_hidden().child(table),
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
                                .child(crate::ui::button(
                                    self,
                                    format!("bind-{name}"),
                                    if waiting { "Waiting…" } else { "Change" },
                                    ButtonKind::Secondary,
                                    true,
                                    move |this, _, cx| {
                                        if let Some(page) = &mut this.settings {
                                            page.capturing_key = Some(name);
                                        }
                                        cx.notify();
                                    },
                                    cx,
                                ))
                                .child(crate::ui::button(
                                    self,
                                    format!("unbind-{name}"),
                                    "Reset",
                                    ButtonKind::Secondary,
                                    true,
                                    move |this, _, cx| {
                                        this.set_config(
                                            |c| {
                                                c.keys.insert(name.to_owned(), default.to_owned());
                                            },
                                            cx,
                                        );
                                    },
                                    cx,
                                )),
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
