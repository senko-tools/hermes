//! A big animation played from its file instead of from memory: a thread decodes a few frames
//! ahead, and each draw takes the frame that is due. All of a full-HD animation's frames would
//! take hundreds of megabytes once decoded (a frame is width × height × 4 bytes, however small
//! the file is); this holds only a handful. A stream decodes only while it is drawn.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use gpui_kit::{RenderImage, Window};
use image::Frame;

/// How many decoded frames wait at most.
const FRAMES_AHEAD: usize = 4;
/// How many streams decode at once; the viewer's always may.
const MAX_RUNNING: usize = 3;
/// Not drawn for this long: its decoder stops.
const IDLE: Duration = Duration::from_millis(500);
/// Behind by more than this: the timing starts again from now instead of catching up.
const LATE: Duration = Duration::from_millis(250);

type Queue = Arc<Mutex<VecDeque<(Duration, Arc<RenderImage>)>>>;

/// The decoding thread. Dropping it stops the thread.
struct Decoder {
    queue: Queue,
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct State {
    /// The frame drawn now; `None` until the first one is decoded (the still shows meanwhile).
    current: Option<Arc<RenderImage>>,
    /// When the next frame is due.
    next_at: Option<Instant>,
    decoder: Option<Decoder>,
    /// When it was last drawn.
    wanted: Option<Instant>,
}

pub struct AnimStream {
    bytes: Arc<Vec<u8>>,
    /// The long side frames are made to fit; `None`: their own size.
    side: Option<u32>,
    /// Played regardless of how many others decode (the viewer's).
    priority: bool,
    state: Mutex<State>,
}

static LIVE: Mutex<Vec<Weak<AnimStream>>> = Mutex::new(Vec::new());

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AnimStream {
    pub fn new(bytes: Arc<Vec<u8>>, side: Option<u32>) -> Arc<Self> {
        let stream = Arc::new(AnimStream {
            bytes,
            side,
            priority: side.is_none(),
            state: Mutex::default(),
        });
        let mut live = lock(&LIVE);
        live.retain(|s| s.strong_count() > 0);
        live.push(Arc::downgrade(&stream));
        stream
    }

    /// The frame to draw now, if one is decoded yet; notes that it is drawn.
    pub fn frame(&self) -> Option<Arc<RenderImage>> {
        let mut state = lock(&self.state);
        state.wanted = Some(Instant::now());
        state.current.clone()
    }

    /// What it holds on the GPU (the frame drawn last), to free with it.
    pub fn current(&self) -> Option<Arc<RenderImage>> {
        lock(&self.state).current.clone()
    }

    fn start(&self) -> Decoder {
        let queue = Queue::default();
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let (bytes, side) = (self.bytes.clone(), self.side);
        let (q, s, f) = (queue.clone(), stop.clone(), failed.clone());
        let spawned = std::thread::Builder::new()
            .name("picture-stream".into())
            .spawn(move || {
                if let Err(err) = decode_loop(&bytes, side, &q, &s) {
                    tracing::warn!("animation stream: {err}");
                    f.store(true, Ordering::Relaxed);
                }
            });
        if let Err(err) = spawned {
            tracing::warn!("animation stream thread: {err}");
            failed.store(true, Ordering::Relaxed);
        }
        Decoder {
            queue,
            stop,
            failed,
        }
    }

    /// Takes the frame due at `now`; the one it replaces goes into `old`. Whether it plays.
    fn advance(&self, now: Instant, running: &mut usize, old: &mut Vec<Arc<RenderImage>>) -> bool {
        let mut state = lock(&self.state);
        if state.wanted.is_none_or(|at| now.duration_since(at) > IDLE) {
            if state.decoder.take().is_some() && !self.priority {
                *running = running.saturating_sub(1);
            }
            state.next_at = None;
            return false;
        }
        if state.decoder.is_none() {
            if !self.priority {
                if *running >= MAX_RUNNING {
                    return false;
                }
                *running += 1;
            }
            state.decoder = Some(self.start());
        }
        let Some(decoder) = &state.decoder else {
            return false;
        };
        if decoder.failed.load(Ordering::Relaxed) && lock(&decoder.queue).is_empty() {
            return false;
        }
        if state.next_at.is_some_and(|at| now < at) {
            return true;
        }
        let Some((delay, picture)) = lock(&decoder.queue).pop_front() else {
            return true;
        };
        let base = match state.next_at {
            Some(at) if now.duration_since(at) < LATE => at,
            _ => now,
        };
        state.next_at = Some(base + delay);
        old.extend(state.current.replace(picture));
        true
    }
}

/// Before each draw: every drawn stream takes its frame due now; replaced frames are freed from
/// the GPU. Whether one plays (and so wants the next draw).
pub fn advance_all(window: &mut Window) -> bool {
    let streams: Vec<Arc<AnimStream>> = {
        let mut live = lock(&LIVE);
        live.retain(|s| s.strong_count() > 0);
        live.iter().filter_map(Weak::upgrade).collect()
    };
    let now = Instant::now();
    let mut running = streams
        .iter()
        .filter(|s| !s.priority && lock(&s.state).decoder.is_some())
        .count();
    let mut old = Vec::new();
    let mut busy = false;
    for stream in &streams {
        busy |= stream.advance(now, &mut running, &mut old);
    }
    for picture in old {
        let _ = window.drop_image(picture);
    }
    busy
}

/// How long a frame shows: like browsers, 10 ms or less counts as 100 ms.
fn delay_of(frame: &Frame) -> Duration {
    let (numer, denom) = frame.delay().numer_denom_ms();
    let ms = numer / denom.max(1);
    Duration::from_millis(if ms <= 10 { 100 } else { u64::from(ms) })
}

/// Decodes the animation over and over into `queue` until `stop`.
fn decode_loop(
    bytes: &[u8],
    side: Option<u32>,
    queue: &Queue,
    stop: &AtomicBool,
) -> Result<(), String> {
    loop {
        let Some(frames) = super::animation_decoder(bytes)? else {
            return Err("not an animation".into());
        };
        let mut any = false;
        for frame in frames {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            let frame = frame.map_err(|e| e.to_string())?;
            any = true;
            let delay = delay_of(&frame);
            let mut pixels = frame.into_buffer();
            if let Some(side) = side {
                pixels = super::fit(pixels, side);
            }
            super::to_bgra(&mut pixels);
            let picture = Arc::new(RenderImage::new(vec![Frame::new(pixels)]));
            let mut picture = Some(picture);
            while let Some(p) = picture.take() {
                if stop.load(Ordering::Relaxed) {
                    return Ok(());
                }
                let mut q = lock(queue);
                if q.len() < FRAMES_AHEAD {
                    q.push_back((delay, p));
                } else {
                    drop(q);
                    picture = Some(p);
                    std::thread::sleep(Duration::from_millis(4));
                }
            }
        }
        if !any {
            return Err("no frames".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_decodes_ahead_and_loops() {
        let bytes = super::super::tests::gif(3, 8);
        let queue = Queue::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (q, s) = (queue.clone(), stop.clone());
        let thread = std::thread::spawn(move || decode_loop(&bytes, None, &q, &s));
        let mut seen = Vec::new();
        let started = Instant::now();
        while seen.len() < 7 && started.elapsed() < Duration::from_secs(10) {
            if let Some((delay, picture)) = lock(&queue).pop_front() {
                assert_eq!(delay, Duration::from_millis(50));
                // BGRA: the red of each frame is in the third byte.
                seen.push(picture.as_bytes(0).unwrap()[2]);
            }
            std::thread::yield_now();
        }
        stop.store(true, Ordering::Relaxed);
        thread.join().unwrap().unwrap();
        assert_eq!(
            seen,
            [0, 20, 40, 0, 20, 40, 0],
            "in order, again from the start"
        );
    }
}
