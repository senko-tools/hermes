//! "New message": start a DM or group DM with people I share a space with (PLAN.md section
//! 15.1, the "+" of the Direct messages section).

use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;
use hermes_client_core::store::InstanceStore;
use iced::widget::{button, column, row, space, text_input};
use iced::{Alignment, Element};

use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::{heading, label, small};

pub const INPUT: &str = "new-dm-input";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDm {
    pub inst: InstanceId,
    pub input: String,
    pub error: Option<String>,
    pub busy: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Input(String),
    Submit,
    Cancel,
}

impl NewDm {
    pub fn new(inst: InstanceId) -> NewDm {
        NewDm {
            inst,
            input: String::new(),
            error: None,
            busy: false,
        }
    }

    pub fn view<'a>(&'a self, fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
        let mut content = column![
            heading("New message", fonts),
            label(
                "Usernames of people you share a space with, separated by commas.",
                fonts,
            ),
            text_input("alice, bob", &self.input)
                .padding(style::INPUT_PAD)
                .style(move |_, st| style::input(theme, st))
                .id(INPUT)
                .on_input(Message::Input)
                .on_submit(Message::Submit)
                .font(fonts.ui)
                .size(fonts.size),
        ]
        .spacing(12);
        if let Some(error) = &self.error {
            content = content.push(small(error.as_str(), fonts).color(theme.text.error.0));
        }
        content
            .push(
                row![
                    space::horizontal(),
                    button(label("Cancel", fonts))
                        .padding(style::BUTTON_PAD)
                        .style(move |_, s| style::secondary(theme, s))
                        .on_press(Message::Cancel),
                    button(label(
                        if self.busy { "Please wait…" } else { "Start" },
                        fonts
                    ))
                    .padding(style::BUTTON_PAD)
                    .style(move |_, s| style::primary(theme, s))
                    .on_press_maybe((!self.busy).then_some(Message::Submit)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .into()
    }
}

/// Turns "alice, Bob" into user IDs, by username or display name (case-insensitive), among the
/// users the store knows. I am left out.
pub fn resolve(store: &InstanceStore, input: &str) -> Result<Vec<Id>, String> {
    let me = store.me_id();
    let mut ids = Vec::new();
    let mut unknown = Vec::new();
    for name in input.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let name = name.trim_start_matches('@');
        let mut matches = store.users.values().filter(|u| {
            Some(u.id) != me
                && (u.username.eq_ignore_ascii_case(name)
                    || u.display_name
                        .as_deref()
                        .is_some_and(|d| d.eq_ignore_ascii_case(name)))
        });
        // A username match wins over a display name.
        let found = store
            .users
            .values()
            .find(|u| Some(u.id) != me && u.username.eq_ignore_ascii_case(name))
            .or_else(|| matches.next());
        match found {
            Some(user) if !ids.contains(&user.id) => ids.push(user.id),
            Some(_) => {}
            None => unknown.push(name.to_owned()),
        }
    }
    if !unknown.is_empty() {
        return Err(format!("Not found: {}", unknown.join(", ")));
    }
    if ids.is_empty() {
        return Err("Enter at least one username.".into());
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn names_resolve_to_known_users() {
        let store = fixtures::store();
        assert_eq!(resolve(&store, "carol"), Ok(vec![Id(4)]));
        assert_eq!(
            resolve(&store, " @Bob , bobby, CAROL "),
            Ok(vec![Id(3), Id(4)]),
            "username or display name, any case, no duplicates"
        );
        assert_eq!(
            resolve(&store, "carol, dave, me"),
            Err("Not found: dave, me".into()),
            "I am not a recipient"
        );
        assert!(resolve(&store, " , ").is_err());
    }
}
