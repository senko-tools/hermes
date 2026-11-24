//! Desktop integration (PLAN.md section 15.7): notifications, the tray icon, taskbar flashing and
//! closing to the tray.

#[cfg(target_os = "linux")]
mod idle;
pub mod taskbar;
pub mod tray;
pub mod window;

use hermes_client_core::config::Notifications;
use hermes_client_core::notify::Notification;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::{InstanceId, Store};

/// Longest preview shown in a toast.
const PREVIEW_CHARS: usize = 200;

/// The app's id on Linux: the windows' Wayland app id and X11 class, and the name of the desktop
/// file and icons that `linux/install.sh` installs, through which the desktop finds the icon.
/// Windows does not use it.
pub const APP_ID: &str = "tools.senko.Hermes";

/// Linux: the windows are Wayland ones (gpui picks Wayland whenever `WAYLAND_DISPLAY` is set),
/// else X11. Decides how keys, screens and idle time are read.
#[cfg(target_os = "linux")]
pub fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty())
}

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

/// How long the user has not touched the mouse or keyboard; `None` where the system does not say.
#[cfg(windows)]
pub fn idle_time() -> Option<std::time::Duration> {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `info` is a valid LASTINPUTINFO with its size filled in; GetTickCount has no
    // preconditions.
    #[allow(unsafe_code)]
    let (ok, now) = unsafe { (GetLastInputInfo(&mut info).as_bool(), GetTickCount()) };
    // Both are the same 32-bit millisecond tick, which wraps after 49.7 days.
    ok.then(|| std::time::Duration::from_millis(u64::from(now.wrapping_sub(info.dwTime))))
}

#[cfg(target_os = "linux")]
pub use idle::idle_time;

#[cfg(not(any(windows, target_os = "linux")))]
pub fn idle_time() -> Option<std::time::Duration> {
    None
}

/// Starts Hermes when the user signs in, or stops doing so.
#[cfg(windows)]
pub fn set_start_on_login(on: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let mut command = std::process::Command::new("reg");
    if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        command.args(["add", KEY, "/v", "Hermes", "/t", "REG_SZ", "/f", "/d"]);
        command.arg(format!("\"{}\"", exe.display()));
    } else {
        command.args(["delete", KEY, "/v", "Hermes", "/f"]);
    }
    let status = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| e.to_string())?;
    // Deleting what was never there is fine too.
    if status.success() || !on {
        Ok(())
    } else {
        Err("the system refused the change".to_owned())
    }
}

/// Starts Hermes when the user signs in, or stops doing so: a desktop file in
/// `~/.config/autostart`, which every freedesktop desktop (KDE, GNOME, Xfce, …) reads.
#[cfg(target_os = "linux")]
pub fn set_start_on_login(on: bool) -> Result<(), String> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config")))
        .ok_or("the home folder is unknown")?
        .join("autostart");
    let file = dir.join(format!("{APP_ID}.desktop"));
    if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&file, autostart_entry(&exe)).map_err(|e| e.to_string())
    } else {
        match std::fs::remove_file(&file) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.to_string()),
            _ => Ok(()),
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn set_start_on_login(_on: bool) -> Result<(), String> {
    Err("starting at sign-in is only available on Windows and Linux for now".to_owned())
}

/// `text` with `&`, `<` and `>` escaped, for a freedesktop notification's body.
#[cfg(any(all(unix, not(target_os = "macos")), test))]
fn markup_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The autostart desktop file that runs `exe`.
#[cfg(any(target_os = "linux", test))]
fn autostart_entry(exe: &std::path::Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Hermes\nExec={}\nIcon={APP_ID}\n\
         X-GNOME-Autostart-enabled=true\n",
        exec_quote(&exe.to_string_lossy())
    )
}

/// `arg` quoted for a desktop file's `Exec` key (desktop entry spec, "The Exec key"): inside
/// double quotes, `"`, `` ` ``, `$` and `\` get a backslash, and since the file's own string
/// escapes are undone first, that backslash is itself written twice (`$` is `\\$`, `\` is
/// `\\\\`). `%` is doubled: it starts a field code.
#[cfg(any(target_os = "linux", test))]
fn exec_quote(arg: &str) -> String {
    let mut out = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' => {
                out.push_str("\\\\");
                out.push(c);
            }
            '\\' => out.push_str("\\\\\\\\"),
            '%' => out.push_str("%%"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
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

/// The number on the taskbar button: mentions everywhere, plus DMs with unread messages that
/// have no mention count of their own.
pub fn badge_count(store: &Store, instances: &[InstanceId]) -> u32 {
    instances
        .iter()
        .filter_map(|i| store.instance(i))
        .map(|s| {
            let mentions: u32 = s.read_states.values().map(|r| r.mention_count).sum();
            let dms = s
                .dms
                .values()
                .filter(|c| matches!(c.kind, ChannelKind::Dm | ChannelKind::GroupDm))
                .filter(|c| s.unread(c.id))
                .filter(|c| {
                    s.read_states
                        .get(&c.id)
                        .is_none_or(|r| r.mention_count == 0)
                })
                .count() as u32;
            mentions + dms
        })
        .sum()
}

/// Shows a toast and waits (on a blocking thread) for it to be clicked; `Some` when clicked.
pub async fn show_toast(
    inst: InstanceId,
    n: Notification,
    settings: Notifications,
) -> Option<(InstanceId, Id)> {
    let (title, body) = toast_text(&n, &settings);
    // Freedesktop servers read the body as simple markup: `a < b` or `R&D` would be cut or lost.
    #[cfg(all(unix, not(target_os = "macos")))]
    let body = markup_escape(&body);
    let channel = n.channel_id;
    tokio::task::spawn_blocking(move || {
        let mut toast = notify_rust::Notification::new();
        // No sound name: the toast stays silent, and the app plays its own soft ping instead.
        toast.summary(&title).body(&body).appname("Hermes");
        // Freedesktop servers report a click on the toast as its "default" action, and find the
        // icon through the desktop file.
        #[cfg(all(unix, not(target_os = "macos")))]
        toast
            .action("default", "Open")
            .icon(APP_ID)
            .hint(notify_rust::Hint::DesktopEntry(APP_ID.to_owned()));
        let handle = match toast.show() {
            Ok(handle) => handle,
            Err(err) => {
                tracing::warn!("notification not shown: {err}");
                return None;
            }
        };
        // Clicks come back while the app runs on Windows and Linux; elsewhere the toast only shows.
        #[cfg(windows)]
        {
            let mut clicked = false;
            let _ = handle.wait_for_response(|r: &notify_rust::NotificationResponse| {
                clicked = matches!(r, notify_rust::NotificationResponse::Default);
            });
            clicked.then_some((inst, channel))
        }
        // Returns when the toast is clicked, dismissed or expires.
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let mut clicked = false;
            handle.wait_for_action(|action| clicked = action == "default");
            clicked.then_some((inst, channel))
        }
        #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
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
    use hermes_client_core::testing::fixtures;

    #[test]
    fn autostart_runs_the_program_with_its_path_quoted() {
        let entry = autostart_entry(std::path::Path::new("/home/me/My Apps/hermes"));
        assert!(entry.starts_with("[Desktop Entry]\n"), "{entry}");
        assert!(
            entry.contains("\nExec=\"/home/me/My Apps/hermes\"\n"),
            "{entry}"
        );
        assert!(entry.contains("\nIcon=tools.senko.Hermes\n"), "{entry}");
        assert_eq!(exec_quote("a$b\"c`d"), r#""a\\$b\\"c\\`d""#);
        assert_eq!(exec_quote(r"a\b"), r#""a\\\\b""#);
        assert_eq!(exec_quote("100%"), "\"100%%\"");
    }

    #[test]
    fn toast_bodies_keep_angle_brackets_and_ampersands() {
        assert_eq!(
            markup_escape("a < b & <b>c</b>"),
            "a &lt; b &amp; &lt;b&gt;c&lt;/b&gt;"
        );
    }

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
