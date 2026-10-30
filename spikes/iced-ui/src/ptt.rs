//! Push-to-talk test sources: `global-hotkey` events and (Windows) `GetAsyncKeyState` polling.

use std::time::Instant;

use iced::Subscription;
use iced::futures::SinkExt;
use iced::futures::channel::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Hotkey,
    #[cfg_attr(not(windows), allow(dead_code))]
    Poll,
}

#[derive(Debug, Clone, Copy)]
pub struct PttEvent {
    pub source: Source,
    pub down: bool,
    /// Taken on the thread that observed the change, not when the UI handles it.
    pub at: Instant,
}

/// Forward every `global-hotkey` event. The manager itself lives on the main thread.
pub fn hotkey_events() -> Subscription<PttEvent> {
    Subscription::run(|| {
        iced::stream::channel(64, async |output: mpsc::Sender<PttEvent>| {
            std::thread::spawn(move || {
                let mut output = output;
                let receiver = global_hotkey::GlobalHotKeyEvent::receiver();
                while let Ok(event) = receiver.recv() {
                    let ptt = PttEvent {
                        source: Source::Hotkey,
                        down: event.state == global_hotkey::HotKeyState::Pressed,
                        at: Instant::now(),
                    };
                    if iced::futures::executor::block_on(output.send(ptt)).is_err() {
                        break;
                    }
                }
            });
            std::future::pending::<()>().await;
        })
    })
}

/// Windows virtual-key code for a key name, for the polling fallback.
pub fn virtual_key(name: &str) -> Option<u16> {
    let upper = name.trim().to_ascii_uppercase();
    let code = match upper.as_str() {
        "MOUSE4" | "XBUTTON1" => 0x05,
        "MOUSE5" | "XBUTTON2" => 0x06,
        "MOUSEMIDDLE" | "MBUTTON" => 0x04,
        "CAPSLOCK" => 0x14,
        "SPACE" => 0x20,
        "PAUSE" => 0x13,
        "SCROLLLOCK" => 0x91,
        "INSERT" => 0x2D,
        "BACKQUOTE" | "GRAVE" => 0xC0,
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

/// Poll one virtual key every 10 ms and report changes. Does not consume the key.
#[cfg(windows)]
pub fn poll_events(vk: u16) -> Subscription<PttEvent> {
    Subscription::run_with(vk, |&vk| {
        iced::stream::channel(64, async move |output: mpsc::Sender<PttEvent>| {
            std::thread::spawn(move || {
                use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
                let mut output = output;
                let mut was_down = false;
                loop {
                    // SAFETY: GetAsyncKeyState has no preconditions; it only reads key state.
                    #[allow(unsafe_code)]
                    let state = unsafe { GetAsyncKeyState(i32::from(vk)) };
                    let down = (state as u16 & 0x8000) != 0;
                    if down != was_down {
                        was_down = down;
                        let ptt = PttEvent {
                            source: Source::Poll,
                            down,
                            at: Instant::now(),
                        };
                        if iced::futures::executor::block_on(output.send(ptt)).is_err() {
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
pub fn poll_events(_vk: u16) -> Subscription<PttEvent> {
    Subscription::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_map_to_virtual_keys() {
        assert_eq!(virtual_key("Mouse4"), Some(0x05));
        assert_eq!(virtual_key("capslock"), Some(0x14));
        assert_eq!(virtual_key("F1"), Some(0x70));
        assert_eq!(virtual_key("F24"), Some(0x87));
        assert_eq!(virtual_key("v"), Some(u16::from(b'V')));
        assert_eq!(virtual_key("7"), Some(u16::from(b'7')));
        assert_eq!(virtual_key("F25"), None);
        assert_eq!(virtual_key("nonsense"), None);
    }
}
