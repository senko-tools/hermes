//! Push-to-talk and the mute key on Wayland: the `org.freedesktop.portal.GlobalShortcuts`
//! portal (KDE, Hyprland, GNOME 47+). The desktop binds the keys: Hermes says which it would
//! like, and the desktop sends Activated / Deactivated for them wherever the focus is. The user
//! can change them in the desktop's shortcut settings; what is bound shows in Voice & Audio.
//!
//! One session holds both keys. When either changes, the session is closed and a new one binds
//! them again.

use std::sync::{LazyLock, Mutex, Once};

use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use futures_util::StreamExt as _;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;

use super::Reach;

const PTT_ID: &str = "push-to-talk";
const MUTE_ID: &str = "toggle-mute";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Wanted {
    /// The preferred triggers, in the XDG shortcuts format (`CTRL+SHIFT+m`).
    ptt: Option<String>,
    mute: Option<String>,
}

struct State {
    wanted: watch::Sender<Wanted>,
    ptt: Mutex<Option<UnboundedSender<bool>>>,
    mute: Mutex<Option<UnboundedSender<()>>>,
    /// How the desktop describes the bound PTT key.
    bound_ptt: Mutex<Option<String>>,
}

static STATE: LazyLock<State> = LazyLock::new(|| State {
    wanted: watch::Sender::new(Wanted::default()),
    ptt: Mutex::new(None),
    mute: Mutex::new(None),
    bound_ptt: Mutex::new(None),
});
static STARTED: Once = Once::new();

fn ensure_started() {
    STARTED.call_once(|| match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn(run());
        }
        Err(err) => tracing::warn!("global shortcuts: no runtime: {err}"),
    });
}

pub fn start_ptt(changes: UnboundedSender<bool>) {
    *STATE.ptt.lock().expect("portal ptt") = Some(changes);
    ensure_started();
}

pub fn start_mute(presses: UnboundedSender<()>) {
    *STATE.mute.lock().expect("portal mute") = Some(presses);
    ensure_started();
}

/// The PTT key from the settings; empty when push-to-talk is off.
pub fn set_ptt(text: &str) {
    let ptt = trigger(text);
    if ptt.is_none() {
        // Nothing to bind (none set, or a mouse button the portal cannot take): the focused
        // window hears it.
        super::set_reach(Reach::Focused);
        *STATE.bound_ptt.lock().expect("bound ptt") = None;
    }
    STATE.wanted.send_if_modified(|w| replace(&mut w.ptt, ptt));
}

pub fn set_mute(text: &str) {
    let mute = trigger(text);
    STATE
        .wanted
        .send_if_modified(|w| replace(&mut w.mute, mute));
}

fn replace(slot: &mut Option<String>, value: Option<String>) -> bool {
    let changed = *slot != value;
    *slot = value;
    changed
}

pub fn bound_ptt() -> Option<String> {
    STATE.bound_ptt.lock().expect("bound ptt").clone()
}

async fn run() {
    let mut wanted_rx = STATE.wanted.subscribe();
    loop {
        let wanted = wanted_rx.borrow_and_update().clone();
        if wanted.ptt.is_none() && wanted.mute.is_none() {
            if wanted_rx.changed().await.is_err() {
                return;
            }
            continue;
        }
        match bind(&wanted).await {
            Ok(bound) => {
                let ptt_bound = bound.ptt_description.is_some();
                *STATE.bound_ptt.lock().expect("bound ptt") = bound.ptt_description.clone();
                if wanted.ptt.is_some() {
                    super::set_reach(if ptt_bound {
                        Reach::System
                    } else {
                        Reach::Focused
                    });
                }
                tracing::info!(
                    ptt = ?bound.ptt_description,
                    mute = ?bound.mute_description,
                    "global shortcuts bound through the portal"
                );
                listen(bound, &mut wanted_rx).await;
            }
            Err(err) => {
                tracing::warn!("global shortcuts portal: {err}; push-to-talk only while focused");
                super::set_reach(Reach::Focused);
                *STATE.bound_ptt.lock().expect("bound ptt") = None;
                if wanted_rx.changed().await.is_err() {
                    return;
                }
            }
        }
    }
}

struct Bound {
    portal: GlobalShortcuts,
    session: ashpd::desktop::Session<GlobalShortcuts>,
    ptt_description: Option<String>,
    mute_description: Option<String>,
}

async fn bind(wanted: &Wanted) -> Result<Bound, ashpd::Error> {
    let portal = GlobalShortcuts::new().await?;
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await?;
    let mut shortcuts = Vec::new();
    if let Some(ptt) = &wanted.ptt {
        shortcuts.push(NewShortcut::new(PTT_ID, "Push to talk").preferred_trigger(ptt.as_str()));
    }
    if let Some(mute) = &wanted.mute {
        shortcuts.push(
            NewShortcut::new(MUTE_ID, "Mute or unmute the microphone")
                .preferred_trigger(mute.as_str()),
        );
    }
    let response = portal
        .bind_shortcuts(&session, &shortcuts, None, BindShortcutsOptions::default())
        .await?
        .response()?;
    let description = |id: &str| {
        response
            .shortcuts()
            .iter()
            .find(|s| s.id() == id)
            .map(|s| s.trigger_description().to_owned())
    };
    Ok(Bound {
        ptt_description: description(PTT_ID),
        mute_description: description(MUTE_ID),
        portal,
        session,
    })
}

/// Forwards the shortcut's signals until the wanted keys change.
async fn listen(bound: Bound, wanted_rx: &mut watch::Receiver<Wanted>) {
    let (activated, deactivated) = match (
        bound.portal.receive_activated().await,
        bound.portal.receive_deactivated().await,
    ) {
        (Ok(a), Ok(d)) => (a, d),
        (Err(err), _) | (_, Err(err)) => {
            tracing::warn!("global shortcuts: no signals: {err}");
            let _ = wanted_rx.changed().await;
            let _ = bound.session.close().await;
            return;
        }
    };
    let mut activated = std::pin::pin!(activated);
    let mut deactivated = std::pin::pin!(deactivated);
    let mut ptt_down = false;
    loop {
        tokio::select! {
            // New keys (or the app ending): `run` binds again.
            _ = wanted_rx.changed() => {
                let _ = bound.session.close().await;
                if ptt_down {
                    send_ptt(false);
                }
                return;
            }
            Some(event) = activated.next() => match event.shortcut_id() {
                PTT_ID => {
                    ptt_down = true;
                    send_ptt(true);
                }
                MUTE_ID => {
                    if let Some(tx) = STATE.mute.lock().expect("portal mute").as_ref() {
                        let _ = tx.send(());
                    }
                }
                _ => {}
            },
            Some(event) = deactivated.next() => {
                if event.shortcut_id() == PTT_ID {
                    ptt_down = false;
                    send_ptt(false);
                }
            }
            else => {
                let _ = wanted_rx.changed().await;
                return;
            }
        }
    }
}

fn send_ptt(down: bool) {
    if let Some(tx) = STATE.ptt.lock().expect("portal ptt").as_ref() {
        let _ = tx.send(down);
    }
}

/// A key text of ours (`Ctrl+Shift+M`, `F13`, `CapsLock`, global-hotkey's `KeyM`) as an XDG
/// shortcut trigger (`CTRL+SHIFT+m`). `None` for nothing or for mouse buttons.
pub fn trigger(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let mut modifiers = Vec::new();
    let mut key = None;
    for (i, part) in parts.iter().enumerate() {
        let upper = part.to_ascii_uppercase();
        let last = i + 1 == parts.len();
        let modifier = match upper.as_str() {
            "CTRL" | "CONTROL" => Some(("CTRL", "Control_L")),
            "SHIFT" => Some(("SHIFT", "Shift_L")),
            "ALT" | "OPTION" => Some(("ALT", "Alt_L")),
            "SUPER" | "META" | "WIN" | "LOGO" | "CMD" | "COMMAND" => Some(("LOGO", "Super_L")),
            _ => None,
        };
        match modifier {
            // A modifier alone is the key itself.
            Some((_, sym)) if last && modifiers.is_empty() => key = Some(sym.to_owned()),
            Some((name, _)) if !last => modifiers.push(name),
            Some(_) => return None,
            None if last => key = Some(keysym_name(&upper)?),
            None => return None,
        }
    }
    let key = key?;
    modifiers.push(&key);
    Some(modifiers.join("+"))
}

fn keysym_name(upper: &str) -> Option<String> {
    let name = match upper {
        "MOUSE3" | "MOUSE4" | "MOUSE5" | "MOUSEMIDDLE" | "MBUTTON" | "XBUTTON1" | "XBUTTON2" => {
            return None;
        }
        "SPACE" => "space",
        "TAB" => "Tab",
        "CAPSLOCK" => "Caps_Lock",
        "PAUSE" => "Pause",
        "SCROLLLOCK" => "Scroll_Lock",
        "INSERT" => "Insert",
        "DELETE" => "Delete",
        "HOME" => "Home",
        "END" => "End",
        "PAGEUP" => "Prior",
        "PAGEDOWN" => "Next",
        "ESCAPE" | "ESC" => "Escape",
        "ENTER" | "RETURN" => "Return",
        "BACKSPACE" => "BackSpace",
        "BACKQUOTE" | "GRAVE" | "`" => "grave",
        "MINUS" | "-" => "minus",
        "EQUAL" | "=" => "equal",
        "COMMA" | "," => "comma",
        "PERIOD" | "." => "period",
        "SLASH" | "/" => "slash",
        "BACKSLASH" | "\\" => "backslash",
        "SEMICOLON" | ";" => "semicolon",
        "QUOTE" | "'" => "apostrophe",
        "BRACKETLEFT" | "[" => "bracketleft",
        "BRACKETRIGHT" | "]" => "bracketright",
        "LSHIFT" => "Shift_L",
        "RSHIFT" => "Shift_R",
        "LCTRL" | "LCONTROL" => "Control_L",
        "RCTRL" | "RCONTROL" => "Control_R",
        "LALT" => "Alt_L",
        "RALT" => "Alt_R",
        "UP" | "ARROWUP" => "Up",
        "DOWN" | "ARROWDOWN" => "Down",
        "LEFT" | "ARROWLEFT" => "Left",
        "RIGHT" | "ARROWRIGHT" => "Right",
        other => {
            let bare = other
                .strip_prefix("KEY")
                .or_else(|| other.strip_prefix("DIGIT"))
                .filter(|rest| rest.len() == 1)
                .unwrap_or(other);
            if bare.len() == 1 && bare.as_bytes()[0].is_ascii_alphanumeric() {
                return Some(bare.to_ascii_lowercase());
            }
            if let Some(n) = bare.strip_prefix('F').and_then(|n| n.parse::<u8>().ok())
                && (1..=24).contains(&n)
            {
                return Some(format!("F{n}"));
            }
            return None;
        }
    };
    Some(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::trigger;

    #[test]
    fn key_texts_become_xdg_triggers() {
        assert_eq!(trigger("Ctrl+Shift+M").as_deref(), Some("CTRL+SHIFT+m"));
        assert_eq!(trigger("ctrl+shift+KeyM").as_deref(), Some("CTRL+SHIFT+m"));
        assert_eq!(trigger("F13").as_deref(), Some("F13"));
        assert_eq!(trigger("CapsLock").as_deref(), Some("Caps_Lock"));
        assert_eq!(trigger("V").as_deref(), Some("v"));
        assert_eq!(trigger("Alt+Digit1").as_deref(), Some("ALT+1"));
        assert_eq!(trigger("Ctrl").as_deref(), Some("Control_L"));
        assert_eq!(trigger("PageUp").as_deref(), Some("Prior"));
        assert_eq!(trigger("Mouse4"), None);
        assert_eq!(trigger(""), None);
        assert_eq!(trigger("Ctrl+Nonsense"), None);
        assert_eq!(trigger("M+Ctrl"), None);
    }
}
