//! Mixing (PLAN.md 12.5 playback step 4): participants summed with their volume, soft-clipped,
//! UI sounds added, output volume applied.

use crate::FRAME;
use crate::dsp::soft_clip;

/// Highest per-user volume (200 %).
pub const MAX_USER_VOLUME: f32 = 2.0;

#[derive(Debug, Clone)]
pub struct Mixer {
    voices: [f32; FRAME],
    sounds: [f32; FRAME],
    out: [f32; FRAME],
}

impl Default for Mixer {
    fn default() -> Self {
        Mixer {
            voices: [0.0; FRAME],
            sounds: [0.0; FRAME],
            out: [0.0; FRAME],
        }
    }
}

impl Mixer {
    /// Starts a new frame.
    pub fn clear(&mut self) {
        self.voices.fill(0.0);
        self.sounds.fill(0.0);
    }

    /// Adds a participant's frame at `volume` (0–200 %).
    pub fn add_voice(&mut self, frame: &[f32; FRAME], volume: f32) {
        let volume = volume.clamp(0.0, MAX_USER_VOLUME);
        for (m, s) in self.voices.iter_mut().zip(frame) {
            *m += s * volume;
        }
    }

    /// Adds UI sound samples (already at their volume).
    pub fn add_sound(&mut self, samples: &[f32]) {
        for (m, s) in self.sounds.iter_mut().zip(samples) {
            *m += s;
        }
    }

    /// The finished frame: voices soft-clipped, sounds added, output volume, clamped to ±1.
    pub fn finish(&mut self, output_volume: f32) -> &[f32; FRAME] {
        for ((o, v), s) in self.out.iter_mut().zip(&self.voices).zip(&self.sounds) {
            *o = ((soft_clip(*v) + s) * output_volume).clamp(-1.0, 1.0);
        }
        &self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_with_volumes_and_clips() {
        let mut mixer = Mixer::default();
        mixer.add_voice(&[0.1; FRAME], 1.0);
        mixer.add_voice(&[0.1; FRAME], 0.5);
        let out = mixer.finish(1.0);
        assert!((out[0] - 0.15f32.tanh()).abs() < 1e-6);

        mixer.clear();
        mixer.add_voice(&[0.9; FRAME], 2.0);
        mixer.add_voice(&[0.9; FRAME], 5.0); // capped at 200 %
        let out = mixer.finish(1.0);
        assert!(out[0] < 1.0 && out[0] > 0.99);

        mixer.clear();
        mixer.add_voice(&[0.2; FRAME], 1.0);
        mixer.add_sound(&[0.1; 10]);
        let out = mixer.finish(0.5);
        assert!((out[0] - (0.2f32.tanh() + 0.1) * 0.5).abs() < 1e-6);
        assert!((out[20] - 0.2f32.tanh() * 0.5).abs() < 1e-6);

        mixer.clear();
        assert_eq!(mixer.finish(1.0), &[0.0; FRAME]);
    }
}
