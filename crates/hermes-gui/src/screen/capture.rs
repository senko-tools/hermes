//! Capturing and encoding my screen while I share it, on two threads that work in parallel
//! (capturing and scaling; encoding and sending). Windows only for now: elsewhere
//! [`Capture::start`] fails with a message (watching works everywhere).

// Capturing is Windows-only for now; the sizing rules are tested everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::Arc;
#[cfg(windows)]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use hermes_client_core::Core;
use hermes_client_core::proto::voice::{MAX_VIDEO_DIMENSION, MAX_VIDEO_FPS};

#[cfg(windows)]
use super::codec::Frame;

/// How large and how often a share is sent (`voice.share_resolution`, `voice.share_fps`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quality {
    /// Pictures are scaled down to fit this (keeping the aspect ratio).
    pub max_width: u32,
    pub max_height: u32,
    pub fps: u32,
}

/// The resolutions offered; anything else counts as the first.
pub const RESOLUTIONS: [&str; 5] = ["720p", "1080p", "1440p", "2160p", "native"];
/// The frame rates offered.
pub const FRAME_RATES: [u32; 3] = [15, 30, 60];

impl Quality {
    pub fn from_config(resolution: &str, fps: u32) -> Quality {
        let max = u32::from(MAX_VIDEO_DIMENSION);
        let (max_width, max_height) = match resolution {
            "1080p" => (1920, 1080),
            "1440p" => (2560, 1440),
            "2160p" => (3840, 2160),
            "native" => (max, max),
            _ => (1280, 720),
        };
        Quality {
            max_width,
            max_height,
            fps: fps.clamp(1, MAX_VIDEO_FPS),
        }
    }
}

/// What a share keeps when the machine or the connection cannot keep up, and what it gives
/// way on first (`voice.share_preference`). Clarity also spends more bits on every picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preference {
    /// Keep the frame rate; lower the bitrate, then the size.
    Smoothness,
    /// Lower the bitrate a little, then the frame rate, then the size.
    Mixed,
    /// Keep the size and sharp pictures; lower the frame rate.
    Clarity,
}

/// The preferences offered, in order.
pub const PREFERENCES: [Preference; 3] = [
    Preference::Smoothness,
    Preference::Mixed,
    Preference::Clarity,
];

/// One step down from the full quality: factors for the frame rate, the size and the bitrate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub fps: f64,
    pub size: f64,
    pub bits: f64,
}

const fn step(fps: f64, size: f64, bits: f64) -> Step {
    Step { fps, size, bits }
}

impl Preference {
    /// Anything unknown is smoothness.
    pub fn from_config(value: &str) -> Preference {
        match value {
            "clarity" => Preference::Clarity,
            "mixed" => Preference::Mixed,
            _ => Preference::Smoothness,
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Preference::Smoothness => "smoothness",
            Preference::Mixed => "mixed",
            Preference::Clarity => "clarity",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Preference::Smoothness => "Smoothness",
            Preference::Mixed => "Mixed",
            Preference::Clarity => "Clarity",
        }
    }

    /// Bits for each picture against [`hardware_bitrate`]: clearer pictures cost more.
    fn bits(self) -> f64 {
        match self {
            Preference::Smoothness => 1.0,
            Preference::Mixed => 1.4,
            Preference::Clarity => 2.0,
        }
    }

    /// Full quality, then each step down while the share falls behind.
    pub fn steps(self) -> [Step; 4] {
        match self {
            Preference::Smoothness => [
                step(1.0, 1.0, 1.0),
                step(1.0, 1.0, 0.6),
                step(1.0, 0.75, 0.6),
                step(1.0, 0.5, 0.6),
            ],
            Preference::Mixed => [
                step(1.0, 1.0, 1.0),
                step(1.0, 1.0, 0.7),
                step(0.5, 1.0, 0.7),
                step(0.5, 0.75, 0.7),
            ],
            Preference::Clarity => [
                step(1.0, 1.0, 1.0),
                step(0.5, 1.0, 1.0),
                step(0.25, 1.0, 1.0),
                step(0.25, 0.75, 1.0),
            ],
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            Preference::Smoothness => 0,
            Preference::Mixed => 1,
            Preference::Clarity => 2,
        }
    }

    fn from_u8(value: u8) -> Preference {
        match value {
            1 => Preference::Mixed,
            2 => Preference::Clarity,
            _ => Preference::Smoothness,
        }
    }
}

/// What the encoder is set to at a moment of a share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub fps: u32,
    pub size: (u32, u32),
    pub bitrate: u32,
}

/// The target for a `screen`-sized picture at `quality` with `preference`, `level` steps
/// down. The bitrate follows the configured frame rate, so fewer frames get more bits each.
pub fn target(
    quality: Quality,
    preference: Preference,
    level: usize,
    screen: (u32, u32),
) -> Target {
    let steps = preference.steps();
    let step = steps[level.min(steps.len() - 1)];
    let (w, h) = fit(screen.0, screen.1, quality);
    let even = |n: f64| ((n as u32) & !1).max(2);
    let size = (
        even(f64::from(w) * step.size),
        even(f64::from(h) * step.size),
    );
    let fps = ((f64::from(quality.fps) * step.fps).round() as u32).clamp(1, quality.fps);
    let bits =
        f64::from(hardware_bitrate(size.0, size.1, quality.fps)) * preference.bits() * step.bits;
    Target {
        fps,
        size,
        bitrate: bits.clamp(1_000_000.0, 50_000_000.0) as u32,
    }
}

/// A share falling behind steps down one [`Step`] at a time (at most every
/// [`Adapt::DOWN_GAP`]); keeping up for [`Adapt::UP_AFTER`] steps it back up.
#[derive(Debug)]
pub struct Adapt {
    level: usize,
    changed: Instant,
    calm_since: Instant,
}

impl Adapt {
    const DOWN_GAP: Duration = Duration::from_secs(2);
    const UP_AFTER: Duration = Duration::from_secs(8);
    const LEVELS: usize = 4;

    pub fn new(now: Instant) -> Adapt {
        Adapt {
            level: 0,
            changed: now,
            calm_since: now,
        }
    }

    pub fn level(&self) -> usize {
        self.level
    }

    /// The share fell behind; `true` if this steps it down.
    pub fn behind(&mut self, now: Instant) -> bool {
        self.calm_since = now;
        if self.level + 1 < Self::LEVELS && now.duration_since(self.changed) >= Self::DOWN_GAP {
            self.level += 1;
            self.changed = now;
            return true;
        }
        false
    }

    /// Nothing went wrong this frame; `true` if this steps it back up.
    pub fn keeping_up(&mut self, now: Instant) -> bool {
        if self.level > 0
            && now.duration_since(self.calm_since) >= Self::UP_AFTER
            && now.duration_since(self.changed) >= Self::UP_AFTER
        {
            self.level -= 1;
            self.changed = now;
            self.calm_since = now;
            return true;
        }
        false
    }
}

/// Settings a running share follows as they change.
#[derive(Debug, Default)]
struct Tuning {
    preference: AtomicU8,
    /// Keyframes the server asked for (watchers lost frames, or started watching).
    requests: AtomicU32,
}

/// A keyframe at least this often, so a watcher who lost one recovers on their own.
#[cfg(windows)]
const KEYFRAME_EVERY: Duration = Duration::from_secs(5);

/// How often the share's numbers are logged.
#[cfg(windows)]
const STATS_EVERY: Duration = Duration::from_secs(10);

/// After the connection refused a frame, the watchers wait for a keyframe; one is made at most
/// this often, so a congested connection does not get a keyframe (the largest frames) after
/// every refused frame.
#[cfg(windows)]
const RESYNC_GAP: Duration = Duration::from_millis(500);

/// While nobody watches, how often the capture thread looks whether someone started to.
#[cfg(windows)]
const IDLE_POLL: Duration = Duration::from_millis(100);

/// A running capture; dropping it stops the threads.
pub struct Capture {
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    tuning: Arc<Tuning>,
}

impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Capture")
    }
}

impl Capture {
    /// Starts capturing `screen` (its system name; empty or unknown = the primary screen) and
    /// sending it through `core`, which must already share ([`Core::start_screen_share`]).
    pub fn start(
        core: Core,
        screen: &str,
        quality: Quality,
        preference: Preference,
    ) -> Result<Capture, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(true));
        let tuning = Arc::new(Tuning::default());
        tuning
            .preference
            .store(preference.to_u8(), Ordering::Relaxed);
        start_thread(
            core,
            screen,
            quality,
            stop.clone(),
            keyframe.clone(),
            tuning.clone(),
        )?;
        Ok(Capture {
            stop,
            keyframe,
            tuning,
        })
    }

    /// The server asked for a keyframe: the next frame is one.
    pub fn request_keyframe(&self) {
        self.tuning.requests.fetch_add(1, Ordering::Relaxed);
        self.keyframe.store(true, Ordering::Relaxed);
    }

    /// Changes the preference of the running share (from its next frame).
    pub fn set_preference(&self, preference: Preference) {
        self.tuning
            .preference
            .store(preference.to_u8(), Ordering::Relaxed);
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The largest even size within the quality's maximum with the aspect ratio of
/// `width`×`height` (never larger than it).
pub fn fit(width: u32, height: u32, quality: Quality) -> (u32, u32) {
    let scale = (f64::from(quality.max_width) / f64::from(width.max(1)))
        .min(f64::from(quality.max_height) / f64::from(height.max(1)))
        .min(1.0);
    let even = |n: f64| ((n as u32) & !1).max(2);
    (
        even(f64::from(width) * scale),
        even(f64::from(height) * scale),
    )
}

/// What `voice.share_screen` names when it is one window: `window:<id>`.
pub const WINDOW_PREFIX: &str = "window:";

/// Something the share picker offers: a screen or a window.
pub struct Shareable {
    /// Its `voice.share_screen` value: `""` for the primary screen, another screen's name, or
    /// `window:<id>`.
    pub value: String,
    /// "Screen 2", or the window's title.
    pub title: String,
    /// "Primary" or the screen's device name, or the window's app.
    pub detail: String,
    pub window: bool,
    /// A minimized window cannot be captured: it has no picture.
    pub minimized: bool,
    /// A small picture, BGRA, at most [`THUMB_WIDTH`] wide; `None` when it could not be taken.
    pub picture: Option<image::RgbaImage>,
}

/// How wide the share picker's pictures are made.
pub const THUMB_WIDTH: u32 = 240;

/// The screens (the primary one first) and the windows (largest first, minimized ones last)
/// that can be shared, with a small picture of each when `pictures`. Hermes' own windows and
/// untitled or tiny ones are left out. Slow (a few ms per picture): call it off the UI thread.
pub fn sources(pictures: bool) -> Vec<Shareable> {
    #[cfg(windows)]
    {
        let mut out = Vec::new();
        for (info, monitor) in ordered_monitors() {
            out.push(Shareable {
                title: info.title(),
                detail: info.detail(),
                value: info.value,
                window: false,
                minimized: false,
                picture: pictures
                    .then(|| monitor.capture_image().ok().map(thumbnail))
                    .flatten(),
            });
        }
        let Ok(all) = xcap::Window::all() else {
            return out;
        };
        let me = std::process::id();
        let mut windows: Vec<(bool, u64, Shareable, xcap::Window)> = all
            .into_iter()
            .filter(|w| w.pid().is_ok_and(|pid| pid != me))
            .filter_map(|w| {
                let title = w.title().ok().filter(|t| !t.trim().is_empty())?;
                let minimized = w.is_minimized().unwrap_or(false);
                let area = u64::from(w.width().ok()?) * u64::from(w.height().ok()?);
                if !minimized && area < 64 * 64 {
                    return None;
                }
                let source = Shareable {
                    value: format!("{WINDOW_PREFIX}{}", w.id().ok()?),
                    title,
                    detail: w.app_name().unwrap_or_default(),
                    window: true,
                    minimized,
                    picture: None,
                };
                Some((minimized, area, source, w))
            })
            .collect();
        windows.sort_by_key(|w| (w.0, std::cmp::Reverse(w.1)));
        for (minimized, _, mut source, window) in windows {
            if pictures && !minimized {
                source.picture = window.capture_image().ok().map(thumbnail);
            }
            out.push(source);
        }
        out
    }
    #[cfg(not(windows))]
    {
        let _ = pictures;
        Vec::new()
    }
}

/// `picture` made at most [`THUMB_WIDTH`] wide (and as tall as a 4:3 picture would be), in BGRA.
/// Each pixel averages a 4x4 grid of samples from its patch of the original: far cheaper than
/// filtering every pixel of a 4K screen, and smooth enough for a picture this small.
#[cfg(windows)]
fn thumbnail(picture: image::RgbaImage) -> image::RgbaImage {
    const SAMPLES: u32 = 4;
    let (w, h) = picture.dimensions();
    let scale = (THUMB_WIDTH as f32 / w.max(1) as f32)
        .min(THUMB_WIDTH as f32 * 0.75 / h.max(1) as f32)
        .min(1.);
    let (tw, th) = (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    );
    let src = picture.as_raw();
    let mut out = image::RgbaImage::new(tw, th);
    for (x, y, pixel) in out.enumerate_pixels_mut() {
        let mut sum = [0u32; 4];
        for sy in 0..SAMPLES {
            let py = ((u64::from(y) * u64::from(SAMPLES) + u64::from(sy)) * u64::from(h)
                / (u64::from(th) * u64::from(SAMPLES))) as usize;
            for sx in 0..SAMPLES {
                let px = ((u64::from(x) * u64::from(SAMPLES) + u64::from(sx)) * u64::from(w)
                    / (u64::from(tw) * u64::from(SAMPLES))) as usize;
                let i = (py * w as usize + px) * 4;
                for (c, total) in sum.iter_mut().enumerate() {
                    *total += u32::from(src[i + c]);
                }
            }
        }
        let n = SAMPLES * SAMPLES;
        // BGRA for gpui.
        *pixel = image::Rgba([
            (sum[2] / n) as u8,
            (sum[1] / n) as u8,
            (sum[0] / n) as u8,
            (sum[3] / n) as u8,
        ]);
    }
    out
}

/// A screen as people see it in settings and in the share picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenInfo {
    /// The `voice.share_screen` value: empty for the primary screen, else the system name.
    pub value: String,
    /// Its place in the list, from 1 (the primary screen).
    pub number: usize,
    pub primary: bool,
    pub width: u32,
    pub height: u32,
}

impl ScreenInfo {
    /// "Screen 1", "Screen 2"…
    pub fn title(&self) -> String {
        format!("Screen {}", self.number)
    }

    /// "Screen 1 (primary)", for chips.
    pub fn label(&self) -> String {
        if self.primary {
            format!("{} (primary)", self.title())
        } else {
            self.title()
        }
    }

    /// "2560×1440 · Primary", under the title in the picker.
    pub fn detail(&self) -> String {
        let size = format!("{}\u{d7}{}", self.width, self.height);
        if self.primary {
            format!("{size} \u{b7} Primary")
        } else {
            size
        }
    }
}

/// The screens in a stable order: the primary one first, then left to right, top to bottom.
#[cfg(windows)]
fn ordered_monitors() -> Vec<(ScreenInfo, xcap::Monitor)> {
    let Ok(monitors) = xcap::Monitor::all() else {
        return Vec::new();
    };
    let mut keyed: Vec<_> = monitors
        .into_iter()
        .filter_map(|m| {
            let name = m.name().ok()?;
            let primary = m.is_primary().unwrap_or(false);
            let key = (
                !primary,
                m.x().unwrap_or(0),
                m.y().unwrap_or(0),
                name.clone(),
            );
            Some((key, name, m))
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    keyed
        .into_iter()
        .enumerate()
        .map(|(n, ((secondary, ..), name, m))| {
            let info = ScreenInfo {
                value: if secondary { name } else { String::new() },
                number: n + 1,
                primary: !secondary,
                width: m.width().unwrap_or(0),
                height: m.height().unwrap_or(0),
            };
            (info, m)
        })
        .collect()
}

/// The screens, the primary one first (empty where capture is unavailable).
pub fn screens() -> Vec<ScreenInfo> {
    #[cfg(windows)]
    {
        ordered_monitors()
            .into_iter()
            .map(|(info, _)| info)
            .collect()
    }
    #[cfg(not(windows))]
    Vec::new()
}

#[cfg(not(windows))]
fn start_thread(
    _core: Core,
    _screen: &str,
    _quality: Quality,
    _stop: Arc<AtomicBool>,
    _keyframe: Arc<AtomicBool>,
    _tuning: Arc<Tuning>,
) -> Result<(), String> {
    Err("screen sharing is only available on Windows for now".into())
}

#[cfg(windows)]
fn start_thread(
    core: Core,
    screen: &str,
    quality: Quality,
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    tuning: Arc<Tuning>,
) -> Result<(), String> {
    if let Some(id) = screen.strip_prefix(WINDOW_PREFIX) {
        let id: u32 = id.parse().map_err(|_| "no such window".to_owned())?;
        return start_window_thread(core, id, quality, stop, keyframe);
    }
    // A monitor cannot move between threads: the capture thread finds it and reports back.
    let screen = screen.to_owned();
    let (found, result) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("hermes-screen-capture".into())
        .spawn(move || match pick_monitor(&screen) {
            Ok(monitor) => {
                let _ = found.send(Ok(()));
                run(&core, &monitor, quality, &stop, &keyframe, &tuning);
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

/// Shares one window: captured on its own (covered parts too, where the app allows it), on the
/// CPU path. A resized window restarts the encoder at its new size.
#[cfg(windows)]
fn start_window_thread(
    core: Core,
    id: u32,
    quality: Quality,
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
) -> Result<(), String> {
    let (found, result) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("hermes-window-capture".into())
        .spawn(move || {
            let window = xcap::Window::all()
                .ok()
                .and_then(|all| all.into_iter().find(|w| w.id().ok() == Some(id)));
            match window {
                Some(window) => {
                    let _ = found.send(Ok(()));
                    run_window(&core, &window, quality, &stop, &keyframe);
                }
                None => {
                    let _ = found.send(Err("that window is gone".to_owned()));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    result
        .recv()
        .map_err(|_| "the capture thread stopped".to_owned())?
}

#[cfg(windows)]
fn run_window(
    core: &Core,
    window: &xcap::Window,
    quality: Quality,
    stop: &AtomicBool,
    keyframe: &Arc<AtomicBool>,
) {
    let stats = Arc::new(Stats::default());
    let (frames, encoder_frames) = std::sync::mpsc::sync_channel::<Arc<Frame>>(1);
    let spawned = {
        let core = core.clone();
        let keyframe = keyframe.clone();
        let stats = stats.clone();
        std::thread::Builder::new()
            .name("hermes-screen-encode".into())
            .spawn(move || encode_loop(&core, quality, &encoder_frames, &keyframe, &stats))
    };
    if let Err(err) = spawned {
        tracing::warn!("window share encoder thread: {err}");
        core.stop_screen_share();
        return;
    }
    let interval = Duration::from_millis(1000 / u64::from(quality.fps));
    let mut stats_since = Instant::now();
    while !stop.load(Ordering::Relaxed) && core.sharing_screen() {
        if core.share_watchers() == Some(0) {
            std::thread::sleep(IDLE_POLL);
            keyframe.store(true, Ordering::Relaxed);
            continue;
        }
        let begin = Instant::now();
        // Minimized, it has nothing to show: the last picture stays on the watchers' screens.
        if window.is_minimized().unwrap_or(false) {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        match window.capture_image() {
            Ok(image) => {
                let frame = scale(
                    Frame {
                        width: image.width(),
                        height: image.height(),
                        bgra: false,
                        pixels: image.into_raw(),
                    },
                    quality,
                );
                let size = (frame.width, frame.height);
                if stats_since.elapsed() >= STATS_EVERY {
                    let over = stats_since.elapsed();
                    log_stats(&stats, over, size, quality.fps, "window, OpenH264");
                    stats_since = Instant::now();
                }
                stats.captured.fetch_add(1, Ordering::Relaxed);
                stats
                    .capture_us
                    .fetch_add(micros(begin.elapsed()), Ordering::Relaxed);
                match frames.try_send(Arc::new(frame)) {
                    Ok(()) => {}
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {
                        stats.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => return,
                }
            }
            // Closed: the share ends.
            Err(err) => {
                tracing::info!("window share: the window is gone ({err})");
                core.stop_screen_share();
                return;
            }
        }
        std::thread::sleep(interval.saturating_sub(begin.elapsed()));
    }
}

#[cfg(not(windows))]
fn start_window_thread(
    _core: Core,
    _id: u32,
    _quality: Quality,
    _stop: Arc<AtomicBool>,
    _keyframe: Arc<AtomicBool>,
) -> Result<(), String> {
    Err("window sharing is only available on Windows for now".into())
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

/// What the last [`STATS_EVERY`] of a share looked like (logged, to see where time goes).
#[cfg(windows)]
#[derive(Default)]
struct Stats {
    captured: AtomicU32,
    /// Captured, but the encoder was still busy with the one before.
    dropped: AtomicU32,
    capture_us: AtomicU64,
    sent: AtomicU32,
    encode_us: AtomicU64,
    bytes: AtomicU64,
    keyframes: AtomicU32,
    /// Encoded, but the connection's queue was full (each one forces a keyframe).
    refused: AtomicU32,
}

/// Where pictures come from: Desktop Duplication, or GDI if that is unavailable.
#[cfg(windows)]
#[allow(clippy::large_enum_variant)]
enum Source {
    Dxgi(super::dxgi::Duplicator),
    Gdi,
}

/// The capture thread: the GPU path (capture, conversion and the GPU's own H.264 encoder) if
/// this machine has one, else (or if it fails) the CPU path.
#[cfg(windows)]
fn run(
    core: &Core,
    monitor: &xcap::Monitor,
    quality: Quality,
    stop: &AtomicBool,
    keyframe: &Arc<AtomicBool>,
    tuning: &Tuning,
) {
    let name = monitor.name().unwrap_or_default();
    let source = match super::dxgi::Duplicator::open(&name) {
        Ok(duplicator) if duplicator.video() => {
            match run_hardware(core, duplicator, &name, quality, stop, keyframe, tuning) {
                Ok(()) => return,
                Err(err) => {
                    tracing::warn!(
                        "screen share: hardware encoding unavailable ({err}); using the \
                         software encoder"
                    );
                    match super::dxgi::Duplicator::open(&name) {
                        Ok(duplicator) => Source::Dxgi(duplicator),
                        Err(_) => Source::Gdi,
                    }
                }
            }
        }
        Ok(duplicator) => Source::Dxgi(duplicator),
        Err(err) => {
            tracing::warn!("screen share: desktop duplication unavailable ({err}); using GDI");
            Source::Gdi
        }
    };
    run_software(core, monitor, quality, stop, keyframe, source);
}

/// The GPU path: Desktop Duplication hands over a picture at most at the target frame rate
/// (only when the screen changed, or the last one again when a keyframe is due), the video
/// processor scales and converts it to NV12, and the GPU's encoder encodes it; the picture
/// never leaves the GPU. One thread suffices: the CPU only coordinates.
///
/// The frame rate, size and bitrate follow the preference ([`target`]); falling behind (frames
/// the connection refuses, watchers losing frames, encoding slower than the frame rate) steps
/// them down the preference's way ([`Adapt`]), keeping up steps them back up.
///
/// `Err` when the encoder cannot be set up or fails; the caller then uses the CPU path.
#[cfg(windows)]
fn run_hardware(
    core: &Core,
    mut duplicator: super::dxgi::Duplicator,
    name: &str,
    quality: Quality,
    stop: &AtomicBool,
    keyframe: &AtomicBool,
    tuning: &Tuning,
) -> Result<(), String> {
    use super::gpu_convert::{GpuConvert, Nv12};
    use super::mf_encode::MfEncoder;
    use hermes_client_core::voice::ScreenFrame;

    /// Keyframe requests within this window beyond the first mean watchers are losing frames.
    const REQUESTS_WINDOW: Duration = Duration::from_secs(3);
    /// Frames in a row encoded slower than the frame rate before the share steps down.
    const SLOW_FRAMES: u32 = 15;

    let stats = Stats::default();
    let started = Instant::now();
    let mut preference = Preference::from_u8(tuning.preference.load(Ordering::Relaxed));
    let mut adapt = Adapt::new(started);
    let mut current: Option<Target> = None;
    let mut convert: Option<GpuConvert> = None;
    let mut encoder: Option<MfEncoder> = None;
    let mut last: Option<Nv12> = None;
    let mut last_sent = Instant::now();
    let mut last_keyframe: Option<Instant> = None;
    // A frame was refused: nothing the watchers could decode goes out until a keyframe.
    let mut resync = false;
    let mut idle = false;
    let mut next = Instant::now();
    let mut stats_since = Instant::now();
    let mut seen_requests = tuning.requests.load(Ordering::Relaxed);
    let mut requests: std::collections::VecDeque<Instant> = std::collections::VecDeque::new();
    let mut slow_frames = 0u32;
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
        let chosen = Preference::from_u8(tuning.preference.load(Ordering::Relaxed));
        if chosen != preference {
            tracing::info!("screen share: preferring {}", chosen.label());
            preference = chosen;
            adapt = Adapt::new(Instant::now());
        }

        // At most the target's frames a second: wait for this one's turn, then take the newest
        // picture (Desktop Duplication keeps only the latest).
        let interval = Duration::from_secs(1) / current.map_or(quality.fps, |t| t.fps).max(1);
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
        next += interval;
        if next < Instant::now() {
            next = Instant::now();
        }

        let begin = Instant::now();
        let wait_ms = u32::try_from(interval.as_millis()).unwrap_or(100).max(1);
        let shot = match duplicator.next_gpu(wait_ms) {
            Ok(shot) => shot,
            Err(super::dxgi::Error::Lost) => {
                // A mode change or the secure desktop: start over (the size may have changed).
                std::thread::sleep(Duration::from_millis(200));
                duplicator = super::dxgi::Duplicator::open(name)?;
                convert = None;
                last = None;
                keyframe.store(true, Ordering::Relaxed);
                continue;
            }
            Err(super::dxgi::Error::Other(err)) => return Err(err),
        };
        let picture = match shot {
            Some(shot) => {
                let input = (shot.width, shot.height);
                let want = target(quality, preference, adapt.level(), input);
                if convert
                    .as_ref()
                    .is_none_or(|c| (c.input_size(), c.output_size()) != (input, want.size))
                {
                    convert = Some(GpuConvert::new(
                        duplicator.device(),
                        input,
                        want.size,
                        want.fps,
                    )?);
                }
                let same = |t: Target| (t.fps, t.size) == (want.fps, want.size);
                if encoder.is_none() || !current.is_some_and(same) {
                    let created = MfEncoder::new(
                        duplicator.device(),
                        duplicator.luid(),
                        want.size,
                        want.fps,
                        want.bitrate,
                    )?;
                    tracing::info!(
                        "screen share: {}x{} at {} fps, {} kbit/s, with {}",
                        want.size.0,
                        want.size.1,
                        want.fps,
                        want.bitrate / 1000,
                        created.name
                    );
                    tracing::info!("screen share encoder: {}", created.settings());
                    encoder = Some(created);
                    keyframe.store(true, Ordering::Relaxed);
                } else if current.is_some_and(|t| t.bitrate != want.bitrate)
                    && let Some(encoder) = &encoder
                {
                    encoder.set_bitrate(want.bitrate);
                }
                current = Some(want);
                let Some(convert) = convert.as_mut() else {
                    continue;
                };
                let nv12 = convert.convert(&shot.texture)?;
                stats.captured.fetch_add(1, Ordering::Relaxed);
                stats
                    .capture_us
                    .fetch_add(micros(begin.elapsed()), Ordering::Relaxed);
                last = Some(nv12.clone());
                Some(nv12)
            }
            // An unchanged screen sends nothing, except the last picture again when a keyframe
            // is due (someone started watching, the periodic one, or after a refused frame).
            None if keyframe.load(Ordering::Relaxed)
                || last_sent.elapsed() >= KEYFRAME_EVERY
                || (resync && last_keyframe.is_none_or(|t| t.elapsed() >= RESYNC_GAP)) =>
            {
                last.clone()
            }
            None => None,
        };
        let mut behind = false;
        if let Some(picture) = picture
            && let Some(encoder) = encoder.as_mut()
        {
            let requested = keyframe.swap(false, Ordering::Relaxed);
            let periodic = last_keyframe.is_none_or(|t| t.elapsed() >= KEYFRAME_EVERY);
            let resync_due = resync && last_keyframe.is_none_or(|t| t.elapsed() >= RESYNC_GAP);
            let key = requested || periodic || resync_due;
            if resync && !key {
                stats.dropped.fetch_add(1, Ordering::Relaxed);
            } else {
                let encode_begin = Instant::now();
                let encoded = encoder.encode(&picture, key)?;
                let took = encode_begin.elapsed();
                stats.encode_us.fetch_add(micros(took), Ordering::Relaxed);
                // The GPU cannot keep up with this frame rate and size.
                slow_frames = if took > interval { slow_frames + 1 } else { 0 };
                if slow_frames >= SLOW_FRAMES {
                    slow_frames = 0;
                    behind = true;
                }
                last_sent = Instant::now();
                for (data, is_key) in encoded {
                    stats.sent.fetch_add(1, Ordering::Relaxed);
                    stats.bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
                    if is_key {
                        stats.keyframes.fetch_add(1, Ordering::Relaxed);
                        last_keyframe = Some(Instant::now());
                        resync = false;
                    }
                    let sent = core.send_screen_frame(ScreenFrame {
                        keyframe: is_key,
                        width: picture.width as u16,
                        height: picture.height as u16,
                        timestamp: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
                        data,
                    });
                    if !sent {
                        // My connection is behind.
                        stats.refused.fetch_add(1, Ordering::Relaxed);
                        resync = true;
                        behind = true;
                    }
                }
            }
        }
        // Watchers losing frames: the server asks for keyframes again and again.
        let now = Instant::now();
        let total = tuning.requests.load(Ordering::Relaxed);
        for _ in seen_requests..total {
            requests.push_back(now);
        }
        seen_requests = total;
        while requests
            .front()
            .is_some_and(|t| now.duration_since(*t) > REQUESTS_WINDOW)
        {
            requests.pop_front();
        }
        if requests.len() > 2 {
            requests.clear();
            behind = true;
        }
        let changed = if behind {
            adapt.behind(now)
        } else {
            adapt.keeping_up(now)
        };
        if changed {
            tracing::info!(
                "screen share: {} (level {} of {}, preferring {})",
                if behind {
                    "falling behind, stepping down"
                } else {
                    "keeping up, stepping back up"
                },
                adapt.level(),
                Adapt::LEVELS - 1,
                preference.label()
            );
        }
        if stats_since.elapsed() >= STATS_EVERY {
            let size = last.as_ref().map_or((0, 0), |p| (p.width, p.height));
            let what = encoder
                .as_ref()
                .map_or("hardware".to_owned(), |e| format!("GPU, {}", e.name));
            let fps = current.map_or(quality.fps, |t| t.fps);
            log_stats(&stats, stats_since.elapsed(), size, fps, &what);
            stats_since = Instant::now();
        }
    }
    Ok(())
}

/// The CPU path: takes a picture `quality.fps` times a second (only when the screen changed,
/// or the last one again when a keyframe is due), scales it and hands it to the encoder
/// thread, which works in parallel. A picture the encoder has no time for is dropped.
#[cfg(windows)]
fn run_software(
    core: &Core,
    monitor: &xcap::Monitor,
    quality: Quality,
    stop: &AtomicBool,
    keyframe: &Arc<AtomicBool>,
    mut source: Source,
) {
    let name = monitor.name().unwrap_or_default();
    let stats = Arc::new(Stats::default());
    let (frames, encoder_frames) = std::sync::mpsc::sync_channel::<Arc<Frame>>(1);
    let spawned = {
        let core = core.clone();
        let keyframe = keyframe.clone();
        let stats = stats.clone();
        std::thread::Builder::new()
            .name("hermes-screen-encode".into())
            .spawn(move || encode_loop(&core, quality, &encoder_frames, &keyframe, &stats))
    };
    if let Err(err) = spawned {
        tracing::warn!("screen share encoder thread: {err}");
        core.stop_screen_share();
        return;
    }

    let interval = Duration::from_millis(1000 / u64::from(quality.fps));
    let mut last: Option<Arc<Frame>> = None;
    let mut last_sent = Instant::now();
    let mut stats_since = Instant::now();
    let mut idle = false;
    while !stop.load(Ordering::Relaxed) && core.sharing_screen() {
        // Nobody watches: frames would go nowhere, so nothing is captured or encoded. The
        // duplication stays open, so the first watcher gets a picture (a keyframe) at once.
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
            // `last` stays: if the screen did not change meanwhile, Desktop Duplication has no
            // new picture, and the keyframe repeats the last one.
        }
        let begin = Instant::now();
        let shot = match &mut source {
            Source::Dxgi(duplicator) => {
                // Waits until the next frame is due for the screen to change.
                let wait = u32::try_from(interval.as_millis()).unwrap_or(100);
                match duplicator.next(wait) {
                    Ok(shot) => shot,
                    Err(super::dxgi::Error::Lost) => {
                        // A mode change or the secure desktop: start over.
                        std::thread::sleep(Duration::from_millis(200));
                        match super::dxgi::Duplicator::open(&name) {
                            Ok(fresh) => *duplicator = fresh,
                            Err(err) => {
                                tracing::warn!("screen share: duplication lost ({err}); using GDI");
                                source = Source::Gdi;
                            }
                        }
                        continue;
                    }
                    Err(super::dxgi::Error::Other(err)) => {
                        tracing::warn!("screen share: duplication failed ({err}); using GDI");
                        source = Source::Gdi;
                        continue;
                    }
                }
            }
            Source::Gdi => match monitor.capture_image() {
                Ok(image) => Some(Frame {
                    width: image.width(),
                    height: image.height(),
                    bgra: false,
                    pixels: image.into_raw(),
                }),
                Err(err) => {
                    tracing::warn!("screen capture: {err}");
                    std::thread::sleep(Duration::from_millis(500));
                    None
                }
            },
        };
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
            // An unchanged screen sends nothing, except the last picture again when a keyframe
            // is wanted (someone started watching, or it is time for the periodic one).
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
            let what = if matches!(source, Source::Dxgi(_)) {
                "desktop duplication, OpenH264"
            } else {
                "GDI, OpenH264"
            };
            let size = last.as_deref().map_or((0, 0), |f| (f.width, f.height));
            log_stats(&stats, stats_since.elapsed(), size, quality.fps, what);
            stats_since = Instant::now();
        }
        std::thread::sleep(interval.saturating_sub(begin.elapsed()));
    }
}

/// Bitrate of a hardware-encoded share (bits per second): about 0.07 bit per pixel per frame,
/// 2 to 40 Mbit/s (1440p at 60 fps: 15 Mbit/s; 4K at 60 fps: 35 Mbit/s).
pub fn hardware_bitrate(width: u32, height: u32, fps: u32) -> u32 {
    let bits = f64::from(width) * f64::from(height) * f64::from(fps) * 0.07;
    bits.clamp(2_000_000.0, 40_000_000.0) as u32
}

#[cfg(windows)]
fn micros(d: Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

/// Scales a picture down to the quality's size (BGRA or RGBA alike).
#[cfg(windows)]
fn scale(frame: Frame, quality: Quality) -> Frame {
    let (w, h) = fit(frame.width, frame.height, quality);
    if (frame.width, frame.height) == (w, h) {
        return frame;
    }
    Frame {
        width: w,
        height: h,
        bgra: frame.bgra,
        pixels: super::scale::down(&frame.pixels, frame.width, frame.height, w, h),
    }
}

/// The encoder thread: encodes and sends what the capture thread hands over, until it stops.
#[cfg(windows)]
fn encode_loop(
    core: &Core,
    quality: Quality,
    frames: &std::sync::mpsc::Receiver<Arc<Frame>>,
    keyframe: &AtomicBool,
    stats: &Stats,
) {
    use hermes_client_core::voice::ScreenFrame;

    let started = Instant::now();
    let mut encoder: Option<super::codec::Encoder> = None;
    let mut last_keyframe: Option<Instant> = None;
    // A frame was refused: nothing the watchers could decode goes out until a keyframe.
    let mut resync = false;
    while let Ok(frame) = frames.recv() {
        let begin = Instant::now();
        let requested = keyframe.swap(false, Ordering::Relaxed);
        let periodic = last_keyframe.is_none_or(|t| t.elapsed() >= KEYFRAME_EVERY);
        let resync_due = resync && last_keyframe.is_none_or(|t| t.elapsed() >= RESYNC_GAP);
        let key = requested || periodic || resync_due;
        if resync && !key {
            stats.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        // A shared window changed size: a new encoder at the new size, from a keyframe.
        let resized = encoder
            .as_ref()
            .is_some_and(|e| e.size() != (frame.width, frame.height));
        if resized {
            encoder = None;
        }
        let key = key || resized;
        if encoder.is_none() {
            match super::codec::Encoder::new(frame.width, frame.height, quality.fps as f32) {
                Ok(e) => encoder = Some(e),
                Err(err) => {
                    tracing::warn!("screen share encoder: {err}");
                    core.stop_screen_share();
                    return;
                }
            }
        }
        match encoder.as_mut().map(|e| e.encode(&frame, key)) {
            Some(Ok(Some((data, is_key)))) => {
                if is_key {
                    resync = false;
                    last_keyframe = Some(Instant::now());
                    stats.keyframes.fetch_add(1, Ordering::Relaxed);
                }
                stats.sent.fetch_add(1, Ordering::Relaxed);
                stats.bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
                stats
                    .encode_us
                    .fetch_add(micros(begin.elapsed()), Ordering::Relaxed);
                let sent = core.send_screen_frame(ScreenFrame {
                    keyframe: is_key,
                    width: frame.width as u16,
                    height: frame.height as u16,
                    timestamp: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
                    data,
                });
                if !sent {
                    // Dropped (the connection is behind): resynchronize watchers.
                    stats.refused.fetch_add(1, Ordering::Relaxed);
                    resync = true;
                }
            }
            Some(Ok(None)) | None => {}
            Some(Err(err)) => tracing::warn!("screen share encoding: {err}"),
        }
    }
}

#[cfg(windows)]
fn log_stats(stats: &Stats, over: Duration, (w, h): (u32, u32), fps: u32, what: &str) {
    let captured = stats.captured.swap(0, Ordering::Relaxed);
    let dropped = stats.dropped.swap(0, Ordering::Relaxed);
    let capture_us = stats.capture_us.swap(0, Ordering::Relaxed);
    let sent = stats.sent.swap(0, Ordering::Relaxed);
    let encode_us = stats.encode_us.swap(0, Ordering::Relaxed);
    let bytes = stats.bytes.swap(0, Ordering::Relaxed);
    let keyframes = stats.keyframes.swap(0, Ordering::Relaxed);
    let refused = stats.refused.swap(0, Ordering::Relaxed);
    let secs = over.as_secs_f64();
    let per = |us: u64, n: u32| us as f64 / 1000.0 / f64::from(n.max(1));
    tracing::info!(
        "screen share ({}, {w}x{h}, target {} fps): captured {:.1} fps ({:.1} ms each), sent \
         {:.1} fps ({:.1} ms to encode, {keyframes} keyframes), {dropped} dropped by a busy          encoder, {refused} refused by a full connection queue, {:.0} kbit/s",
        what,
        fps,
        f64::from(captured) / secs,
        per(capture_us, captured),
        f64::from(sent) / secs,
        per(encode_us, sent),
        bytes as f64 * 8.0 / 1000.0 / secs,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a desktop: `cargo test -p hermes-gui windows_are_listed -- --ignored`.
    #[test]
    #[ignore = "needs a desktop"]
    #[cfg(windows)]
    fn windows_are_listed_and_captured() {
        let list = sources(true);
        let window = list
            .iter()
            .find(|s| s.window && !s.minimized)
            .expect("no windows");
        let picture = window.picture.as_ref().expect("no picture");
        eprintln!("{}: {}x{}", window.title, picture.width(), picture.height());
        assert!(picture.width() <= THUMB_WIDTH);
        assert!(
            list.iter().any(|s| !s.window && s.value.is_empty()),
            "no primary screen"
        );
    }

    #[test]
    fn shares_fit_their_resolution_and_stay_even() {
        let hd = Quality::from_config("720p", 10);
        assert_eq!(fit(2560, 1440, hd), (1280, 720));
        assert_eq!(fit(1920, 1080, hd), (1280, 720));
        assert_eq!(fit(1280, 720, hd), (1280, 720));
        assert_eq!(fit(1024, 768, hd), (960, 720));
        assert_eq!(fit(3440, 1440, hd), (1280, 534));
        assert_eq!(fit(801, 601, hd), (800, 600));
        assert_eq!(fit(1, 1, hd), (2, 2));
        let full = Quality::from_config("1080p", 30);
        assert_eq!(fit(2560, 1440, full), (1920, 1080));
        assert_eq!(fit(1280, 720, full), (1280, 720), "never scaled up");
        let native = Quality::from_config("native", 60);
        assert_eq!(
            fit(5120, 1440, native),
            (3840, 1080),
            "within the protocol's limit"
        );
        assert_eq!(native.fps, 60);
        assert_eq!(
            Quality::from_config("native", 120).fps,
            60,
            "the protocol's limit"
        );
        assert_eq!(Quality::from_config("bogus", 10), hd);
    }

    #[test]
    fn preferences_give_way_differently() {
        let q = Quality::from_config("1440p", 60);
        let screen = (2560, 1440);
        let full = |p| target(q, p, 0, screen);
        // At full quality all keep size and frame rate; clearer pictures cost more bits.
        for p in PREFERENCES {
            assert_eq!((full(p).size, full(p).fps), ((2560, 1440), 60));
        }
        assert!(full(Preference::Smoothness).bitrate < full(Preference::Mixed).bitrate);
        assert!(full(Preference::Mixed).bitrate < full(Preference::Clarity).bitrate);
        assert_eq!(
            full(Preference::Smoothness).bitrate,
            hardware_bitrate(2560, 1440, 60)
        );

        // Falling behind all the way: smoothness keeps 60 fps, clarity keeps the size.
        let worst = |p| target(q, p, 3, screen);
        assert_eq!(worst(Preference::Smoothness).fps, 60);
        assert_eq!(worst(Preference::Smoothness).size, (1280, 720));
        assert_eq!(worst(Preference::Mixed).fps, 30);
        assert_eq!(worst(Preference::Clarity).fps, 15);
        assert_eq!(target(q, Preference::Clarity, 2, screen).size, (2560, 1440));
        // Clarity's fewer frames keep the bits per second: more bits each.
        assert_eq!(
            target(q, Preference::Clarity, 1, screen).bitrate,
            full(Preference::Clarity).bitrate
        );
        // Levels past the last count as the last.
        assert_eq!(
            target(q, Preference::Mixed, 9, screen),
            worst(Preference::Mixed)
        );

        for p in PREFERENCES {
            assert_eq!(Preference::from_config(p.config_value()), p);
            assert_eq!(Preference::from_u8(p.to_u8()), p);
        }
        assert_eq!(Preference::from_config("bogus"), Preference::Smoothness);
        assert_eq!(Quality::from_config("2160p", 60).max_height, 2160);
    }

    #[test]
    fn falling_behind_steps_down_and_keeping_up_steps_back() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut adapt = Adapt::new(start);
        // Not more often than every two seconds.
        assert!(!adapt.behind(at(100)));
        assert!(adapt.behind(at(2_000)));
        assert!(!adapt.behind(at(3_000)));
        assert!(adapt.behind(at(4_000)));
        assert!(adapt.behind(at(6_000)));
        assert_eq!(adapt.level(), 3);
        assert!(!adapt.behind(at(9_000)), "no lower than the last step");
        // Back up only after eight calm seconds, one step at a time.
        assert!(!adapt.keeping_up(at(12_000)));
        assert!(adapt.keeping_up(at(17_000)));
        assert_eq!(adapt.level(), 2);
        assert!(!adapt.keeping_up(at(20_000)));
        assert!(adapt.keeping_up(at(25_000)));
        // Trouble restarts the calm (and steps down no sooner than two seconds after a change).
        assert!(!adapt.behind(at(26_000)));
        assert!(adapt.behind(at(27_000)));
        assert!(!adapt.keeping_up(at(34_000)));
        assert!(adapt.keeping_up(at(35_000)));
    }

    /// Captures this machine's primary screen both ways, then scales, encodes and decodes it,
    /// printing how long each step takes. Needs a desktop:
    /// `cargo test -p hermes-gui real_screen -- --ignored --nocapture`.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs a real screen"]
    fn real_screen_is_captured_and_encoded() {
        let monitor = pick_monitor("").unwrap();
        let name = monitor.name().unwrap();
        let quality = Quality::from_config("1080p", 30);
        let mut duplicator = super::super::dxgi::Duplicator::open(&name).unwrap();
        // The first call returns the current desktop; later ones only on change.
        let begin = Instant::now();
        let shot = (0..20)
            .find_map(|_| duplicator.next(100).unwrap())
            .expect("a picture within two seconds");
        println!(
            "dxgi {}x{} in {:?}",
            shot.width,
            shot.height,
            begin.elapsed()
        );
        let begin = Instant::now();
        let gdi = monitor.capture_image().unwrap();
        println!(
            "gdi {}x{} in {:?}",
            gdi.width(),
            gdi.height(),
            begin.elapsed()
        );
        assert_eq!((shot.width, shot.height), gdi.dimensions());

        let begin = Instant::now();
        let frame = scale(shot, quality);
        println!(
            "dxgi scaled to {}x{} in {:?}",
            frame.width,
            frame.height,
            begin.elapsed()
        );
        let begin = Instant::now();
        let mut yuv =
            openh264::formats::YUVBuffer::new(frame.width as usize, frame.height as usize);
        yuv.read_bgra8(openh264::formats::BgraSliceU8::new(
            &frame.pixels,
            (frame.width as usize, frame.height as usize),
        ));
        println!("dxgi to YUV in {:?}", begin.elapsed());
        let mut encoder =
            super::super::codec::Encoder::new(frame.width, frame.height, 30.0).unwrap();
        let begin = Instant::now();
        let (key, _) = encoder.encode(&frame, true).unwrap().unwrap();
        println!("dxgi keyframe {} bytes in {:?}", key.len(), begin.elapsed());
        let begin = Instant::now();
        let (delta, _) = encoder.encode(&frame, false).unwrap().unwrap();
        println!(
            "dxgi delta frame {} bytes in {:?}",
            delta.len(),
            begin.elapsed()
        );
        let mut decoder = super::super::codec::Decoder::new().unwrap();
        let begin = Instant::now();
        let picture = decoder.decode(&key).unwrap().unwrap();
        println!("dxgi decoded in {:?}", begin.elapsed());
        assert_eq!((picture.width, picture.height), (frame.width, frame.height));
    }

    /// Whether the GPU's encoder spends the bitrate it is given and how close the pictures come
    /// out: moving, detailed pictures at 1440p and 60 fps. Needs a screen (for its GPU), not
    /// its content. The picture is noisy on purpose, so the PSNR is low and only comparable with
    /// other runs of this test:
    /// `cargo test -p hermes-gui encoder_spends -- --ignored --nocapture`.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs a GPU with an H.264 encoder"]
    #[allow(unsafe_code)]
    fn encoder_spends_its_bitrate() {
        use super::super::gpu_convert::GpuConvert;
        use super::super::mf_encode::MfEncoder;
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_DEFAULT,
        };
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
        };

        const FRAMES: usize = 300;
        const SCROLL: usize = 11;
        let (w, h) = (2560usize, 1440usize);
        let monitor = pick_monitor("").unwrap();
        let duplicator = super::super::dxgi::Duplicator::open(&monitor.name().unwrap()).unwrap();
        let device = duplicator.device().clone();
        let context = unsafe { device.GetImmediateContext().unwrap() };
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w as u32,
            Height: h as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }.unwrap();
        let texture = texture.unwrap();

        // A wide, detailed picture that scrolls under the frame, like a game panning; a band of
        // it changes completely every frame.
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut source = vec![255u8; 2 * w * h * 4];
        for y in 0..h {
            for x in 0..2 * w {
                let shade = ((x as f32 * 0.021).sin() * (y as f32 * 0.017).cos() * 70.
                    + (x as f32 * 0.13 + y as f32 * 0.07).sin() * 25.
                    + 128.) as i32
                    + (random() % 33) as i32
                    - 16;
                let at = (y * 2 * w + x) * 4;
                source[at] = shade.clamp(0, 255) as u8;
                source[at + 1] = (shade + 18).clamp(0, 255) as u8;
                source[at + 2] = (255 - shade).clamp(0, 255) as u8;
            }
        }

        let bitrate = hardware_bitrate(w as u32, h as u32, 60);
        let mut convert =
            GpuConvert::new(&device, (w as u32, h as u32), (w as u32, h as u32), 60).unwrap();
        let mut encoder = MfEncoder::new(
            &device,
            duplicator.luid(),
            (w as u32, h as u32),
            60,
            bitrate,
        )
        .unwrap();
        let mut sizes = Vec::new();
        let mut times = Vec::new();
        let mut stream = Vec::new();
        let mut references = std::collections::HashMap::new();
        for n in 0..FRAMES {
            let offset = (n * SCROLL) % w;
            // The band is inside the part shown this frame.
            for y in h / 2..h / 2 + h / 8 {
                let row = (y * 2 * w + offset) * 4;
                for chunk in source[row..row + w * 4].as_chunks_mut::<8>().0 {
                    chunk.copy_from_slice(&(random() | 0xFF00_0000_FF00_0000).to_le_bytes());
                }
            }
            let start = offset * 4;
            unsafe {
                context.UpdateSubresource(
                    &texture,
                    0,
                    None,
                    source[start..].as_ptr().cast(),
                    (2 * w * 4) as u32,
                    0,
                );
            }
            // What the picture should look like, for the frames compared afterwards.
            if (200..215).contains(&n) {
                let mut luma = vec![0u8; w * h];
                for (y, line) in luma.chunks_exact_mut(w).enumerate() {
                    let row = (y * 2 * w + offset) * 4;
                    for (out, px) in line.iter_mut().zip(source[row..].as_chunks::<4>().0) {
                        let (b, g, r) = (f32::from(px[0]), f32::from(px[1]), f32::from(px[2]));
                        *out = (16.5 + 0.257 * r + 0.504 * g + 0.098 * b) as u8;
                    }
                }
                references.insert(n, luma);
            }
            let nv12 = convert.convert(&texture).unwrap();
            let begin = Instant::now();
            for (data, is_key) in encoder.encode(&nv12, n == 0).unwrap() {
                if !is_key {
                    sizes.push(data.len());
                }
                stream.push(data);
            }
            times.push(begin.elapsed());
        }
        times.sort();
        // How close the picture came out, without the band that changes completely.
        let mut decoder = super::super::codec::Decoder::software().unwrap();
        let mut decoded = 0;
        let (mut error, mut samples) = (0f64, 0f64);
        for data in &stream {
            let Some(picture) = decoder.decode(data).unwrap() else {
                continue;
            };
            if let Some(reference) = references.get(&decoded) {
                for (y, (got, want)) in picture
                    .y()
                    .chunks_exact(w)
                    .zip(reference.chunks_exact(w))
                    .enumerate()
                {
                    if (h / 2..h / 2 + h / 8).contains(&y) {
                        continue;
                    }
                    for (a, b) in got.iter().zip(want) {
                        let d = f64::from(*a) - f64::from(*b);
                        error += d * d;
                    }
                    samples += w as f64;
                }
            }
            decoded += 1;
        }
        let psnr = 10. * (255f64 * 255. / (error / samples.max(1.))).log10();
        let worst = times[times.len() * 99 / 100];
        println!("PSNR {psnr:.2} dB, encode p99 {worst:?}");
        let total: usize = sizes.iter().sum();
        let delivered = total as f64 * 8. * 60. / sizes.len().max(1) as f64 / 1000.;
        println!(
            "asked {} kbit/s, delivered {delivered:.0} kbit/s ({:.0} %), {} frames, \
                 largest {} kB, encode median {:?}\n    {}",
            bitrate / 1000,
            delivered / f64::from(bitrate / 1000) * 100.,
            sizes.len(),
            sizes.iter().max().copied().unwrap_or(0) / 1000,
            times[times.len() / 2],
            encoder.settings(),
        );
    }

    /// The hardware path on this machine's primary screen: capture on the GPU, convert to NV12
    /// there, encode with the GPU's encoder, at the screen's size, 1080p and 4K (scaled up),
    /// printing how long each step takes, then decodes the result with OpenH264 (what older
    /// watchers use). `cargo test -p hermes-gui --release real_screen_hardware -- --ignored --nocapture`.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs a real screen and a GPU with an H.264 encoder"]
    fn real_screen_hardware_is_captured_and_encoded() {
        use super::super::{gpu_convert::GpuConvert, mf_encode::MfEncoder};

        let monitor = pick_monitor("").unwrap();
        let mut duplicator =
            super::super::dxgi::Duplicator::open(&monitor.name().unwrap()).unwrap();
        assert!(duplicator.video(), "the device has video support");
        let shot = (0..20)
            .find_map(|_| duplicator.next_gpu(100).unwrap())
            .expect("a picture within two seconds");
        println!("gpu capture {}x{}", shot.width, shot.height);
        for output in [
            (shot.width & !1, shot.height & !1),
            (1920, 1080),
            (3840, 2160),
        ] {
            let mut convert =
                GpuConvert::new(duplicator.device(), (shot.width, shot.height), output, 60)
                    .unwrap();
            let mut encoder = MfEncoder::new(
                duplicator.device(),
                duplicator.luid(),
                output,
                60,
                hardware_bitrate(output.0, output.1, 60),
            )
            .unwrap();
            let mut frames = Vec::new();
            let mut times = Vec::new();
            for n in 0..60 {
                let begin = Instant::now();
                let nv12 = convert.convert(&shot.texture).unwrap();
                frames.extend(encoder.encode(&nv12, n == 0).unwrap());
                times.push(begin.elapsed());
            }
            times.sort();
            let (key, is_key) = frames.first().cloned().expect("encoded frames");
            assert!(is_key, "the first frame is a keyframe");
            let delta: usize = frames[1..].iter().map(|(d, _)| d.len()).sum::<usize>()
                / frames.len().saturating_sub(1).max(1);
            println!(
                "{}x{} with {}: {} frames out of 60, convert+encode median {:?}, max {:?}; \
                 keyframe {} bytes, deltas {} bytes on average",
                output.0,
                output.1,
                encoder.name,
                frames.len(),
                times[times.len() / 2],
                times[times.len() - 1],
                key.len(),
                delta,
            );
            let mut decoder = super::super::codec::Decoder::new().unwrap();
            let begin = Instant::now();
            let picture = decoder.decode(&key).unwrap().expect("a decoded picture");
            println!("  OpenH264 decoded its keyframe in {:?}", begin.elapsed());
            assert_eq!((picture.width, picture.height), output);
            for (data, _) in &frames[1..] {
                decoder.decode(data).unwrap();
            }
            let mut best = super::super::codec::Decoder::new().unwrap();
            let mut times = Vec::new();
            for (data, _) in &frames {
                let begin = Instant::now();
                let _ = best.decode(data).unwrap();
                times.push(begin.elapsed());
            }
            times.sort();
            println!(
                "  {}: decode median {:?}, max {:?}",
                best.kind(),
                times[times.len() / 2],
                times[times.len() - 1]
            );
            let mut sw = super::super::codec::Decoder::software().unwrap();
            let mut times = Vec::new();
            for (data, _) in &frames {
                let begin = Instant::now();
                let _ = sw.decode(data).unwrap();
                times.push(begin.elapsed());
            }
            times.sort();
            println!(
                "  OpenH264: decode median {:?}, max {:?}",
                times[times.len() / 2],
                times[times.len() - 1]
            );
        }
    }
}
