//! The playback pipeline before the output ring buffer (PLAN.md 12.5 "Playback"): one jitter
//! buffer and decoder per remote participant, per-user volume and mute, the mixer with UI sounds,
//! output volume, and resampling to the device rate.

use std::collections::HashMap;

use hermes_proto::voice::AudioHeader;

use crate::codec::Decoder;
use crate::dsp::MonoResampler;
use crate::jitter::{JitterBuffer, Pop};
use crate::mixer::Mixer;
use crate::sounds::{Sound, SoundPlayer};
use crate::{FRAME, SAMPLE_RATE};

/// A participant counts as speaking while voiced packets arrived within this time.
pub const SPEAKING_WINDOW_MS: f64 = 250.0;

struct Remote {
    jitter: JitterBuffer,
    decoder: Decoder,
    frame: [f32; FRAME],
    volume: f32,
    muted: bool,
    last_voice_ms: Option<f64>,
}

impl Remote {
    fn new(volume: f32) -> Self {
        Remote {
            jitter: JitterBuffer::default(),
            decoder: Decoder::default(),
            frame: [0.0; FRAME],
            volume,
            muted: false,
            last_voice_ms: None,
        }
    }

    /// Decodes the next frame; `false` when there is nothing to play.
    fn render(&mut self) -> bool {
        match self.jitter.pop() {
            Pop::Packet(packet) => self.decoder.decode(&packet, &mut self.frame),
            Pop::Lost { fec: Some(next) } => self.decoder.decode_fec(&next, &mut self.frame),
            Pop::Lost { fec: None } => self.decoder.conceal(&mut self.frame),
            Pop::Idle => return false,
        }
        true
    }
}

/// Receive statistics of all participants.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PlaybackStats {
    pub lost_frames: u64,
    pub late_packets: u64,
    pub played_frames: u64,
}

pub struct Playback {
    remotes: HashMap<u16, Remote>,
    mixer: Mixer,
    sounds: SoundPlayer,
    pub output_volume: f32,
    pub sounds_volume: f32,
    pub sounds_enabled: bool,
    /// Deafened: participants are not played (sounds still are).
    pub deafened: bool,
    resampler: MonoResampler,
    at48: Vec<f32>,
    out: Vec<f32>,
    played_frames: u64,
}

impl Playback {
    pub fn new(output_rate: u32) -> Self {
        Playback {
            remotes: HashMap::new(),
            mixer: Mixer::default(),
            sounds: SoundPlayer::default(),
            output_volume: 1.0,
            sounds_volume: 0.6,
            sounds_enabled: true,
            deafened: false,
            resampler: MonoResampler::new(SAMPLE_RATE, output_rate),
            at48: Vec::with_capacity(FRAME * 2),
            out: Vec::with_capacity(FRAME * 4),
            played_frames: 0,
        }
    }

    /// Adds a received datagram from `participant` (created on first use at `default_volume`).
    pub fn push(&mut self, participant: u16, datagram: &[u8], now_ms: f64, default_volume: f32) {
        let Ok((header, opus)) = AudioHeader::decode(datagram) else {
            return;
        };
        let remote = self
            .remotes
            .entry(participant)
            .or_insert_with(|| Remote::new(default_volume));
        if header.voice_active {
            remote.last_voice_ms = Some(now_ms);
        }
        remote.jitter.push(
            header.sequence,
            header.timestamp,
            header.voice_active,
            opus.to_vec(),
            now_ms,
        );
    }

    pub fn remove(&mut self, participant: u16) {
        self.remotes.remove(&participant);
    }

    pub fn clear(&mut self) {
        self.remotes.clear();
    }

    /// Per-user volume, 0–2 (200 %).
    pub fn set_volume(&mut self, participant: u16, volume: f32) {
        self.remotes
            .entry(participant)
            .or_insert_with(|| Remote::new(volume))
            .volume = volume;
    }

    pub fn set_muted(&mut self, participant: u16, muted: bool) {
        self.remotes
            .entry(participant)
            .or_insert_with(|| Remote::new(1.0))
            .muted = muted;
    }

    pub fn play_sound(&mut self, sound: Sound) {
        if self.sounds_enabled {
            self.sounds.play(sound, self.sounds_volume);
        }
    }

    pub fn stop_sounds(&mut self) {
        self.sounds.stop_all();
    }

    /// Whether anything is (or may soon be) audible.
    pub fn is_active(&self) -> bool {
        self.sounds.is_playing() || !self.remotes.is_empty()
    }

    /// Whether `participant` sent voiced packets within the last 250 ms.
    pub fn speaking(&self, participant: u16, now_ms: f64) -> bool {
        self.remotes
            .get(&participant)
            .and_then(|r| r.last_voice_ms)
            .is_some_and(|t| now_ms - t < SPEAKING_WINDOW_MS)
    }

    pub fn participants(&self) -> impl Iterator<Item = u16> + '_ {
        self.remotes.keys().copied()
    }

    pub fn stats(&self) -> PlaybackStats {
        PlaybackStats {
            lost_frames: self.remotes.values().map(|r| r.jitter.lost).sum(),
            late_packets: self.remotes.values().map(|r| r.jitter.late).sum(),
            played_frames: self.played_frames,
        }
    }

    /// Mixes the next 20 ms at 48 kHz.
    pub fn render_48k(&mut self) -> &[f32; FRAME] {
        self.mixer.clear();
        for remote in self.remotes.values_mut() {
            // Always pop, so buffers keep moving while muted or deafened.
            if remote.render() {
                self.played_frames += 1;
                if !remote.muted && !self.deafened {
                    self.mixer.add_voice(&remote.frame, remote.volume);
                }
            }
        }
        let sounds = self.sounds.next(FRAME);
        self.mixer.add_sound(sounds);
        self.mixer.finish(self.output_volume)
    }

    /// Mixes the next 20 ms and returns it at the output rate (mono).
    pub fn render(&mut self) -> &[f32] {
        let frame = *self.render_48k();
        self.at48.extend_from_slice(&frame);
        self.out.clear();
        self.resampler.process(&mut self.at48, &mut self.out);
        &self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::Capture;
    use crate::{AudioSettings, dsp};

    fn tone_packets(n: usize) -> Vec<Vec<u8>> {
        let settings = AudioSettings {
            noise_suppression: false,
            ..AudioSettings::default()
        };
        let mut capture = Capture::new(48_000, 1, &settings, 64_000);
        let samples: Vec<f32> = (0..n * FRAME)
            .map(|i| 0.3 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin())
            .collect();
        let mut out = Vec::new();
        capture.push(&samples, |d| out.push(d.to_vec()));
        out
    }

    #[test]
    fn plays_a_stream_with_reordering_and_loss() {
        let packets = tone_packets(50);
        let mut playback = Playback::new(48_000);
        let mut rendered = Vec::new();
        for (i, chunk) in packets.chunks(2).enumerate() {
            let now = i as f64 * 40.0;
            // Swap every pair, and lose every tenth packet.
            for (j, p) in chunk.iter().enumerate().rev() {
                if (i * 2 + j) % 10 != 9 {
                    playback.push(7, p, now, 1.0);
                }
            }
            rendered.extend_from_slice(playback.render_48k());
            rendered.extend_from_slice(playback.render_48k());
        }
        for _ in 0..10 {
            rendered.extend_from_slice(playback.render_48k());
        }
        let level = dsp::rms(&rendered[FRAME * 10..FRAME * 40]);
        assert!((0.12..0.28).contains(&level), "{level}");
        let stats = playback.stats();
        assert!(stats.lost_frames >= 4, "{stats:?}");
        assert!(playback.speaking(7, 1000.0));
        assert!(!playback.speaking(7, 5000.0));
        assert!(!playback.speaking(8, 1000.0));
    }

    #[test]
    fn volume_mute_and_deafen() {
        let packets = tone_packets(20);
        let level_with = |configure: &dyn Fn(&mut Playback)| {
            let mut playback = Playback::new(48_000);
            configure(&mut playback);
            let mut out = Vec::new();
            for (i, p) in packets.iter().enumerate() {
                playback.push(1, p, i as f64 * 20.0, 1.0);
                out.extend_from_slice(playback.render_48k());
            }
            dsp::rms(&out[FRAME * 8..])
        };
        let normal = level_with(&|_| {});
        let half = level_with(&|p| p.set_volume(1, 0.5));
        assert!((half / normal - 0.5).abs() < 0.1, "{normal} {half}");
        assert_eq!(level_with(&|p| p.set_muted(1, true)), 0.0);
        assert_eq!(level_with(&|p| p.deafened = true), 0.0);
        let quiet = level_with(&|p| p.output_volume = 0.25);
        assert!((quiet / normal - 0.25).abs() < 0.05);
    }

    #[test]
    fn sounds_play_without_participants_and_resample() {
        let mut playback = Playback::new(44_100);
        playback.play_sound(Sound::Join);
        assert!(playback.is_active());
        let mut out = Vec::new();
        for _ in 0..20 {
            out.extend_from_slice(playback.render());
        }
        // Audible (the UI sounds are soft on purpose).
        assert!(dsp::rms(&out) > 0.02, "{}", dsp::rms(&out));
        assert!(out.len() > 44_100 * 380 / 1000, "{}", out.len());
        assert!(!playback.is_active());
        playback.sounds_enabled = false;
        playback.play_sound(Sound::Join);
        assert!(!playback.is_active());
    }
}
