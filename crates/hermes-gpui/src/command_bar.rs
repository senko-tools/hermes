//! The command bar (PLAN.md section 15.1, Ctrl+K): fuzzy search over channels, DMs, users and
//! actions.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Entity, Subscription, Window, deferred, div};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::proto::permissions::Permissions;
use hermes_client_core::{InstanceId, Store, view};

use crate::keybinds::{ACTIONS, Action, Keybinds};
use crate::menu_entries::{Action as M, Target};
use crate::results::Key;
use crate::shell::Shell;
use crate::space_settings::Tab as SpaceTab;
use crate::theme::{self, px};

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
    Results(Key),
    /// A menu action (create space, create channel, edit profile, …).
    Menu(Target, M),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    /// Shown dimmed on the right: the space or instance, or the key combo.
    pub detail: String,
    pub command: Command,
}

pub struct CommandBar {
    input: Entity<InputState>,
    entries: Vec<Entry>,
    /// Indexes into `entries` matching the input, best first.
    shown: Vec<usize>,
    pub selected: usize,
    _events: Subscription,
}

impl CommandBar {
    fn filter(&mut self, query: &str) {
        let mut scored: Vec<(i32, usize)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| score(query, &e.label).map(|s| (s, i)))
            .collect();
        // Best score first; ties keep the entry order.
        scored.sort_by_key(|(s, i)| (std::cmp::Reverse(*s), *i));
        self.shown = scored.into_iter().take(MAX_SHOWN).map(|(_, i)| i).collect();
        self.selected = 0;
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.shown.is_empty() {
            return;
        }
        let len = self.shown.len() as i32;
        self.selected = (self.selected as i32 + delta).rem_euclid(len) as usize;
    }

    /// The command of the `n`th shown entry.
    fn command(&self, n: usize) -> Option<Command> {
        self.shown.get(n).map(|i| self.entries[*i].command.clone())
    }
}

impl Shell {
    pub fn open_command_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = {
            let store = self.core.store();
            entries(
                &store,
                &self.core.instance_ids(),
                &self.themes,
                &self.config.keys,
            )
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Jump to a channel, a person or an action…")
        });
        let events = cx.subscribe_in(
            &input,
            window,
            |this, input, event, window, cx| match event {
                InputEvent::Change => {
                    let query = input.read(cx).value().to_string();
                    if let Some(bar) = &mut this.command_bar {
                        bar.filter(&query);
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    let chosen = this.command_bar.as_ref().map_or(0, |bar| bar.selected);
                    this.run_command_bar(chosen, window, cx);
                }
                _ => {}
            },
        );
        input.update(cx, |input, cx| input.focus(window, cx));
        let mut bar = CommandBar {
            input,
            entries,
            shown: Vec::new(),
            selected: 0,
            _events: events,
        };
        bar.filter("");
        self.command_bar = Some(bar);
        cx.notify();
    }

    pub fn run_command_bar(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let command = self.command_bar.as_ref().and_then(|bar| bar.command(n));
        self.command_bar = None;
        match command {
            Some(Command::Open(inst, channel)) => self.select(inst, channel, window, cx),
            Some(Command::MessageUser(inst, user)) => self.message_user(inst, user, window, cx),
            Some(Command::Action(action)) => self.key_action(action, window, cx),
            Some(Command::Theme(name)) => {
                self.persist("general.theme", &toml::Value::String(name).to_string(), cx);
                self.reload_theme(cx);
            }
            Some(Command::AddInstance) => self.open_add_instance(String::new(), window, cx),
            Some(Command::JoinVoice(inst, channel)) => self.join_voice(inst, channel, cx),
            Some(Command::LeaveVoice) => self.leave_voice(cx),
            Some(Command::SpaceSettings(inst, space)) => {
                self.open_space_settings(inst, space, SpaceTab::Overview, window, cx);
            }
            Some(Command::Results(key)) => self.open_results(key, window, cx),
            Some(Command::Menu(target, action)) => self.menu_action(target, action, window, cx),
            None => {}
        }
        cx.notify();
    }

    pub fn render_command_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bar = self.command_bar.as_ref()?;
        let rows: Vec<AnyElement> = bar
            .shown
            .iter()
            .enumerate()
            .map(|(n, index)| {
                let entry = &bar.entries[*index];
                div()
                    .id(("command", n))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(n == bar.selected, |d| d.bg(theme::selected()))
                    .when(n != bar.selected, |d| d.hover(|d| d.bg(theme::hover())))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::small_size())
                            .child(entry.label.clone()),
                    )
                    .child(
                        div()
                            .text_size(theme::tiny_size())
                            .text_color(theme::text_tertiary())
                            .child(entry.detail.clone()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_command_bar(n, window, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        Some(
            deferred(
                div()
                    .id("command-bar-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .pt(px(80.))
                    .bg(theme::scrim())
                    .occlude()
                    .font_family(theme::font())
                    .text_color(theme::text())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.command_bar = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .id("command-bar")
                            .occlude()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .w(px(560.))
                            .max_h(px(460.))
                            .p(px(12.))
                            .rounded(px(10.))
                            .bg(theme::background())
                            .border_1()
                            .border_color(theme::border())
                            .on_click(|_, _, cx| cx.stop_propagation())
                            .child(Input::new(&bar.input))
                            .child(
                                div()
                                    .id("command-bar-rows")
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h_0()
                                    .gap(px(2.))
                                    .overflow_y_scroll()
                                    .children(rows)
                                    .when(empty, |d| {
                                        d.child(crate::settings::muted("Nothing matches"))
                                    }),
                            ),
                    ),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
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
            command: Command::Results(Key::Search(inst.clone(), None)),
        });
        for (space_id, space) in &s.spaces {
            out.push(Entry {
                label: format!("Search in {}", space.space.name),
                detail: inst.clone(),
                command: Command::Results(Key::Search(inst.clone(), Some(*space_id))),
            });
            out.push(Entry {
                label: format!("Space settings: {}", space.space.name),
                detail: inst.clone(),
                command: Command::SpaceSettings(inst.clone(), *space_id),
            });
            if view::can_in_space(s, *space_id, Permissions::MANAGE_CHANNELS) {
                out.push(Entry {
                    label: format!("Create channel in {}", space.space.name),
                    detail: inst.clone(),
                    command: Command::Menu(
                        Target::Space(inst.clone(), *space_id),
                        M::CreateChannel,
                    ),
                });
            }
            if view::can_in_space(s, *space_id, Permissions::CREATE_INVITES) {
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
        command: Command::Results(Key::Mentions),
    });
    for (action, _, _, title) in ACTIONS {
        // Split panes come later.
        if matches!(
            action,
            Action::CommandBar
                | Action::MarkRead
                | Action::FocusNextPane
                | Action::FocusPrevPane
                | Action::SplitRight
                | Action::SplitDown
                | Action::ClosePane
                | Action::MaximizePane
        ) {
            continue;
        }
        out.push(Entry {
            label: title.to_owned(),
            detail: Keybinds::combo_text(keys, action).unwrap_or_default(),
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
            "♪ Lounge",
            "Join voice: Lounge",
            "Trio",
            "Message @carol",
            "Open settings",
            "Theme: light",
            "Mentions",
        ] {
            assert!(labels.contains(&want), "{want} missing from {labels:?}");
        }
    }
}
