//! Cheap fuzzing in the normal test run (PLAN.md section 20, Phase 8): many random and mutated
//! inputs through the voice datagram parser, the gateway decoder and the markup parser must
//! never panic. Deeper coverage-guided fuzzing: the `fuzz/` crate (`cargo +nightly fuzz run …`).

use hermes_proto::gateway::ClientMessage;
use hermes_proto::markup;
use hermes_proto::voice::{AudioHeader, frame_len};

/// xorshift64*: deterministic, so a failure can be reproduced from the seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let len = self.below(max + 1);
        (0..len).map(|_| self.next() as u8).collect()
    }

    /// `seed` with a few random bytes changed, inserted or removed.
    fn mutate(&mut self, seed: &[u8]) -> Vec<u8> {
        let mut out = seed.to_vec();
        for _ in 0..=self.below(4) {
            match self.below(3) {
                0 if !out.is_empty() => {
                    let i = self.below(out.len());
                    out[i] = self.next() as u8;
                }
                1 => {
                    let i = self.below(out.len() + 1);
                    out.insert(i, self.next() as u8);
                }
                _ if !out.is_empty() => {
                    let i = self.below(out.len());
                    out.remove(i);
                }
                _ => {}
            }
        }
        out
    }
}

const ROUNDS: usize = 20_000;

#[test]
fn datagrams_never_panic() {
    let mut rng = Rng(0x5eed_0001);
    let valid = AudioHeader {
        voice_active: true,
        sender_id: 7,
        timestamp: 960,
        sequence: 3,
        audio_level: 40,
    }
    .encode(&[1, 2, 3, 4]);
    for _ in 0..ROUNDS {
        let input = if rng.below(2) == 0 {
            rng.bytes(1400)
        } else {
            rng.mutate(&valid)
        };
        if let Ok((header, payload)) = AudioHeader::decode(&input) {
            assert!(header.audio_level <= 127);
            assert!(payload.len() < input.len());
        }
        let mut prefix = [0u8; 4];
        for (p, b) in prefix.iter_mut().zip(&input) {
            *p = *b;
        }
        let _ = frame_len(prefix);
    }
}

#[test]
fn gateway_frames_never_panic() {
    let mut rng = Rng(0x5eed_0002);
    let seeds: [&[u8]; 4] = [
        br#"{"op":"identify","d":{"token":"hms_x","client":{"name":"t","version":"1"},"presence":"online"}}"#,
        br#"{"op":"resume","d":{"token":"hms_x","session_id":"abc","last_seq":5}}"#,
        br#"{"op":"heartbeat","d":{"last_seq":12}}"#,
        br#"{"op":"presence_update","d":{"status":"dnd"}}"#,
    ];
    for _ in 0..ROUNDS {
        let input = if rng.below(4) == 0 {
            rng.bytes(300)
        } else {
            let seed = seeds[rng.below(seeds.len())];
            rng.mutate(seed)
        };
        let text = String::from_utf8_lossy(&input);
        let _ = ClientMessage::decode(&text);
    }
    // Deep nesting must be rejected, not overflow the stack.
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(ClientMessage::decode(&deep).is_err());
}

#[test]
fn markup_never_panics() {
    let mut rng = Rng(0x5eed_0003);
    let alphabet = "*_~`|>[]()<@&#!:\\\n abcd🙂é https://x.y ```rust\n";
    let chars: Vec<char> = alphabet.chars().collect();
    for _ in 0..ROUNDS {
        let len = rng.below(120);
        let text: String = (0..len).map(|_| chars[rng.below(chars.len())]).collect();
        let _ = markup::parse(&text);
        let _ = markup::extract_mentions(&text);
    }
}
