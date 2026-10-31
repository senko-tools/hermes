use hermes_client_core::proto::id::Id;
use hermes_client_core::{CoreEvent, InstanceId};
use iced::keyboard::key::Physical;
use iced::keyboard::{Key, Modifiers};
use iced::widget::pane_grid;
use iced::{Size, window};

use crate::buffer::{channel, settings};
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
    Channel(pane_grid::Pane, channel::Message),
    /// A thumbnail arrived (instance, attachment).
    ImageLoaded(InstanceId, Id, Result<Vec<u8>, String>),
    /// The viewer's full-size image arrived (attachment).
    ViewerLoaded(Id, Result<Vec<u8>, String>),
    CloseViewer,
    /// Saves the viewer's image to the Downloads folder.
    DownloadViewer,
    /// Files chosen in the "+" dialog for a channel.
    FilesPicked(InstanceId, Id, Vec<std::path::PathBuf>),
    /// Files dropped on the window (for the focused pane's channel).
    FileDropped(std::path::PathBuf),
    /// An upload finished (instance, channel, local upload number).
    Uploaded(
        InstanceId,
        Id,
        u64,
        Result<hermes_client_core::proto::models::Attachment, String>,
    ),
    Tray(crate::desktop::tray::TrayAction),
    /// A notification toast was clicked: open its channel.
    NotificationClicked(InstanceId, Id),
}
