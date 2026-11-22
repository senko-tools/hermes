//! Privacy and safety.

use super::*;
use crate::ui::ButtonKind;

const DM_FROM: [(DmFrom, &str, &str); 3] = [
    (
        DmFrom::Everyone,
        "Everyone",
        "Anyone on this Hermes server.",
    ),
    (
        DmFrom::Spaces,
        "People in my servers",
        "Only people who share a server with you, and your friends.",
    ),
    (DmFrom::Nobody, "Nobody", "Only your friends."),
];

impl Shell {
    /// The account's server-side privacy and this device's keys, for the Privacy tab; empty when
    /// not signed in.
    pub(super) fn account_privacy_groups(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(inst) = self.account_instance() else {
            return Vec::new();
        };
        let privacy = {
            let store = self.core.store();
            match store.instance(&inst) {
                Some(s) if s.me.is_some() => s.privacy,
                _ => return Vec::new(),
            }
        };
        let many = self.core.instance_ids().len() > 1;
        let device = self.core.my_device_id(&inst).unwrap_or_default();
        let dm_from = privacy.dm_from;
        let privacy_rows = vec![
            setting_below(
                "Direct messages from",
                "Who can start a conversation with you.",
                crate::ui::radio_list(
                    self,
                    "dm-from",
                    DM_FROM
                        .iter()
                        .map(|(_, name, hint)| {
                            (SharedString::from(*name), Some(SharedString::from(*hint)))
                        })
                        .collect(),
                    DM_FROM.iter().position(|(v, ..)| *v == dm_from),
                    |this, n, _, cx| {
                        this.set_privacy(
                            UpdatePrivacyRequest {
                                dm_from: Some(DM_FROM[n].0),
                                ..UpdatePrivacyRequest::default()
                            },
                            cx,
                        );
                    },
                    cx,
                ),
            ),
            setting_fit(
                "Message requests",
                "Messages from anyone else wait in Requests. Off: they cannot message you.",
                {
                    let on = privacy.message_requests;
                    crate::ui::switch(
                        self,
                        "allow-requests",
                        on,
                        true,
                        move |this, _, cx| {
                            this.set_privacy(
                                UpdatePrivacyRequest {
                                    message_requests: Some(!on),
                                    ..UpdatePrivacyRequest::default()
                                },
                                cx,
                            );
                        },
                        cx,
                    )
                },
            ),
            setting_fit(
                "Show when I was last online",
                "On your profile, for people who share a server with you and your friends.",
                {
                    let on = privacy.show_last_online;
                    crate::ui::switch(
                        self,
                        "show-last-online",
                        on,
                        true,
                        move |this, _, cx| {
                            this.set_privacy(
                                UpdatePrivacyRequest {
                                    show_last_online: Some(!on),
                                    ..UpdatePrivacyRequest::default()
                                },
                                cx,
                            );
                        },
                        cx,
                    )
                },
            ),
        ];
        let title = if many {
            format!("Who can reach you on {inst}")
        } else {
            "Who can reach you".to_owned()
        };
        vec![
            group(&title, privacy_rows),
            group(
                "End-to-end encryption",
                vec![setting_fit(
                    "This device",
                    &format!(
                        "{} · only your and their devices can read DMs, files and calls.",
                        device.chars().take(16).collect::<String>()
                    ),
                    crate::ui::button(
                        self,
                        "device-id-copy",
                        self.copy_label("device-id-copy", "Copy ID"),
                        ButtonKind::Secondary,
                        !device.is_empty(),
                        move |this, _, cx| {
                            this.copy_from("device-id-copy", device.clone(), "Device ID", cx);
                        },
                        cx,
                    ),
                )],
            ),
        ]
    }

    pub(super) fn tab_privacy(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut groups = self.account_privacy_groups(cx);
        let device = group(
            "What this device tells the server",
            vec![
                flag!(
                    self,
                    cx,
                    "Show others when I am typing",
                    "",
                    privacy.send_typing
                ),
                flag!(
                    self,
                    cx,
                    "Sync what I have read",
                    "Off: the server is not told, and your other devices will not know either.",
                    privacy.send_read_marks
                ),
                setting_fit(
                    "Close the microphone while muted",
                    "The system then shows it as unused. Unmuting takes a moment longer.",
                    {
                        let on = self.config.voice.close_mic_when_muted;
                        crate::ui::switch(
                            self,
                            "close-mic",
                            on,
                            true,
                            move |this, _, cx| {
                                this.change_voice(cx, |voice| voice.close_mic_when_muted = !on);
                            },
                            cx,
                        )
                    },
                ),
                flag!(
                    self,
                    cx,
                    "Search on this device only",
                    "Older unencrypted messages are then not found.",
                    privacy.local_search_only
                ),
                setting_below(
                    "Warn about unencrypted chats",
                    "A popup when opening a chat the server can read. The note above \
                        the message box is always there.",
                    // Long choices: one under the other, not a bar across the card.
                    {
                        let current = self.config.privacy.warn_unencrypted;
                        crate::ui::radio_list(
                            self,
                            "settings-warn-unencrypted",
                            crate::unencrypted::WARN_CHOICES
                                .iter()
                                .map(|(_, label)| (SharedString::from(*label), None))
                                .collect(),
                            crate::unencrypted::WARN_CHOICES
                                .iter()
                                .position(|(v, _)| *v == current),
                            |this, n, _, cx| {
                                this.set_warn_unencrypted(
                                    crate::unencrypted::WARN_CHOICES[n].0,
                                    cx,
                                );
                            },
                            cx,
                        )
                    },
                ),
            ],
        );
        // Before the keys: what the server is told comes with who can reach you.
        let at = groups.len().min(1);
        groups.insert(at, device);
        page_body("Privacy & safety", None, groups)
    }
}
