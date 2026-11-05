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

/// The capture thread: takes a picture `quality.fps` times a second (only when the screen
/// changed, or the last one again when a keyframe is due), scales it and hands it to the
/// encoder thread, which works in parallel. A picture the encoder has no time for is dropped.
#[cfg(windows)]
fn run(
    core: &Core,
    monitor: &xcap::Monitor,
    quality: Quality,
    stop: &AtomicBool,
    keyframe: &Arc<AtomicBool>,
) {
    let name = monitor.name().unwrap_or_default();
    let mut source = match super::dxgi::Duplicator::open(&name) {
        Ok(duplicator) => Source::Dxgi(duplicator),
        Err(err) => {
            tracing::warn!("screen share: desktop duplication unavailable ({err}); using GDI");
            Source::Gdi
        }
    };
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
            let dxgi = matches!(source, Source::Dxgi(_));
            log_stats(
                &stats,
                stats_since.elapsed(),
                last.as_deref(),
                quality,
                dxgi,
            );
            stats_since = Instant::now();
        }
        std::thread::sleep(interval.saturating_sub(begin.elapsed()));
    }
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
    while let Ok(frame) = frames.recv() {
        let begin = Instant::now();
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
        let key = keyframe.swap(false, Ordering::Relaxed)
            || last_keyframe.is_none_or(|t| t.elapsed() >= KEYFRAME_EVERY);
        match encoder.as_mut().map(|e| e.encode(&frame, key)) {
            Some(Ok(Some((data, is_key)))) => {
                if is_key {
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
                    keyframe.store(true, Ordering::Relaxed);
                }
            }
            Some(Ok(None)) | None => {}
            Some(Err(err)) => tracing::warn!("screen share encoding: {err}"),
        }
    }
}

#[cfg(windows)]
fn log_stats(stats: &Stats, over: Duration, last: Option<&Frame>, quality: Quality, dxgi: bool) {
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
    let (w, h) = last.map_or((0, 0), |f| (f.width, f.height));
    tracing::info!(
        "screen share ({}, {w}x{h}, target {} fps): captured {:.1} fps ({:.1} ms each), sent \
         {:.1} fps ({:.1} ms to encode, {keyframes} keyframes), {dropped} dropped by a busy          encoder, {refused} refused by a full connection queue, {:.0} kbit/s",
        if dxgi { "desktop duplication" } else { "GDI" },
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
        assert_eq!(native.fps, 30, "the protocol's limit");
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
}
