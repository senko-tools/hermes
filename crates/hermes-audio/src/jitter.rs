//! Per-participant jitter buffer (PLAN.md 12.5 playback step 1): packets ordered by sequence,
//! an adaptive target delay of 40–200 ms (starting at 60 ms) from the observed jitter, reordering,
//! late packets dropped, and loss reported so the decoder can use FEC or concealment.
//!
//! Time is passed in (milliseconds on any monotonic clock) so the buffer is testable.

use std::collections::BTreeMap;

const FRAME_MS: f32 = 20.0;
const SAMPLES_PER_MS: f64 = 48.0;
pub const MIN_DELAY_MS: f32 = 40.0;
pub const MAX_DELAY_MS: f32 = 200.0;
pub const START_DELAY_MS: f32 = 60.0;
/// Consecutive lost frames that are concealed before the stream counts as paused.
pub const MAX_CONCEALED: u32 = 5;
/// Frames beyond the target that are tolerated before the oldest are skipped (clock drift,
/// bursts after a network hiccup).
const MAX_EXCESS_FRAMES: u64 = 4;
/// How fast the target follows the jitter estimate, per packet.
const TARGET_SMOOTHING: f32 = 0.05;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pop {
    /// The next frame.
    Packet(Vec<u8>),
    /// The next frame is missing; `fec` is the following packet if it is already here (its
    /// in-band FEC data recovers the missing frame).
    Lost { fec: Option<Vec<u8>> },
    /// Nothing to play (buffering, or the speaker paused).
    Idle,
}

#[derive(Debug, Clone)]
struct Packet {
    payload: Vec<u8>,
    voice_active: bool,
}

#[derive(Debug, Clone)]
pub struct JitterBuffer {
    packets: BTreeMap<u64, Packet>,
    /// Highest extended sequence seen.
    highest: Option<u64>,
    /// Next sequence to play while playing; `None` while buffering.
    next: Option<u64>,
    /// Everything below this is late.
    floor: u64,
    last_arrival: Option<(f64, u32)>,
    jitter_ms: f32,
    target_ms: f32,
    concealed: u32,
    last_voice_active: bool,
    /// Packets dropped because they came too late (for statistics).
    pub late: u64,
    /// Frames that were missing at playout time.
    pub lost: u64,
}

impl Default for JitterBuffer {
    fn default() -> Self {
        JitterBuffer {
            packets: BTreeMap::new(),
            highest: None,
            next: None,
            floor: 0,
            last_arrival: None,
            jitter_ms: 0.0,
            target_ms: START_DELAY_MS,
            concealed: 0,
            last_voice_active: true,
            late: 0,
            lost: 0,
        }
    }
}

impl JitterBuffer {
    pub fn target_ms(&self) -> f32 {
        self.target_ms
    }

    pub fn jitter_ms(&self) -> f32 {
        self.jitter_ms
    }

    pub fn is_playing(&self) -> bool {
        self.next.is_some()
    }

    fn target_frames(&self) -> u64 {
        (self.target_ms / FRAME_MS).ceil() as u64
    }

    /// Unwraps a 16-bit sequence number relative to the highest one seen.
    fn extend(&self, seq: u16) -> u64 {
        match self.highest {
            // Start high so sequences just below the first one still fit.
            None => (1 << 32) + u64::from(seq),
            Some(highest) => {
                let delta = i64::from(seq.wrapping_sub(highest as u16) as i16);
                highest.saturating_add_signed(delta)
            }
        }
    }

    /// Adds a packet that arrived at `now_ms`. Returns `false` if it was late or a duplicate.
    pub fn push(
        &mut self,
        seq: u16,
        timestamp: u32,
        voice_active: bool,
        payload: Vec<u8>,
        now_ms: f64,
    ) -> bool {
        let ext = self.extend(seq);
        if ext < self.floor || self.next.is_some_and(|next| ext < next) {
            self.late += 1;
            return false;
        }
        if self.packets.contains_key(&ext) {
            return false;
        }
        if self.highest.is_none_or(|h| ext > h) {
            // RFC 3550 interarrival jitter, from in-order packets only.
            if let Some((arrival, ts)) = self.last_arrival {
                let sent = f64::from(timestamp.wrapping_sub(ts) as i32) / SAMPLES_PER_MS;
                let d = ((now_ms - arrival) - sent).abs() as f32;
                // Gaps in sending (a closed gate) are not jitter.
                if d < 1000.0 {
                    self.jitter_ms += (d - self.jitter_ms) / 16.0;
                }
            }
            self.last_arrival = Some((now_ms, timestamp));
            self.highest = Some(ext);
            let wanted = (MIN_DELAY_MS + 3.0 * self.jitter_ms).clamp(MIN_DELAY_MS, MAX_DELAY_MS);
            self.target_ms += (wanted - self.target_ms) * TARGET_SMOOTHING;
        }
        self.packets.insert(
            ext,
            Packet {
                payload,
                voice_active,
            },
        );
        true
    }

    /// The frame to play now; call once per 20 ms.
    pub fn pop(&mut self) -> Pop {
        let Some(mut next) = self.next else {
            return self.start();
        };
        // Too much queued: skip the oldest frames down to the target.
        if let Some(highest) = self.highest {
            let queued = highest.saturating_sub(next) + 1;
            let keep = self.target_frames();
            if queued > keep + MAX_EXCESS_FRAMES {
                next = highest + 1 - keep;
                self.packets.retain(|&seq, _| seq >= next);
            }
        }
        if let Some(packet) = self.packets.remove(&next) {
            self.advance(next);
            self.concealed = 0;
            self.last_voice_active = packet.voice_active;
            return Pop::Packet(packet.payload);
        }
        // Nothing queued and the speaker closed their gate (their last packet said so), or
        // the loss went on too long: pause. The slot is not used up, so the speaker's next
        // packet is not late.
        if self.packets.is_empty() && (!self.last_voice_active || self.concealed >= MAX_CONCEALED) {
            self.floor = next;
            self.pause();
            return Pop::Idle;
        }
        self.advance(next);
        self.concealed += 1;
        self.lost += 1;
        Pop::Lost {
            fec: self.packets.get(&(next + 1)).map(|p| p.payload.clone()),
        }
    }

    /// Starts playing once the queued packets span the target delay.
    fn start(&mut self) -> Pop {
        let (Some((&first, _)), Some(highest)) = (self.packets.first_key_value(), self.highest)
        else {
            return Pop::Idle;
        };
        if (highest - first + 1) < self.target_frames() {
            return Pop::Idle;
        }
        self.next = Some(first);
        self.concealed = 0;
        self.pop()
    }

    fn advance(&mut self, played: u64) {
        self.next = Some(played + 1);
        self.floor = played + 1;
    }

    fn pause(&mut self) {
        self.next = None;
        self.concealed = 0;
        self.last_voice_active = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(seq: u16) -> Vec<u8> {
        seq.to_be_bytes().to_vec()
    }

    /// Pushes `seqs` at a steady 20 ms pace (timestamps match).
    fn feed(jb: &mut JitterBuffer, seqs: impl IntoIterator<Item = u16>, start_ms: f64) {
        for seq in seqs {
            let t = start_ms + f64::from(seq) * 20.0;
            jb.push(seq, u32::from(seq) * 960, true, packet(seq), t);
        }
    }

    fn played(pop: Pop) -> Option<u16> {
        match pop {
            Pop::Packet(p) => Some(u16::from_be_bytes([p[0], p[1]])),
            _ => None,
        }
    }

    #[test]
    fn buffers_to_the_target_then_plays_in_order() {
        let mut jb = JitterBuffer::default();
        jb.push(0, 0, true, packet(0), 0.0);
        jb.push(1, 960, true, packet(1), 20.0);
        // 60 ms = 3 frames needed.
        assert_eq!(jb.pop(), Pop::Idle);
        jb.push(2, 1920, true, packet(2), 40.0);
        assert_eq!(played(jb.pop()), Some(0));
        assert_eq!(played(jb.pop()), Some(1));
        assert_eq!(played(jb.pop()), Some(2));
    }

    #[test]
    fn reorders_packets() {
        let mut jb = JitterBuffer::default();
        for seq in [1u16, 0, 3, 2, 4] {
            jb.push(seq, u32::from(seq) * 960, true, packet(seq), 0.0);
        }
        let order: Vec<_> = (0..5).filter_map(|_| played(jb.pop())).collect();
        assert_eq!(order, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn loss_offers_fec_from_the_next_packet() {
        let mut jb = JitterBuffer::default();
        feed(&mut jb, [0, 1, 3, 4], 0.0);
        assert_eq!(played(jb.pop()), Some(0));
        assert_eq!(played(jb.pop()), Some(1));
        assert_eq!(
            jb.pop(),
            Pop::Lost {
                fec: Some(packet(3))
            }
        );
        assert_eq!(played(jb.pop()), Some(3));
        assert_eq!(jb.lost, 1);
        // Two missing in a row: the first has no FEC available.
        feed(&mut jb, [7], 0.0);
        assert_eq!(played(jb.pop()), Some(4));
        assert_eq!(jb.pop(), Pop::Lost { fec: None });
        assert_eq!(
            jb.pop(),
            Pop::Lost {
                fec: Some(packet(7))
            }
        );
        assert_eq!(played(jb.pop()), Some(7));
    }

    #[test]
    fn late_packets_and_duplicates_are_dropped() {
        let mut jb = JitterBuffer::default();
        feed(&mut jb, [0, 2, 3], 0.0);
        assert_eq!(played(jb.pop()), Some(0));
        assert!(matches!(jb.pop(), Pop::Lost { .. }));
        // 1 arrives after its playout time.
        assert!(!jb.push(1, 960, true, packet(1), 100.0));
        assert_eq!(jb.late, 1);
        assert!(!jb.push(3, 2880, true, packet(3), 100.0));
        assert_eq!(played(jb.pop()), Some(2));
    }

    #[test]
    fn conceals_five_frames_then_pauses_and_restarts() {
        let mut jb = JitterBuffer::default();
        feed(&mut jb, 0..3, 0.0);
        for seq in 0..3 {
            assert_eq!(played(jb.pop()), Some(seq));
        }
        for _ in 0..MAX_CONCEALED {
            assert_eq!(jb.pop(), Pop::Lost { fec: None });
        }
        assert_eq!(jb.pop(), Pop::Idle);
        assert!(!jb.is_playing());
        // The sender resumes much later with the next sequence numbers.
        feed(&mut jb, 8..11, 10_000.0);
        assert_eq!(played(jb.pop()), Some(8));
    }

    #[test]
    fn a_closed_gate_pauses_without_concealment() {
        let mut jb = JitterBuffer::default();
        feed(&mut jb, 0..2, 0.0);
        jb.push(2, 1920, false, packet(2), 40.0);
        for seq in 0..3 {
            assert_eq!(played(jb.pop()), Some(seq));
        }
        assert_eq!(jb.pop(), Pop::Idle);
    }

    #[test]
    fn sequence_numbers_wrap() {
        let mut jb = JitterBuffer::default();
        for (i, seq) in [65534u16, 65535, 0, 1].into_iter().enumerate() {
            jb.push(seq, i as u32 * 960, true, packet(seq), i as f64 * 20.0);
        }
        let order: Vec<_> = (0..4).filter_map(|_| played(jb.pop())).collect();
        assert_eq!(order, vec![65534, 65535, 0, 1]);
    }

    #[test]
    fn target_adapts_to_jitter() {
        let mut steady = JitterBuffer::default();
        feed(&mut steady, 0..200, 0.0);
        assert!(steady.target_ms() < 45.0, "{}", steady.target_ms());

        let mut jittery = JitterBuffer::default();
        for seq in 0..200u16 {
            // Arrivals off by up to ±30 ms.
            let wobble = if seq % 2 == 0 { 30.0 } else { -30.0 };
            jittery.push(
                seq,
                u32::from(seq) * 960,
                true,
                packet(seq),
                f64::from(seq) * 20.0 + wobble,
            );
        }
        assert!(jittery.target_ms() > 150.0, "{}", jittery.target_ms());
        assert!(jittery.target_ms() <= MAX_DELAY_MS);
    }

    #[test]
    fn too_much_queued_skips_ahead() {
        let mut jb = JitterBuffer::default();
        feed(&mut jb, 0..3, 0.0);
        assert_eq!(played(jb.pop()), Some(0));
        // A burst of 20 frames arrives at once.
        feed(&mut jb, 3..23, 0.0);
        let next = played(jb.pop()).unwrap();
        assert!(next > 10, "{next}");
    }
}
