use hermes_client_core::proto::id::Id;
use hermes_client_core::{CoreEvent, InstanceId};
use iced::keyboard::key::Physical;
use iced::keyboard::{Key, Modifiers};
use iced::widget::pane_grid;
use iced::{Size, window};

use crate::buffer::settings;
use crate::modal::{add_instance, command_bar, new_dm};
use crate::panes::PaneAction;
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
    /// Closes the top menu or dialog, else marks the focused channel read.
    Escape,
    PaneClicked(pane_grid::Pane),
    PaneDragged(pane_grid::DragEvent),
    PaneResized(pane_grid::ResizeEvent),
    PaneMenu(pane_grid::Pane, PaneAction),
    /// A key press; `true` if a widget (a text input) already used it.
    Key(Key, Physical, Modifiers, bool),
    CloseRequested(window::Id),
    CommandBar(command_bar::Message),
}
