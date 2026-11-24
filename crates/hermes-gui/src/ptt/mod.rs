//! Push-to-talk and the global mute key (PLAN.md sections 15.6 and 21).
//!
//! Push-to-talk polls the key state every 10 ms with `GetAsyncKeyState` on Windows (the Spike C
//! decision in docs/DECISIONS.md): the key keeps working in other apps and mouse buttons work.
//! On Linux under X11, XInput2 raw events do the same without polling (`x11`). Wayland does not
//! let apps read keys globally: there the system's GlobalShortcuts portal binds the key
//! (`portal`), and where it cannot (no portal, a mouse button), the key works while Hermes is
//! focused (`Shell::ptt_focused_key`). The global mute key uses `global-hotkey` (Windows, X11),
//! which takes the key away from other apps; fine for a one-shot action. On Wayland it goes
//! through the same portal.

#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "linux")]
mod x11;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU8, Ordering};

/// Windows virtual-key codes for a PTT key such as `Mouse4`, `CapsLock`, `F13`, `V` or
/// `Ctrl+Space` (all keys of a combo must be held). `None` if a part is unknown.
pub fn virtual_keys(text: &str) -> Option<Vec<u16>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    text.split('+').map(virtual_key).collect()
}

fn virtual_key(name: &str) -> Option<u16> {
    let upper = name.trim().to_ascii_uppercase();
    let code = match upper.as_str() {
        "MOUSE4" | "XBUTTON1" => 0x05,
        "MOUSE5" | "XBUTTON2" => 0x06,
        "MOUSEMIDDLE" | "MOUSE3" | "MBUTTON" => 0x04,
        "CTRL" | "CONTROL" => 0x11,
        "SHIFT" => 0x10,
        "ALT" => 0x12,
        "CAPSLOCK" => 0x14,
        "SPACE" => 0x20,
        "TAB" => 0x09,
        "PAUSE" => 0x13,
        "SCROLLLOCK" => 0x91,
        "INSERT" => 0x2D,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        "BACKQUOTE" | "GRAVE" | "`" => 0xC0,
        "LSHIFT" => 0xA0,
        "RSHIFT" => 0xA1,
        "LCTRL" | "LCONTROL" => 0xA2,
        "RCTRL" | "RCONTROL" => 0xA3,
        "LALT" => 0xA4,
        "RALT" => 0xA5,
        s if s.len() == 1 && s.as_bytes()[0].is_ascii_alphanumeric() => u16::from(s.as_bytes()[0]),
        s if s.starts_with('F') => match s[1..].parse::<u16>() {
            Ok(n @ 1..=24) => 0x6F + n,
            _ => return None,
        },
        _ => return None,
    };
    Some(code)
}

/// Mouse buttons as virtual keys (Middle, Back, Forward).
pub const MOUSE_MIDDLE: u16 = 0x04;
pub const MOUSE_BACK: u16 = 0x05;
pub const MOUSE_FORWARD: u16 = 0x06;
pub const CTRL: u16 = 0x11;
pub const SHIFT: u16 = 0x10;
pub const ALT: u16 = 0x12;

/// The keys held right now, as virtual-key codes, from whatever reports them.
#[derive(Debug, Default, Clone)]
pub struct Held(HashSet<u16>);

impl Held {
    /// Notes a press or a release; `true` if that changed anything.
    pub fn set(&mut self, vk: u16, down: bool) -> bool {
        if down {
            self.0.insert(vk)
        } else {
            self.0.remove(&vk)
        }
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Every key of `combo` is held. `Ctrl` is either Ctrl key, and `LCtrl` counts as held when
    /// only "a Ctrl key" is known (the focused window reports modifiers without their side).
    pub fn holds(&self, combo: &[u16]) -> bool {
        !combo.is_empty() && combo.iter().all(|vk| self.has(*vk))
    }

    fn has(&self, vk: u16) -> bool {
        self.0.contains(&vk) || same_key(vk).iter().any(|other| self.0.contains(other))
    }
}

/// The other codes of a modifier: either side for the plain one, the plain one for a side.
fn same_key(vk: u16) -> &'static [u16] {
    match vk {
        0x10 => &[0xA0, 0xA1],
        0x11 => &[0xA2, 0xA3],
        0x12 => &[0xA4, 0xA5],
        0xA0 | 0xA1 => &[0x10],
        0xA2 | 0xA3 => &[0x11],
        0xA4 | 0xA5 => &[0x12],
        _ => &[],
    }
}

/// The virtual key of a key the focused window reported (gpui's key names, e.g. `a`, `f13`,
/// `space`, `caps_lock`, `` ` ``).
pub fn focused_key(key: &str) -> Option<u16> {
    let name = key.replace('_', "");
    // Single characters other than letters and digits are not key names of ours, except `.
    if name.chars().count() == 1
        && !name.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '`')
    {
        return None;
    }
    virtual_key(&name).filter(|vk| ![MOUSE_MIDDLE, MOUSE_BACK, MOUSE_FORWARD].contains(vk))
}

/// The virtual key of a mouse button the focused window reported.
pub fn mouse_vk(button: gpui_kit::MouseButton) -> Option<u16> {
    use gpui_kit::{MouseButton, NavigationDirection};
    match button {
        MouseButton::Middle => Some(MOUSE_MIDDLE),
        MouseButton::Navigate(NavigationDirection::Back) => Some(MOUSE_BACK),
        MouseButton::Navigate(NavigationDirection::Forward) => Some(MOUSE_FORWARD),
        _ => None,
    }
}

/// Where push-to-talk hears its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Everywhere, read from the system (Windows, X11).
    Everywhere,
    /// Everywhere, through the system's shortcut settings (the Wayland portal).
    System,
    /// Only while a Hermes window has focus.
    Focused,
}

static REACH: AtomicU8 = AtomicU8::new(if cfg!(windows) { 0 } else { 2 });

pub fn reach() -> Reach {
    match REACH.load(Ordering::Relaxed) {
        0 => Reach::Everywhere,
        1 => Reach::System,
        _ => Reach::Focused,
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn set_reach(reach: Reach) {
    let value = match reach {
        Reach::Everywhere => 0,
        Reach::System => 1,
        Reach::Focused => 2,
    };
    REACH.store(value, Ordering::Relaxed);
}

/// The push-to-talk key setting's hint: where the key works.
pub fn hint(key: &str) -> String {
    match reach() {
        Reach::Everywhere => "e.g. Mouse4, F13, Ctrl+Space. Enter saves.".to_owned(),
        Reach::System => match system_trigger().filter(|t| !t.is_empty()) {
            Some(bound) => format!(
                "The desktop bound it to {bound}; change it in its shortcut settings. Enter saves."
            ),
            None => "Bound through the desktop's shortcut settings. Enter saves.".to_owned(),
        },
        Reach::Focused if virtual_keys(key).is_some_and(|k| k.iter().any(|vk| *vk <= 0x06)) => {
            "The desktop cannot bind mouse buttons: only while Hermes is focused. Enter saves."
                .to_owned()
        }
        Reach::Focused => "Only while Hermes is focused here. Enter saves.".to_owned(),
    }
}

/// The key the system bound push-to-talk to, as the system describes it (portal only).
pub fn system_trigger() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        portal::bound_ptt()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// The PTT key text changed in the settings (the portal binds the new one).
pub fn ptt_key_changed(text: &str) {
    #[cfg(target_os = "linux")]
    if crate::desktop::wayland() {
        portal::set_ptt(text);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = text;
}

/// Polls the PTT key on a thread of its own: `keys` says what to watch (changed from settings),
/// and every change of the key's state goes to `changes`.
#[cfg(windows)]
pub fn poll(
    keys: std::sync::Arc<std::sync::Mutex<Vec<u16>>>,
    changes: tokio::sync::mpsc::UnboundedSender<bool>,
) {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

    let spawned = std::thread::Builder::new()
        .name("hermes-ptt".into())
        .spawn(move || {
            let mut was_down = false;
            loop {
                // Read in place (no copy 100 times a second); the lock is held only for the
                // few key state reads, not through the sleep.
                let held = keys.lock().expect("ptt keys");
                let down = !held.is_empty()
                    && held.iter().all(|vk| {
                        // SAFETY: GetAsyncKeyState has no preconditions; it only reads key state.
                        #[allow(unsafe_code)]
                        let state = unsafe { GetAsyncKeyState(i32::from(*vk)) };
                        (state as u16 & 0x8000) != 0
                    });
                let no_keys = held.is_empty();
                drop(held);
                if down != was_down {
                    was_down = down;
                    if changes.send(down).is_err() {
                        break;
                    }
                }
                // Without a key there is nothing to catch quickly.
                let idle = if no_keys { 250 } else { 10 };
                std::thread::sleep(std::time::Duration::from_millis(idle));
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("push-to-talk: {err}");
    }
}

/// X11: XInput2 raw events, wherever the focus is. Wayland: the GlobalShortcuts portal, with
/// the PTT key from `ptt_key_changed`. Either falls back to the focused window.
#[cfg(target_os = "linux")]
pub fn poll(
    keys: std::sync::Arc<std::sync::Mutex<Vec<u16>>>,
    changes: tokio::sync::mpsc::UnboundedSender<bool>,
) {
    if crate::desktop::wayland() {
        portal::start_ptt(changes);
    } else {
        match x11::start(keys, changes) {
            Ok(()) => set_reach(Reach::Everywhere),
            Err(err) => {
                tracing::warn!("push-to-talk only while focused: {err}");
                set_reach(Reach::Focused);
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn poll(
    _keys: std::sync::Arc<std::sync::Mutex<Vec<u16>>>,
    _changes: tokio::sync::mpsc::UnboundedSender<bool>,
) {
}

/// The registered global mute key. Must be created on the main thread (the event loop's).
pub struct GlobalKeys {
    inner: Inner,
    /// The `[keys] toggle_mute_global` text that `mute` came from.
    pub combo: String,
}

enum Inner {
    Hotkey {
        manager: global_hotkey::GlobalHotKeyManager,
        mute: Option<global_hotkey::hotkey::HotKey>,
    },
    /// Wayland: the GlobalShortcuts portal binds it.
    #[cfg(target_os = "linux")]
    Portal,
}

impl GlobalKeys {
    pub fn new() -> Result<GlobalKeys, String> {
        #[cfg(target_os = "linux")]
        if crate::desktop::wayland() {
            return Ok(GlobalKeys {
                inner: Inner::Portal,
                combo: String::new(),
            });
        }
        Ok(GlobalKeys {
            inner: Inner::Hotkey {
                manager: global_hotkey::GlobalHotKeyManager::new().map_err(|e| e.to_string())?,
                mute: None,
            },
            combo: String::new(),
        })
    }

    /// Registers `combo` (e.g. `Ctrl+Shift+F9`) as the global mute key; empty unregisters.
    pub fn set_mute(&mut self, combo: &str) -> Result<(), String> {
        if combo == self.combo {
            return Ok(());
        }
        self.combo = combo.to_owned();
        match &mut self.inner {
            Inner::Hotkey { manager, mute } => {
                if let Some(old) = mute.take() {
                    let _ = manager.unregister(old);
                }
                if combo.trim().is_empty() {
                    return Ok(());
                }
                let hotkey: global_hotkey::hotkey::HotKey = combo
                    .parse()
                    .map_err(|e| format!("`{combo}` is not a global key: {e}"))?;
                manager
                    .register(hotkey)
                    .map_err(|e| format!("could not register `{combo}`: {e}"))?;
                *mute = Some(hotkey);
                Ok(())
            }
            #[cfg(target_os = "linux")]
            Inner::Portal => {
                if !combo.trim().is_empty() && portal::trigger(combo).is_none() {
                    return Err(format!("`{combo}` is not a global key"));
                }
                portal::set_mute(combo);
                Ok(())
            }
        }
    }
}

/// Sends one message per press of a registered global key.
pub fn global_presses(presses: tokio::sync::mpsc::UnboundedSender<()>) {
    #[cfg(target_os = "linux")]
    if crate::desktop::wayland() {
        portal::start_mute(presses);
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("hermes-global-keys".into())
        .spawn(move || {
            let receiver = global_hotkey::GlobalHotKeyEvent::receiver();
            while let Ok(event) = receiver.recv() {
                if event.state == global_hotkey::HotKeyState::Pressed && presses.send(()).is_err() {
                    break;
                }
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("global keys: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_map_to_virtual_keys() {
        assert_eq!(virtual_keys("Mouse4"), Some(vec![0x05]));
        assert_eq!(virtual_keys("capslock"), Some(vec![0x14]));
        assert_eq!(virtual_keys("F13"), Some(vec![0x7C]));
        assert_eq!(virtual_keys("v"), Some(vec![u16::from(b'V')]));
        assert_eq!(virtual_keys("Ctrl+Space"), Some(vec![0x11, 0x20]));
        assert_eq!(virtual_keys(""), None);
        assert_eq!(virtual_keys("F25"), None);
        assert_eq!(virtual_keys("Ctrl+Nonsense"), None);
    }

    #[test]
    fn held_keys_match_combos_with_either_side() {
        let mut held = Held::default();
        assert!(!held.holds(&[]));
        held.set(0xA3, true); // right Ctrl
        held.set(0x20, true);
        assert!(held.holds(&[CTRL, 0x20]));
        assert!(!held.holds(&[0xA2, 0x20]), "left Ctrl is not the right one");
        held.clear();
        held.set(CTRL, true); // a Ctrl key, side unknown
        assert!(held.holds(&[0xA2]));
        held.set(CTRL, false);
        assert!(!held.holds(&[0xA2]));
    }

    #[test]
    fn focused_key_names() {
        assert_eq!(focused_key("caps_lock"), Some(0x14));
        assert_eq!(focused_key("f13"), Some(0x7C));
        assert_eq!(focused_key("v"), Some(u16::from(b'V')));
        assert_eq!(focused_key("space"), Some(0x20));
        assert_eq!(focused_key("`"), Some(0xC0));
        assert_eq!(focused_key("scroll_lock"), Some(0x91));
        assert_eq!(focused_key(","), None);
        assert_eq!(focused_key("mouse4"), None);
    }
}
