//! Push-to-talk on X11: XInput2 raw key and button events on the root window. They arrive
//! wherever the focus is and do not take the key from other apps, like `GetAsyncKeyState` on
//! Windows, but without polling: the thread sleeps until a key or button changes.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use super::Held;

/// Connects, asks for raw events and starts the thread; an error means X11 cannot do it.
pub fn start(keys: Arc<Mutex<Vec<u16>>>, changes: UnboundedSender<bool>) -> Result<(), String> {
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("no X display: {e}"))?;
    let root = conn.setup().roots[screen].root;
    // 2.1 or later: raw events also arrive during another client's grab (a button press grabs
    // the pointer for the window under it, so with 2.0 the release never came).
    let version = conn
        .xinput_xi_query_version(2, 2)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| format!("no XInput2: {e}"))?;
    if version.major_version < 2 {
        return Err("no XInput2".into());
    }
    let mask = xinput::XIEventMask::RAW_KEY_PRESS
        | xinput::XIEventMask::RAW_KEY_RELEASE
        | xinput::XIEventMask::RAW_BUTTON_PRESS
        | xinput::XIEventMask::RAW_BUTTON_RELEASE;
    conn.xinput_xi_select_events(
        root,
        &[xinput::EventMask {
            deviceid: xinput::Device::ALL_MASTER.into(),
            mask: vec![mask],
        }],
    )
    .map_err(|e| e.to_string())?
    .check()
    .map_err(|e| format!("raw events refused: {e}"))?;
    let keymap = Keymap::load(&conn)?;
    std::thread::Builder::new()
        .name("hermes-ptt".into())
        .spawn(move || run(&conn, keymap, &keys, &changes))
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn run(
    conn: &RustConnection,
    mut keymap: Keymap,
    keys: &Mutex<Vec<u16>>,
    changes: &UnboundedSender<bool>,
) {
    let mut held = Held::default();
    let mut was_down = false;
    loop {
        let event = match conn.wait_for_event() {
            Ok(event) => event,
            Err(err) => {
                tracing::warn!("push-to-talk: X connection lost: {err}");
                if was_down {
                    let _ = changes.send(false);
                }
                return;
            }
        };
        let changed = match event {
            Event::XinputRawKeyPress(e) => keymap.vk(e.detail).is_some_and(|vk| held.set(vk, true)),
            Event::XinputRawKeyRelease(e) => {
                keymap.vk(e.detail).is_some_and(|vk| held.set(vk, false))
            }
            Event::XinputRawButtonPress(e) => button(e.detail).is_some_and(|vk| held.set(vk, true)),
            Event::XinputRawButtonRelease(e) => {
                button(e.detail).is_some_and(|vk| held.set(vk, false))
            }
            Event::MappingNotify(_) => {
                if let Ok(fresh) = Keymap::load(conn) {
                    keymap = fresh;
                }
                false
            }
            _ => false,
        };
        if !changed {
            continue;
        }
        let down = held.holds(&keys.lock().expect("ptt keys"));
        if down != was_down {
            was_down = down;
            if changes.send(down).is_err() {
                return;
            }
        }
    }
}

/// X button numbers: 2 middle, 8 back, 9 forward.
fn button(detail: u32) -> Option<u16> {
    match detail {
        2 => Some(super::MOUSE_MIDDLE),
        8 => Some(super::MOUSE_BACK),
        9 => Some(super::MOUSE_FORWARD),
        _ => None,
    }
}

/// The first keysym of every keycode: the key's own name without Shift.
struct Keymap {
    min: u32,
    per_code: usize,
    syms: Vec<u32>,
}

impl Keymap {
    fn load(conn: &RustConnection) -> Result<Keymap, String> {
        let setup = conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let reply = conn
            .get_keyboard_mapping(min, max - min + 1)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        Ok(Keymap {
            min: u32::from(min),
            per_code: usize::from(reply.keysyms_per_keycode).max(1),
            syms: reply.keysyms,
        })
    }

    fn vk(&self, keycode: u32) -> Option<u16> {
        let index = usize::try_from(keycode.checked_sub(self.min)?).ok()? * self.per_code;
        keysym_vk(*self.syms.get(index)?)
    }
}

/// The virtual-key code of an X keysym (the keys `virtual_key` knows).
pub fn keysym_vk(sym: u32) -> Option<u16> {
    Some(match sym {
        // Latin letters (lower and upper case) and digits.
        0x61..=0x7a => (sym - 0x20) as u16,
        0x41..=0x5a | 0x30..=0x39 => sym as u16,
        0x20 => 0x20,                                      // space
        0x60 => 0xC0,                                      // grave
        0xff09 => 0x09,                                    // Tab
        0xff13 => 0x13,                                    // Pause
        0xff14 => 0x91,                                    // Scroll_Lock
        0xff50 => 0x24,                                    // Home
        0xff55 => 0x21,                                    // Prior
        0xff56 => 0x22,                                    // Next
        0xff57 => 0x23,                                    // End
        0xff63 => 0x2D,                                    // Insert
        0xffe1 => 0xA0,                                    // Shift_L
        0xffe2 => 0xA1,                                    // Shift_R
        0xffe3 => 0xA2,                                    // Control_L
        0xffe4 => 0xA3,                                    // Control_R
        0xffe5 => 0x14,                                    // Caps_Lock
        0xffe9 => 0xA4,                                    // Alt_L
        0xffea => 0xA5,                                    // Alt_R
        0xfe03 => 0xA5,                                    // ISO_Level3_Shift (AltGr)
        0xffbe..=0xffd5 => (0x70 + (sym - 0xffbe)) as u16, // F1..F24
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysyms_map_to_the_same_codes_as_names() {
        let named = |name: &str| super::super::virtual_keys(name).map(|v| v[0]);
        assert_eq!(keysym_vk(0x76), named("V")); // v
        assert_eq!(keysym_vk(0x35), named("5"));
        assert_eq!(keysym_vk(0xffbe), named("F1"));
        assert_eq!(keysym_vk(0xffca), named("F13"));
        assert_eq!(keysym_vk(0xffd5), named("F24"));
        assert_eq!(keysym_vk(0xffe5), named("CapsLock"));
        assert_eq!(keysym_vk(0xffe3), named("LCtrl"));
        assert_eq!(keysym_vk(0x60), named("`"));
        assert_eq!(keysym_vk(0xff55), named("PageUp"));
    }
}
