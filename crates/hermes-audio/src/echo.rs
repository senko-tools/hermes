//! Echo cancellation (WebRTC's AEC3, ported to Rust): takes what the speakers played out of what
//! the microphone hears, so people on speakers or a laptop's own microphone don't send the call
//! back to it. It runs before noise suppression.
//!
//! What was played reaches the canceller through one queue for the process: playback adds each
//! mixed frame while cancellation is on, capture takes them as it goes. There is one call at a
//! time, and both run on the engine's thread.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use aec3::nodes::audio::AudioFormat;
use aec3::pipelines::linear::{self, LinearPipeline};

use crate::{FRAME, SAMPLE_RATE};

/// 10 ms at 48 kHz: what the canceller takes at a time.
const HALF: usize = FRAME / 2;
/// Played sound kept for the canceller at most (anything older is of no use to it).
const MAX_QUEUED: usize = FRAME * 25;
/// Where the canceller starts guessing how late the microphone hears the speakers.
const INITIAL_DELAY_MS: i32 = 60;

/// Whether playback keeps what it plays for the canceller.
static ON: AtomicBool = AtomicBool::new(false);
static PLAYED: Mutex<VecDeque<f32>> = Mutex::new(VecDeque::new());

/// Playback: one mixed mono frame, as it goes to the speakers.
pub fn played(frame: &[f32]) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let mut played = PLAYED.lock().expect("played sound");
    played.extend(frame.iter().copied());
    let extra = played.len().saturating_sub(MAX_QUEUED);
    played.drain(..extra);
}

/// Takes 10 ms of what was played, or silence when nothing was.
fn take_played(out: &mut [f32; HALF]) {
    let mut played = PLAYED.lock().expect("played sound");
    for sample in out.iter_mut() {
        *sample = played.pop_front().unwrap_or(0.0);
    }
}

pub struct EchoCanceller {
    pipeline: LinearPipeline,
    reference: [f32; HALF],
    out: [f32; HALF],
}

impl EchoCanceller {
    /// `None` if the canceller can't be built (it then simply stays off).
    pub fn new() -> Option<Self> {
        let format = AudioFormat::ten_ms(SAMPLE_RATE, 1);
        // Only the canceller: noise suppression and gain have their own steps here.
        let pipeline = linear::builder(format, format)
            .enable_noise_suppression(false)
            .enable_gain_controller2(false)
            .enable_post_filter(false)
            .initial_delay_ms(INITIAL_DELAY_MS)
            .build()
            .inspect_err(|err| tracing::warn!("echo cancellation unavailable: {err}"))
            .ok()?;
        ON.store(true, Ordering::Relaxed);
        PLAYED.lock().expect("played sound").clear();
        Some(EchoCanceller {
            pipeline,
            reference: [0.0; HALF],
            out: [0.0; HALF],
        })
    }

    /// Cancels the echo in one 20 ms microphone frame, in place.
    pub fn process(&mut self, frame: &mut [f32; FRAME]) {
        for half in frame.as_chunks_mut::<HALF>().0 {
            take_played(&mut self.reference);
            if self.pipeline.handle_render_frame(&self.reference).is_err() {
                continue;
            }
            if let Ok(true) = self.pipeline.process_capture_frame(half, &mut self.out) {
                half.copy_from_slice(&self.out);
            }
        }
    }
}

impl Drop for EchoCanceller {
    fn drop(&mut self) {
        ON.store(false, Ordering::Relaxed);
        PLAYED.lock().expect("played sound").clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tone played and heard back by the microphone a little later and quieter is mostly gone
    /// once the canceller has settled.
    #[test]
    fn an_echo_goes_away() {
        let mut aec = EchoCanceller::new().expect("canceller");
        // Noise-like, as speech is to the filter: a hash of the sample index.
        let tone = |n: usize| {
            let x = (n as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .rotate_left(17);
            ((x >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.4
        };
        let delay = 48 * 40; // 40 ms
        let (mut before, mut after) = (0.0f32, 0.0f32);
        for i in 0..600 {
            let start = i * FRAME;
            let speaker: Vec<f32> = (start..start + FRAME).map(tone).collect();
            played(&speaker);
            let mut mic = [0.0; FRAME];
            for (k, s) in mic.iter_mut().enumerate() {
                let n = start + k;
                *s = if n >= delay {
                    tone(n - delay) * 0.5
                } else {
                    0.0
                };
            }
            let energy = |f: &[f32]| f.iter().map(|s| s * s).sum::<f32>();
            let heard = energy(&mic);
            aec.process(&mut mic);
            if i >= 500 {
                before += heard;
                after += energy(&mic);
            }
        }
        assert!(after < before * 0.1, "left {after} of {before}");
    }
}
