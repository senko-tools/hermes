//! Mixing (PLAN.md 12.5 playback step 4): participants summed with their volume, screen share
//! sound (stereo) added, soft-clipped, UI sounds added, output volume applied. The result is
//! stereo; voices and sounds are mono and go to both sides.

use crate::FRAME;
use crate::dsp::soft_clip;

/// Highest per-user amplitude factor: a slider at [`crate::MAX_VOLUME`].
pub const MAX_USER_VOLUME: f32 = 4.0;

/// One 20 ms stereo frame, interleaved (left, right).
pub const STEREO_FRAME: usize = FRAME * 2;

#[derive(Debug, Clone)]
pub struct Mixer {
    voices: [f32; FRAME],
    shares: [f32; STEREO_FRAME],
    sounds: [f32; FRAME],
    out: [f32; STEREO_FRAME],
}

impl Default for Mixer {
    fn default() -> Self {
        Mixer {
            voices: [0.0; FRAME],
            shares: [0.0; STEREO_FRAME],
            sounds: [0.0; FRAME],
            out: [0.0; STEREO_FRAME],
        }
    }
}

impl Mixer {
    /// Starts a new frame.
    pub fn clear(&mut self) {
        self.voices.fill(0.0);
        self.shares.fill(0.0);
        self.sounds.fill(0.0);
    }

    /// Adds a screen share's stereo frame at the amplitude factor `volume`.
    pub fn add_share(&mut self, frame: &[f32; STEREO_FRAME], volume: f32) {
        let volume = volume.clamp(0.0, MAX_USER_VOLUME);
        for (m, s) in self.shares.iter_mut().zip(frame) {
            *m += s * volume;
        }
    }

    /// Adds a participant's frame at the amplitude factor `volume`.
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

    /// The finished stereo frame: voices and shares soft-clipped, sounds added, output volume,
    /// clamped to ±1.
    pub fn finish(&mut self, output_volume: f32) -> &[f32; STEREO_FRAME] {
        for (i, (v, s)) in self.voices.iter().zip(&self.sounds).enumerate() {
            for side in 0..2 {
                let at = i * 2 + side;
                let mixed = soft_clip(*v + self.shares[at]) + s;
                self.out[at] = (mixed * output_volume).clamp(-1.0, 1.0);
            }
        }
        &self.out
    }
}

/// The average of the two sides of a stereo frame (for mono outputs).
pub fn to_mono(stereo: &[f32; STEREO_FRAME], out: &mut [f32; FRAME]) {
    for (o, pair) in out.iter_mut().zip(stereo.as_chunks::<2>().0) {
        *o = (pair[0] + pair[1]) * 0.5;
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
        // A voice is the same on both sides.
        assert!((out[0] - 0.15f32.tanh()).abs() < 1e-6);
        assert_eq!(out[0], out[1]);

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
        assert!((out[1] - (0.2f32.tanh() + 0.1) * 0.5).abs() < 1e-6);
        // Sample 10 (interleaved 20, 21) has no sound.
        assert!((out[20] - 0.2f32.tanh() * 0.5).abs() < 1e-6);

        mixer.clear();
        assert_eq!(mixer.finish(1.0), &[0.0; STEREO_FRAME]);
    }

    #[test]
    fn shares_keep_their_sides() {
        let mut mixer = Mixer::default();
        let mut share = [0.0; STEREO_FRAME];
        for pair in share.as_chunks_mut::<2>().0 {
            *pair = [0.3, -0.1];
        }
        mixer.add_share(&share, 0.5);
        mixer.add_voice(&[0.1; FRAME], 1.0);
        let out = mixer.finish(1.0);
        assert!((out[0] - (0.1f32 + 0.15).tanh()).abs() < 1e-6);
        assert!((out[1] - (0.1f32 - 0.05).tanh()).abs() < 1e-6);
        let mut mono = [0.0; FRAME];
        to_mono(out, &mut mono);
        assert!((mono[0] - (out[0] + out[1]) / 2.0).abs() < 1e-6);
    }
}
