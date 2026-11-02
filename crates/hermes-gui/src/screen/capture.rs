//! Capturing and encoding my screen on its own thread while I share it. Windows only for now:
//! elsewhere [`Capture::start`] fails with a message (watching works everywhere).

// Capturing is Windows-only for now; the sizing rules are tested everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use std::time::{Duration, Instant};

use hermes_client_core::Core;

/// A share is sent at most this large, scaled down to fit (keeping the aspect ratio).
pub const MAX_WIDTH: u32 = 1280;
pub const MAX_HEIGHT: u32 = 720;
/// Frames per second.
pub const FPS: u32 = 10;
/// A keyframe at least this often, so a watcher who lost one recovers on their own.
#[cfg(windows)]
const KEYFRAME_EVERY: Duration = Duration::from_secs(5);

/// A running capture; dropping it stops the thread.
pub struct Capture {
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
}

impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Capture")
    }
}

impl Capture {
    /// Starts capturing `screen` (its system name; empty or unknown = the primary screen) and
    /// sending it through `core`, which must already share ([`Core::start_screen_share`]).
    pub fn start(core: Core, screen: &str) -> Result<Capture, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(true));
        start_thread(core, screen, stop.clone(), keyframe.clone())?;
        Ok(Capture { stop, keyframe })
    }

    /// The next frame is sent as a keyframe.
    pub fn request_keyframe(&self) {
        self.keyframe.store(true, Ordering::Relaxed);
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The largest even size within [`MAX_WIDTH`]×[`MAX_HEIGHT`] with the aspect ratio of
/// `width`×`height` (never larger than it).
pub fn fit(width: u32, height: u32) -> (u32, u32) {
    let scale = (f64::from(MAX_WIDTH) / f64::from(width.max(1)))
        .min(f64::from(MAX_HEIGHT) / f64::from(height.max(1)))
        .min(1.0);
    let even = |n: f64| ((n as u32) & !1).max(2);
    (
        even(f64::from(width) * scale),
        even(f64::from(height) * scale),
    )
}

/// The names of the screens, the primary one first (empty where capture is unavailable).
pub fn screens() -> Vec<String> {
    #[cfg(windows)]
    {
        let Ok(monitors) = xcap::Monitor::all() else {
            return Vec::new();
        };
        let mut named: Vec<(bool, String)> = monitors
            .iter()
            .filter_map(|m| Some((!m.is_primary().unwrap_or(false), m.name().ok()?)))
            .collect();
        named.sort();
        named.into_iter().map(|(_, name)| name).collect()
    }
    #[cfg(not(windows))]
    Vec::new()
}

#[cfg(not(windows))]
fn start_thread(
    _core: Core,
    _screen: &str,
    _stop: Arc<AtomicBool>,
    _keyframe: Arc<AtomicBool>,
) -> Result<(), String> {
    Err("screen sharing is only available on Windows for now".into())
}

#[cfg(windows)]
fn start_thread(
    core: Core,
    screen: &str,
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
) -> Result<(), String> {
    // A monitor cannot move between threads: the capture thread finds it and reports back.
    let screen = screen.to_owned();
    let (found, result) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("hermes-screen-capture".into())
        .spawn(move || match pick_monitor(&screen) {
            Ok(monitor) => {
                let _ = found.send(Ok(()));
                run(&core, &monitor, &stop, &keyframe);
            }
            Err(err) => {
                let _ = found.send(Err(err));
            }
        })
        .map_err(|e| e.to_string())?;
    result
        .recv()
        .map_err(|_| "the capture thread stopped".to_owned())?
}

/// The screen named `screen`, else the primary one, else the first.
#[cfg(windows)]
fn pick_monitor(screen: &str) -> Result<xcap::Monitor, String> {
    let monitors = xcap::Monitor::all().map_err(|e| e.to_string())?;
    let named = monitors
        .iter()
        .position(|m| !screen.is_empty() && m.name().ok().as_deref() == Some(screen));
    let primary = monitors
        .iter()
        .position(|m| m.is_primary().unwrap_or(false));
    let index = named
        .or(primary)
        .or((!monitors.is_empty()).then_some(0))
        .ok_or("no screen found")?;
    monitors
        .into_iter()
        .nth(index)
        .ok_or_else(|| "no screen found".to_owned())
}

#[cfg(windows)]
fn run(core: &Core, monitor: &xcap::Monitor, stop: &AtomicBool, keyframe: &AtomicBool) {
    use hermes_client_core::voice::ScreenFrame;

    let interval = Duration::from_millis(1000 / u64::from(FPS));
    let started = Instant::now();
    let mut encoder: Option<super::codec::Encoder> = None;
    let mut last_keyframe: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        let begin = Instant::now();
        match monitor.capture_image() {
            Ok(shot) => {
                let (w, h) = fit(shot.width(), shot.height());
                let frame = if shot.dimensions() == (w, h) {
                    shot
                } else {
                    image::imageops::thumbnail(&shot, w, h)
                };
                if encoder.is_none() {
                    match super::codec::Encoder::new(w, h, FPS as f32) {
                        Ok(e) => encoder = Some(e),
                        Err(err) => {
                            tracing::warn!("screen share encoder: {err}");
                            core.stop_screen_share();
                            return;
                        }
                    }
                }
                let key = keyframe.swap(false, Ordering::Relaxed)
                    || last_keyframe.is_none_or(|t| t.elapsed() >= KEYFRAME_EVERY);
                let encoded = encoder.as_mut().map(|e| e.encode(&frame, key));
                match encoded {
                    Some(Ok(Some((data, is_key)))) => {
                        if is_key {
                            last_keyframe = Some(Instant::now());
                        }
                        let sent = core.send_screen_frame(ScreenFrame {
                            keyframe: is_key,
                            width: w as u16,
                            height: h as u16,
                            timestamp: u32::try_from(started.elapsed().as_millis())
                                .unwrap_or(u32::MAX),
                            data,
                        });
                        if !sent {
                            // Dropped (the connection is behind): resynchronize watchers.
                            keyframe.store(true, Ordering::Relaxed);
                        }
                    }
                    Some(Ok(None)) | None => {}
                    Some(Err(err)) => tracing::warn!("screen share encoding: {err}"),
                }
            }
            Err(err) => {
                tracing::warn!("screen capture: {err}");
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        if !core.sharing_screen() {
            return;
        }
        std::thread::sleep(interval.saturating_sub(begin.elapsed()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_fit_720p_and_stay_even() {
        assert_eq!(fit(2560, 1440), (1280, 720));
        assert_eq!(fit(1920, 1080), (1280, 720));
        assert_eq!(fit(1280, 720), (1280, 720));
        assert_eq!(fit(1024, 768), (960, 720));
        assert_eq!(fit(3440, 1440), (1280, 534));
        assert_eq!(fit(801, 601), (800, 600));
        assert_eq!(fit(1, 1), (2, 2));
    }
}
