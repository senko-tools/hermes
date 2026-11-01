//! Context menu entries of the sidebar and the pane headers (PLAN.md section 15.1). Space
//! settings, notification level and mute come with Phase 6.

use hermes_client_core::InstanceId;
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::id::Id;
use iced::widget::pane_grid;

/// What was right-clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Instance(InstanceId),
    Space(InstanceId, Id),
    Channel(InstanceId, Id),
    /// A pane header's `⋮`.
    Pane(pane_grid::Pane),
    /// A message's `⋯`: instance, channel, message.
    Message(InstanceId, Id, Id),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Reconnect,
    GoOffline,
    GoOnline,
    LogOut,
    RemoveInstance,
    InvitePeople,
    CopySpaceId,
    LeaveSpace,
    MarkRead,
    CopyChannelId,
    SplitRight,
    SplitDown,
    MaximizePane,
    ToggleMembers,
    ClosePane,
    CopyText,
    CopyMessageId,
    Pin,
    Unpin,
    DeleteMessage,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Reconnect => "Reconnect",
            Action::GoOffline => "Go offline",
            Action::GoOnline => "Go online",
            Action::LogOut => "Log out",
            Action::RemoveInstance => "Remove instance",
            Action::InvitePeople => "Invite people (copy link)",
            Action::CopySpaceId => "Copy space ID",
            Action::LeaveSpace => "Leave space",
            Action::MarkRead => "Mark as read",
            Action::CopyChannelId => "Copy channel ID",
            Action::SplitRight => "Split right",
            Action::SplitDown => "Split down",
            Action::MaximizePane => "Maximize / restore",
            Action::ToggleMembers => "Toggle member list",
            Action::ClosePane => "Close pane",
            Action::CopyText => "Copy text",
            Action::CopyMessageId => "Copy message ID",
            Action::Pin => "Pin message",
            Action::Unpin => "Unpin message",
            Action::DeleteMessage => "Delete message",
        }
    }

    /// Shown in red and confirmed first.
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            Action::RemoveInstance | Action::LeaveSpace | Action::DeleteMessage
        )
    }
}

/// The entries for `target`; `status` is the instance's connection status.
pub fn actions(target: &Target, status: &ConnStatus) -> Vec<Action> {
    match target {
        Target::Instance(_) => {
            let mut actions = Vec::new();
            match status {
                ConnStatus::LoggedOut => {}
                ConnStatus::Offline => actions.push(Action::GoOnline),
                _ => actions.extend([Action::Reconnect, Action::GoOffline]),
            }
            if *status != ConnStatus::LoggedOut {
                actions.push(Action::LogOut);
            }
            actions.push(Action::RemoveInstance);
            actions
        }
        Target::Space(..) => vec![
            Action::InvitePeople,
            Action::CopySpaceId,
            Action::LeaveSpace,
        ],
        Target::Channel(..) => vec![Action::MarkRead, Action::CopyChannelId],
        Target::Pane(_) => vec![
            Action::SplitRight,
            Action::SplitDown,
            Action::MaximizePane,
            Action::ToggleMembers,
            Action::ClosePane,
        ],
        // Needs the store; see `message_actions`.
        Target::Message(..) => vec![Action::CopyText, Action::CopyMessageId],
    }
}

/// The entries for a message: pin and delete depend on what I may do.
pub fn message_actions(own: bool, manage: bool, pinned: bool) -> Vec<Action> {
    let mut actions = vec![Action::CopyText, Action::CopyMessageId];
    if manage {
        actions.push(if pinned { Action::Unpin } else { Action::Pin });
    }
    if own || manage {
        actions.push(Action::DeleteMessage);
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_entries_follow_the_status() {
        let target = Target::Instance("h".into());
        assert_eq!(
            actions(&target, &ConnStatus::Connected),
            [
                Action::Reconnect,
                Action::GoOffline,
                Action::LogOut,
                Action::RemoveInstance
            ]
        );
        assert_eq!(
            actions(&target, &ConnStatus::Offline),
            [Action::GoOnline, Action::LogOut, Action::RemoveInstance]
        );
        assert_eq!(
            actions(&target, &ConnStatus::LoggedOut),
            [Action::RemoveInstance]
        );
        assert!(Action::LeaveSpace.is_destructive());
        assert_eq!(
            message_actions(true, false, false),
            [
                Action::CopyText,
                Action::CopyMessageId,
                Action::DeleteMessage
            ]
        );
        assert_eq!(
            message_actions(false, true, true),
            [
                Action::CopyText,
                Action::CopyMessageId,
                Action::Unpin,
                Action::DeleteMessage
            ]
        );
        assert_eq!(
            message_actions(false, false, false),
            [Action::CopyText, Action::CopyMessageId]
        );
        assert!(!Action::MarkRead.is_destructive());
    }
}
