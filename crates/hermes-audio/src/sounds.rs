//! UI sounds (join/leave/mute/ping/ring), synthesized at 48 kHz so no asset files are needed.

use std::f32::consts::TAU;

use crate::SAMPLE_RATE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    /// Someone (or you) joined the room.
    Join,
    /// Someone (or you) left the room.
    Leave,
    Mute,
    Unmute,
    /// A mention or message ping.
    Ping,
    /// One ring of an incoming call (about 1.6 s; the UI repeats it for up to 30 s).
    Ring,
}

impl Sound {
    pub const ALL: [Sound; 6] = [
        Sound::Join,
        Sound::Leave,
        Sound::Mute,
        Sound::Unmute,
        Sound::Ping,
        Sound::Ring,
    ];

    /// The samples of this sound at 48 kHz mono, peak about 0.5.
    pub fn samples(self) -> Vec<f32> {
        match self {
            // Low and soft (G4 and C5): a rising pair to join, falling to leave.
            Sound::Join => soft_notes(&[(392.0, 0.09), (523.25, 0.16)]),
            Sound::Leave => soft_notes(&[(523.25, 0.09), (392.0, 0.16)]),
            Sound::Mute => soft_notes(&[(330.0, 0.08)]),
            Sound::Unmute => soft_notes(&[(440.0, 0.08)]),
            Sound::Ping => notes(&[(1046.5, 0.05), (1318.5, 0.09)]),
            Sound::Ring => {
                let mut out = notes(&[(784.0, 0.15), (659.3, 0.15), (784.0, 0.15), (659.3, 0.15)]);
                // A pause before the next ring.
                out.resize(out.len() + SAMPLE_RATE as usize, 0.0);
                out
            }
        }
    }
}

/// Sine notes `(frequency, seconds)` one after another, each with a short fade in and out.
fn notes(notes: &[(f32, f32)]) -> Vec<f32> {
    let rate = SAMPLE_RATE as f32;
    let mut out = Vec::new();
    for &(freq, secs) in notes {
        let len = (secs * rate) as usize;
        let fade = (0.008 * rate) as usize;
        for i in 0..len {
            let env = (i.min(len - i) as f32 / fade as f32).min(1.0);
            out.push(0.5 * env * (TAU * freq * i as f32 / rate).sin());
        }
    }
    out
}

/// Like [`notes`], but rounder: a slower attack and a decay through each note, at a lower
/// level, so they do not sting.
fn soft_notes(notes: &[(f32, f32)]) -> Vec<f32> {
    let rate = SAMPLE_RATE as f32;
    let mut out = Vec::new();
    for &(freq, secs) in notes {
        let len = (secs * rate) as usize;
        let attack = (0.012 * rate) as usize;
        let release = (0.02 * rate) as usize;
        for i in 0..len {
            let t = i as f32 / len as f32;
            let rise = (i as f32 / attack as f32).min(1.0);
            let fall = ((len - i) as f32 / release as f32).min(1.0);
            let decay = (-2.5 * t).exp();
            out.push(0.38 * rise * fall * decay * (TAU * freq * i as f32 / rate).sin());
        }
    }
    out
}

/// Sounds currently playing; [`next`](Self::next) mixes them frame by frame.
#[derive(Debug, Default)]
pub struct SoundPlayer {
    playing: Vec<(Vec<f32>, usize)>,
    buf: Vec<f32>,
}

impl SoundPlayer {
    pub fn play(&mut self, sound: Sound, volume: f32) {
        let samples = sound
            .samples()
            .into_iter()
            .map(|s| s * volume.clamp(0.0, 1.0))
            .collect();
        self.playing.push((samples, 0));
    }

    pub fn is_playing(&self) -> bool {
        !self.playing.is_empty()
    }

    pub fn stop_all(&mut self) {
        self.playing.clear();
    }

    /// The next `len` samples of everything playing, summed.
    pub fn next(&mut self, len: usize) -> &[f32] {
        self.buf.clear();
        self.buf.resize(len, 0.0);
        for (samples, pos) in &mut self.playing {
            let end = (*pos + len).min(samples.len());
            for (b, s) in self.buf.iter_mut().zip(&samples[*pos..end]) {
                *b += s;
            }
            *pos = end;
        }
        self.playing.retain(|(samples, pos)| *pos < samples.len());
        &self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_are_short_and_bounded() {
        for sound in Sound::ALL {
            let s = sound.samples();
            assert!(!s.is_empty());
            assert!(s.len() < SAMPLE_RATE as usize * 2, "{sound:?}");
            assert!(s.iter().all(|x| x.abs() <= 0.5), "{sound:?}");
            // Fades in and out, so no clicks.
            assert!(s[0].abs() < 0.01);
        }
    }

    #[test]
    fn player_mixes_and_finishes() {
        let mut player = SoundPlayer::default();
        player.play(Sound::Mute, 1.0);
        player.play(Sound::Mute, 1.0);
        let len = Sound::Mute.samples().len();
        let first = player.next(960).to_vec();
        assert!((first[500] - 2.0 * Sound::Mute.samples()[500]).abs() < 1e-6);
        let mut total = 960;
        while player.is_playing() {
            player.next(960);
            total += 960;
        }
        assert!(total >= len && total < len + 960);
        assert!(player.next(960).iter().all(|s| *s == 0.0));
    }
}
