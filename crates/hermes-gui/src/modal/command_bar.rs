//! The command bar (PLAN.md section 15.1, Ctrl+K): fuzzy search over channels, DMs, users and
//! actions.

use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::{InstanceId, Store, view};
use iced::widget::{button, column, container, row, scrollable, stack, text_input};
use iced::{Alignment, Element, Fill, Padding};

use crate::font::Fonts;
use crate::keybinds::{ACTIONS, Action};
use crate::theme::{Theme, style};
use crate::widget::icon::{self, icon};
use crate::widget::{caption, label};

pub const INPUT: &str = "command-bar-input";
const MAX_SHOWN: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Open(InstanceId, Id),
    /// Open (or start) the DM with a user.
    MessageUser(InstanceId, Id),
    Action(Action),
    Theme(String),
    AddInstance,
    JoinVoice(InstanceId, Id),
    LeaveVoice,
    SpaceSettings(InstanceId, Id),
    Results(crate::buffer::results::Key),
    /// A menu action (create space, create channel, edit profile, …).
    Menu(crate::sidebar::menu::Target, crate::sidebar::menu::Action),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    /// Shown dimmed on the right: the space or instance, or the key combo.
    pub detail: String,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandBar {
    pub input: String,
    entries: Vec<Entry>,
    /// Indexes into `entries` matching `input`, best first.
    shown: Vec<usize>,
    pub selected: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Input(String),
    Run(usize),
    Submit,
    Close,
}

impl CommandBar {
    pub fn new(entries: Vec<Entry>) -> CommandBar {
        let mut bar = CommandBar {
            input: String::new(),
            entries,
            shown: Vec::new(),
            selected: 0,
        };
        bar.filter();
        bar
    }

    pub fn set_input(&mut self, input: String) {
        self.input = input;
        self.filter();
    }

    fn filter(&mut self) {
        let mut scored: Vec<(i32, usize)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| score(&self.input, &e.label).map(|s| (s, i)))
            .collect();
        // Best score first; ties keep the entry order.
        scored.sort_by_key(|(s, i)| (std::cmp::Reverse(*s), *i));
        self.shown = scored.into_iter().take(MAX_SHOWN).map(|(_, i)| i).collect();
        self.selected = 0;
    }

    pub fn shown(&self) -> impl Iterator<Item = &Entry> {
        self.shown.iter().map(|i| &self.entries[*i])
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.shown.is_empty() {
            return;
        }
        let len = self.shown.len() as i32;
        self.selected = (self.selected as i32 + delta).rem_euclid(len) as usize;
    }

    /// The command of the `n`th shown entry.
    pub fn command(&self, n: usize) -> Option<Command> {
        self.shown.get(n).map(|i| self.entries[*i].command.clone())
    }

    pub fn view<'a>(&'a self, fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
        let mut list = column![].spacing(2).padding(iced::Padding::ZERO.right(12));
        for (n, entry) in self.shown().enumerate() {
            let selected = n == self.selected;
            list = list.push(
                button(
                    row![
                        label(entry.label.as_str(), fonts).width(Fill),
                        caption(entry.detail.as_str(), fonts).color(theme.text.tertiary.0),
                    ]
                    .spacing(style::GAP_S)
                    .align_y(Alignment::Center),
                )
                .width(Fill)
                .padding([style::GAP_S, style::GAP_S])
                .style(move |_, s| style::sidebar_item(theme, s, selected))
                .on_press(Message::Run(n)),
            );
        }
        if self.shown.is_empty() {
            list = list.push(
                container(caption("Nothing matches", fonts).color(theme.text.tertiary.0))
                    .padding(style::GAP_S),
            );
        }
        // The search icon is drawn over the input's left padding: `text_input::Icon` only
        // takes font glyphs, and the icons are SVGs.
        const SEARCH_ICON: f32 = 16.0;
        let input = text_input("Jump to a channel, a person or an action…", &self.input)
            .padding(Padding {
                left: style::INPUT_PAD.left + SEARCH_ICON + style::GAP_S,
                ..style::INPUT_PAD
            })
            .style(move |_, st| style::input(theme, st))
            .id(INPUT)
            .on_input(Message::Input)
            .on_submit(Message::Submit)
            .font(fonts.ui)
            .size(fonts.size);
        let search = container(icon(icon::SEARCH, SEARCH_ICON).color(theme.text.tertiary.0))
            .padding(Padding {
                left: style::INPUT_PAD.left,
                ..Padding::ZERO
            })
            .center_y(Fill);
        column![
            stack![input, search],
            scrollable(list)
                .style(move |_, st| style::scrollbar(theme, st))
                .height(360),
        ]
        .spacing(style::GAP)
        .into()
    }
}

/// Fuzzy score of `query` against `text`: `None` unless every query character appears in order
/// (case-insensitive). Matches at word starts and runs of consecutive matches score higher, gaps
/// and a long text lower.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut score = 0;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (ti, c) in text.iter().enumerate() {
        if qi == query.len() {
            break;
        }
        if *c != query[qi] {
            continue;
        }
        let word_start = ti == 0 || !text[ti - 1].is_alphanumeric();
        score += 10;
        if word_start {
            score += 15;
        }
        match last {
            Some(l) if l + 1 == ti => score += 10,
            Some(l) => score -= (ti - l - 1).min(10) as i32,
            None => score -= ti.min(10) as i32,
        }
        last = Some(ti);
        qi += 1;
    }
    (qi == query.len()).then_some(score - (text.len() as i32) / 10)
}

/// Every entry: channels and DMs of each instance, users to message, actions and themes.
pub fn entries(
    store: &Store,
    instances: &[InstanceId],
    themes: &[String],
    keys: &std::collections::BTreeMap<String, String>,
) -> Vec<Entry> {
    let mut out = Vec::new();
    for inst in instances {
        let Some(s) = store.instance(inst) else {
            continue;
        };
        use crate::sidebar::menu::{Action as M, Target};
        out.push(Entry {
            label: "Create a space".into(),
            detail: inst.clone(),
            command: Command::Menu(Target::Instance(inst.clone()), M::CreateSpace),
        });
        out.push(Entry {
            label: "Edit profile".into(),
            detail: inst.clone(),
            command: Command::Menu(Target::Instance(inst.clone()), M::EditProfile),
        });
        out.push(Entry {
            label: "Search everywhere".into(),
            detail: inst.clone(),
            command: Command::Results(crate::buffer::results::Key::Search(inst.clone(), None)),
        });
        for (space_id, space) in &s.spaces {
            out.push(Entry {
                label: format!("Search in {}", space.space.name),
                detail: inst.clone(),
                command: Command::Results(crate::buffer::results::Key::Search(
                    inst.clone(),
                    Some(*space_id),
                )),
            });
            out.push(Entry {
                label: format!("Space settings: {}", space.space.name),
                detail: inst.clone(),
                command: Command::SpaceSettings(inst.clone(), *space_id),
            });
            if view::can_in_space(
                s,
                *space_id,
                hermes_client_core::proto::permissions::Permissions::MANAGE_CHANNELS,
            ) {
                out.push(Entry {
                    label: format!("Create channel in {}", space.space.name),
                    detail: inst.clone(),
                    command: Command::Menu(
                        Target::Space(inst.clone(), *space_id),
                        M::CreateChannel,
                    ),
                });
            }
            if view::can_in_space(
                s,
                *space_id,
                hermes_client_core::proto::permissions::Permissions::CREATE_INVITES,
            ) {
                out.push(Entry {
                    label: format!("Invite people to {}", space.space.name),
                    detail: inst.clone(),
                    command: Command::Menu(Target::Space(inst.clone(), *space_id), M::InvitePeople),
                });
            }
            for c in view::channel_tree(s, *space_id) {
                if c.kind == ChannelKind::Category {
                    continue;
                }
                let prefix = if c.kind == ChannelKind::Voice {
                    "♪ "
                } else {
                    "#"
                };
                out.push(Entry {
                    label: format!("{prefix}{}", c.name),
                    detail: space.space.name.clone(),
                    command: Command::Open(inst.clone(), c.id),
                });
                if c.kind == ChannelKind::Voice {
                    out.push(Entry {
                        label: format!("Join voice: {}", c.name),
                        detail: space.space.name.clone(),
                        command: Command::JoinVoice(inst.clone(), c.id),
                    });
                }
            }
        }
        for dm in view::dm_list(s) {
            out.push(Entry {
                label: dm.name,
                detail: format!("Direct message · {inst}"),
                command: Command::Open(inst.clone(), dm.id),
            });
        }
        let me = s.me_id();
        let mut users: Vec<_> = s.users.values().filter(|u| Some(u.id) != me).collect();
        users.sort_by(|a, b| a.username.cmp(&b.username));
        for user in users {
            out.push(Entry {
                label: format!("Message @{}", user.username),
                detail: inst.clone(),
                command: Command::MessageUser(inst.clone(), user.id),
            });
        }
    }
    out.push(Entry {
        label: "Mentions".into(),
        detail: "Messages that mention you".into(),
        command: Command::Results(crate::buffer::results::Key::Mentions),
    });
    for (action, _, _, title) in ACTIONS {
        if matches!(action, Action::CommandBar | Action::MarkRead) {
            continue;
        }
        out.push(Entry {
            label: title.to_owned(),
            detail: crate::keybinds::Keybinds::combo_text(keys, action).unwrap_or_default(),
            command: Command::Action(action),
        });
    }
    out.push(Entry {
        label: "Leave voice".into(),
        detail: String::new(),
        command: Command::LeaveVoice,
    });
    out.push(Entry {
        label: "Add an instance".into(),
        detail: String::new(),
        command: Command::AddInstance,
    });
    for theme in themes {
        out.push(Entry {
            label: format!("Theme: {theme}"),
            detail: String::new(),
            command: Command::Theme(theme.clone()),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn fuzzy_scores() {
        assert!(score("gen", "#general").is_some());
        assert!(score("GNRL", "#general").is_some());
        assert!(score("lg", "#general").is_none());
        assert!(score("", "anything").is_some());
        // Word starts and consecutive letters beat scattered ones.
        assert!(score("sp", "Split pane right").unwrap() > score("sp", "Display").unwrap());
        assert!(score("gen", "#general").unwrap() > score("gen", "#greenhouse-energy").unwrap());
    }

    #[test]
    fn entries_cover_channels_dms_users_actions_and_themes() {
        let mut store = Store::default();
        store.instances.insert("h".into(), fixtures::store());
        let entries = entries(
            &store,
            &["h".into()],
            &["dark".into(), "light".into()],
            &Default::default(),
        );
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        for want in [
            "#general",
            "#chess",
            "♪ Lounge",
            "carol",
            "Trio",
            "Message @bob",
            "Split pane right",
            "Theme: light",
            "Join voice: Lounge",
            "Toggle mute",
            "Leave voice",
        ] {
            assert!(labels.contains(&want), "{want} missing from {labels:?}");
        }
        assert!(!labels.contains(&"#Games"), "categories are not entries");
        assert!(!labels.contains(&"Message @me"));

        let mut bar = CommandBar::new(entries);
        bar.set_input("chess".into());
        assert_eq!(bar.command(0), Some(Command::Open("h".into(), Id(13))));
        bar.set_input("split right".into());
        assert_eq!(bar.command(0), Some(Command::Action(Action::SplitRight)));
        bar.move_selection(-1);
        assert_eq!(bar.selected, bar.shown().count() - 1, "wraps around");
    }
}
