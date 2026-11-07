//! Keybinds (PLAN.md section 15.6): every action has a default, and `[keys]` in `config.toml`
//! overrides it (`command_bar = "Ctrl+K"`). Combos are written `Ctrl+Shift+M`, `Alt+Down`,
//! `Ctrl+\`, `Ctrl+,`, `Escape`, `F5`.
//!
//! A punctuation key in a combo also matches the key in its place on a US keyboard, so
//! `Ctrl+\` works on layouts where `\` needs AltGr (approved, see docs/DECISIONS.md).

use std::collections::BTreeMap;

use gpui_kit::Keystroke;

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
    ZoomIn,
    ZoomOut,
    ZoomReset,
}

/// Every action: config name, default combo and the name shown in the command bar.
pub const ACTIONS: [(Action, &str, &str, &str); 21] = [
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
    (Action::ZoomIn, "zoom_in", "Ctrl+=", "Zoom in"),
    (Action::ZoomOut, "zoom_out", "Ctrl+-", "Zoom out"),
    (Action::ZoomReset, "zoom_reset", "Ctrl+0", "Reset zoom"),
];

/// `[keys]` entry of the global mute key (unbound by default). It is registered system-wide with
/// `global-hotkey` instead of being a window keybind (see `crate::ptt`).
pub const GLOBAL_MUTE: &str = "toggle_mute_global";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
    /// The key as gpui names it: a lowercase character, `up`, `escape`, `f5`…
    pub key: String,
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

    /// Whether a key press matches.
    pub fn matches(&self, keystroke: &Keystroke) -> bool {
        let m = keystroke.modifiers;
        m.control == self.ctrl
            && m.shift == self.shift
            && m.alt == self.alt
            && m.platform == self.logo
            && keystroke.key == self.key
    }
}

fn key_name(text: &str) -> Option<String> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    let named = match lower.as_str() {
        "tab" => "tab",
        "enter" | "return" => "enter",
        "escape" | "esc" => "escape",
        "space" => "space",
        "up" => "up",
        "down" => "down",
        "left" => "left",
        "right" => "right",
        "pageup" => "pageup",
        "pagedown" => "pagedown",
        "home" => "home",
        "end" => "end",
        "delete" => "delete",
        "backspace" => "backspace",
        f if f.len() >= 2 && f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| n <= 12) => {
            return Some(lower);
        }
        _ => {
            let mut chars = text.chars();
            return match (chars.next(), chars.next()) {
                (Some(c), None) if !c.is_whitespace() => Some(c.to_lowercase().collect()),
                _ => None,
            };
        }
    };
    Some(named.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keybinds {
    binds: Vec<(Combo, Action)>,
}

impl Keybinds {
    /// The defaults, overridden by `[keys]`. An empty value unbinds an action.
    pub fn from_config(keys: &BTreeMap<String, String>) -> (Keybinds, Vec<String>) {
        let mut warnings = Vec::new();
        for name in keys.keys() {
            if name != GLOBAL_MUTE && !ACTIONS.iter().any(|(_, n, ..)| n == name) {
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

    pub fn action(&self, keystroke: &Keystroke) -> Option<Action> {
        self.binds
            .iter()
            .find(|(combo, _)| combo.matches(keystroke))
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

    fn key(text: &'static str) -> Keystroke {
        Keystroke::parse(text).expect("a keystroke")
    }

    #[test]
    fn combos_parse() {
        let c = Combo::parse("Ctrl+Shift+M").unwrap();
        assert!(c.ctrl && c.shift && !c.alt);
        assert_eq!(c.key, "m");
        assert_eq!(Combo::parse("Ctrl+\\").unwrap().key, "\\");
        assert_eq!(Combo::parse("ctrl+,").unwrap().key, ",");
        assert_eq!(Combo::parse("Ctrl++").unwrap().key, "+");
        assert_eq!(Combo::parse("Alt+Shift+Down").unwrap().key, "down");
        assert_eq!(Combo::parse("F5").unwrap().key, "f5");
        assert_eq!(Combo::parse("Ctrl+Nonsense"), None);
    }

    #[test]
    fn actions_come_from_defaults_and_config() {
        let (binds, warnings) = Keybinds::from_config(&BTreeMap::new());
        assert!(warnings.is_empty());
        assert_eq!(binds.action(&key("ctrl-k")), Some(Action::CommandBar));
        assert_eq!(binds.action(&key("alt-down")), Some(Action::NextChannel));
        assert_eq!(binds.action(&key("k")), None);

        let mut keys = BTreeMap::new();
        keys.insert("command_bar".to_owned(), "Ctrl+P".to_owned());
        keys.insert("search".to_owned(), String::new());
        keys.insert("nope".to_owned(), "Ctrl+Q".to_owned());
        let (binds, warnings) = Keybinds::from_config(&keys);
        assert_eq!(warnings.len(), 1, "the unknown action");
        assert_eq!(binds.action(&key("ctrl-p")), Some(Action::CommandBar));
        assert_eq!(binds.action(&key("ctrl-k")), None);
        assert_eq!(binds.action(&key("ctrl-f")), None, "an empty value unbinds");
    }
}
