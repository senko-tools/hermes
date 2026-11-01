//! A small dialog with a few fields (PLAN.md section 15.4): create space, create channel, edit
//! channel, ban, nickname, invite, profile. What happens on submit depends on its [`Purpose`]
//! (see `app::forms`).

use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;
use iced::widget::{button, checkbox, column, pick_list, row, space, text_input};
use iced::{Alignment, Element, Fill};

use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::{heading, label, small};

/// The first text field gets the focus when the dialog opens.
pub const FIRST_INPUT: &str = "form-first-input";

/// What the form is for, with everything the submit needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    CreateSpace(InstanceId),
    /// A text or voice channel, at the top level or in a category.
    CreateChannel {
        inst: InstanceId,
        space: Id,
        parent: Option<Id>,
    },
    CreateCategory {
        inst: InstanceId,
        space: Id,
    },
    EditChannel {
        inst: InstanceId,
        channel: Id,
    },
    Ban {
        inst: InstanceId,
        space: Id,
        user: Id,
    },
    Nickname {
        inst: InstanceId,
        space: Id,
        user: Id,
    },
    CreateInvite {
        inst: InstanceId,
        space: Id,
    },
    EditProfile(InstanceId),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    Text {
        label: String,
        hint: String,
        placeholder: &'static str,
        value: String,
    },
    Choice {
        label: String,
        hint: String,
        options: Vec<String>,
        selected: String,
    },
    Check {
        label: String,
        hint: String,
        value: bool,
    },
}

impl Field {
    pub fn text(
        label: impl Into<String>,
        placeholder: &'static str,
        value: impl Into<String>,
    ) -> Field {
        Field::Text {
            label: label.into(),
            hint: String::new(),
            placeholder,
            value: value.into(),
        }
    }

    pub fn choice(label: impl Into<String>, options: &[&str], selected: &str) -> Field {
        Field::Choice {
            label: label.into(),
            hint: String::new(),
            options: options.iter().map(|o| (*o).to_owned()).collect(),
            selected: selected.to_owned(),
        }
    }

    pub fn check(label: impl Into<String>, value: bool) -> Field {
        Field::Check {
            label: label.into(),
            hint: String::new(),
            value,
        }
    }

    /// The same field with a hint under it.
    pub fn hint(mut self, text: impl Into<String>) -> Field {
        match &mut self {
            Field::Text { hint, .. } | Field::Choice { hint, .. } | Field::Check { hint, .. } => {
                *hint = text.into();
            }
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub purpose: Purpose,
    pub title: String,
    pub text: String,
    pub fields: Vec<Field>,
    pub submit: &'static str,
    /// The submit button is red (ban).
    pub danger: bool,
    pub busy: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Input(usize, String),
    Choose(usize, String),
    Check(usize, bool),
    Submit,
    Cancel,
}

impl Form {
    pub fn new(purpose: Purpose, title: impl Into<String>, submit: &'static str) -> Form {
        Form {
            purpose,
            title: title.into(),
            text: String::new(),
            fields: Vec::new(),
            submit,
            danger: false,
            busy: false,
            error: None,
        }
    }

    pub fn text(mut self, text: impl Into<String>) -> Form {
        self.text = text.into();
        self
    }

    pub fn field(mut self, field: Field) -> Form {
        self.fields.push(field);
        self
    }

    pub fn danger(mut self) -> Form {
        self.danger = true;
        self
    }

    /// Applies an edit; returns `true` for Submit (Cancel is handled by the app).
    pub fn update(&mut self, message: Message) -> bool {
        match message {
            Message::Input(i, text) => {
                if let Some(Field::Text { value, .. }) = self.fields.get_mut(i) {
                    *value = text;
                }
            }
            Message::Choose(i, choice) => {
                if let Some(Field::Choice { selected, .. }) = self.fields.get_mut(i) {
                    *selected = choice;
                }
            }
            Message::Check(i, on) => {
                if let Some(Field::Check { value, .. }) = self.fields.get_mut(i) {
                    *value = on;
                }
            }
            Message::Submit => return !self.busy,
            Message::Cancel => {}
        }
        self.error = None;
        false
    }

    /// The trimmed value of text field `i` (empty if it is not one).
    pub fn value(&self, i: usize) -> &str {
        match self.fields.get(i) {
            Some(Field::Text { value, .. }) => value.trim(),
            _ => "",
        }
    }

    /// The selected option of choice field `i`.
    pub fn selected(&self, i: usize) -> &str {
        match self.fields.get(i) {
            Some(Field::Choice { selected, .. }) => selected,
            _ => "",
        }
    }

    pub fn checked(&self, i: usize) -> bool {
        matches!(self.fields.get(i), Some(Field::Check { value: true, .. }))
    }

    pub fn view<'a>(&'a self, fonts: &Fonts, theme: &'a Theme) -> Element<'a, Message> {
        let mut content = column![heading(self.title.as_str(), fonts)].spacing(12);
        if !self.text.is_empty() {
            content = content.push(label(self.text.as_str(), fonts).color(theme.text.secondary.0));
        }
        let mut first_text = true;
        for (i, field) in self.fields.iter().enumerate() {
            let (name, hint, control): (&str, &str, Element<'a, Message>) = match field {
                Field::Text {
                    label: name,
                    hint,
                    placeholder,
                    value,
                } => {
                    let mut input = text_input(placeholder, value)
                        .on_input(move |t| Message::Input(i, t))
                        .on_submit(Message::Submit)
                        .font(fonts.ui)
                        .size(fonts.size)
                        .padding(8);
                    if first_text {
                        input = input.id(FIRST_INPUT);
                        first_text = false;
                    }
                    (name, hint, input.into())
                }
                Field::Choice {
                    label: name,
                    hint,
                    options,
                    selected,
                } => (
                    name,
                    hint,
                    pick_list(options.as_slice(), Some(selected.clone()), move |o| {
                        Message::Choose(i, o)
                    })
                    .font(fonts.ui)
                    .text_size(fonts.size)
                    .width(Fill)
                    .into(),
                ),
                Field::Check {
                    label: name,
                    hint,
                    value,
                } => (
                    "",
                    hint,
                    checkbox(*value)
                        .label(name.as_str())
                        .on_toggle(move |on| Message::Check(i, on))
                        .font(fonts.ui)
                        .text_size(fonts.size)
                        .into(),
                ),
            };
            let mut block = column![].spacing(4);
            if !name.is_empty() {
                block = block.push(small(name, fonts).color(theme.text.secondary.0));
            }
            block = block.push(control);
            if !hint.is_empty() {
                block = block.push(small(hint, fonts).color(theme.text.tertiary.0));
            }
            content = content.push(block);
        }
        if let Some(error) = &self.error {
            content = content.push(small(error.as_str(), fonts).color(theme.text.error.0));
        }
        let danger = self.danger;
        content
            .push(
                row![
                    space::horizontal(),
                    button(label("Cancel", fonts))
                        .style(move |_, s| style::secondary(theme, s))
                        .on_press(Message::Cancel),
                    button(label(
                        if self.busy {
                            "Please wait…"
                        } else {
                            self.submit
                        },
                        fonts
                    ))
                    .style(move |_, s| if danger {
                        style::danger(theme, s)
                    } else {
                        style::primary(theme, s)
                    })
                    .on_press_maybe((!self.busy).then_some(Message::Submit)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .width(400)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_reach_the_right_field() {
        let mut form = Form::new(Purpose::CreateSpace("h".into()), "Create a space", "Create")
            .field(Field::text("Name", "", ""))
            .field(Field::choice("Kind", &["Text", "Voice"], "Text"))
            .field(Field::check("Private", false));
        assert!(!form.update(Message::Input(0, "  Friends ".into())));
        form.update(Message::Choose(1, "Voice".into()));
        form.update(Message::Check(2, true));
        assert_eq!(form.value(0), "Friends");
        assert_eq!(form.selected(1), "Voice");
        assert!(form.checked(2));
        assert_eq!(form.value(1), "", "not a text field");
        assert!(form.update(Message::Submit));
        form.busy = true;
        assert!(!form.update(Message::Submit), "no double submit");
    }
}
