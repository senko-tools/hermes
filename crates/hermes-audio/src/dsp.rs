//! Signal processing building blocks (PLAN.md 12.5): resampling, levels, noise suppression, AGC
//! and the voice gate. All of it works on 48 kHz mono `f32` (full scale = 1.0) except the
//! resampler input.

use std::time::Duration;

use nnnoiseless::DenoiseState;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Resampler as _, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use crate::{FRAME, SAMPLE_RATE};

/// Resampler for one mono stream, or a passthrough when the rates match.
pub type MonoResampler = Resampler;

/// Resampler for one stream of interleaved channels, or a passthrough when the rates match.
pub struct Resampler {
    inner: Option<Async<f32>>,
    channels: usize,
    out_buf: Vec<f32>,
}

impl Resampler {
    /// A mono resampler.
    pub fn new(from: u32, to: u32) -> Self {
        Self::interleaved(from, to, 1)
    }

    /// A resampler for `channels` interleaved channels.
    pub fn interleaved(from: u32, to: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        if from == to {
            return Self {
                inner: None,
                channels,
                out_buf: Vec::new(),
            };
        }
        let mut params = SincInterpolationParameters::new(64, WindowFunction::BlackmanHarris2);
        params.interpolation = SincInterpolationType::Linear;
        // 10 ms input chunks keep the extra buffering small.
        let chunk = (from / 100).max(1) as usize;
        let inner = Async::<f32>::new_sinc(
            f64::from(to) / f64::from(from),
            1.1,
            &params,
            chunk,
            channels,
            FixedAsync::Input,
        )
        .expect("valid resampler parameters");
        let out_buf = vec![0.0; inner.output_frames_max() * channels];
        Self {
            inner: Some(inner),
            channels,
            out_buf,
        }
    }

    /// Consumes as many whole chunks of `input` (interleaved) as possible and appends the
    /// result to `output`.
    pub fn process(&mut self, input: &mut Vec<f32>, output: &mut Vec<f32>) {
        let Some(r) = self.inner.as_mut() else {
            output.append(input);
            return;
        };
        let ch = self.channels;
        let mut used = 0;
        loop {
            let need = r.input_frames_next();
            if (input.len() - used) / ch < need {
                break;
            }
            let src = InterleavedSlice::new(&input[used..used + need * ch], ch, need)
                .expect("input adapter");
            let frames = self.out_buf.len() / ch;
            let mut dst =
                InterleavedSlice::new_mut(&mut self.out_buf, ch, frames).expect("output adapter");
            match r.process_into_buffer(&src, &mut dst, None) {
                Ok((n_in, n_out)) => {
                    used += n_in * ch;
                    output.extend_from_slice(&self.out_buf[..n_out * ch]);
                }
                Err(_) => {
                    used += need * ch;
                }
            }
        }
        input.drain(..used);
    }
}

/// Appends the average of each interleaved frame of `data`, scaled by `gain`.
/// Interleaved device samples to mono, with the microphone volume applied. Channels with next to no
/// signal are left out of the average: many devices offer a stereo stream with the microphone on one
/// side, and averaging with the silent side would halve its level.
#[derive(Debug, Default)]
pub struct Downmix {
    /// Each channel's recent power, smoothed so a pause on one side does not change the level.
    power: Vec<f32>,
}

impl Downmix {
    /// A channel counts unless it is this far below the loudest (-30 dB).
    const QUIET: f32 = 0.001;
    /// How much of the smoothed power each block of samples replaces.
    const SMOOTHING: f32 = 0.05;

    pub fn process(&mut self, data: &[f32], channels: usize, gain: f32, out: &mut Vec<f32>) {
        let channels = channels.max(1);
        let frames = (data.len() / channels).max(1);
        let mut block = vec![0.0_f32; channels];
        for frame in data.chunks_exact(channels) {
            for (power, sample) in block.iter_mut().zip(frame) {
                *power += sample * sample;
            }
        }
        if self.power.len() != channels {
            self.power = block.iter().map(|p| p / frames as f32).collect();
        } else {
            for (smooth, power) in self.power.iter_mut().zip(&block) {
                *smooth += (power / frames as f32 - *smooth) * Self::SMOOTHING;
            }
        }
        let loudest = self.power.iter().copied().fold(0.0, f32::max);
        let floor = loudest * Self::QUIET;
        let counted: Vec<bool> = self.power.iter().map(|p| *p >= floor).collect();
        let count = counted.iter().filter(|c| **c).count().max(1) as f32;
        for frame in data.chunks_exact(channels) {
            let sum: f32 = frame
                .iter()
                .zip(&counted)
                .filter(|(_, counted)| **counted)
                .map(|(sample, _)| sample)
                .sum();
            let sample = sum / count * gain;
            out.push(if gain > 1.0 {
                soft_limit(sample)
            } else {
                sample
            });
        }
    }
}

/// Keeps a boosted sample below full scale without the crackle of clipping: below the knee it is
/// untouched, above it bends smoothly towards 1.
pub fn soft_limit(sample: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let size = sample.abs();
    if size <= KNEE {
        return sample;
    }
    sample.signum() * (KNEE + (1.0 - KNEE) * ((size - KNEE) / (1.0 - KNEE)).tanh())
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Linear level → dBFS; silence is `-100`.
pub fn to_db(level: f32) -> f32 {
    if level > 1e-5 {
        20.0 * level.log10()
    } else {
        -100.0
    }
}

pub fn from_db(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// RNNoise in 480-sample frames. Returns the voice probability of each 960-sample frame.
pub struct Denoiser {
    state: Box<DenoiseState<'static>>,
    input: [f32; DenoiseState::FRAME_SIZE],
    output: [f32; DenoiseState::FRAME_SIZE],
    first: bool,
}

impl Default for Denoiser {
    fn default() -> Self {
        Denoiser {
            state: DenoiseState::new(),
            input: [0.0; DenoiseState::FRAME_SIZE],
            output: [0.0; DenoiseState::FRAME_SIZE],
            first: true,
        }
    }
}

impl Denoiser {
    /// Denoises one 20 ms frame in place; returns the higher voice probability of its halves.
    pub fn process(&mut self, frame: &mut [f32; FRAME]) -> f32 {
        let mut vad = 0.0f32;
        for half in frame.as_chunks_mut::<{ DenoiseState::FRAME_SIZE }>().0 {
            for (d, s) in self.input.iter_mut().zip(half.iter()) {
                *d = s * 32768.0;
            }
            vad = vad.max(self.state.process_frame(&mut self.output, &self.input));
            if self.first {
                // The first output frame contains fade-in artifacts.
                self.first = false;
                self.output.fill(0.0);
            }
            for (d, s) in half.iter_mut().zip(self.output.iter()) {
                *d = s / 32768.0;
            }
        }
        vad
    }
}

/// Slow automatic gain control towards −18 dBFS (PLAN.md 12.5 step 4). Only voiced frames move
/// the gain, so background noise is not pumped up.
#[derive(Debug, Clone)]
pub struct Agc {
    gain: f32,
}

impl Default for Agc {
    fn default() -> Self {
        Agc { gain: 1.0 }
    }
}

impl Agc {
    pub const TARGET_DB: f32 = -18.0;
    const MIN_GAIN: f32 = 0.1;
    const MAX_GAIN: f32 = 10.0;
    /// Frames quieter than this do not change the gain.
    const VOICE_FLOOR_DB: f32 = -50.0;
    /// Per 20 ms frame: lowering is faster than raising.
    const ATTACK: f32 = 0.10;
    const RELEASE: f32 = 0.02;

    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Applies the gain to `frame` and adapts it.
    pub fn process(&mut self, frame: &mut [f32]) {
        let level_db = to_db(rms(frame));
        if level_db > Self::VOICE_FLOOR_DB {
            let wanted = from_db(Self::TARGET_DB - level_db).clamp(Self::MIN_GAIN, Self::MAX_GAIN);
            let rate = if wanted < self.gain {
                Self::ATTACK
            } else {
                Self::RELEASE
            };
            self.gain += (wanted - self.gain) * rate;
        }
        for s in frame.iter_mut() {
            *s = (*s * self.gain).clamp(-1.0, 1.0);
        }
    }
}

/// How the gate decides to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    VoiceActivity,
    PushToTalk,
}

/// The voice gate (PLAN.md 12.5 step 5).
#[derive(Debug, Clone)]
pub struct Gate {
    pub mode: InputMode,
    /// RMS threshold of voice activity, dBFS.
    pub sensitivity_db: f32,
    pub ptt_release_delay: Duration,
    pub muted: bool,
    ptt_pressed: bool,
    /// Frames the gate stays open after the last voiced (or held) frame.
    hold_frames: u32,
}

impl Default for Gate {
    fn default() -> Self {
        Gate {
            mode: InputMode::VoiceActivity,
            sensitivity_db: -50.0,
            ptt_release_delay: Duration::from_millis(150),
            muted: false,
            ptt_pressed: false,
            hold_frames: 0,
        }
    }
}

impl Gate {
    /// RNNoise voice probability that opens the gate on its own.
    pub const VOICE_PROBABILITY: f32 = 0.9;
    /// Voice activity stays open this long after the last voiced frame.
    pub const HANGOVER: Duration = Duration::from_millis(300);

    pub fn set_ptt(&mut self, pressed: bool) {
        if self.ptt_pressed && !pressed {
            self.hold_frames = frames(self.ptt_release_delay);
        }
        self.ptt_pressed = pressed;
    }

    /// Whether the gate is open for this 20 ms frame. `voice_probability` is `None` without
    /// noise suppression.
    pub fn update(&mut self, level_db: f32, voice_probability: Option<f32>) -> bool {
        if self.muted {
            self.hold_frames = 0;
            return false;
        }
        let active = match self.mode {
            InputMode::PushToTalk => self.ptt_pressed,
            InputMode::VoiceActivity => {
                let voiced = level_db > self.sensitivity_db
                    || voice_probability.is_some_and(|p| p > Self::VOICE_PROBABILITY);
                if voiced {
                    self.hold_frames = frames(Self::HANGOVER);
                }
                voiced
            }
        };
        if active {
            return true;
        }
        if self.hold_frames > 0 {
            self.hold_frames -= 1;
            return true;
        }
        false
    }
}

/// Whole 20 ms frames in `d`.
fn frames(d: Duration) -> u32 {
    (d.as_millis() / 20) as u32
}

/// Duration of `samples` at 48 kHz.
pub fn duration_of(samples: usize) -> Duration {
    Duration::from_micros(samples as u64 * 1_000_000 / u64::from(SAMPLE_RATE))
}

/// Soft clipping: linear-ish near zero, never beyond ±1.
pub fn soft_clip(s: f32) -> f32 {
    s.tanh()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_channels() {
        let mut out = Vec::new();
        Downmix::default().process(&[0.2, 0.1, 0.4, 0.3], 2, 1.0, &mut out);
        assert!((out[0] - 0.15).abs() < 1e-6, "{out:?}");
        assert!((out[1] - 0.35).abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn downmix_ignores_a_silent_channel() {
        let mut out = Vec::new();
        Downmix::default().process(&[0.4, 0.0, -0.4, 0.0], 2, 1.0, &mut out);
        assert_eq!(out, [0.4, -0.4], "the level is not halved");
    }

    #[test]
    fn downmix_keeps_counting_a_channel_during_a_short_pause() {
        let mut downmix = Downmix::default();
        let mut out = Vec::new();
        downmix.process(&[0.4, 0.4, 0.4, 0.4], 2, 1.0, &mut out);
        out.clear();
        downmix.process(&[0.4, 0.0], 2, 1.0, &mut out);
        assert_eq!(out, [0.2]);
    }

    #[test]
    fn the_limiter_only_touches_what_would_clip() {
        for quiet in [0.0, 0.3, -0.5, 0.8] {
            assert_eq!(soft_limit(quiet), quiet);
        }
        let mut last = soft_limit(0.8);
        for step in 1..200 {
            let limited = soft_limit(0.8 + step as f32 * 0.05);
            assert!(limited > last - 1e-6 && limited <= 1.0, "{limited}");
            last = limited;
        }
        assert_eq!(soft_limit(-3.0), -soft_limit(3.0));
    }

    #[test]
    fn a_boost_goes_through_the_limiter_but_unity_does_not() {
        let mut out = Vec::new();
        Downmix::default().process(&[0.9], 1, 1.0, &mut out);
        assert_eq!(out, [0.9]);
        out.clear();
        Downmix::default().process(&[0.9], 1, 4.0, &mut out);
        assert!(out[0] <= 1.0 && out[0] > 0.8, "{out:?}");
    }

    #[test]
    fn db_conversions() {
        assert!((to_db(1.0)).abs() < 1e-6);
        assert!((to_db(0.5) + 6.02).abs() < 0.01);
        assert_eq!(to_db(0.0), -100.0);
        assert!((from_db(-6.0206) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn resampler_keeps_the_duration() {
        let mut r = MonoResampler::new(44_100, 48_000);
        let mut input = vec![0.1; 44_100];
        let mut out = Vec::new();
        r.process(&mut input, &mut out);
        assert!(input.is_empty());
        assert!((47_000..=48_000).contains(&out.len()), "{}", out.len());
        let mut same = MonoResampler::new(48_000, 48_000);
        let mut input = vec![0.2; 10];
        let mut out = Vec::new();
        same.process(&mut input, &mut out);
        assert_eq!(out, vec![0.2; 10]);
    }

    #[test]
    fn agc_moves_towards_the_target() {
        let mut agc = Agc::default();
        // A quiet voice at −35 dBFS (a square wave's RMS is its amplitude) is raised slowly,
        // towards +17 dB.
        for _ in 0..500 {
            let mut frame = vec![from_db(-35.0); FRAME];
            for (i, s) in frame.iter_mut().enumerate() {
                if i % 2 == 1 {
                    *s = -*s;
                }
            }
            agc.process(&mut frame);
        }
        assert!((agc.gain() - from_db(17.0)).abs() < 0.2, "{}", agc.gain());
        // Silence does not move it.
        let before = agc.gain();
        agc.process(&mut [0.0; FRAME]);
        assert_eq!(agc.gain(), before);
        // A loud voice is lowered quickly.
        let mut agc = Agc::default();
        for _ in 0..50 {
            agc.process(&mut [0.9; FRAME]);
        }
        assert!(agc.gain() < 0.2, "{}", agc.gain());
    }

    #[test]
    fn voice_activity_gate_has_a_hangover() {
        let mut gate = Gate::default();
        assert!(!gate.update(-70.0, None));
        assert!(gate.update(-30.0, None));
        // 300 ms = 15 frames of hangover.
        for _ in 0..15 {
            assert!(gate.update(-70.0, None));
        }
        assert!(!gate.update(-70.0, None));
        // RNNoise alone can open it.
        assert!(gate.update(-70.0, Some(0.95)));
        assert!(gate.update(-70.0, Some(0.1)));
        gate.muted = true;
        assert!(!gate.update(-10.0, Some(1.0)));
    }

    #[test]
    fn push_to_talk_gate_has_a_release_delay() {
        let mut gate = Gate {
            mode: InputMode::PushToTalk,
            ..Gate::default()
        };
        assert!(!gate.update(-10.0, Some(1.0)));
        gate.set_ptt(true);
        assert!(gate.update(-90.0, None));
        gate.set_ptt(false);
        // 150 ms = 7 whole frames.
        for _ in 0..7 {
            assert!(gate.update(-90.0, None));
        }
        assert!(!gate.update(-90.0, None));
    }

    #[test]
    fn denoiser_runs_on_frames() {
        let mut d = Denoiser::default();
        let mut frame = [0.0f32; FRAME];
        for (i, s) in frame.iter_mut().enumerate() {
            *s = 0.3 * (i as f32 * 0.05).sin();
        }
        let vad = d.process(&mut frame);
        assert!((0.0..=1.0).contains(&vad));
        assert!(frame.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn soft_clip_is_bounded() {
        assert!(soft_clip(10.0) <= 1.0);
        assert!((soft_clip(0.01) - 0.01).abs() < 1e-4);
    }
}
