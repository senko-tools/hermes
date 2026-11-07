//! Desktop integration (PLAN.md section 15.7): notifications, the tray icon, taskbar flashing and
//! closing to the tray.

pub mod tray;
pub mod window;

use hermes_client_core::config::Notifications;
use hermes_client_core::notify::Notification;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::{InstanceId, Store};

/// Longest preview shown in a toast.
const PREVIEW_CHARS: usize = 200;

/// Title and body of a toast: `Alice in #general` / the message, or just `Alice` for a DM.
pub fn toast_text(n: &Notification, settings: &Notifications) -> (String, String) {
    let title = if n.space_id.is_some() {
        format!("{} in {}", n.author, n.title)
    } else if n.title == n.author || n.title.is_empty() {
        n.author.clone()
    } else {
        // A group DM.
        format!("{} in {}", n.author, n.title)
    };
    let body = if settings.show_preview {
        let preview: String = n.preview.chars().take(PREVIEW_CHARS).collect();
        if n.preview.chars().count() > PREVIEW_CHARS {
            format!("{preview}…")
        } else if preview.trim().is_empty() {
            "Sent an attachment".to_owned()
        } else {
            preview
        }
    } else {
        "New message".to_owned()
    };
    (title, body)
}

/// Whether a notification flashes the taskbar: DMs and mentions only.
pub fn flashes(n: &Notification) -> bool {
    n.mention || n.space_id.is_none()
}

/// Whether anything needs attention: a mention anywhere, or an unread DM.
pub fn has_badge(store: &Store, instances: &[InstanceId]) -> bool {
    instances.iter().filter_map(|i| store.instance(i)).any(|s| {
        s.read_states.values().any(|r| r.mention_count > 0)
            || s.dms
                .values()
                .any(|c| matches!(c.kind, ChannelKind::Dm | ChannelKind::GroupDm) && s.unread(c.id))
    })
}

/// Shows a toast and waits (on a blocking thread) for it to be clicked; `Some` when clicked.
pub async fn show_toast(
    inst: InstanceId,
    n: Notification,
    settings: Notifications,
) -> Option<(InstanceId, Id)> {
    let (title, body) = toast_text(&n, &settings);
    let channel = n.channel_id;
    tokio::task::spawn_blocking(move || {
        let mut toast = notify_rust::Notification::new();
        toast.summary(&title).body(&body).appname("Hermes");
        #[cfg(any(windows, target_os = "macos"))]
        if settings.sound {
            toast.sound_name("Default");
        }
        let handle = match toast.show() {
            Ok(handle) => handle,
            Err(err) => {
                tracing::warn!("notification not shown: {err}");
                return None;
            }
        };
        // Clicks come back while the app runs on Windows; elsewhere the toast only shows.
        #[cfg(windows)]
        {
            let mut clicked = false;
            let _ = handle.wait_for_response(|r: &notify_rust::NotificationResponse| {
                clicked = matches!(r, notify_rust::NotificationResponse::Default);
            });
            clicked.then_some((inst, channel))
        }
        #[cfg(not(windows))]
        {
            drop(handle);
            let _ = (inst, channel);
            None
        }
    })
    .await
    .ok()
    .flatten()
}

/// What closing the window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// Hide it; the tray icon brings it back.
    Hide,
    Quit,
}

pub fn on_close(close_to_tray: bool, tray_available: bool) -> CloseAction {
    if close_to_tray && tray_available {
        CloseAction::Hide
    } else {
        CloseAction::Quit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    fn notification(space: Option<u64>, title: &str, preview: &str, mention: bool) -> Notification {
        Notification {
            channel_id: Id(11),
            space_id: space.map(Id),
            message_id: Id(100),
            author: "Bob".into(),
            title: title.into(),
            preview: preview.into(),
            mention,
        }
    }

    #[test]
    fn toast_texts() {
        let settings = Notifications::default();
        assert_eq!(
            toast_text(&notification(Some(10), "#general", "hi", true), &settings),
            ("Bob in #general".into(), "hi".into())
        );
        assert_eq!(
            toast_text(&notification(None, "Bob", "psst", false), &settings),
            ("Bob".into(), "psst".into())
        );
        assert_eq!(
            toast_text(&notification(None, "Trio", "yo", false), &settings).0,
            "Bob in Trio"
        );
        let long = "x".repeat(500);
        let (_, body) = toast_text(&notification(None, "Bob", &long, false), &settings);
        assert_eq!(body.chars().count(), PREVIEW_CHARS + 1);
        assert_eq!(
            toast_text(&notification(None, "Bob", "", false), &settings).1,
            "Sent an attachment"
        );
        let hidden = Notifications {
            show_preview: false,
            ..Notifications::default()
        };
        assert_eq!(
            toast_text(&notification(None, "Bob", "secret", false), &hidden).1,
            "New message"
        );
    }

    #[test]
    fn flashing_and_badges() {
        assert!(flashes(&notification(None, "Bob", "", false)));
        assert!(flashes(&notification(Some(10), "#general", "", true)));
        assert!(!flashes(&notification(Some(10), "#general", "", false)));

        let mut store = Store::default();
        assert!(!has_badge(&store, &["h".into()]));
        // The fixture has 2 mentions in #general and an unread DM.
        store.instances.insert("h".into(), fixtures::store());
        assert!(has_badge(&store, &["h".into()]));
        assert!(!has_badge(&store, &["other".into()]));
    }

    #[test]
    fn closing() {
        assert_eq!(on_close(true, true), CloseAction::Hide);
        assert_eq!(
            on_close(true, false),
            CloseAction::Quit,
            "no tray: never hide"
        );
        assert_eq!(on_close(false, true), CloseAction::Quit);
    }
}
