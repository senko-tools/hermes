//! Privacy and safety.

use super::*;

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
        let dm_chips = [
            (DmFrom::Everyone, "Everyone"),
            (DmFrom::Spaces, "People in my servers"),
            (DmFrom::Nobody, "Nobody"),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (value, name))| {
            crate::ui::segment(self, ("dm-from", n), name, privacy.dm_from == value, cx).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.set_privacy(
                        UpdatePrivacyRequest {
                            dm_from: Some(value),
                            ..UpdatePrivacyRequest::default()
                        },
                        cx,
                    );
                }),
            )
        });
        let privacy_rows = vec![
            setting(
                "Direct messages from",
                "Who can start a conversation with you.",
                crate::ui::segments().children(dm_chips),
            ),
            setting(
                "Message requests",
                "Messages from anyone else wait in Requests. Off: they cannot message you.",
                Switch::new("allow-requests")
                    .cursor_pointer()
                    .checked(privacy.message_requests)
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.set_privacy(
                            UpdatePrivacyRequest {
                                message_requests: Some(*on),
                                ..UpdatePrivacyRequest::default()
                            },
                            cx,
                        );
                    })),
            ),
            setting(
                "Show when I was last online",
                "On your profile, for people who share a server with you and your friends.",
                Switch::new("show-last-online")
                    .cursor_pointer()
                    .checked(privacy.show_last_online)
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.set_privacy(
                            UpdatePrivacyRequest {
                                show_last_online: Some(*on),
                                ..UpdatePrivacyRequest::default()
                            },
                            cx,
                        );
                    })),
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
                vec![setting(
                    "This device",
                    "Only your and their devices can read DMs, files and calls.",
                    muted(
                        self.core
                            .my_device_id(&inst)
                            .map_or_else(String::new, |d| d.chars().take(16).collect::<String>()),
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
                setting(
                    "Close the microphone while muted",
                    "The system then shows it as unused. Unmuting takes a moment longer.",
                    Switch::new("close-mic")
                        .cursor_pointer()
                        .checked(self.config.voice.close_mic_when_muted)
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            let on = *on;
                            this.change_voice(cx, |voice| voice.close_mic_when_muted = on);
                        })),
                ),
                flag!(
                    self,
                    cx,
                    "Search on this device only",
                    "Older unencrypted messages are then not found.",
                    privacy.local_search_only
                ),
                setting(
                    "Warn about unencrypted chats",
                    "A popup when opening a chat the server can read. The note above \
                        the message box is always there.",
                    self.warn_unencrypted_choices("settings-warn-unencrypted", cx),
                ),
            ],
        );
        // Before the keys: what the server is told comes with who can reach you.
        let at = groups.len().min(1);
        groups.insert(at, device);
        page_body("Privacy & safety", None, groups)
    }
}
