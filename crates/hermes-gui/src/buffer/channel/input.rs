//! The input box (PLAN.md section 15.3): a multi-line editor with autocomplete, reply and edit
//! modes and attachments. One [`Composer`] per channel keeps the draft when switching channels.

use std::path::PathBuf;

use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Attachment;
use hermes_client_core::view::Suggestion;
use iced::keyboard::{self, key::Named};
use iced::widget::text_editor::{self, Binding, KeyPress};
use iced::widget::{Space, button, column, container, row, text_editor as editor};
use iced::{Alignment, Element, Fill, Length};

use super::Context;
use super::message::human_size;
use crate::theme::style;
use crate::widget::icon::{self, icon};
use crate::widget::{label, small};

/// Suggestions shown at most.
pub const MAX_SUGGESTIONS: usize = 8;

/// A file being attached.
#[derive(Debug, Clone, PartialEq)]
pub struct Upload {
    pub local: u64,
    pub name: String,
    pub size: u64,
    pub state: UploadState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UploadState {
    Uploading,
    Ready(Attachment),
    Failed(String),
}

/// The draft of one channel.
#[derive(Debug)]
pub struct Composer {
    pub editor_id: iced::widget::Id,
    pub content: text_editor::Content,
    pub reply_to: Option<Id>,
    /// Editing this message instead of sending a new one.
    pub editing: Option<Id>,
    pub uploads: Vec<Upload>,
    pub suggestions: Vec<Suggestion>,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub enum Message {
    Edit(text_editor::Action),
    Submit,
    /// Moves the autocomplete selection.
    Suggest(i32),
    /// Accepts suggestion `n`.
    Accept(usize),
    /// Escape: closes the popup, else leaves reply/edit mode.
    Cancel,
    /// Up in an empty box: edit my last message.
    EditLast,
    /// Ctrl+V: an image from the clipboard, else text.
    Paste,
    PickFiles,
    RemoveUpload(u64),
}

impl Default for Composer {
    fn default() -> Self {
        Composer {
            editor_id: iced::widget::Id::unique(),
            content: text_editor::Content::new(),
            reply_to: None,
            editing: None,
            uploads: Vec::new(),
            suggestions: Vec::new(),
            selected: 0,
        }
    }
}

impl Composer {
    /// The draft without trailing whitespace (what is sent).
    pub fn text(&self) -> String {
        self.content.text().trim_end().to_owned()
    }

    pub fn is_empty(&self) -> bool {
        self.text().trim().is_empty()
    }

    pub fn clear(&mut self) {
        self.content = text_editor::Content::new();
        self.reply_to = None;
        self.editing = None;
        self.uploads.clear();
        self.suggestions.clear();
    }

    pub fn set_text(&mut self, text: &str) {
        self.content = text_editor::Content::with_text(text);
        self.content
            .perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
    }

    /// The token being typed: the text from the last whitespace before the cursor to the cursor,
    /// if it starts with `@`, `#`, `:` (two or more characters) or `/` (at the very start).
    pub fn token(&self) -> Option<String> {
        let cursor = self.content.cursor();
        if cursor.selection.is_some() {
            return None;
        }
        let line = self.content.line(cursor.position.line)?;
        let before = prefix_at(&line.text, cursor.position.column);
        token_in(before, cursor.position.line == 0)
    }

    /// Replaces the token before the cursor with `insert`.
    pub fn replace_token(&mut self, token: &str, insert: &str) {
        for _ in 0..token.chars().count() {
            self.content
                .perform(text_editor::Action::Select(text_editor::Motion::Left));
        }
        self.content
            .perform(text_editor::Action::Edit(text_editor::Edit::Paste(
                std::sync::Arc::new(insert.to_owned()),
            )));
    }

    /// Attachments that finished uploading; `Err` while some are still uploading.
    pub fn ready_attachments(&self) -> Result<Vec<Attachment>, &'static str> {
        let mut out = Vec::new();
        for u in &self.uploads {
            match &u.state {
                UploadState::Ready(a) => out.push(a.clone()),
                UploadState::Uploading => return Err("Wait until the files are uploaded."),
                UploadState::Failed(_) => {}
            }
        }
        Ok(out)
    }
}

/// The part of `line` before byte or char column `column` (cosmic-text counts bytes; anything
/// that is not a char boundary is moved back to one).
fn prefix_at(line: &str, column: usize) -> &str {
    let mut end = column.min(line.len());
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    &line[..end]
}

fn token_in(before: &str, first_line: bool) -> Option<String> {
    let start = before.rfind(char::is_whitespace).map_or(0, |i| {
        i + before[i..].chars().next().map_or(1, char::len_utf8)
    });
    let token = &before[start..];
    let first = token.chars().next()?;
    let ok = match first {
        '@' | '#' => true,
        ':' => token.chars().count() >= 3,
        '/' => first_line && start == 0,
        _ => false,
    };
    ok.then(|| token.to_owned())
}

/// What a key does in the editor: send, newline, autocomplete keys, edit-last and paste.
pub fn binding(
    key_press: KeyPress,
    enter_sends: bool,
    suggesting: bool,
    empty: bool,
) -> Option<Binding<Message>> {
    // Every editor is asked, focused or not; only the focused one may act (or Enter would send
    // the draft of every pane).
    if !matches!(key_press.status, text_editor::Status::Focused { .. }) {
        return None;
    }
    let modifiers = key_press.modifiers;
    match key_press.key.as_ref() {
        keyboard::Key::Named(Named::Enter) => {
            if suggesting && !modifiers.shift() {
                return Some(Binding::Custom(Message::Accept(usize::MAX)));
            }
            let send = if enter_sends {
                !modifiers.shift()
            } else {
                modifiers.control() || modifiers.shift()
            };
            Some(if send {
                Binding::Custom(Message::Submit)
            } else {
                Binding::Enter
            })
        }
        keyboard::Key::Named(Named::Tab) if suggesting => {
            Some(Binding::Custom(Message::Accept(usize::MAX)))
        }
        keyboard::Key::Named(Named::ArrowUp) if suggesting => {
            Some(Binding::Custom(Message::Suggest(-1)))
        }
        keyboard::Key::Named(Named::ArrowDown) if suggesting => {
            Some(Binding::Custom(Message::Suggest(1)))
        }
        keyboard::Key::Named(Named::ArrowUp) if empty => Some(Binding::Custom(Message::EditLast)),
        keyboard::Key::Named(Named::Escape) => Some(Binding::Custom(Message::Cancel)),
        keyboard::Key::Character("v") if modifiers.command() && !modifiers.shift() => {
            Some(Binding::Custom(Message::Paste))
        }
        _ => Binding::from_key_press(key_press),
    }
}

pub fn view<'a>(
    ctx: &Context<'a>,
    composer: Option<&'a Composer>,
    placeholder: String,
    can_send: bool,
    reply_name: Option<String>,
) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    // Every part has a fixed place (empty when unused), so the editor never moves in the tree
    // and keeps its focus when the popup or a bar appears.
    let mut col = column![].padding(iced::Padding {
        top: 2.0,
        bottom: 14.0,
        left: 16.0,
        right: 16.0,
    });
    let Some(composer) = composer else {
        return col.into();
    };
    if !can_send {
        return container(
            row![
                icon(icon::LOCK, 14.0).color(theme.text.tertiary.0),
                small("You cannot send messages in this channel.", fonts)
                    .color(theme.text.tertiary.0),
            ]
            .spacing(style::GAP)
            .align_y(Alignment::Center),
        )
        .padding([14, 20])
        .into();
    }

    let popup: Element<'a, Message> = if composer.suggestions.is_empty() {
        Space::new().into()
    } else {
        let mut list = column![].spacing(2);
        for (i, s) in composer.suggestions.iter().enumerate() {
            let selected = i == composer.selected;
            list = list.push(
                button(label(s.label.clone(), fonts))
                    .width(Fill)
                    .padding(style::ROW_PAD)
                    .style(move |_, st| style::sidebar_item(theme, st, selected))
                    .on_press(Message::Accept(i)),
            );
        }
        column![
            container(list)
                .padding(6)
                .width(Fill)
                .style(move |_| style::popover(theme)),
            Space::new().height(6)
        ]
        .into()
    };
    col = col.push(popup);

    let mode = match (&composer.editing, &reply_name) {
        (Some(_), _) => Some((icon::PENCIL, "Editing message".to_owned())),
        (None, Some(name)) => Some((icon::REPLY, format!("Replying to {name}"))),
        _ => None,
    };
    // The reply/edit bar sits inside the composer, above a hairline, so both read as one field.
    let hairline = move || {
        container(Space::new().height(1))
            .width(Fill)
            .style(move |_| container::Style {
                background: Some(theme.general.border.0.into()),
                ..container::Style::default()
            })
    };
    let mode_bar: Element<'a, Message> = match mode {
        None => Space::new().into(),
        Some((glyph, mode)) => column![
            row![
                icon(glyph, 16.0).color(theme.buttons.primary.background.0),
                small(mode, fonts)
                    .font(fonts.medium())
                    .color(theme.text.secondary.0)
                    .width(Fill),
                button(icon(icon::CLOSE, 16.0))
                    .padding(4)
                    .style(move |_, st| style::ghost(theme, st, false))
                    .on_press(Message::Cancel),
            ]
            .spacing(style::GAP)
            .align_y(Alignment::Center)
            .padding([2, 8]),
            hairline(),
        ]
        .spacing(4)
        .into(),
    };

    let uploads: Element<'a, Message> = if composer.uploads.is_empty() {
        Space::new().into()
    } else {
        let mut chips = row![].spacing(style::GAP);
        for u in &composer.uploads {
            let (state, color) = match &u.state {
                UploadState::Uploading => ("uploading…".to_owned(), theme.text.tertiary.0),
                UploadState::Ready(_) => (human_size(u.size), theme.text.tertiary.0),
                UploadState::Failed(e) => (format!("failed: {e}"), theme.text.error.0),
            };
            chips = chips.push(
                container(
                    row![
                        icon(icon::FILE, 14.0).color(theme.text.secondary.0),
                        small(u.name.clone(), fonts),
                        small(state, fonts).color(color),
                        button(icon(icon::CLOSE, 12.0))
                            .padding(3)
                            .style(move |_, st| style::ghost(theme, st, false))
                            .on_press(Message::RemoveUpload(u.local)),
                    ]
                    .spacing(style::GAP_S)
                    .align_y(Alignment::Center),
                )
                .padding([4, 10])
                .style(move |_| style::panel(theme)),
            );
        }
        column![container(chips.wrap()).padding([4, 6]), hairline(),]
            .spacing(4)
            .into()
    };

    let enter_sends = ctx.config.buffer.enter_sends;
    let suggesting = !composer.suggestions.is_empty();
    let empty = composer.content.is_empty() || composer.is_empty();
    let input = editor(&composer.content)
        .id(composer.editor_id.clone())
        .placeholder(placeholder)
        .on_action(Message::Edit)
        .key_binding(move |kp| binding(kp, enter_sends, suggesting, empty))
        .font(fonts.ui)
        .size(fonts.size + 1.0)
        // A one-line editor is exactly as tall as the attach button (34 px), so the first line
        // and the button share a center.
        .padding([
            ((style::ICON_BUTTON - (fonts.size + 1.0) * 1.3) / 2.0).max(4.0),
            4.0,
        ])
        .max_height((fonts.size + 1.0) * 1.3 * 10.0 + style::ICON_BUTTON)
        .height(Length::Shrink)
        .style(move |_, st| style::editor(theme, st));
    let attach = crate::widget::tip_at(
        crate::widget::icon_button(icon::ATTACH, false, theme).on_press(Message::PickFiles),
        "Attach files (or drop them here)",
        iced::widget::tooltip::Position::Top,
        fonts,
        theme,
    );
    let field = container(
        column![
            mode_bar,
            uploads,
            row![attach, input]
                .spacing(style::GAP_XS)
                .align_y(Alignment::Start),
        ]
        .spacing(0),
    )
    .padding(4)
    .width(Fill)
    .style(move |_| style::composer(theme, false));
    col = col.push(field);
    col.into()
}

/// Files to upload: paths that exist and are files.
pub fn files_only(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.into_iter().filter(|p| p.is_file()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::{Key, Modifiers, key::Physical};

    #[test]
    fn tokens() {
        assert_eq!(token_in("hi @bo", true).as_deref(), Some("@bo"));
        assert_eq!(token_in("hi @", true).as_deref(), Some("@"));
        assert_eq!(token_in("see #gen", false).as_deref(), Some("#gen"));
        assert_eq!(token_in("nice :sm", true).as_deref(), Some(":sm"));
        assert_eq!(
            token_in("time 12:3", true),
            None,
            "a colon mid-word is no emoji"
        );
        assert_eq!(token_in("nice :s", true), None, "too short for emoji");
        assert_eq!(token_in("/me", true).as_deref(), Some("/me"));
        assert_eq!(
            token_in("/me", false),
            None,
            "commands only start the message"
        );
        assert_eq!(token_in("a /me", true), None);
        assert_eq!(token_in("hello", true), None);
        assert_eq!(token_in("", true), None);
        assert_eq!(prefix_at("héllo", 2), "h", "a column inside é moves back");
    }

    fn press(key: Key, modifiers: Modifiers) -> KeyPress {
        KeyPress {
            key: key.clone(),
            modified_key: key,
            physical_key: Physical::Unidentified(keyboard::key::NativeCode::Unidentified),
            modifiers,
            text: None,
            status: text_editor::Status::Focused { is_hovered: false },
        }
    }

    fn is(b: Option<Binding<Message>>, want: &str) -> bool {
        let got = match b {
            Some(Binding::Custom(Message::Submit)) => "submit",
            Some(Binding::Custom(Message::Accept(_))) => "accept",
            Some(Binding::Custom(Message::Suggest(_))) => "suggest",
            Some(Binding::Custom(Message::EditLast)) => "edit-last",
            Some(Binding::Custom(Message::Paste)) => "paste",
            Some(Binding::Enter) => "newline",
            _ => "other",
        };
        got == want
    }

    #[test]
    fn enter_shift_enter_and_popups() {
        let enter = Key::Named(Named::Enter);
        assert!(is(
            binding(press(enter.clone(), Modifiers::empty()), true, false, false),
            "submit"
        ));
        assert!(is(
            binding(press(enter.clone(), Modifiers::SHIFT), true, false, false),
            "newline"
        ));
        // enter_sends = false: Enter is a new line, Ctrl/Shift+Enter sends.
        assert!(is(
            binding(
                press(enter.clone(), Modifiers::empty()),
                false,
                false,
                false
            ),
            "newline"
        ));
        assert!(is(
            binding(press(enter.clone(), Modifiers::CTRL), false, false, false),
            "submit"
        ));
        // With suggestions, Enter/Tab accept and arrows move.
        assert!(is(
            binding(press(enter, Modifiers::empty()), true, true, false),
            "accept"
        ));
        assert!(is(
            binding(
                press(Key::Named(Named::Tab), Modifiers::empty()),
                true,
                true,
                false
            ),
            "accept"
        ));
        assert!(is(
            binding(
                press(Key::Named(Named::ArrowDown), Modifiers::empty()),
                true,
                true,
                false
            ),
            "suggest"
        ));
        // Up in an empty box edits the last message; otherwise it moves the cursor.
        assert!(is(
            binding(
                press(Key::Named(Named::ArrowUp), Modifiers::empty()),
                true,
                false,
                true
            ),
            "edit-last"
        ));
        assert!(is(
            binding(
                press(Key::Named(Named::ArrowUp), Modifiers::empty()),
                true,
                false,
                false
            ),
            "other"
        ));
        assert!(is(
            binding(
                press(Key::Character("v".into()), Modifiers::CTRL),
                true,
                false,
                false
            ),
            "paste"
        ));
    }

    #[test]
    fn unfocused_editors_ignore_keys() {
        let mut kp = press(Key::Named(Named::Enter), Modifiers::empty());
        kp.status = text_editor::Status::Active;
        assert!(
            binding(kp, true, false, false).is_none(),
            "Enter must not send other panes"
        );
        let mut kp = press(Key::Character("v".into()), Modifiers::CTRL);
        kp.status = text_editor::Status::Hovered;
        assert!(binding(kp, true, false, false).is_none());
    }

    #[test]
    fn uploads_must_finish_before_sending() {
        let mut c = Composer::default();
        assert_eq!(c.ready_attachments(), Ok(vec![]));
        c.uploads.push(Upload {
            local: 1,
            name: "a".into(),
            size: 1,
            state: UploadState::Uploading,
        });
        assert!(c.ready_attachments().is_err());
        c.uploads[0].state = UploadState::Failed("x".into());
        assert_eq!(
            c.ready_attachments(),
            Ok(vec![]),
            "failed uploads are left out"
        );
    }

    #[test]
    fn text_tokens_and_replacing() {
        let mut c = Composer::default();
        c.set_text("hello @bo");
        assert_eq!(c.token().as_deref(), Some("@bo"));
        c.replace_token("@bo", "<@3> ");
        assert!(c.content.text().starts_with("hello <@3> "));
        assert_eq!(c.text(), "hello <@3>", "sent without the trailing space");
        assert_eq!(c.token(), None);
        c.clear();
        assert!(c.is_empty());
    }
}
