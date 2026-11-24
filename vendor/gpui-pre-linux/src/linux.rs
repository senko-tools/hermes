mod dispatcher;
mod display_connection;
mod headless;
mod keyboard;
mod platform;
#[cfg(any(feature = "wayland", feature = "x11"))]
mod power;
mod system_notifications;
#[cfg(any(feature = "wayland", feature = "x11"))]
mod text_system;
#[cfg(feature = "wayland")]
mod wayland;
#[cfg(feature = "x11")]
mod x11;

#[cfg(any(feature = "wayland", feature = "x11"))]
mod xdg_desktop_portal;

pub use dispatcher::*;
pub(crate) use display_connection::{Backend, DisplayConnection, select_backend};
pub(crate) use headless::*;
pub(crate) use keyboard::*;
pub(crate) use platform::*;
#[cfg(any(feature = "wayland", feature = "x11"))]
pub(crate) use text_system::*;
#[cfg(feature = "wayland")]
pub(crate) use wayland::*;
#[cfg(feature = "x11")]
pub(crate) use x11::*;

use std::rc::Rc;

use gpui::WindowingModes;

/// Returns the default platform implementation for the current OS.
///
/// A windowed platform connects to the display server the process environment names, or starts
/// headless when it names none, and can switch between those modes later. A headless platform
/// stays headless.
pub fn current_platform(headless: bool) -> Rc<dyn gpui::Platform> {
    if headless || std::env::var_os("ZED_HEADLESS").is_some() {
        linux_platform(WindowingModes::HEADLESS)
    } else {
        linux_platform(WindowingModes::all())
    }
}

/// Returns a platform that may switch among `allowed_modes`.
///
/// It starts windowed in the process's own environment, or headless if that names no allowed
/// display server. Set another initial mode with [`gpui::Application::with_windowing`].
pub fn linux_platform(allowed_modes: WindowingModes) -> Rc<dyn gpui::Platform> {
    Rc::new(LinuxPlatform::new(allowed_modes))
}

/// Hermes patch: hides (`true`) or shows a Wayland toplevel, given its `wl_surface` pointer (the
/// `surface` of its raw window handle). gpui has no call for this; see
/// `wayland::window::hermes_hideable`. `false` where it cannot: not a Wayland window of this
/// thread, or not a toplevel.
pub fn set_wayland_window_hidden(surface: *mut std::ffi::c_void, hidden: bool) -> bool {
    #[cfg(feature = "wayland")]
    {
        wayland::hermes_set_hidden(surface as usize, hidden)
    }
    #[cfg(not(feature = "wayland"))]
    {
        let _ = (surface, hidden);
        false
    }
}

/// Hermes patch: brings a Wayland window to the front with an activation token another process
/// was given (a second launch's `XDG_ACTIVATION_TOKEN`); without one, compositors only mark the
/// window as wanting attention. `false` where it cannot.
pub fn activate_wayland_window(surface: *mut std::ffi::c_void, token: &str) -> bool {
    #[cfg(feature = "wayland")]
    {
        wayland::hermes_activate(surface as usize, token)
    }
    #[cfg(not(feature = "wayland"))]
    {
        let _ = (surface, token);
        false
    }
}
