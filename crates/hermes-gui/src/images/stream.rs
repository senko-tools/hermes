//! A big animation played from its file instead of from memory: a thread decodes a few frames
//! ahead, and each draw takes the frame that is due. All of a full-HD animation's frames would
//! take hundreds of megabytes once decoded (a frame is width × height × 4 bytes, however small
//! the file is); this holds only a handful. A stream decodes only while it is drawn, and the
//! window is drawn again only when its next frame is due (#60).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant};

use gpui_kit::{RenderImage, Window};
use image::Frame;

/// How many decoded frames wait at most.
const FRAMES_AHEAD: usize = 2;
/// How many streams decode at once; the viewer's always may.
const MAX_RUNNING: usize = 3;
/// Not drawn for this long: its decoder stops.
const IDLE: Duration = Duration::from_millis(500);
/// Behind by more than this: the timing starts again from now instead of catching up.
const LATE: Duration = Duration::from_millis(250);
/// Waiting for the decoder: the window looks again this often.
const WAITING: Duration = Duration::from_millis(10);
/// Not allowed to start yet (too many others run): it asks again this often.
const QUEUED: Duration = Duration::from_millis(500);

/// The decoded frames waiting, and a signal for the decoder when one is taken.
#[derive(Default)]
struct Queue {
    frames: Mutex<VecDeque<(Duration, Arc<RenderImage>)>>,
    taken: Condvar,
}

/// Milliseconds since the first stream, for a time several threads read.
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

fn millis(at: Instant) -> u64 {
    at.saturating_duration_since(*EPOCH).as_millis() as u64
}

/// The decoding thread. Dropping it stops the thread.
struct Decoder {
    queue: Arc<Queue>,
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    /// The thread has ended (failed, stopped, or not drawn for a while).
    ended: Arc<AtomicBool>,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.queue.taken.notify_all();
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
    /// Waiting for a turn: too many other streams decode.
    queued: bool,
}

pub struct AnimStream {
    bytes: Arc<Vec<u8>>,
    /// The long side frames are made to fit; `None`: their own size.
    side: Option<u32>,
    /// Played regardless of how many others decode (the viewer's).
    priority: bool,
    state: Mutex<State>,
    /// When it was last drawn, for the decoder thread ([`millis`]).
    drawn: Arc<AtomicU64>,
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
            drawn: Arc::new(AtomicU64::new(0)),
        });
        let mut live = lock(&LIVE);
        live.retain(|s| s.strong_count() > 0);
        live.push(Arc::downgrade(&stream));
        stream
    }

    /// The frame to draw now, if one is decoded yet; notes that it is drawn and when the window
    /// should be drawn again for it.
    pub fn frame(&self) -> Option<Arc<RenderImage>> {
        let now = Instant::now();
        self.drawn.store(millis(now), Ordering::Relaxed);
        let mut state = lock(&self.state);
        state.wanted = Some(now);
        let failed = state
            .decoder
            .as_ref()
            .is_some_and(|d| d.failed.load(Ordering::Relaxed) && lock(&d.queue.frames).is_empty());
        let wake = match state.next_at {
            _ if failed => None,
            _ if state.queued => Some(now + QUEUED),
            // Not started yet, or waiting for its first frame.
            None => Some(now + WAITING),
            // Due already but not decoded yet: look again soon.
            Some(at) if at <= now => Some(now + WAITING),
            Some(at) => Some(at),
        };
        if let Some(at) = wake {
            super::wake_at(at);
        }
        state.current.clone()
    }

    /// What it holds on the GPU (the frame drawn last), to free with it.
    pub fn current(&self) -> Option<Arc<RenderImage>> {
        lock(&self.state).current.clone()
    }

    fn start(&self) -> Decoder {
        let queue = Arc::new(Queue::default());
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(AtomicBool::new(false));
        let (bytes, side, drawn) = (self.bytes.clone(), self.side, self.drawn.clone());
        let (q, s, f, e) = (queue.clone(), stop.clone(), failed.clone(), ended.clone());
        let spawned = std::thread::Builder::new()
            .name("picture-stream".into())
            .spawn(move || {
                let wanted = || {
                    !s.load(Ordering::Relaxed)
                        && millis(Instant::now()).saturating_sub(drawn.load(Ordering::Relaxed))
                            <= IDLE.as_millis() as u64
                };
                if let Err(err) = decode_loop(&bytes, side, &q, &wanted) {
                    tracing::warn!("animation stream: {err}");
                    f.store(true, Ordering::Relaxed);
                }
                e.store(true, Ordering::Relaxed);
            });
        if let Err(err) = spawned {
            tracing::warn!("animation stream thread: {err}");
            failed.store(true, Ordering::Relaxed);
            ended.store(true, Ordering::Relaxed);
        }
        Decoder {
            queue,
            stop,
            failed,
            ended,
        }
    }

    /// Takes the frame due at `now`; the one it replaces goes into `old`.
    fn advance(&self, now: Instant, running: &mut usize, old: &mut Vec<Arc<RenderImage>>) {
        let mut state = lock(&self.state);
        state.queued = false;
        let idle = state.wanted.is_none_or(|at| now.duration_since(at) > IDLE);
        // Stopped for not being drawn while it was not: it starts again now that it is.
        let gone_idle = state
            .decoder
            .as_ref()
            .is_some_and(|d| d.ended.load(Ordering::Relaxed) && !d.failed.load(Ordering::Relaxed));
        if idle || gone_idle {
            if state.decoder.take().is_some() && !self.priority {
                *running = running.saturating_sub(1);
            }
            state.next_at = None;
            if idle {
                return;
            }
        }
        if state.decoder.is_none() {
            if !self.priority {
                if *running >= MAX_RUNNING {
                    state.queued = true;
                    return;
                }
                *running += 1;
            }
            state.decoder = Some(self.start());
        }
        let Some(decoder) = &state.decoder else {
            return;
        };
        if state.next_at.is_some_and(|at| now < at) {
            return;
        }
        let Some((delay, picture)) = lock(&decoder.queue.frames).pop_front() else {
            return;
        };
        decoder.queue.taken.notify_one();
        let base = match state.next_at {
            Some(at) if now.duration_since(at) < LATE => at,
            _ => now,
        };
        state.next_at = Some(base + delay);
        old.extend(state.current.replace(picture));
    }
}

/// Before each draw: every drawn stream takes its frame due now; replaced frames are freed from
/// the GPU.
pub fn advance_all(window: &mut Window) {
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
    for stream in &streams {
        stream.advance(now, &mut running, &mut old);
    }
    for picture in old {
        let _ = window.drop_image(picture);
    }
}

/// Decodes the animation over and over into `queue` while `wanted`.
fn decode_loop(
    bytes: &[u8],
    side: Option<u32>,
    queue: &Queue,
    wanted: &dyn Fn() -> bool,
) -> Result<(), String> {
    loop {
        let Some(mut frames) = super::BgraFrames::new(bytes)? else {
            return Err("not an animation".into());
        };
        let mut any = false;
        while let Some(frame) = frames.next_frame() {
            if !wanted() {
                return Ok(());
            }
            let (mut pixels, delay) = frame?;
            any = true;
            if let Some(side) = side {
                pixels = super::fit(pixels, side);
            }
            let picture = Arc::new(RenderImage::new(vec![Frame::new(pixels)]));
            let mut frames_waiting = lock(&queue.frames);
            while frames_waiting.len() >= FRAMES_AHEAD {
                if !wanted() {
                    return Ok(());
                }
                frames_waiting = queue
                    .taken
                    .wait_timeout(frames_waiting, Duration::from_millis(100))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            frames_waiting.push_back((super::frame_delay(delay), picture));
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
        let queue = Arc::new(Queue::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (q, s) = (queue.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            decode_loop(&bytes, None, &q, &|| !s.load(Ordering::Relaxed))
        });
        let mut seen = Vec::new();
        let started = Instant::now();
        while seen.len() < 7 && started.elapsed() < Duration::from_secs(10) {
            let taken = lock(&queue.frames).pop_front();
            if let Some((delay, picture)) = taken {
                queue.taken.notify_one();
                assert_eq!(delay, Duration::from_millis(50));
                // BGRA: the red of each frame is in the third byte.
                seen.push(picture.as_bytes(0).unwrap()[2]);
            }
            std::thread::yield_now();
        }
        stop.store(true, Ordering::Relaxed);
        queue.taken.notify_all();
        thread.join().unwrap().unwrap();
        assert_eq!(
            seen,
            [0, 20, 40, 0, 20, 40, 0],
            "in order, again from the start"
        );
    }
}
