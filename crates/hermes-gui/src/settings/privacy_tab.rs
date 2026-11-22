//! Privacy and safety.

use super::*;
use crate::ui::Segment;

const DM_FROM: [(DmFrom, &str); 3] = [
    (DmFrom::Everyone, "Everyone"),
    (DmFrom::Spaces, "People in my servers"),
    (DmFrom::Nobody, "Nobody"),
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
        let dm_from = privacy.dm_from;
        let privacy_rows = vec![
            setting(
                "Direct messages from",
                "Who can start a conversation with you.",
                crate::ui::segmented(
                    self,
                    "dm-from",
                    DM_FROM
                        .iter()
                        .map(|(_, name)| Segment::new(*name))
                        .collect(),
                    DM_FROM.iter().position(|(v, _)| *v == dm_from),
                    true,
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
            setting(
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
            setting(
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
