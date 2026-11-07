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
