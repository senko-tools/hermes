use hermes_client_core::proto::id::Id;
use hermes_client_core::{CoreEvent, InstanceId};
use iced::Size;

use crate::buffer::settings;
use crate::modal::{add_instance, new_dm};
use crate::sidebar::collapsed::Node;
use crate::sidebar::menu::{Action, Target};

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
    Toggle(Node),
    Select(InstanceId, Id),
    /// The selected channel's first page loaded.
    ChannelOpened,
    OpenMenu(Target),
    CloseMenu,
    Menu(Target, Action),
    Confirmed,
    CancelConfirm,
    NewDm(InstanceId),
    NewDmMsg(new_dm::Message),
    DmCreated(InstanceId, Result<Id, String>),
    /// Put text on the clipboard; the second field names it for the status line.
    Copy(String, &'static str),
    /// A background action finished: success or error text for the status line.
    Done(Result<String, String>),
    WindowResized(Size),
    Escape,
}
