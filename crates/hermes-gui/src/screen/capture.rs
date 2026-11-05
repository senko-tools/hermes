//! Capturing and encoding my screen while I share it, on two threads that work in parallel
//! (capturing and scaling; encoding and sending). Windows only for now: elsewhere
//! [`Capture::start`] fails with a message (watching works everywhere).

// Capturing is Windows-only for now; the sizing rules are tested everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use std::sync::atomic::{AtomicU32, AtomicU64};
#[cfg(windows)]
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
pub const RESOLUTIONS: [&str; 4] = ["720p", "1080p", "1440p", "native"];
/// The frame rates offered.
pub const FRAME_RATES: [u32; 4] = [5, 10, 15, 30];

impl Quality {
    pub fn from_config(resolution: &str, fps: u32) -> Quality {
        let max = u32::from(MAX_VIDEO_DIMENSION);
        let (max_width, max_height) = match resolution {
            "1080p" => (1920, 1080),
            "1440p" => (2560, 1440),
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
}

impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Capture")
    }
}

impl Capture {
    /// Starts capturing `screen` (its system name; empty or unknown = the primary screen) and
    /// sending it through `core`, which must already share ([`Core::start_screen_share`]).
    pub fn start(core: Core, screen: &str, quality: Quality) -> Result<Capture, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(true));
        start_thread(core, screen, quality, stop.clone(), keyframe.clone())?;
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
    _quality: Quality,
    _stop: Arc<AtomicBool>,
    _keyframe: Arc<AtomicBool>,
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
) -> Result<(), String> {
    // A monitor cannot move between threads: the capture thread finds it and reports back.
    let screen = screen.to_owned();
    let (found, result) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("hermes-screen-capture".into())
        .spawn(move || match pick_monitor(&screen) {
            Ok(monitor) => {
                let _ = found.send(Ok(()));
                run(&core, &monitor, quality, &stop, &keyframe);
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
) {
    let name = monitor.name().unwrap_or_default();
    let source = match super::dxgi::Duplicator::open(&name) {
        Ok(duplicator) if duplicator.video() => {
            match run_hardware(core, duplicator, &name, quality, stop, keyframe) {
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

/// The GPU path: Desktop Duplication hands over a picture `quality.fps` times a second at
/// most (only when the screen changed, or the last one again when a keyframe is due), the
/// video processor scales and converts it to NV12, and the GPU's encoder encodes it; the
/// picture never leaves the GPU. One thread suffices: the CPU only coordinates.
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
) -> Result<(), String> {
    use super::gpu_convert::{GpuConvert, Nv12};
    use super::mf_encode::MfEncoder;
    use hermes_client_core::voice::ScreenFrame;

    let stats = Stats::default();
    let interval = Duration::from_secs(1) / quality.fps;
    let wait_ms = u32::try_from(interval.as_millis()).unwrap_or(100).max(1);
    let started = Instant::now();
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
        // At most `quality.fps` pictures a second: wait for this one's turn, then take the
        // newest picture (Desktop Duplication keeps only the latest).
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
        next += interval;
        if next < Instant::now() {
            next = Instant::now();
        }

        let begin = Instant::now();
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
                let output = fit(shot.width, shot.height, quality);
                if convert
                    .as_ref()
                    .is_none_or(|c| (c.input_size(), c.output_size()) != (input, output))
                {
                    convert = Some(GpuConvert::new(
                        duplicator.device(),
                        input,
                        output,
                        quality.fps,
                    )?);
                }
                if encoder.as_ref().is_none_or(|e| e.size() != output) {
                    let created = MfEncoder::new(
                        duplicator.device(),
                        duplicator.luid(),
                        output,
                        quality.fps,
                        hardware_bitrate(output.0, output.1, quality.fps),
                    )?;
                    tracing::info!("screen share: encoding with {}", created.name);
                    encoder = Some(created);
                    keyframe.store(true, Ordering::Relaxed);
                }
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
                stats
                    .encode_us
                    .fetch_add(micros(encode_begin.elapsed()), Ordering::Relaxed);
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
                        stats.refused.fetch_add(1, Ordering::Relaxed);
                        resync = true;
                    }
                }
            }
        }
        if stats_since.elapsed() >= STATS_EVERY {
            let size = last.as_ref().map_or((0, 0), |p| (p.width, p.height));
            let what = encoder
                .as_ref()
                .map_or("hardware".to_owned(), |e| format!("GPU, {}", e.name));
            log_stats(&stats, stats_since.elapsed(), size, quality, &what);
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
            log_stats(&stats, stats_since.elapsed(), size, quality, what);
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
fn log_stats(stats: &Stats, over: Duration, (w, h): (u32, u32), quality: Quality, what: &str) {
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
        quality.fps,
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
