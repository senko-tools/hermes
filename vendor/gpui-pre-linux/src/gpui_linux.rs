#![cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;

pub use linux::{
    activate_wayland_window, current_platform, linux_platform, set_wayland_window_hidden,
};
