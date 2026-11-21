//! UI sounds (join/leave/mute/message/ping/ring), synthesized at 48 kHz so no asset files are needed.

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
    /// A new message (DMs, and channels set to notify for every message).
    Message,
    /// A mention, or one of my words.
    Ping,
    /// One ring of an incoming call (about 1.6 s; the UI repeats it for up to 30 s).
    Ring,
}

impl Sound {
    pub const ALL: [Sound; 7] = [
        Sound::Join,
        Sound::Leave,
        Sound::Mute,
        Sound::Unmute,
        Sound::Message,
        Sound::Ping,
        Sound::Ring,
    ];

    /// The notes of this sound, `(frequency, seconds)`. A test keeps them all low, so none of
    /// them is sharp.
    fn notes(self) -> &'static [(f32, f32)] {
        match self {
            // A rising pair to join, falling to leave.
            Sound::Join => &[(392.0, 0.09), (523.25, 0.16)],
            Sound::Leave => &[(523.25, 0.09), (392.0, 0.16)],
            Sound::Mute => &[(330.0, 0.08)],
            Sound::Unmute => &[(440.0, 0.08)],
            // A low, short pair under the ping, so a plain message stands out less.
            Sound::Message => &[(349.23, 0.08), (440.0, 0.14)],
            Sound::Ping => &[(440.0, 0.10), (554.37, 0.18)],
            Sound::Ring => &[(523.25, 0.30), (392.0, 0.30)],
        }
    }

    /// The samples of this sound at 48 kHz mono, peak at most [`PEAK`].
    pub fn samples(self) -> Vec<f32> {
        let mut out = soft_notes(self.notes());
        if self == Sound::Ring {
            // A pause before the next ring.
            out.resize(out.len() + SAMPLE_RATE as usize, 0.0);
        }
        out
    }
}

/// How loud a sound gets before the volume setting, which is well under full scale.
const PEAK: f32 = 0.26;
/// The highest note any sound may use: higher ones sting.
#[cfg(test)]
const HIGHEST_HZ: f32 = 660.0;
/// Seconds a note takes to fade in; long enough that it never clicks or pricks.
const ATTACK: f32 = 0.02;
/// Seconds a note takes to fade out.
const RELEASE: f32 = 0.04;

/// A curve from 0 to 1 that starts and ends flat.
fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Sine notes `(frequency, seconds)` one after another: a gentle attack and a decay through each
/// note, so they do not sting.
fn soft_notes(notes: &[(f32, f32)]) -> Vec<f32> {
    let rate = SAMPLE_RATE as f32;
    let mut out = Vec::new();
    for &(freq, secs) in notes {
        let len = (secs * rate) as usize;
        let attack = ATTACK * rate;
        let release = RELEASE * rate;
        for i in 0..len {
            let t = i as f32 / len as f32;
            let rise = smooth(i as f32 / attack);
            let fall = smooth((len - i) as f32 / release);
            let decay = (-2.5 * t).exp();
            out.push(PEAK * rise * fall * decay * (TAU * freq * i as f32 / rate).sin());
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
            assert!(s.iter().all(|x| x.abs() <= PEAK), "{sound:?}");
            // Fades in and out, so no clicks.
            assert!(s[0].abs() < 0.01);
        }
    }

    #[test]
    fn sounds_are_soft() {
        for sound in Sound::ALL {
            for &(freq, secs) in sound.notes() {
                assert!(freq <= HIGHEST_HZ, "{sound:?} has a sharp note: {freq} Hz");
                assert!(secs > ATTACK + RELEASE, "{sound:?} has no room to fade");
            }
            // Nothing jumps out: the first millisecond is still nearly silent.
            let first_ms = SAMPLE_RATE as usize / 1000;
            let s = sound.samples();
            assert!(s[..first_ms].iter().all(|x| x.abs() < 0.01), "{sound:?}");
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
