//! Context menu entries of the sidebar (PLAN.md section 15.1). Space settings, notification
//! level and mute come with Phase 6.

use hermes_client_core::InstanceId;
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::id::Id;

/// What was right-clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Instance(InstanceId),
    Space(InstanceId, Id),
    Channel(InstanceId, Id),
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
        }
    }

    /// Shown in red and confirmed first.
    pub fn is_destructive(self) -> bool {
        matches!(self, Action::RemoveInstance | Action::LeaveSpace)
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
    }
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
        assert!(!Action::MarkRead.is_destructive());
    }
}
