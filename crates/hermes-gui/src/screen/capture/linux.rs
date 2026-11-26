//! Capture on Linux. X11: our own picker lists screens and windows, and `screen::x11` takes
//! the pictures. Wayland does not let apps see other windows: the desktop's own dialog picks
//! what to share (the ScreenCast portal, `screen::portal`), and frames arrive over PipeWire.
//!
//! Both hand frames to the same encoder thread as on Windows (`encode_loop`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use hermes_client_core::Core;

use super::{
    Frame, IDLE_POLL, KEYFRAME_EVERY, Quality, STATS_EVERY, ScreenInfo, Shareable, Stats,
    THUMB_WIDTH, Tuning, WINDOW_PREFIX, encode_loop, log_stats, micros, scale, thumbnail_of,
};
use crate::screen::x11::{self, GrabError, Grabber};

/// Screens and windows for our picker (X11). Wayland: none, the desktop's dialog chooses.
pub(super) fn sources(pictures: bool) -> Vec<Shareable> {
    if crate::desktop::wayland() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (info, monitor) in ordered_monitors() {
        let picture = pictures
            .then(|| Grabber::screen(&monitor).ok())
            .flatten()
            .and_then(|mut g| g.grab().ok())
            .map(|f| thumbnail_of(&f.pixels, f.width, f.height, true));
        out.push(Shareable {
            title: info.title(),
            detail: info.detail(),
            value: info.value,
            window: false,
            minimized: false,
            picture,
        });
    }
    let me = std::process::id();
    let mut windows: Vec<_> = x11::windows()
        .unwrap_or_default()
        .into_iter()
        .filter(|w| w.pid != Some(me) && !w.title.trim().is_empty())
        .filter(|w| w.minimized || u64::from(w.width) * u64::from(w.height) >= 64 * 64)
        .collect();
    // Topmost first already; minimized ones go last, as on Windows.
    windows.sort_by_key(|w| w.minimized);
    for w in windows {
        let picture = (pictures && !w.minimized)
            .then(|| Grabber::window(w.id).ok())
            .flatten()
            .and_then(|mut g| g.grab().ok())
            .map(|f| thumbnail_of(&f.pixels, f.width, f.height, true));
        debug_assert!(picture.as_ref().is_none_or(|p| p.width() <= THUMB_WIDTH));
        out.push(Shareable {
            value: format!("{WINDOW_PREFIX}{}", w.id),
            title: w.title,
            detail: w.app,
            window: true,
            minimized: w.minimized,
            picture,
        });
    }
    out
}

/// The screens, the primary one first (X11; Wayland: none, the desktop's dialog chooses).
pub(super) fn screens() -> Vec<ScreenInfo> {
    if crate::desktop::wayland() {
        return Vec::new();
    }
    ordered_monitors()
        .into_iter()
        .map(|(info, _)| info)
        .collect()
}

/// The primary monitor first, then left to right, top to bottom (as on Windows).
fn ordered_monitors() -> Vec<(ScreenInfo, x11::Monitor)> {
    let Ok(monitors) = x11::monitors() else {
        return Vec::new();
    };
    let mut keyed: Vec<_> = monitors
        .into_iter()
        .map(|m| ((!m.primary, m.x, m.y, m.name.clone()), m))
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    keyed
        .into_iter()
        .enumerate()
        .map(|(n, ((secondary, ..), m))| {
            let info = ScreenInfo {
                value: if secondary {
                    m.name.clone()
                } else {
                    String::new()
                },
                number: n + 1,
                primary: !secondary,
                width: u32::from(m.width),
                height: u32::from(m.height),
            };
            (info, m)
        })
        .collect()
}

pub(super) fn start_thread(
    core: Core,
    screen: &str,
    quality: Quality,
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    _tuning: Arc<Tuning>,
) -> Result<(), String> {
    if crate::desktop::wayland() {
        // `Capture::start_portal` after the desktop's dialog instead.
        return Err("on Wayland, the desktop's dialog picks what to share".into());
    }
    let (grabber, window) = match screen.strip_prefix(WINDOW_PREFIX) {
        Some(id) => {
            let id: u32 = id.parse().map_err(|_| "no such window".to_owned())?;
            (Grabber::window(id)?, true)
        }
        None => {
            let monitors = ordered_monitors();
            let monitor = monitors
                .iter()
                .find(|(info, _)| info.value == screen)
                .or_else(|| monitors.first())
                .map(|(_, m)| m.clone())
                .ok_or("no screen found")?;
            (Grabber::screen(&monitor)?, false)
        }
    };
    std::thread::Builder::new()
        .name("hermes-screen-capture".into())
        .spawn(move || run(&core, grabber, window, quality, &stop, &keyframe))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Takes a picture `quality.fps` times a second and hands it to the encoder thread. A screen
/// that did not change sends nothing (as with Desktop Duplication), except the last picture
/// again when a keyframe is due.
fn run(
    core: &Core,
    mut grabber: Grabber,
    window: bool,
    quality: Quality,
    stop: &AtomicBool,
    keyframe: &Arc<AtomicBool>,
) {
    let stats = Arc::new(Stats::default());
    let Some(frames) = spawn_encoder(core, quality, keyframe, &stats) else {
        return;
    };
    let what = format!("{}, OpenH264", grabber.describe());
    let interval = Duration::from_millis(1000 / u64::from(quality.fps));
    let mut last: Option<Arc<Frame>> = None;
    let mut last_raw: Vec<u8> = Vec::new();
    let mut last_sent = Instant::now();
    let mut stats_since = Instant::now();
    let mut idle = false;
    while !stop.load(Ordering::Relaxed) && core.sharing_screen() {
        if core.share_watchers() == Some(0) {
            if !idle {
                tracing::info!("screen share: nobody is watching, capture paused");
                idle = true;
            }
            std::thread::sleep(IDLE_POLL);
            continue;
        }
        if idle {
            tracing::info!("screen share: someone is watching, capture resumed");
            idle = false;
            keyframe.store(true, Ordering::Relaxed);
            stats_since = Instant::now();
        }
        let begin = Instant::now();
        let shot = match grabber.grab() {
            Ok(shot) => Some(shot),
            // Minimized: the last picture stays on the watchers' screens.
            Err(GrabError::Hidden) => {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            Err(GrabError::Gone(err)) if window => {
                tracing::info!("window share: the window is gone ({err})");
                core.stop_screen_share();
                return;
            }
            Err(GrabError::Gone(err)) => {
                tracing::warn!("screen capture: {err}");
                std::thread::sleep(Duration::from_millis(500));
                None
            }
        };
        // Unchanged pictures are not sent again (comparing costs far less than encoding).
        let shot = shot.filter(|shot| {
            let changed = shot.pixels != last_raw;
            if changed {
                last_raw.clone_from(&shot.pixels);
            }
            changed
        });
        let frame = match shot {
            Some(shot) => {
                let frame = Arc::new(scale(shot, quality));
                stats.captured.fetch_add(1, Ordering::Relaxed);
                stats
                    .capture_us
                    .fetch_add(micros(begin.elapsed()), Ordering::Relaxed);
                last = Some(frame.clone());
                Some(frame)
            }
            None if keyframe.load(Ordering::Relaxed) || last_sent.elapsed() >= KEYFRAME_EVERY => {
                last.clone()
            }
            None => None,
        };
        if let Some(frame) = frame {
            match frames.try_send(frame) {
                Ok(()) => last_sent = Instant::now(),
                Err(std::sync::mpsc::TrySendError::Full(_)) => {
                    stats.dropped.fetch_add(1, Ordering::Relaxed);
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => return,
            }
        }
        if stats_since.elapsed() >= STATS_EVERY {
            let size = last.as_deref().map_or((0, 0), |f| (f.width, f.height));
            log_stats(&stats, stats_since.elapsed(), size, quality.fps, &what);
            stats_since = Instant::now();
        }
        std::thread::sleep(interval.saturating_sub(begin.elapsed()));
    }
}

/// Starts the encoder thread; frames handed to the sender are encoded and sent.
pub(crate) fn spawn_encoder(
    core: &Core,
    quality: Quality,
    keyframe: &Arc<AtomicBool>,
    stats: &Arc<Stats>,
) -> Option<std::sync::mpsc::SyncSender<Arc<Frame>>> {
    let (frames, encoder_frames) = std::sync::mpsc::sync_channel::<Arc<Frame>>(1);
    let spawned = {
        let core = core.clone();
        let keyframe = keyframe.clone();
        let stats = stats.clone();
        std::thread::Builder::new()
            .name("hermes-screen-encode".into())
            .spawn(move || encode_loop(&core, quality, &encoder_frames, &keyframe, &stats))
    };
    match spawned {
        Ok(_) => Some(frames),
        Err(err) => {
            tracing::warn!("screen share encoder thread: {err}");
            core.stop_screen_share();
            None
        }
    }
}
