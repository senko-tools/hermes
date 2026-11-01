//! Push-to-talk and the global mute key (PLAN.md sections 15.6 and 21).
//!
//! Push-to-talk polls the key state every 10 ms with `GetAsyncKeyState` on Windows (the Spike C
//! decision in docs/DECISIONS.md): the key keeps working in other apps and mouse buttons work.
//! Elsewhere it only works while the window is focused (see `App::on_key`). The global mute key
//! uses `global-hotkey`, which takes the key away from other apps; fine for a one-shot action.

use iced::Subscription;

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

/// `true` when the PTT key goes down, `false` when it is released.
#[cfg(windows)]
pub fn poll(keys: Vec<u16>) -> Subscription<bool> {
    use iced::futures::SinkExt;

    Subscription::run_with(keys, |keys| {
        let keys = keys.clone();
        iced::stream::channel(16, async move |output| {
            std::thread::spawn(move || {
                use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
                let mut output = output;
                let mut was_down = false;
                loop {
                    let down = keys.iter().all(|vk| {
                        // SAFETY: GetAsyncKeyState has no preconditions; it only reads key state.
                        #[allow(unsafe_code)]
                        let state = unsafe { GetAsyncKeyState(i32::from(*vk)) };
                        (state as u16 & 0x8000) != 0
                    });
                    if down != was_down {
                        was_down = down;
                        if iced::futures::executor::block_on(output.send(down)).is_err() {
                            break;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            });
            std::future::pending::<()>().await;
        })
    })
}

#[cfg(not(windows))]
pub fn poll(_keys: Vec<u16>) -> Subscription<bool> {
    Subscription::none()
}

/// The registered global mute key. Must be created on the main thread (the event loop's).
pub struct GlobalKeys {
    manager: global_hotkey::GlobalHotKeyManager,
    mute: Option<global_hotkey::hotkey::HotKey>,
    /// The `[keys] toggle_mute_global` text that `mute` came from.
    pub combo: String,
}

impl GlobalKeys {
    pub fn new() -> Result<GlobalKeys, String> {
        Ok(GlobalKeys {
            manager: global_hotkey::GlobalHotKeyManager::new().map_err(|e| e.to_string())?,
            mute: None,
            combo: String::new(),
        })
    }

    /// Registers `combo` (e.g. `Ctrl+Shift+F9`) as the global mute key; empty unregisters.
    pub fn set_mute(&mut self, combo: &str) -> Result<(), String> {
        if combo == self.combo {
            return Ok(());
        }
        if let Some(old) = self.mute.take() {
            let _ = self.manager.unregister(old);
        }
        self.combo = combo.to_owned();
        if combo.trim().is_empty() {
            return Ok(());
        }
        let hotkey: global_hotkey::hotkey::HotKey = combo
            .parse()
            .map_err(|e| format!("`{combo}` is not a global key: {e}"))?;
        self.manager
            .register(hotkey)
            .map_err(|e| format!("could not register `{combo}`: {e}"))?;
        self.mute = Some(hotkey);
        Ok(())
    }
}

/// One message per press of a registered global key.
pub fn global_presses() -> Subscription<()> {
    use iced::futures::SinkExt;

    Subscription::run(|| {
        iced::stream::channel(16, async |output| {
            std::thread::spawn(move || {
                let mut output = output;
                let receiver = global_hotkey::GlobalHotKeyEvent::receiver();
                while let Ok(event) = receiver.recv() {
                    if event.state == global_hotkey::HotKeyState::Pressed
                        && iced::futures::executor::block_on(output.send(())).is_err()
                    {
                        break;
                    }
                }
            });
            std::future::pending::<()>().await;
        })
    })
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
}
