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

/// The highest microphone volume, 400 % (+12 dB).
pub const MAX_INPUT_VOLUME: f32 = 4.0;

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
