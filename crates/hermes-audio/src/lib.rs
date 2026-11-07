//! Hermes audio engine (PLAN.md 12.5): capture and playback pipelines, resampling, noise
//! suppression, AGC, the voice gate, Opus, the jitter buffer, the mixer, UI sounds and devices.
//!
//! - [`engine`]: the worker thread that owns the devices (or a virtual device) and the pipelines
//! - [`capture`] / [`playback`]: the pipelines themselves, usable without devices
//! - [`dsp`], [`codec`], [`jitter`], [`mixer`], [`sounds`]: building blocks
//! - [`devices`]: listing and opening cpal devices

pub mod capture;
pub mod codec;
pub mod devices;
pub mod dsp;
pub mod engine;
pub mod jitter;
pub mod mixer;
pub mod playback;
pub mod sounds;

use std::time::Duration;

pub use devices::AudioDevices;
pub use dsp::InputMode;

/// The highest position of every volume slider: 300 %, which is +12 dB.
pub const MAX_VOLUME: f32 = 3.0;
/// The highest microphone volume.
pub const MAX_INPUT_VOLUME: f32 = MAX_VOLUME;
/// Decibels per 100 % above 100 %: an amplitude twice as high.
const DB_PER_STEP: f32 = 20.0 * std::f32::consts::LOG10_2;

/// The amplitude factor of a volume slider's position (1.0 is 100 %). Volumes are in decibels, not
/// a percentage of the signal: every 100 % above 100 % doubles the amplitude, so 200 % is twice
/// 100 % and 300 % twice 200 %. Below 100 % every halving of the position halves the amplitude,
/// which reaches silence at 0 %.
pub fn volume_gain(position: f32) -> f32 {
    let position = position.clamp(0.0, MAX_VOLUME);
    if position <= 1.0 {
        position
    } else {
        (position - 1.0).exp2()
    }
}

/// A volume slider's position in decibels, `None` when it is silent.
pub fn volume_db(position: f32) -> Option<f32> {
    let position = position.clamp(0.0, MAX_VOLUME);
    if position <= 0.0 {
        None
    } else if position <= 1.0 {
        Some(20.0 * position.log10())
    } else {
        Some((position - 1.0) * DB_PER_STEP)
    }
}

/// What a volume slider says: `200 % (+6.0 dB)`.
pub fn volume_label(position: f32) -> String {
    let position = position.clamp(0.0, MAX_VOLUME);
    let percent = (position * 100.0).round();
    match volume_db(position) {
        Some(db) => format!("{percent:.0} % ({db:+.1} dB)"),
        None => format!("{percent:.0} % (silent)"),
    }
}

#[cfg(test)]
mod volume_tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn every_hundred_percent_above_full_doubles_the_volume() {
        assert!(close(volume_gain(1.0), 1.0));
        assert!(close(volume_gain(2.0), 2.0));
        assert!(close(volume_gain(3.0), 4.0));
        assert!(close(volume_gain(2.5) / volume_gain(1.5), 2.0));
    }

    #[test]
    fn below_full_each_halving_halves_and_zero_is_silent() {
        assert!(close(volume_gain(0.5), 0.5));
        assert!(close(volume_gain(0.25), 0.25));
        assert_eq!(volume_gain(0.0), 0.0);
        assert_eq!(volume_db(0.0), None);
    }

    #[test]
    fn the_label_gives_percent_and_decibels() {
        assert_eq!(volume_label(1.0), "100 % (+0.0 dB)");
        assert_eq!(volume_label(2.0), "200 % (+6.0 dB)");
        assert_eq!(volume_label(3.0), "300 % (+12.0 dB)");
        assert_eq!(volume_label(0.5), "50 % (-6.0 dB)");
        assert_eq!(volume_label(0.0), "0 % (silent)");
    }

    #[test]
    fn positions_outside_the_slider_are_clamped() {
        assert!(close(volume_gain(9.0), 4.0));
        assert_eq!(volume_gain(-1.0), 0.0);
    }
}

pub use engine::{AudioEvent, Engine, Io, VirtualIo};
pub use sounds::Sound;

/// Everything runs at 48 kHz mono internally.
pub const SAMPLE_RATE: u32 = 48_000;
/// One 20 ms frame.
pub const FRAME: usize = 960;

/// The audio settings exposed to the UI (PLAN.md 12.5 "Settings", section 17 `[voice]`).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSettings {
    /// `None` = the system default.
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    /// 0–2.
    pub input_volume: f32,
    /// 0–2.
    pub output_volume: f32,
    pub mode: InputMode,
    pub ptt_release_delay: Duration,
    /// Voice activity threshold, dBFS.
    pub vad_sensitivity_db: f32,
    pub noise_suppression: bool,
    pub auto_gain: bool,
    /// Join/leave/mute sounds.
    pub sounds: bool,
    /// 0–1.
    pub sounds_volume: f32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        AudioSettings {
            input_device: None,
            output_device: None,
            input_volume: 1.0,
            output_volume: 1.0,
            mode: InputMode::VoiceActivity,
            ptt_release_delay: Duration::from_millis(150),
            vad_sensitivity_db: -50.0,
            noise_suppression: true,
            auto_gain: false,
            sounds: true,
            sounds_volume: 0.6,
        }
    }
}
