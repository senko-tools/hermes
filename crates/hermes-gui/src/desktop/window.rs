//! Hiding the window to the tray and bringing it back. gpui has no way to hide a window, so this
//! calls Windows directly with the window's own handle.

use gpui_kit::Window;

/// Hides or shows the window without closing it. Elsewhere windows stay shown: there is no tray
/// to come back from everywhere, so closing quits there.
#[cfg(windows)]
pub fn set_visible(window: &Window, visible: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_RESTORE, ShowWindowAsync};

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    if let RawWindowHandle::Win32(handle) = handle.as_raw() {
        let hwnd = HWND(handle.hwnd.get() as *mut _);
        // SAFETY: the handle comes from the live window this call borrows.
        #[allow(unsafe_code)]
        let _ = unsafe { ShowWindowAsync(hwnd, if visible { SW_RESTORE } else { SW_HIDE }) };
    }
}

#[cfg(not(windows))]
pub fn set_visible(_window: &Window, _visible: bool) {}

/// Colours the native title bar like the app: dark mode when `background` is dark, and the
/// background itself where Windows allows it (Windows 11; Windows 10 only has dark and light).
#[cfg(windows)]
pub fn set_title_bar(window: &Window, background: gpui_kit::Rgba) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{COLORREF, HWND};
    use windows::Win32::Graphics::Dwm::{
        DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
    };
    use windows::core::BOOL;

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);
    let dark = BOOL::from(background.r * 0.299 + background.g * 0.587 + background.b * 0.114 < 0.5);
    let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
    let caption = COLORREF(byte(background.r) | byte(background.g) << 8 | byte(background.b) << 16);
    // SAFETY: the handle comes from the live window, the pointers to values living through the
    // calls, with their sizes. Windows 10 refuses the caption colour, which changes nothing.
    #[allow(unsafe_code)]
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::from_ref(&dark).cast(),
            size_of::<BOOL>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            std::ptr::from_ref(&caption).cast(),
            size_of::<COLORREF>() as u32,
        );
    }
}

#[cfg(not(windows))]
pub fn set_title_bar(_window: &Window, _background: gpui_kit::Rgba) {}

/// Flashes the taskbar button until the window gets the focus.
#[cfg(windows)]
pub fn flash(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx,
    };

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    if let RawWindowHandle::Win32(handle) = handle.as_raw() {
        let info = FLASHWINFO {
            cbSize: size_of::<FLASHWINFO>() as u32,
            hwnd: HWND(handle.hwnd.get() as *mut _),
            dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        // SAFETY: the structure is filled in and the handle comes from the live window.
        #[allow(unsafe_code)]
        let _ = unsafe { FlashWindowEx(&info) };
    }
}

#[cfg(not(windows))]
pub fn flash(_window: &Window) {}

/// Asks Windows for 1 ms timers while it lives. By default they fire every 15.6 ms, which makes a
/// 60 fps animation stutter (frames 31 and 47 ms apart); the level meter holds one while it shows.
pub struct FineTimers(());

impl FineTimers {
    pub fn new() -> FineTimers {
        #[cfg(windows)]
        // SAFETY: plain call, paired with `timeEndPeriod` in `drop`.
        #[allow(unsafe_code)]
        unsafe {
            windows::Win32::Media::timeBeginPeriod(1);
        }
        FineTimers(())
    }
}

impl Drop for FineTimers {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: undoes the `timeBeginPeriod` of `new`.
        #[allow(unsafe_code)]
        unsafe {
            windows::Win32::Media::timeEndPeriod(1);
        }
    }
}

impl Default for FineTimers {
    fn default() -> FineTimers {
        FineTimers::new()
    }
}
