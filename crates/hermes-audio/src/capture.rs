//! The capture pipeline after the input ring buffer (PLAN.md 12.5 "Capture" steps 1–7): mono,
//! 48 kHz, noise suppression, AGC, the gate, Opus, and the datagram.

use hermes_proto::voice::{self, AudioHeader};

use crate::codec::Encoder;
use crate::dsp::{self, Agc, Denoiser, Downmix, Gate, MonoResampler};
use crate::{AudioSettings, FRAME, SAMPLE_RATE, volume_gain};

pub struct Capture {
    channels: usize,
    resampler: MonoResampler,
    mono: Vec<f32>,
    at48: Vec<f32>,
    frame: [f32; FRAME],
    denoiser: Option<Denoiser>,
    agc: Option<Agc>,
    pub gate: Gate,
    encoder: Encoder,
    input_volume: f32,
    downmix: Downmix,
    timestamp: u32,
    sequence: u16,
    was_open: bool,
    level_db: f32,
}

impl Capture {
    pub fn new(input_rate: u32, channels: usize, settings: &AudioSettings, bitrate: u32) -> Self {
        let mut capture = Capture {
            channels: channels.max(1),
            resampler: MonoResampler::new(input_rate, SAMPLE_RATE),
            mono: Vec::with_capacity(input_rate as usize / 10),
            at48: Vec::with_capacity(SAMPLE_RATE as usize / 10),
            frame: [0.0; FRAME],
            denoiser: None,
            agc: None,
            gate: Gate::default(),
            encoder: Encoder::new(bitrate),
            input_volume: 1.0,
            downmix: Downmix::default(),
            // Random start (RFC 3550); the sequence starts at 0.
            timestamp: rand_u32(),
            sequence: 0,
            was_open: false,
            level_db: -100.0,
        };
        capture.apply(settings);
        capture
    }

    /// Takes over changed settings without losing the stream position.
    pub fn apply(&mut self, settings: &AudioSettings) {
        self.input_volume = volume_gain(settings.input_volume);
        self.gate.mode = settings.mode;
        self.gate.sensitivity_db = settings.vad_sensitivity_db;
        self.gate.ptt_release_delay = settings.ptt_release_delay;
        if settings.noise_suppression != self.denoiser.is_some() {
            self.denoiser = settings.noise_suppression.then(Denoiser::default);
        }
        if settings.auto_gain != self.agc.is_some() {
            self.agc = settings.auto_gain.then(Agc::default);
        }
    }

    pub fn set_bitrate(&mut self, bitrate: u32) {
        self.encoder.set_bitrate(bitrate);
    }

    /// The level of the last processed frame, dBFS (for the input meter).
    pub fn level_db(&self) -> f32 {
        self.level_db
    }

    /// Whether the gate was open for the last frame (your own speaking indicator).
    pub fn is_open(&self) -> bool {
        self.was_open
    }

    /// Processes interleaved device samples; `send` gets each datagram to send (sender ID 0,
    /// the server sets it).
    pub fn push(&mut self, interleaved: &[f32], mut send: impl FnMut(&[u8])) {
        self.downmix.process(
            interleaved,
            self.channels,
            self.input_volume,
            &mut self.mono,
        );
        self.resampler.process(&mut self.mono, &mut self.at48);
        let mut used = 0;
        while self.at48.len() - used >= FRAME {
            self.frame.copy_from_slice(&self.at48[used..used + FRAME]);
            used += FRAME;
            if let Some(datagram) = self.process_frame() {
                send(&datagram);
            }
        }
        self.at48.drain(..used);
    }

    /// One 20 ms frame: denoise, AGC, gate, encode. Returns the datagram to send, if any.
    fn process_frame(&mut self) -> Option<Vec<u8>> {
        let probability = self.denoiser.as_mut().map(|d| d.process(&mut self.frame));
        if let Some(agc) = &mut self.agc {
            agc.process(&mut self.frame);
        }
        let level = dsp::rms(&self.frame);
        self.level_db = dsp::to_db(level);
        let open = self.gate.update(self.level_db, probability);
        // Encode every frame so the encoder state stays continuous.
        let opus = self.encoder.encode(&self.frame).map(<[u8]>::to_vec);
        let timestamp = self.timestamp;
        self.timestamp = self.timestamp.wrapping_add(voice::FRAME_SAMPLES);
        let was_open = std::mem::replace(&mut self.was_open, open);
        // Closed: nothing, except one final frame right after the gate closes.
        if !open && !was_open {
            return None;
        }
        let opus = opus?;
        let header = AudioHeader {
            voice_active: open,
            sender_id: 0,
            timestamp,
            sequence: self.sequence,
            audio_level: voice::audio_level(level),
        };
        self.sequence = self.sequence.wrapping_add(1);
        Some(header.encode(&opus))
    }
}

/// A random `u32` from the standard library's randomly seeded hasher (no `rand` needed here).
fn rand_u32() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64),
    );
    hasher.finish() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::InputMode;

    /// `secs` of a stereo 440 Hz tone at `rate`, interleaved, at `amplitude`.
    pub fn tone(rate: u32, secs: f32, amplitude: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let s =
                    amplitude * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate as f32).sin();
                [s, s]
            })
            .collect()
    }

    fn settings() -> AudioSettings {
        AudioSettings {
            noise_suppression: false,
            ..AudioSettings::default()
        }
    }

    fn run(capture: &mut Capture, samples: &[f32]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        // 10 ms chunks, like a device callback.
        for chunk in samples.chunks(882) {
            capture.push(chunk, |d| out.push(d.to_vec()));
        }
        out
    }

    #[test]
    fn a_loud_tone_is_sent_every_20_ms() {
        let mut capture = Capture::new(44_100, 2, &settings(), 64_000);
        let packets = run(&mut capture, &tone(44_100, 1.0, 0.3));
        // ~50 frames; the resampler holds back a little.
        assert!((47..=50).contains(&packets.len()), "{}", packets.len());
        let headers: Vec<AudioHeader> = packets
            .iter()
            .map(|p| AudioHeader::decode(p).unwrap().0)
            .collect();
        for pair in headers.windows(2) {
            assert_eq!(pair[1].sequence, pair[0].sequence.wrapping_add(1));
            assert_eq!(pair[1].timestamp, pair[0].timestamp.wrapping_add(960));
        }
        assert!(headers[10].voice_active);
        assert!(headers[10].audio_level < 20, "{}", headers[10].audio_level);
        assert!(capture.is_open());
        assert!(capture.level_db() > -20.0);
    }

    #[test]
    fn silence_sends_one_final_frame_after_the_hangover() {
        let mut capture = Capture::new(48_000, 2, &settings(), 64_000);
        let mut samples = tone(48_000, 0.2, 0.3);
        samples.extend(vec![0.0; 48_000 * 2]);
        let packets = run(&mut capture, &samples);
        let last = AudioHeader::decode(packets.last().unwrap()).unwrap().0;
        assert!(!last.voice_active);
        // 10 tone frames + 15 hangover frames + the final one.
        assert_eq!(packets.len(), 10 + 15 + 1);
        assert!(!capture.is_open());
    }

    #[test]
    fn push_to_talk_and_mute() {
        let mut ptt = AudioSettings {
            mode: InputMode::PushToTalk,
            ..settings()
        };
        ptt.ptt_release_delay = std::time::Duration::ZERO;
        let mut capture = Capture::new(48_000, 2, &ptt, 64_000);
        assert!(run(&mut capture, &tone(48_000, 0.2, 0.3)).is_empty());
        capture.gate.set_ptt(true);
        assert_eq!(run(&mut capture, &tone(48_000, 0.2, 0.3)).len(), 10);
        capture.gate.muted = true;
        // The final frame after closing, then nothing.
        assert_eq!(run(&mut capture, &tone(48_000, 0.2, 0.3)).len(), 1);
    }

    #[test]
    fn noise_suppression_and_agc_run() {
        let mut capture = Capture::new(
            48_000,
            2,
            &AudioSettings {
                noise_suppression: true,
                auto_gain: true,
                ..AudioSettings::default()
            },
            64_000,
        );
        let packets = run(&mut capture, &tone(48_000, 0.5, 0.3));
        assert!(!packets.is_empty());
    }
}
