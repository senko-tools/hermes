//! The unread count on the app's taskbar button: a small red dot with the number over the icon
//! (Windows' overlay icon). Elsewhere there is no such thing, and this does nothing.

/// The number on the dot: up to 9, then "9+".
#[cfg_attr(not(windows), allow(dead_code))] // only Windows has a taskbar overlay
pub fn label(count: u32) -> String {
    if count > 9 {
        "9+".to_owned()
    } else {
        count.to_string()
    }
}

/// 3 x 5 pixel glyphs, one row per entry, the top bit on the left.
#[cfg_attr(not(windows), allow(dead_code))] // only Windows has a taskbar overlay
fn glyph(c: char) -> [u8; 5] {
    match c {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        _ => [0; 5],
    }
}

/// A `size` x `size` BGRA picture (top row first, as Windows wants it): a soft red dot with
/// `text` in white, centred.
#[cfg_attr(not(windows), allow(dead_code))] // only Windows has a taskbar overlay
pub fn picture(size: u32, text: &str) -> Vec<u8> {
    let s = size as i32;
    let mut out = vec![0u8; (size * size * 4) as usize];
    let r = size as f32 / 2.;
    for y in 0..s {
        for x in 0..s {
            let (dx, dy) = (x as f32 + 0.5 - r, y as f32 + 0.5 - r);
            // Antialiased edge: fully in a pixel inside, fading over the last one.
            let cover = (r - (dx * dx + dy * dy).sqrt()).clamp(0., 1.);
            if cover > 0. {
                let i = ((y * s + x) * 4) as usize;
                let a = (cover * 255.).round() as u8;
                // Premultiplied #E5484D.
                let pm = |c: u8| (u16::from(c) * u16::from(a) / 255) as u8;
                out[i..i + 4].copy_from_slice(&[pm(0x4D), pm(0x48), pm(0xE5), a]);
            }
        }
    }
    // Digits as large as fit inside the dot, at a whole number of pixels per dot.
    let chars: Vec<char> = text.chars().collect();
    let cols = chars.len() as i32 * 4 - 1;
    let scale = ((s * 6 / 10) / cols.max(5)).max(1);
    let (w, h) = (cols * scale, 5 * scale);
    let (x0, y0) = ((s - w) / 2, (s - h) / 2);
    for (n, c) in chars.iter().enumerate() {
        for (row, bits) in glyph(*c).iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) == 0 {
                    continue;
                }
                for py in 0..scale {
                    for px in 0..scale {
                        let x = x0 + (n as i32 * 4 + col) * scale + px;
                        let y = y0 + row as i32 * scale + py;
                        if (0..s).contains(&x) && (0..s).contains(&y) {
                            let i = ((y * s + x) * 4) as usize;
                            out[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
                        }
                    }
                }
            }
        }
    }
    out
}

/// Shows `count` on the window's taskbar button, or nothing for 0.
#[cfg(windows)]
pub fn set_count(window: &gpui_kit::Window, count: u32) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateIcon, DestroyIcon, GetSystemMetrics, HICON, SM_CXSMICON,
    };
    use windows::core::{HSTRING, PCWSTR};

    thread_local! {
        static TASKBAR: std::cell::RefCell<Option<ITaskbarList3>> = const { std::cell::RefCell::new(None) };
    }

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);
    TASKBAR.with_borrow_mut(|taskbar| {
        // SAFETY: plain COM calls on this (UI) thread, where gpui has set COM up; the list is
        // kept for the thread's life.
        #[allow(unsafe_code)]
        unsafe {
            if taskbar.is_none() {
                match CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER)
                {
                    Ok(list) if list.HrInit().is_ok() => *taskbar = Some(list),
                    Ok(_) | Err(_) => {
                        tracing::debug!("no taskbar list: no unread count on the taskbar");
                        return;
                    }
                }
            }
            let Some(list) = taskbar.as_ref() else {
                return;
            };
            if count == 0 {
                let _ = list.SetOverlayIcon(hwnd, HICON::default(), PCWSTR::null());
                return;
            }
            let size = GetSystemMetrics(SM_CXSMICON).clamp(16, 64) as u32;
            let bgra = picture(size, &label(count));
            // An all-zero AND mask: the colour bits' alpha decides what shows.
            let mask = vec![0u8; (size * size / 8) as usize];
            let Ok(icon) = CreateIcon(
                None,
                size as i32,
                size as i32,
                1,
                32,
                mask.as_ptr(),
                bgra.as_ptr(),
            ) else {
                return;
            };
            let text = HSTRING::from(format!("{count} unread"));
            let _ = list.SetOverlayIcon(hwnd, icon, &text);
            // The taskbar keeps its own copy.
            let _ = DestroyIcon(icon);
        }
    });
}

#[cfg(not(windows))]
pub fn set_count(_window: &gpui_kit::Window, _count: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_stop_at_nine() {
        assert_eq!(label(3), "3");
        assert_eq!(label(9), "9");
        assert_eq!(label(42), "9+");
    }

    #[test]
    fn the_dot_is_round_and_the_digits_are_white() {
        let size = 16;
        let p = picture(size, "7");
        let at = |x: u32, y: u32| {
            let i = ((y * size + x) * 4) as usize;
            [p[i], p[i + 1], p[i + 2], p[i + 3]]
        };
        assert_eq!(at(0, 0)[3], 0, "corners are clear");
        assert_eq!(at(8, 1)[3], 255, "inside the dot is solid");
        assert!(
            p.chunks(4).any(|px| px == [255, 255, 255, 255]),
            "the digit is drawn"
        );
    }
}
