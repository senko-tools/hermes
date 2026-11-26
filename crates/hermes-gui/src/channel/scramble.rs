//! Stand-ins for messages this device cannot read yet: a made-up name and made-up text in the
//! characters encrypted text is written in. Nothing about them comes from the message but its
//! id, which only picks the lengths, so they say nothing about who wrote what. Each character
//! changes on its own every few seconds (never all at once, and not at all with reduced motion).

use std::time::Duration;

/// How often the window redraws while a stand-in is on screen.
pub const TICK: Duration = Duration::from_millis(500);

const GLYPHS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Each character keeps its look for 3 to 6 seconds.
const SHORTEST: u64 = 3_000;
const SPREAD: u64 = 3_000;

const NAME: u64 = 0x6e61_6d65;
const TEXT: u64 = 0x7465_7874;

/// SplitMix64: a fast, well-mixed hash of a number.
fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The character at `index` at `now_ms`; `still` keeps the first one.
fn glyph(seed: u64, index: usize, now_ms: u64, still: bool) -> char {
    let own = mix(seed ^ mix(index as u64));
    let period = SHORTEST + own % SPREAD;
    let phase = (own >> 20) % period;
    let turn = if still { 0 } else { (now_ms + phase) / period };
    char::from(GLYPHS[(mix(own ^ turn) % GLYPHS.len() as u64) as usize])
}

/// The word lengths of a stand-in: `count` words, each `shortest..=longest` characters.
fn words(seed: u64, count: usize, shortest: u64, longest: u64) -> Vec<usize> {
    (0..count)
        .map(|n| (shortest + mix(seed ^ (n as u64 + 1)) % (longest - shortest + 1)) as usize)
        .collect()
}

fn fill(seed: u64, lengths: &[usize], now_ms: u64, still: bool) -> String {
    let mut out = String::new();
    let mut index = 0;
    for (n, &len) in lengths.iter().enumerate() {
        if n > 0 {
            out.push(' ');
        }
        for _ in 0..len {
            out.push(glyph(seed, index, now_ms, still));
            index += 1;
        }
    }
    out
}

/// A made-up author name for message `id`: 5 to 12 characters.
pub fn name(id: u64, now_ms: u64, still: bool) -> String {
    let seed = mix(id ^ NAME);
    let len = 5 + (seed % 8) as usize;
    fill(seed, &[len], now_ms, still)
}

/// Made-up text for message `id`: 3 to 24 words of 2 to 10 characters.
pub fn text(id: u64, now_ms: u64, still: bool) -> String {
    let seed = mix(id ^ TEXT);
    let count = 3 + (seed % 22) as usize;
    fill(seed, &words(seed, count, 2, 10), now_ms, still)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_stay_while_characters_change() {
        for id in [1u64, 42, 0xdead_beef, u64::MAX] {
            let (n0, t0) = (name(id, 0, false), text(id, 0, false));
            assert!((5..=12).contains(&n0.len()), "{n0}");
            assert!(t0.split(' ').count() >= 3, "{t0}");
            let (n1, t1) = (name(id, 60_000, false), text(id, 60_000, false));
            assert_eq!((n0.len(), t0.len()), (n1.len(), t1.len()));
            // The spaces stay where they are; a minute later most characters are new.
            let same = |a: &str, b: &str| a.chars().zip(b.chars()).filter(|(x, y)| x == y).count();
            assert_eq!(
                t0.match_indices(' ').collect::<Vec<_>>(),
                t1.match_indices(' ').collect::<Vec<_>>()
            );
            assert!(same(&t0, &t1) < t0.len(), "{t0} / {t1}");
            // Reduced motion: nothing changes.
            assert_eq!(text(id, 0, true), text(id, 60_000, true));
        }
    }

    #[test]
    fn characters_change_one_by_one() {
        // Between two ticks only a few characters change, never the whole text at once.
        let id = 7;
        let mut most = 0;
        for tick in 0..200u64 {
            let now = tick * TICK.as_millis() as u64;
            let (a, b) = (
                text(id, now, false),
                text(id, now + TICK.as_millis() as u64, false),
            );
            let changed = a.chars().zip(b.chars()).filter(|(x, y)| x != y).count();
            most = most.max(changed * 100 / a.len());
        }
        assert!(most < 50, "up to {most}% changed in one tick");
    }

    #[test]
    fn different_messages_look_different() {
        assert_ne!(text(1, 0, true), text(2, 0, true));
        assert_ne!(name(1, 0, true), name(2, 0, true));
    }
}
