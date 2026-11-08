//! Playing decoded media (a sound or video file in a chat), apart from calls: a thread that owns
//! an output device and keeps it fed from a queue that a decoder fills. The frames it has given
//! the device are the clock a video follows.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use ringbuf::traits::{Observer as _, Producer as _};

use crate::devices;

/// How much the thread keeps in the device's ring: little, so pausing and seeking feel immediate.
const AHEAD_MS: u32 = 60;
/// How much a decoder may queue before it waits.
const QUEUE_SECONDS: usize = 2;

struct Shared {
    /// Interleaved stereo samples at [`MediaOutput::rate`].
    queue: Mutex<VecDeque<f32>>,
    paused: AtomicBool,
    stop: AtomicBool,
    /// The gain, as `f32` bits.
    gain: AtomicU32,
    /// Stereo frames the device has played (given to it, less what still waits in its ring).
    played: AtomicU64,
}

/// An output for one media file. Dropping it stops the sound.
pub struct MediaOutput {
    shared: Arc<Shared>,
    rate: u32,
    thread: Option<JoinHandle<()>>,
}

impl MediaOutput {
    /// Opens `device` (`None` or `"default"`: the system's). Starts paused.
    pub fn open(device: Option<&str>) -> Result<MediaOutput, String> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            paused: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            gain: AtomicU32::new(1.0f32.to_bits()),
            played: AtomicU64::new(0),
        });
        let (opened, rate) = mpsc::channel();
        let device = device.map(str::to_owned);
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("media-output".into())
            .spawn(move || feed(worker, device.as_deref(), &opened))
            .map_err(|e| e.to_string())?;
        let rate = rate
            .recv()
            .map_err(|_| "the media output stopped".to_owned())??;
        Ok(MediaOutput {
            shared,
            rate,
            thread: Some(thread),
        })
    }

    /// The device's sample rate: decoders produce stereo at this rate.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// A handle a decoder thread fills the queue through.
    pub fn sink(&self) -> MediaSink {
        MediaSink {
            shared: self.shared.clone(),
            rate: self.rate,
        }
    }

    /// Stereo frames waiting to be played.
    pub fn queued_frames(&self) -> u64 {
        let queue = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.len() as u64 / 2
    }

    /// Drops everything queued (a seek).
    pub fn clear(&self) {
        self.shared
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    /// The volume as a slider position (see [`crate::volume_gain`]).
    pub fn set_volume(&self, position: f32) {
        let gain = crate::volume_gain(position);
        self.shared.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// Stereo frames played since the output opened.
    pub fn played_frames(&self) -> u64 {
        self.shared.played.load(Ordering::Relaxed)
    }
}

/// Where a decoder puts its samples.
#[derive(Clone)]
pub struct MediaSink {
    shared: Arc<Shared>,
    rate: u32,
}

impl MediaSink {
    /// Queues interleaved stereo samples.
    pub fn push(&self, samples: &[f32]) {
        let mut queue = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.extend(samples.iter().copied());
    }

    /// Whether the queue has room for more: a decoder waits while it has not.
    pub fn wants_more(&self) -> bool {
        let queue = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.len() < self.rate as usize * 2 * QUEUE_SECONDS
    }
}

impl Drop for MediaOutput {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The output thread: opens the device (cpal streams stay on the thread that made them) and
/// tops its ring up from the queue every few milliseconds.
fn feed(shared: Arc<Shared>, device: Option<&str>, opened: &mpsc::Sender<Result<u32, String>>) {
    let output = devices::find(device, false)
        .or_else(|_| devices::find(None, false))
        .and_then(|d| devices::open_output(&d));
    let mut output = match output {
        Ok(output) => {
            let _ = opened.send(Ok(output.stream.rate));
            output
        }
        Err(err) => {
            let _ = opened.send(Err(err));
            return;
        }
    };
    let rate = output.stream.rate;
    let ahead = (rate * AHEAD_MS / 1000) as usize * 2;
    let mut given: u64 = 0;
    while !shared.stop.load(Ordering::Relaxed) {
        let in_ring = output.ring.occupied_len();
        if !shared.paused.load(Ordering::Relaxed) && in_ring < ahead {
            let gain = f32::from_bits(shared.gain.load(Ordering::Relaxed));
            let mut queue = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            // Whole stereo frames only.
            let wanted = (ahead - in_ring).min(queue.len()) & !1;
            for sample in queue.drain(..wanted) {
                let _ = output.ring.try_push(sample * gain);
            }
            given += wanted as u64 / 2;
        }
        let waiting = output.ring.occupied_len() as u64 / 2;
        shared
            .played
            .store(given.saturating_sub(waiting), Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(5));
    }
}
