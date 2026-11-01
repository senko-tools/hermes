//! When a message notifies (PLAN.md section 14.5). The UI decides how to show it.

use hermes_proto::id::Id;
use hermes_proto::models::{Channel, Message, NotificationLevel, PresenceStatus};

use crate::store::InstanceStore;

/// A message that should produce a toast/sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub channel_id: Id,
    pub space_id: Option<Id>,
    pub message_id: Id,
    pub author: String,
    /// `#general` / `Friends`, or the DM partner's name.
    pub title: String,
    pub preview: String,
    pub mention: bool,
}

/// The notification level for a channel: its own setting, else its space's, else the default
/// (`all` for DMs, `mentions` for spaces). `None` when muted.
pub fn level(store: &InstanceStore, channel: Id, now: i64) -> Option<NotificationLevel> {
    let space = store.space_of(channel);
    for target in [Some(channel), space].into_iter().flatten() {
        if let Some(setting) = store.notification_settings.get(&target) {
            if setting.is_muted(now) {
                return None;
            }
            return Some(setting.level);
        }
    }
    Some(if space.is_some() {
        NotificationLevel::Mentions
    } else {
        NotificationLevel::All
    })
}

/// Whether `message` notifies. `focused` = the channel is shown in a visible pane of a focused
/// window.
pub fn check(
    store: &InstanceStore,
    message: &Message,
    focused: bool,
    my_status: PresenceStatus,
    now: i64,
) -> Option<Notification> {
    if focused
        || my_status == PresenceStatus::Dnd
        || Some(message.author.id) == store.me_id()
        || message.deleted
        // A message request notified once, when it arrived.
        || store.channel(message.channel_id).is_some_and(|c| c.request)
    {
        return None;
    }
    let mention = store.mentions_me(message);
    match level(store, message.channel_id, now)? {
        NotificationLevel::All => {}
        NotificationLevel::Mentions if mention => {}
        NotificationLevel::Mentions | NotificationLevel::None => return None,
    }
    let space = store.space_of(message.channel_id);
    let channel = store.channel(message.channel_id)?;
    let title = match space {
        Some(space_id) => {
            let space_name = store
                .spaces
                .get(&space_id)
                .map_or("", |s| s.space.name.as_str());
            format!(
                "#{} ({space_name})",
                channel.name.as_deref().unwrap_or("channel")
            )
        }
        None => match store.dm_partner(channel) {
            Some(user) => user.name().to_owned(),
            None => channel.name.clone().unwrap_or_else(|| "Group DM".into()),
        },
    };
    let mut preview: String = message.content.chars().take(140).collect();
    if preview.is_empty() && !message.attachments.is_empty() {
        preview = format!("[{} attachment(s)]", message.attachments.len());
    }
    Some(Notification {
        channel_id: message.channel_id,
        space_id: space,
        message_id: message.id,
        author: store.display_name(space, message.author.id),
        title,
        preview,
        mention,
    })
}

/// The one notification of a new message request (`dm_create` with `request`).
pub fn request(
    store: &InstanceStore,
    dm: &Channel,
    my_status: PresenceStatus,
) -> Option<Notification> {
    if !dm.request || my_status == PresenceStatus::Dnd {
        return None;
    }
    let me = store.me_id();
    let from = dm
        .recipients
        .iter()
        .find(|u| Some(u.id) != me)
        .map_or_else(|| "Someone".to_owned(), |u| u.name().to_owned());
    Some(Notification {
        channel_id: dm.id,
        space_id: None,
        message_id: dm.last_message_id.unwrap_or(dm.id),
        author: from.clone(),
        title: "Message request".into(),
        preview: format!("{from} wants to send you messages"),
        mention: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::{channel, message, ready, user};
    use hermes_proto::gateway::ServerEvent;
    use hermes_proto::models::{ChannelKind, NotificationSetting};

    fn store() -> InstanceStore {
        let mut s = InstanceStore::default();
        s.apply_ready(ready());
        let mut dm = channel(50, None, ChannelKind::Dm, "");
        dm.name = None;
        dm.recipients = vec![user(1, "me"), user(2, "bob")];
        s.apply(ServerEvent::DmCreate(dm), 0);
        s
    }

    #[test]
    fn message_requests_notify_once_and_stay_quiet() {
        let mut s = store();
        let stranger = user(7, "stranger");
        let mut dm = channel(60, None, ChannelKind::Dm, "");
        dm.name = None;
        dm.recipients = vec![user(1, "me"), stranger.clone()];
        dm.request = true;
        let n = request(&s, &dm, PresenceStatus::Online).unwrap();
        assert_eq!(n.title, "Message request");
        assert_eq!(n.author, "stranger");
        assert!(request(&s, &dm, PresenceStatus::Dnd).is_none());
        s.apply(ServerEvent::DmCreate(dm.clone()), 0);
        let hello = message(70, 60, &stranger, "hello");
        assert!(check(&s, &hello, false, PresenceStatus::Online, 0).is_none());
        assert_eq!(crate::view::dm_requests(&s).len(), 1);
        assert!(crate::view::dm_list(&s).iter().all(|d| d.id != Id(60)));

        // Accepted, it is a normal DM.
        dm.request = false;
        assert!(request(&s, &dm, PresenceStatus::Online).is_none());
        s.apply(ServerEvent::DmUpdate(dm), 0);
        assert!(check(&s, &hello, false, PresenceStatus::Online, 0).is_some());
        assert!(crate::view::dm_requests(&s).is_empty());
    }

    #[test]
    fn defaults_dms_all_spaces_mentions() {
        let s = store();
        let bob = user(2, "bob");
        let online = PresenceStatus::Online;
        let dm = message(1, 50, &bob, "hey");
        let n = check(&s, &dm, false, online, 0).unwrap();
        assert_eq!(n.title, "bob");
        assert!(!n.mention);
        assert!(check(&s, &dm, true, online, 0).is_none(), "focused");
        assert!(
            check(&s, &dm, false, PresenceStatus::Dnd, 0).is_none(),
            "dnd"
        );

        let plain = message(2, 11, &bob, "hello all");
        assert!(check(&s, &plain, false, online, 0).is_none());
        let mut mention = message(3, 11, &bob, "@everyone");
        mention.mentions.everyone = true;
        let n = check(&s, &mention, false, online, 0).unwrap();
        assert_eq!(n.title, "#general (Friends)");
        assert!(n.mention);

        let mine = message(4, 50, &user(1, "me"), "my own");
        assert!(check(&s, &mine, false, online, 0).is_none());
    }

    #[test]
    fn settings_and_mutes() {
        let mut s = store();
        let bob = user(2, "bob");
        let set = |s: &mut InstanceStore, target, level, muted_until| {
            s.apply(
                ServerEvent::NotificationSettingsUpdate(NotificationSetting {
                    target_id: Id(target),
                    level,
                    muted_until,
                }),
                0,
            );
        };
        set(&mut s, 10, NotificationLevel::All, None);
        let plain = message(2, 11, &bob, "hello all");
        assert!(check(&s, &plain, false, PresenceStatus::Online, 0).is_some());
        // The channel setting wins over the space.
        set(&mut s, 11, NotificationLevel::None, None);
        assert!(check(&s, &plain, false, PresenceStatus::Online, 0).is_none());
        set(&mut s, 11, NotificationLevel::All, Some(1000));
        assert!(
            check(&s, &plain, false, PresenceStatus::Online, 999).is_none(),
            "muted"
        );
        assert!(check(&s, &plain, false, PresenceStatus::Online, 1000).is_some());
        set(
            &mut s,
            50,
            NotificationLevel::All,
            Some(NotificationSetting::MUTED_FOREVER),
        );
        let dm = message(5, 50, &bob, "hey");
        assert!(check(&s, &dm, false, PresenceStatus::Online, 0).is_none());
    }
}
