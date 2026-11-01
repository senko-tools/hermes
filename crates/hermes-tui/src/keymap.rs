//! Normal-mode keys (PLAN.md section 16.2), rebindable in `[tui.keys]` as `action = "key"`.
//! Keys are written like `J`, `m`, `Ctrl+u`, `Tab`, `Enter`, `Esc`, `Up`, or two-key sequences
//! like `gg`.

use std::collections::{BTreeMap, HashMap};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    MoveDown,
    MoveUp,
    FocusLeft,
    FocusRight,
    FocusNext,
    Open,
    JoinVoice,
    Insert,
    Reply,
    Edit,
    Delete,
    React,
    Retry,
    Top,
    Bottom,
    PageUp,
    PageDown,
    ToggleMute,
    ToggleDeafen,
    LeaveVoice,
    ToggleMembers,
    Search,
    Command,
    Help,
    Quit,
    /// The actions of the selection (the TUI's right-click).
    Menu,
}

/// `(action, config name, default key)`.
pub const ACTIONS: &[(Action, &str, &str)] = &[
    (Action::MoveDown, "move_down", "j"),
    (Action::MoveUp, "move_up", "k"),
    (Action::FocusLeft, "focus_left", "h"),
    (Action::FocusRight, "focus_right", "l"),
    (Action::FocusNext, "focus_next", "Tab"),
    (Action::Open, "open", "Enter"),
    (Action::JoinVoice, "join_voice", "J"),
    (Action::Insert, "insert", "i"),
    (Action::Reply, "reply", "r"),
    (Action::Edit, "edit", "e"),
    (Action::Delete, "delete", "x"),
    (Action::React, "react", "+"),
    (Action::Retry, "retry", "R"),
    (Action::Top, "top", "gg"),
    (Action::Bottom, "bottom", "G"),
    (Action::PageUp, "page_up", "Ctrl+u"),
    (Action::PageDown, "page_down", "Ctrl+d"),
    (Action::ToggleMute, "toggle_mute", "m"),
    (Action::ToggleDeafen, "toggle_deafen", "D"),
    (Action::LeaveVoice, "leave_voice", "L"),
    (Action::ToggleMembers, "toggle_members", "u"),
    (Action::Search, "search", "/"),
    (Action::Command, "command", ":"),
    (Action::Help, "help", "?"),
    (Action::Menu, "menu", "a"),
    (Action::Quit, "quit", "q"),
];

/// One key press, normalized (shift is folded into the character).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
}

impl Key {
    pub fn from_event(event: &KeyEvent) -> Self {
        // Letters arrive already uppercased with Shift, so Shift itself is ignored.
        Key {
            code: event.code,
            ctrl: event.modifiers.contains(KeyModifiers::CONTROL),
            alt: event.modifiers.contains(KeyModifiers::ALT),
        }
    }

    /// Parses one key like `Ctrl+u`, `Enter`, `J`.
    pub fn parse(s: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut rest = s;
        loop {
            let lower = rest.to_ascii_lowercase();
            if lower.starts_with("ctrl+") && rest.len() > 5 {
                ctrl = true;
                rest = &rest[5..];
            } else if lower.starts_with("alt+") && rest.len() > 4 {
                alt = true;
                rest = &rest[4..];
            } else {
                break;
            }
        }
        let code = match rest.to_ascii_lowercase().as_str() {
            "enter" | "return" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "esc" | "escape" => KeyCode::Esc,
            "space" => KeyCode::Char(' '),
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            _ => {
                let mut chars = rest.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                // Ctrl+U and Ctrl+u are the same key.
                KeyCode::Char(if ctrl { c.to_ascii_lowercase() } else { c })
            }
        };
        Some(Key { code, ctrl, alt })
    }
}

/// A binding: one key, or two in sequence (`gg`).
fn parse_binding(s: &str) -> Option<Vec<Key>> {
    if let Some(key) = Key::parse(s) {
        return Some(vec![key]);
    }
    // Two plain characters.
    let chars: Vec<char> = s.chars().collect();
    if chars.len() == 2 && chars.iter().all(|c| !c.is_whitespace()) {
        return chars
            .iter()
            .map(|&c| Key::parse(&c.to_string()))
            .collect::<Option<_>>();
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    bindings: HashMap<Vec<Key>, Action>,
}

/// What a key press did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    Action(Action),
    /// The start of a sequence; wait for the next key.
    Prefix,
    None,
}

impl Keymap {
    /// Defaults, overridden by `[tui.keys]`. Returns warnings for keys that do not parse.
    pub fn new(overrides: &BTreeMap<String, String>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut bindings = HashMap::new();
        for &(action, name, default) in ACTIONS {
            let key = match overrides.get(name) {
                Some(custom) => match parse_binding(custom) {
                    Some(keys) => keys,
                    None => {
                        warnings.push(format!("[tui.keys] {name} = {custom:?}: not a key"));
                        parse_binding(default).expect("default keys parse")
                    }
                },
                None => parse_binding(default).expect("default keys parse"),
            };
            bindings.insert(key, action);
        }
        for name in overrides.keys() {
            if !ACTIONS.iter().any(|(_, n, _)| n == name) {
                warnings.push(format!("[tui.keys] unknown action {name:?}"));
            }
        }
        (Keymap { bindings }, warnings)
    }

    /// Looks up `pending` (an earlier prefix key) followed by `key`.
    pub fn lookup(&self, pending: Option<Key>, key: Key) -> Lookup {
        let seq: Vec<Key> = pending.into_iter().chain([key]).collect();
        if let Some(&action) = self.bindings.get(&seq) {
            return Lookup::Action(action);
        }
        if pending.is_none() && self.bindings.keys().any(|k| k.len() == 2 && k[0] == key) {
            return Lookup::Prefix;
        }
        if pending.is_some() {
            // The prefix led nowhere; try the key on its own.
            return self.lookup(None, key);
        }
        Lookup::None
    }

    /// The key text bound to an action, for hints.
    pub fn describe(&self, action: Action) -> String {
        self.bindings
            .iter()
            .find(|(_, a)| **a == action)
            .map(|(keys, _)| keys.iter().map(describe_key).collect::<String>())
            .unwrap_or_default()
    }
}

fn describe_key(key: &Key) -> String {
    let base = match key.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::Esc => "Esc".into(),
        other => format!("{other:?}"),
    };
    match (key.ctrl, key.alt) {
        (true, _) => format!("Ctrl+{base}"),
        (_, true) => format!("Alt+{base}"),
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(c: char) -> Key {
        Key {
            code: KeyCode::Char(c),
            ctrl: false,
            alt: false,
        }
    }

    #[test]
    fn defaults_and_sequences() {
        let (map, warnings) = Keymap::new(&BTreeMap::new());
        assert!(warnings.is_empty());
        assert_eq!(
            map.lookup(None, press('j')),
            Lookup::Action(Action::MoveDown)
        );
        assert_eq!(
            map.lookup(None, press('J')),
            Lookup::Action(Action::JoinVoice)
        );
        assert_eq!(map.lookup(None, press('g')), Lookup::Prefix);
        assert_eq!(
            map.lookup(Some(press('g')), press('g')),
            Lookup::Action(Action::Top)
        );
        assert_eq!(
            map.lookup(Some(press('g')), press('j')),
            Lookup::Action(Action::MoveDown)
        );
        let ctrl_u = Key {
            code: KeyCode::Char('u'),
            ctrl: true,
            alt: false,
        };
        assert_eq!(map.lookup(None, ctrl_u), Lookup::Action(Action::PageUp));
        assert_eq!(map.lookup(None, press('z')), Lookup::None);
        assert_eq!(map.describe(Action::Top), "gg");
    }

    #[test]
    fn overrides() {
        let overrides: BTreeMap<String, String> = [
            ("join_voice", "v"),
            ("quit", "Ctrl+Q"),
            ("bogus", "x"),
            ("insert", "not a key"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_owned(), b.to_owned()))
        .collect();
        let (map, warnings) = Keymap::new(&overrides);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(
            map.lookup(None, press('v')),
            Lookup::Action(Action::JoinVoice)
        );
        assert_eq!(map.lookup(None, press('J')), Lookup::None);
        let ctrl_q = Key {
            code: KeyCode::Char('q'),
            ctrl: true,
            alt: false,
        };
        assert_eq!(map.lookup(None, ctrl_q), Lookup::Action(Action::Quit));
        assert_eq!(map.lookup(None, press('i')), Lookup::Action(Action::Insert));
    }
}
