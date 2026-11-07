//! Sample data for the layout shell: nothing here talks to a server yet.

/// Consecutive messages of one author within this many minutes show their name once (the
/// iced client's default, `view::is_grouped` in `hermes-client-core`).
const GROUP_MINUTES: u32 = 7;

/// A conversation or channel that has a message list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChatId(pub usize);

/// A voice channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VoiceId(pub usize);

#[derive(Clone, Debug)]
pub struct Message {
    pub author: String,
    /// Minutes since the start of the sample's day.
    pub minute: u32,
    pub text: String,
}

impl Message {
    pub fn time(&self) -> String {
        format!("{:02}:{:02}", self.minute / 60 % 24, self.minute % 60)
    }
}

#[derive(Clone, Debug)]
pub struct Chat {
    pub name: String,
    /// A DM or group DM, not a space's channel.
    pub dm: bool,
    pub messages: Vec<Message>,
    pub unread: u32,
}

impl Chat {
    /// Whether message `ix` continues the group of the one before it: no name or avatar.
    pub fn is_grouped(&self, ix: usize) -> bool {
        let Some(previous) = ix.checked_sub(1).map(|p| &self.messages[p]) else {
            return false;
        };
        let message = &self.messages[ix];
        previous.author == message.author
            && message.minute.saturating_sub(previous.minute) <= GROUP_MINUTES
    }

    /// The window's title while this chat is open.
    pub fn title(&self) -> String {
        if self.dm {
            self.name.clone()
        } else {
            format!("#{}", self.name)
        }
    }
}

#[derive(Clone, Debug)]
pub struct VoiceRoom {
    pub name: String,
    pub people: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaceItem {
    Text(ChatId),
    Voice(VoiceId),
}

#[derive(Clone, Debug)]
pub struct Space {
    pub name: String,
    pub items: Vec<SpaceItem>,
}

impl Space {
    /// The first text channel, opened when the space is selected.
    pub fn first_chat(&self) -> Option<ChatId> {
        self.items.iter().find_map(|item| match item {
            SpaceItem::Text(id) => Some(*id),
            SpaceItem::Voice(_) => None,
        })
    }

    /// The two letters on its rail icon.
    pub fn initials(&self) -> String {
        self.name.chars().take(2).collect()
    }
}

#[derive(Clone, Debug)]
pub struct Person {
    pub name: String,
    pub status: String,
    pub dm: ChatId,
}

pub struct Data {
    pub me: String,
    pub chats: Vec<Chat>,
    pub voice: Vec<VoiceRoom>,
    pub spaces: Vec<Space>,
    /// Message requests: chats waiting for an answer.
    pub requests: Vec<ChatId>,
    /// Recent DMs and group DMs, newest first.
    pub recent: Vec<ChatId>,
    pub people: Vec<Person>,
}

impl Data {
    pub fn chat(&self, id: ChatId) -> &Chat {
        &self.chats[id.0]
    }

    /// What the Friends icon's badge shows: everything unread in DMs and requests.
    pub fn dm_unread(&self) -> u32 {
        self.requests
            .iter()
            .chain(&self.recent)
            .map(|id| self.chat(*id).unread)
            .sum()
    }

    /// Marks a chat as read, as opening it does.
    pub fn mark_read(&mut self, id: ChatId) {
        self.chats[id.0].unread = 0;
    }

    /// Adds my message to a chat. Returns false when there is nothing to send.
    pub fn send(&mut self, id: ChatId, text: &str) -> bool {
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        let author = self.me.clone();
        let messages = &mut self.chats[id.0].messages;
        let minute = messages.last().map_or(720, |m| m.minute);
        messages.push(Message {
            author,
            minute,
            text: text.into(),
        });
        true
    }

    /// The sample world. `no_instances` is the state with no saved instance: nothing to show.
    pub fn sample(no_instances: bool) -> Self {
        if no_instances {
            return Self {
                me: "you".into(),
                chats: Vec::new(),
                voice: Vec::new(),
                spaces: Vec::new(),
                requests: Vec::new(),
                recent: Vec::new(),
                people: Vec::new(),
            };
        }
        let chat = |name: &str, dm: bool, unread: u32, count: usize| Chat {
            name: name.into(),
            dm,
            messages: messages(count),
            unread,
        };
        let chats = vec![
            chat("general", false, 0, 200),
            chat("memes", false, 0, 40),
            chat("announcements", false, 0, 5),
            chat("study-notes", false, 0, 25),
            chat("Carol", true, 1, 12),
            chat("Alice, Bob", true, 0, 30),
            chat("Dave", true, 2, 6),
            chat("Erin", true, 0, 0),
            chat("Frank", true, 1, 1),
        ];
        let voice = vec![
            VoiceRoom {
                name: "Lounge".into(),
                people: vec!["Alice".into(), "Bob".into()],
            },
            VoiceRoom {
                name: "Gaming".into(),
                people: Vec::new(),
            },
            VoiceRoom {
                name: "Library".into(),
                people: vec!["Dave".into()],
            },
        ];
        let spaces = vec![
            Space {
                name: "Hangout".into(),
                items: vec![
                    SpaceItem::Text(ChatId(0)),
                    SpaceItem::Text(ChatId(1)),
                    SpaceItem::Text(ChatId(2)),
                    SpaceItem::Voice(VoiceId(0)),
                    SpaceItem::Voice(VoiceId(1)),
                ],
            },
            Space {
                name: "Study Group".into(),
                items: vec![SpaceItem::Text(ChatId(3)), SpaceItem::Voice(VoiceId(2))],
            },
        ];
        let person = |name: &str, status: &str, dm: usize| Person {
            name: name.into(),
            status: status.into(),
            dm: ChatId(dm),
        };
        Self {
            me: "you".into(),
            chats,
            voice,
            spaces,
            requests: vec![ChatId(8)],
            recent: vec![ChatId(4), ChatId(5), ChatId(6)],
            people: vec![
                person("Carol", "Online", 4),
                person("Dave", "In voice: Library", 6),
                person("Erin", "Offline", 7),
            ],
        }
    }
}

/// Authors take turns writing a few messages each, so grouping can be seen.
fn messages(count: usize) -> Vec<Message> {
    const AUTHORS: [&str; 3] = ["Alice", "Bob", "Carol"];
    const LINES: [&str; 4] = [
        "Sample message, nothing here comes from a server yet.",
        "Short one.",
        "A longer sample message, so that wrapping and rows of different heights can be seen \
         in the list while it scrolls and while the window or the sidebar is being resized.",
        "Another line.\nWith a second line under it.",
    ];
    (0..count)
        .map(|i| Message {
            author: AUTHORS[i / 3 % AUTHORS.len()].into(),
            minute: 720 + i as u32,
            text: LINES[i % LINES.len()].into(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(author: &str, minute: u32) -> Message {
        Message {
            author: author.into(),
            minute,
            text: String::new(),
        }
    }

    #[test]
    fn the_badge_counts_dms_and_requests_and_opening_clears_it() {
        let mut data = Data::sample(false);
        assert_eq!(data.dm_unread(), 4);
        data.mark_read(ChatId(6));
        assert_eq!(data.dm_unread(), 2);
    }

    #[test]
    fn sending_trims_and_ignores_empty_text() {
        let mut data = Data::sample(false);
        let before = data.chat(ChatId(7)).messages.len();
        assert!(!data.send(ChatId(7), " \n "));
        assert_eq!(data.chat(ChatId(7)).messages.len(), before);
        assert!(data.send(ChatId(7), " hi\n"));
        assert_eq!(data.chat(ChatId(7)).messages[before].text, "hi");
    }

    #[test]
    fn no_instances_means_nothing_to_show() {
        let data = Data::sample(true);
        assert!(data.spaces.is_empty());
        assert_eq!(data.dm_unread(), 0);
    }

    #[test]
    fn messages_group_by_author_within_the_time_limit() {
        let chat = Chat {
            name: "x".into(),
            dm: false,
            messages: vec![
                message("a", 0),
                message("a", 7),
                message("a", 15),
                message("b", 15),
            ],
            unread: 0,
        };
        let grouped: Vec<bool> = (0..4).map(|ix| chat.is_grouped(ix)).collect();
        assert_eq!(grouped, [false, true, false, false]);
    }

    #[test]
    fn titles_mark_channels_but_not_dms() {
        let data = Data::sample(false);
        assert_eq!(data.chat(ChatId(0)).title(), "#general");
        assert_eq!(data.chat(ChatId(4)).title(), "Carol");
    }

    #[test]
    fn a_space_opens_its_first_text_channel() {
        let data = Data::sample(false);
        assert_eq!(data.spaces[1].first_chat(), Some(ChatId(3)));
        assert_eq!(data.spaces[1].initials(), "St");
    }
}
