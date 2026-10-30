//! Snowflake ID generator (PLAN.md section 7.2). The bit layout lives in [`hermes_proto::id`].

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use hermes_proto::id::{HERMES_EPOCH_MS, Id, MAX_SEQUENCE};

/// Worker ID of every ID in v1.
const WORKER: u16 = 0;

/// Hands out unique, strictly increasing IDs. Cheap to share: one short lock per ID.
#[derive(Debug, Default)]
pub struct Snowflake {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    /// Milliseconds since the Hermes epoch of the last ID.
    last_ms: u64,
    sequence: u16,
}

impl Snowflake {
    pub fn new() -> Self {
        Self::default()
    }

    /// A new ID for the current time.
    pub fn next(&self) -> Id {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        self.next_at(now)
    }

    /// A new ID for the Unix time `unix_ms`.
    ///
    /// IDs never go backwards: if the clock moves back, the last timestamp is reused. If all
    /// 4096 sequence numbers of a millisecond are used, the next millisecond is borrowed instead of
    /// waiting for it, so this never blocks.
    fn next_at(&self, unix_ms: u64) -> Id {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = unix_ms.saturating_sub(HERMES_EPOCH_MS);
        if now > state.last_ms {
            state.last_ms = now;
            state.sequence = 0;
        } else if state.sequence < MAX_SEQUENCE {
            state.sequence += 1;
        } else {
            state.last_ms += 1;
            state.sequence = 0;
        }
        Id::from_parts(state.last_ms, WORKER, state.sequence)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use super::*;

    const T: u64 = HERMES_EPOCH_MS + 1_000_000;

    #[test]
    fn layout_matches_time() {
        let id = Snowflake::new().next_at(T);
        assert_eq!(id.timestamp_ms(), T);
        assert_eq!(id.worker(), 0);
        assert_eq!(id.sequence(), 0);
    }

    #[test]
    fn same_millisecond_increments_sequence() {
        let ids = Snowflake::new();
        assert_eq!(ids.next_at(T).sequence(), 0);
        assert_eq!(ids.next_at(T).sequence(), 1);
        assert_eq!(ids.next_at(T).sequence(), 2);
        let later = ids.next_at(T + 1);
        assert_eq!((later.timestamp_ms(), later.sequence()), (T + 1, 0));
    }

    #[test]
    fn sequence_rollover_borrows_next_millisecond() {
        let ids = Snowflake::new();
        let mut last = ids.next_at(T);
        for _ in 0..MAX_SEQUENCE {
            last = ids.next_at(T);
        }
        assert_eq!((last.timestamp_ms(), last.sequence()), (T, MAX_SEQUENCE));
        let next = ids.next_at(T);
        assert_eq!((next.timestamp_ms(), next.sequence()), (T + 1, 0));
        // The real clock catching up continues after the borrowed millisecond.
        assert_eq!(ids.next_at(T + 1).sequence(), 1);
    }

    #[test]
    fn clock_going_backwards_stays_monotonic() {
        let ids = Snowflake::new();
        let a = ids.next_at(T);
        let b = ids.next_at(T - 5_000);
        assert!(b > a);
        assert_eq!(b.timestamp_ms(), T);
    }

    #[test]
    fn unique_and_increasing_across_threads() {
        let ids = Arc::new(Snowflake::new());
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let ids = Arc::clone(&ids);
                std::thread::spawn(move || {
                    let v: Vec<Id> = (0..10_000).map(|_| ids.next()).collect();
                    assert!(v.windows(2).all(|w| w[0] < w[1]));
                    v
                })
            })
            .collect();
        let mut all = HashSet::new();
        for handle in handles {
            for id in handle.join().unwrap() {
                assert!(all.insert(id), "duplicate {id}");
            }
        }
        assert_eq!(all.len(), 80_000);
    }
}
