//! Opus at 48 kHz mono with 20 ms frames (PLAN.md 12.5 step 6, playback step 2), and stereo
//! for the sound of screen shares.

use opus::{Application, Bitrate, Channels};

use crate::mixer::STEREO_FRAME;
use crate::{FRAME, SAMPLE_RATE};

/// The bitrate of a screen share's sound: music and films, stereo.
pub const SHARE_BITRATE: i32 = 128_000;

/// Largest encoded frame we produce (well below the 1000-byte datagram limit).
pub const MAX_PACKET: usize = 1000 - hermes_proto::voice::HEADER_LEN;

pub struct Encoder {
    inner: opus::Encoder,
    buf: Vec<u8>,
}

impl Encoder {
    /// `application = VOIP`, in-band FEC on, expected loss 10 %, DTX on.
    pub fn new(bitrate: u32) -> Self {
        let mut inner = opus::Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip)
            .expect("Opus supports 48 kHz mono");
        let _ = inner.set_inband_fec(true);
        let _ = inner.set_packet_loss_perc(10);
        let _ = inner.set_dtx(true);
        let mut encoder = Encoder {
            inner,
            buf: vec![0; MAX_PACKET],
        };
        encoder.set_bitrate(bitrate);
        encoder
    }

    pub fn set_bitrate(&mut self, bitrate: u32) {
        let bits = i32::try_from(bitrate.clamp(6_000, 510_000)).unwrap_or(64_000);
        let _ = self.inner.set_bitrate(Bitrate::Bits(bits));
    }

    /// Encodes one 20 ms frame.
    pub fn encode(&mut self, frame: &[f32; FRAME]) -> Option<&[u8]> {
        let len = self.inner.encode_float(frame, &mut self.buf).ok()?;
        Some(&self.buf[..len])
    }
}

pub struct Decoder {
    inner: opus::Decoder,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder {
            inner: opus::Decoder::new(SAMPLE_RATE, Channels::Mono)
                .expect("Opus supports 48 kHz mono"),
        }
    }
}

impl Decoder {
    /// Decodes a packet into one frame; a corrupt packet becomes concealment.
    pub fn decode(&mut self, packet: &[u8], out: &mut [f32; FRAME]) {
        if self.inner.decode_float(packet, out, false).is_err() {
            self.conceal(out);
        }
    }

    /// Recovers the frame *before* `next_packet` from its in-band FEC data.
    pub fn decode_fec(&mut self, next_packet: &[u8], out: &mut [f32; FRAME]) {
        if self.inner.decode_float(next_packet, out, true).is_err() {
            self.conceal(out);
        }
    }

    /// Packet-loss concealment for one missing frame.
    pub fn conceal(&mut self, out: &mut [f32; FRAME]) {
        if self.inner.decode_float(&[], out, false).is_err() {
            out.fill(0.0);
        }
    }
}

/// Encodes a screen share's sound: stereo, tuned for music rather than speech.
pub struct ShareEncoder {
    inner: opus::Encoder,
    buf: Vec<u8>,
}

impl Default for ShareEncoder {
    /// `application = AUDIO`, 128 kbit/s, in-band FEC on, expected loss 5 %, DTX on.
    fn default() -> Self {
        let mut inner = opus::Encoder::new(SAMPLE_RATE, Channels::Stereo, Application::Audio)
            .expect("Opus supports 48 kHz stereo");
        let _ = inner.set_bitrate(Bitrate::Bits(SHARE_BITRATE));
        let _ = inner.set_inband_fec(true);
        let _ = inner.set_packet_loss_perc(5);
        let _ = inner.set_dtx(true);
        ShareEncoder {
            inner,
            buf: vec![0; MAX_PACKET],
        }
    }
}

impl ShareEncoder {
    /// Encodes one interleaved 20 ms stereo frame.
    pub fn encode(&mut self, frame: &[f32; STEREO_FRAME]) -> Option<&[u8]> {
        let len = self.inner.encode_float(frame, &mut self.buf).ok()?;
        Some(&self.buf[..len])
    }
}

/// Decodes a screen share's sound into interleaved stereo frames.
pub struct StereoDecoder {
    inner: opus::Decoder,
}

impl Default for StereoDecoder {
    fn default() -> Self {
        StereoDecoder {
            inner: opus::Decoder::new(SAMPLE_RATE, Channels::Stereo)
                .expect("Opus supports 48 kHz stereo"),
        }
    }
}

impl StereoDecoder {
    pub fn decode(&mut self, packet: &[u8], out: &mut [f32; STEREO_FRAME]) {
        if self.inner.decode_float(packet, out, false).is_err() {
            self.conceal(out);
        }
    }

    pub fn decode_fec(&mut self, next_packet: &[u8], out: &mut [f32; STEREO_FRAME]) {
        if self.inner.decode_float(next_packet, out, true).is_err() {
            self.conceal(out);
        }
    }

    pub fn conceal(&mut self, out: &mut [f32; STEREO_FRAME]) {
        if self.inner.decode_float(&[], out, false).is_err() {
            out.fill(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(phase: usize) -> [f32; FRAME] {
        std::array::from_fn(|i| {
            let t = (phase * FRAME + i) as f32 / SAMPLE_RATE as f32;
            0.3 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
        })
    }

    #[test]
    fn round_trip_fec_and_concealment() {
        let mut enc = Encoder::new(64_000);
        let mut dec = Decoder::default();
        let packets: Vec<Vec<u8>> = (0..20)
            .map(|i| enc.encode(&tone(i)).unwrap().to_vec())
            .collect();
        assert!(packets.iter().all(|p| p.len() < MAX_PACKET && p.len() > 2));
        let mut out = [0.0; FRAME];
        for p in &packets[..10] {
            dec.decode(p, &mut out);
        }
        let level = crate::dsp::rms(&out);
        assert!((0.15..0.28).contains(&level), "{level}");
        // Frame 10 is lost: recover it from packet 11's FEC, then conceal another.
        dec.decode_fec(&packets[11], &mut out);
        assert!(crate::dsp::rms(&out) > 0.05);
        dec.conceal(&mut out);
        assert!(out.iter().all(|s| s.is_finite()));
        dec.decode(b"\xff\xff\xff", &mut out);
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn stereo_keeps_its_sides() {
        let mut enc = ShareEncoder::default();
        let mut dec = StereoDecoder::default();
        let mut out = [0.0; STEREO_FRAME];
        for i in 0..20 {
            // A tone on the left, silence on the right.
            let mono = tone(i);
            let mut frame = [0.0; STEREO_FRAME];
            for (pair, s) in frame.as_chunks_mut::<2>().0.iter_mut().zip(mono) {
                *pair = [s, 0.0];
            }
            let packet = enc.encode(&frame).unwrap().to_vec();
            assert!(packet.len() < MAX_PACKET);
            dec.decode(&packet, &mut out);
        }
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let right: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
        let (l, r) = (crate::dsp::rms(&left), crate::dsp::rms(&right));
        assert!(l > 0.15 && r < 0.02, "{l} {r}");
    }
}
