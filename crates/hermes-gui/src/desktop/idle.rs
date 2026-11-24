//! How long the user has not touched the mouse or keyboard, on Linux (for "Go idle after").
//!
//! - X11: the MIT-SCREEN-SAVER extension's idle time, asked when needed. Without it (XWayland),
//!   XInput2 raw events mark the last input.
//! - Wayland: `ext-idle-notify-v1` on a connection of our own. The compositor says when the
//!   input has been idle for `THRESHOLD`, and when it is used again. The idle time counts from
//!   then (plus the threshold). Version 2 ignores apps that hold the screen awake (a video), like
//!   Windows' `GetLastInputInfo`.
//! - GNOME without that protocol: Mutter's `org.gnome.Mutter.IdleMonitor` over D-Bus, polled.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The Wayland notification fires after this long without input. Short enough that "back
/// within 30 s" (`Shell::idle_for`) and the 5-minute setting both work.
const THRESHOLD: Duration = Duration::from_secs(20);

enum Source {
    X11(Box<x11rb::rust_connection::RustConnection>, u32),
    /// Since when the input is idle, kept by the Wayland thread.
    Wayland,
    /// Milliseconds, kept by the Mutter poller.
    Mutter,
    /// The last raw input event (X11 without MIT-SCREEN-SAVER).
    Raw,
    None,
}

static SOURCE: Mutex<Option<Source>> = Mutex::new(None);
static WAYLAND_IDLE_SINCE: Mutex<Option<Instant>> = Mutex::new(None);
static MUTTER_IDLE_MS: AtomicU64 = AtomicU64::new(0);
static LAST_RAW_INPUT: Mutex<Option<Instant>> = Mutex::new(None);

/// How long there was no input; `None` where nothing can tell.
pub fn idle_time() -> Option<Duration> {
    let mut source = SOURCE.lock().expect("idle source");
    let source = source.get_or_insert_with(start);
    match source {
        Source::X11(conn, root) => {
            use x11rb::protocol::screensaver::ConnectionExt as _;
            let reply = conn.screensaver_query_info(*root).ok()?.reply().ok()?;
            Some(Duration::from_millis(u64::from(reply.ms_since_user_input)))
        }
        Source::Wayland => Some(
            WAYLAND_IDLE_SINCE
                .lock()
                .expect("wayland idle")
                .map_or(Duration::ZERO, |since| since.elapsed() + THRESHOLD),
        ),
        Source::Mutter => Some(Duration::from_millis(
            MUTTER_IDLE_MS.load(Ordering::Relaxed),
        )),
        Source::Raw => Some(
            LAST_RAW_INPUT
                .lock()
                .expect("raw input")
                .map_or(Duration::ZERO, |at| at.elapsed()),
        ),
        Source::None => None,
    }
}

fn start() -> Source {
    if crate::desktop::wayland() {
        match wayland::start() {
            Ok(()) => return Source::Wayland,
            Err(err) => tracing::info!("no Wayland idle notifications ({err}); trying Mutter"),
        }
        if mutter::start() {
            return Source::Mutter;
        }
        tracing::info!("no idle time on this desktop: \"Go idle after\" does nothing");
        return Source::None;
    }
    match x11() {
        Ok(source) => source,
        Err(err) => match raw_input() {
            Ok(()) => {
                tracing::info!("X11 idle time from raw input ({err})");
                Source::Raw
            }
            Err(raw) => {
                tracing::info!("no X11 idle time ({err}; {raw})");
                Source::None
            }
        },
    }
}

/// Notes the time of every key, button and pointer motion (XInput2 raw events on the root).
fn raw_input() -> Result<(), String> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xinput::{self, ConnectionExt as _};

    let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
    let root = conn.setup().roots[screen].root;
    conn.xinput_xi_query_version(2, 2)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| format!("no XInput2: {e}"))?;
    let mask = xinput::XIEventMask::RAW_KEY_PRESS
        | xinput::XIEventMask::RAW_BUTTON_PRESS
        | xinput::XIEventMask::RAW_MOTION;
    conn.xinput_xi_select_events(
        root,
        &[xinput::EventMask {
            deviceid: xinput::Device::ALL_MASTER.into(),
            mask: vec![mask],
        }],
    )
    .map_err(|e| e.to_string())?
    .check()
    .map_err(|e| e.to_string())?;
    *LAST_RAW_INPUT.lock().expect("raw input") = Some(Instant::now());
    std::thread::Builder::new()
        .name("hermes-idle".into())
        .spawn(move || {
            while conn.wait_for_event().is_ok() {
                // Motion comes in bursts: one note per event is cheap enough.
                *LAST_RAW_INPUT.lock().expect("raw input") = Some(Instant::now());
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn x11() -> Result<Source, String> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::screensaver::ConnectionExt as _;

    let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
    let root = conn.setup().roots[screen].root;
    conn.screensaver_query_version(1, 1)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| format!("no MIT-SCREEN-SAVER: {e}"))?;
    Ok(Source::X11(Box::new(conn), root))
}

mod wayland {
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{wl_registry, wl_seat};
    use wayland_client::{Connection, Dispatch, Proxy as _, QueueHandle, delegate_noop};
    use wayland_protocols::ext::idle_notify::v1::client::{
        ext_idle_notification_v1::{self, ExtIdleNotificationV1},
        ext_idle_notifier_v1::ExtIdleNotifierV1,
    };

    use super::{THRESHOLD, WAYLAND_IDLE_SINCE};

    struct State;

    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut State,
            _: &wl_registry::WlRegistry,
            _: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<State>,
        ) {
        }
    }

    impl Dispatch<ExtIdleNotificationV1, ()> for State {
        fn event(
            _: &mut State,
            _: &ExtIdleNotificationV1,
            event: ext_idle_notification_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<State>,
        ) {
            let mut since = WAYLAND_IDLE_SINCE.lock().expect("wayland idle");
            match event {
                ext_idle_notification_v1::Event::Idled => *since = Some(std::time::Instant::now()),
                ext_idle_notification_v1::Event::Resumed => *since = None,
                _ => {}
            }
        }
    }

    delegate_noop!(State: ignore wl_seat::WlSeat);
    delegate_noop!(State: ExtIdleNotifierV1);

    pub fn start() -> Result<(), String> {
        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let seat: wl_seat::WlSeat = globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| format!("no seat: {e}"))?;
        let notifier: ExtIdleNotifierV1 = globals
            .bind(&qh, 1..=2, ())
            .map_err(|e| format!("no ext_idle_notifier_v1: {e}"))?;
        let timeout = THRESHOLD.as_millis() as u32;
        let notification = if notifier.version() >= 2 {
            notifier.get_input_idle_notification(timeout, &seat, &qh, ())
        } else {
            notifier.get_idle_notification(timeout, &seat, &qh, ())
        };
        queue.roundtrip(&mut State).map_err(|e| e.to_string())?;
        std::thread::Builder::new()
            .name("hermes-idle".into())
            .spawn(move || {
                // Kept alive with the thread.
                let _objects = (seat, notifier, notification);
                loop {
                    if let Err(err) = queue.blocking_dispatch(&mut State) {
                        tracing::warn!("idle notifications stopped: {err}");
                        *WAYLAND_IDLE_SINCE.lock().expect("wayland idle") = None;
                        return;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

mod mutter {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use super::MUTTER_IDLE_MS;

    const POLL: Duration = Duration::from_secs(10);

    /// Starts polling if Mutter answers.
    pub fn start() -> bool {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return false;
        };
        // Asked once here (off this thread, which may be inside the runtime), then polled.
        let first = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    runtime.block_on(async {
                        let conn = ashpd::zbus::Connection::session().await.ok()?;
                        let ms = idle_ms(&conn).await?;
                        Some((conn, ms))
                    })
                })
                .join()
                .ok()
                .flatten()
        });
        let Some((conn, ms)) = first else {
            return false;
        };
        MUTTER_IDLE_MS.store(ms, Ordering::Relaxed);
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(POLL).await;
                match idle_ms(&conn).await {
                    Some(ms) => MUTTER_IDLE_MS.store(ms, Ordering::Relaxed),
                    None => return,
                }
            }
        });
        true
    }

    async fn idle_ms(conn: &ashpd::zbus::Connection) -> Option<u64> {
        let reply = conn
            .call_method(
                Some("org.gnome.Mutter.IdleMonitor"),
                "/org/gnome/Mutter/IdleMonitor/Core",
                Some("org.gnome.Mutter.IdleMonitor"),
                "GetIdletime",
                &(),
            )
            .await
            .ok()?;
        reply.body().deserialize::<u64>().ok()
    }
}
