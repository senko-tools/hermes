//! Keybinds (PLAN.md section 15.6): every action has a default, and `[keys]` in `config.toml`
//! overrides it (`command_bar = "Ctrl+K"`). Combos are written `Ctrl+Shift+M`, `Alt+Down`,
//! `Ctrl+\`, `Ctrl+,`, `Escape`, `F5`.
//!
//! A punctuation key in a combo also matches the key in its place on a US keyboard, so
//! `Ctrl+\` works on layouts where `\` needs AltGr (approved, see docs/DECISIONS.md).

use std::collections::BTreeMap;

use iced::keyboard::key::{Code, Named, Physical};
use iced::keyboard::{self, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    CommandBar,
    NextUnread,
    PrevUnread,
    NextChannel,
    PrevChannel,
    FocusNextPane,
    FocusPrevPane,
    SplitRight,
    SplitDown,
    ClosePane,
    MaximizePane,
    ToggleMemberList,
    ToggleSidebar,
    Search,
    MarkRead,
    ToggleMute,
    ToggleDeafen,
    Settings,
}

/// Every action: config name, default combo and the name shown in the command bar.
pub const ACTIONS: [(Action, &str, &str, &str); 18] = [
    (Action::CommandBar, "command_bar", "Ctrl+K", "Command bar"),
    (
        Action::NextUnread,
        "next_unread",
        "Alt+Shift+Down",
        "Next unread channel",
    ),
    (
        Action::PrevUnread,
        "prev_unread",
        "Alt+Shift+Up",
        "Previous unread channel",
    ),
    (
        Action::NextChannel,
        "next_channel",
        "Alt+Down",
        "Next channel",
    ),
    (
        Action::PrevChannel,
        "prev_channel",
        "Alt+Up",
        "Previous channel",
    ),
    (
        Action::FocusNextPane,
        "focus_next_pane",
        "Ctrl+Tab",
        "Focus next pane",
    ),
    (
        Action::FocusPrevPane,
        "focus_prev_pane",
        "Ctrl+Shift+Tab",
        "Focus previous pane",
    ),
    (
        Action::SplitRight,
        "split_right",
        "Ctrl+\\",
        "Split pane right",
    ),
    (
        Action::SplitDown,
        "split_down",
        "Ctrl+Shift+\\",
        "Split pane down",
    ),
    (Action::ClosePane, "close_pane", "Ctrl+W", "Close pane"),
    (
        Action::MaximizePane,
        "maximize_pane",
        "Ctrl+Shift+F",
        "Maximize or restore pane",
    ),
    (
        Action::ToggleMemberList,
        "toggle_member_list",
        "Ctrl+U",
        "Toggle member list",
    ),
    (
        Action::ToggleSidebar,
        "toggle_sidebar",
        "Ctrl+B",
        "Toggle sidebar",
    ),
    (Action::Search, "search", "Ctrl+F", "Search"),
    (Action::MarkRead, "mark_read", "Escape", "Mark channel read"),
    (
        Action::ToggleMute,
        "toggle_mute",
        "Ctrl+Shift+M",
        "Toggle mute",
    ),
    (
        Action::ToggleDeafen,
        "toggle_deafen",
        "Ctrl+Shift+D",
        "Toggle deafen",
    ),
    (Action::Settings, "settings", "Ctrl+,", "Open settings"),
];

/// A key: a character (lowercase) or a named key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyName {
    Char(String),
    Named(Named),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
    pub key: KeyName,
}

impl Combo {
    /// Parses `Ctrl+Shift+M`; `None` if it does not parse.
    pub fn parse(text: &str) -> Option<Combo> {
        let text = text.trim();
        // The last part is the key; it may itself be `+`.
        let (mods, key) = match text.strip_suffix("++") {
            Some(rest) => (rest, "+"),
            None => match text.rsplit_once('+') {
                Some((mods, key)) => (mods, key),
                None => ("", text),
            },
        };
        let mut combo = Combo {
            ctrl: false,
            shift: false,
            alt: false,
            logo: false,
            key: key_name(key)?,
        };
        for part in mods.split('+').filter(|p| !p.is_empty()) {
            match part.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => combo.ctrl = true,
                "shift" => combo.shift = true,
                "alt" => combo.alt = true,
                "super" | "logo" | "win" | "cmd" => combo.logo = true,
                _ => return None,
            }
        }
        Some(combo)
    }

    /// Whether a key press matches. `key` is the key without modifiers applied; `physical` is
    /// its place on the keyboard.
    pub fn matches(&self, key: &keyboard::Key, physical: Physical, modifiers: Modifiers) -> bool {
        if modifiers.control() != self.ctrl
            || modifiers.shift() != self.shift
            || modifiers.alt() != self.alt
            || modifiers.logo() != self.logo
        {
            return false;
        }
        match (&self.key, key) {
            (KeyName::Char(c), keyboard::Key::Character(k)) if k.to_lowercase() == *c => true,
            (KeyName::Char(c), _) => {
                us_position(c).is_some_and(|code| physical == Physical::Code(code))
            }
            (KeyName::Named(n), keyboard::Key::Named(k)) => n == k,
            _ => false,
        }
    }
}

/// Where a punctuation character is on a US keyboard.
fn us_position(c: &str) -> Option<Code> {
    Some(match c {
        "\\" => Code::Backslash,
        "," => Code::Comma,
        "." => Code::Period,
        "/" => Code::Slash,
        ";" => Code::Semicolon,
        "'" => Code::Quote,
        "[" => Code::BracketLeft,
        "]" => Code::BracketRight,
        "-" => Code::Minus,
        "=" => Code::Equal,
        "`" => Code::Backquote,
        _ => return None,
    })
}

fn key_name(text: &str) -> Option<KeyName> {
    let text = text.trim();
    let named = match text.to_ascii_lowercase().as_str() {
        "tab" => Named::Tab,
        "enter" | "return" => Named::Enter,
        "escape" | "esc" => Named::Escape,
        "space" => Named::Space,
        "up" => Named::ArrowUp,
        "down" => Named::ArrowDown,
        "left" => Named::ArrowLeft,
        "right" => Named::ArrowRight,
        "pageup" => Named::PageUp,
        "pagedown" => Named::PageDown,
        "home" => Named::Home,
        "end" => Named::End,
        "delete" => Named::Delete,
        "backspace" => Named::Backspace,
        "f1" => Named::F1,
        "f2" => Named::F2,
        "f3" => Named::F3,
        "f4" => Named::F4,
        "f5" => Named::F5,
        "f6" => Named::F6,
        "f7" => Named::F7,
        "f8" => Named::F8,
        "f9" => Named::F9,
        "f10" => Named::F10,
        "f11" => Named::F11,
        "f12" => Named::F12,
        _ => {
            let mut chars = text.chars();
            return match (chars.next(), chars.next()) {
                (Some(c), None) if !c.is_whitespace() => {
                    Some(KeyName::Char(c.to_lowercase().collect()))
                }
                _ => None,
            };
        }
    };
    Some(KeyName::Named(named))
}

const NO_CODE: keyboard::key::NativeCode = keyboard::key::NativeCode::Unidentified;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keybinds {
    binds: Vec<(Combo, Action)>,
}

impl Keybinds {
    /// The defaults, overridden by `[keys]`. An empty value unbinds an action.
    pub fn from_config(keys: &BTreeMap<String, String>) -> (Keybinds, Vec<String>) {
        let mut warnings = Vec::new();
        for name in keys.keys() {
            if !ACTIONS.iter().any(|(_, n, ..)| n == name) {
                warnings.push(format!("[keys] unknown action `{name}` (ignored)"));
            }
        }
        let mut binds = Vec::new();
        for (action, name, default, _) in ACTIONS {
            let text = keys.get(name).map_or(default, String::as_str);
            if text.trim().is_empty() {
                continue;
            }
            match Combo::parse(text) {
                Some(combo) => binds.push((combo, action)),
                None => {
                    warnings.push(format!(
                        "[keys] `{name} = \"{text}\"` is not a key combo; using {default}"
                    ));
                    binds.extend(Combo::parse(default).map(|c| (c, action)));
                }
            }
        }
        (Keybinds { binds }, warnings)
    }

    pub fn action(
        &self,
        key: &keyboard::Key,
        physical: Physical,
        modifiers: Modifiers,
    ) -> Option<Action> {
        // A key that types the character wins over one in its US place.
        self.binds
            .iter()
            .find(|(combo, _)| combo.matches(key, Physical::Unidentified(NO_CODE), modifiers))
            .or_else(|| {
                self.binds
                    .iter()
                    .find(|(combo, _)| combo.matches(key, physical, modifiers))
            })
            .map(|(_, action)| *action)
    }

    /// The combo bound to `action`, for display.
    pub fn combo_text(keys: &BTreeMap<String, String>, action: Action) -> Option<String> {
        let (_, name, default, _) = ACTIONS.iter().find(|(a, ..)| *a == action)?;
        let text = keys.get(*name).map_or(*default, String::as_str);
        (!text.trim().is_empty()).then(|| text.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(c: &str) -> keyboard::Key {
        keyboard::Key::Character(c.into())
    }

    const ANY: Physical = Physical::Unidentified(NO_CODE);

    #[test]
    fn combos_parse() {
        let c = Combo::parse("Ctrl+Shift+M").unwrap();
        assert!(c.ctrl && c.shift && !c.alt);
        assert_eq!(c.key, KeyName::Char("m".into()));
        assert_eq!(
            Combo::parse("Ctrl+\\").unwrap().key,
            KeyName::Char("\\".into())
        );
        assert_eq!(
            Combo::parse("ctrl+,").unwrap().key,
            KeyName::Char(",".into())
        );
        assert_eq!(
            Combo::parse("Ctrl++").unwrap().key,
            KeyName::Char("+".into())
        );
        assert_eq!(
            Combo::parse("Alt+Shift+Down").unwrap().key,
            KeyName::Named(Named::ArrowDown)
        );
        assert_eq!(Combo::parse("F5").unwrap().key, KeyName::Named(Named::F5));
        for bad in ["", "Ctrl+", "Hyper+K", "Ctrl+Foo"] {
            assert_eq!(Combo::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn defaults_and_overrides() {
        let (keys, warnings) = Keybinds::from_config(&BTreeMap::new());
        assert!(warnings.is_empty());
        assert_eq!(
            keys.action(&ch("k"), ANY, Modifiers::CTRL),
            Some(Action::CommandBar)
        );
        assert_eq!(
            keys.action(&ch("K"), ANY, Modifiers::CTRL),
            Some(Action::CommandBar)
        );
        assert_eq!(
            keys.action(&ch("k"), ANY, Modifiers::CTRL | Modifiers::SHIFT),
            None
        );
        // Shift+\ is reported as `\` without modifiers applied.
        assert_eq!(
            keys.action(&ch("\\"), ANY, Modifiers::CTRL | Modifiers::SHIFT),
            Some(Action::SplitDown)
        );
        assert_eq!(
            keys.action(&keyboard::Key::Named(Named::Tab), ANY, Modifiers::CTRL),
            Some(Action::FocusNextPane)
        );

        let config: BTreeMap<String, String> = [
            ("command_bar", "Ctrl+P"),
            ("toggle_sidebar", ""),
            ("close_pane", "Ctrl+Nope"),
            ("fly", "F1"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_owned(), b.to_owned()))
        .collect();
        let (keys, warnings) = Keybinds::from_config(&config);
        assert_eq!(
            keys.action(&ch("p"), ANY, Modifiers::CTRL),
            Some(Action::CommandBar)
        );
        assert_eq!(keys.action(&ch("k"), ANY, Modifiers::CTRL), None);
        assert_eq!(keys.action(&ch("b"), ANY, Modifiers::CTRL), None, "unbound");
        assert_eq!(
            keys.action(&ch("w"), ANY, Modifiers::CTRL),
            Some(Action::ClosePane)
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(
            Keybinds::combo_text(&config, Action::CommandBar).as_deref(),
            Some("Ctrl+P")
        );
    }

    #[test]
    fn punctuation_also_matches_its_us_position() {
        let (keys, _) = Keybinds::from_config(&BTreeMap::new());
        // Finnish layout: the key left of Enter types `'` but sits where US has `\`.
        assert_eq!(
            keys.action(&ch("'"), Physical::Code(Code::Backslash), Modifiers::CTRL),
            Some(Action::SplitRight)
        );
        // Letters only match what they type (AZERTY's `a` is not Ctrl+Q).
        assert_eq!(
            keys.action(&ch("a"), Physical::Code(Code::KeyQ), Modifiers::CTRL),
            None
        );
        // A key that types the character wins over a US position.
        let config: BTreeMap<String, String> = [("settings", "Ctrl+'")]
            .into_iter()
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
            .collect();
        let (keys, _) = Keybinds::from_config(&config);
        assert_eq!(
            keys.action(&ch("'"), Physical::Code(Code::Backslash), Modifiers::CTRL),
            Some(Action::Settings)
        );
    }
}
