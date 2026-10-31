use hermes_client_core::{CoreEvent, InstanceId};

use crate::buffer::settings;
use crate::modal::add_instance;

#[derive(Debug, Clone)]
pub enum Message {
    Core(CoreEvent),
    /// A file in `<config dir>/themes/` changed.
    ThemesChanged,
    WindowFocused(bool),
    OpenSettings,
    Settings(settings::Message),
    DismissStatus,
    /// A bundled font finished loading.
    FontLoaded,
    OpenAddInstance,
    /// Sign in again to a logged-out instance.
    SignIn(InstanceId),
    AddInstance(add_instance::Message),
    /// A later launch forwarded its link (empty: just show the window).
    Forwarded(String),
}
