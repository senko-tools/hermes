use hermes_client_core::CoreEvent;

use crate::buffer::settings;

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
}
