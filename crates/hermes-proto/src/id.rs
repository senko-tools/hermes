//! Snowflake IDs (PLAN.md section 7.2).
//!
//! ```text
//! bits 63..22 (42 bits): milliseconds since the Hermes epoch 2026-01-01T00:00:00Z
//! bits 21..12 (10 bits): worker id (always 0 in v1)
//! bits 11..0  (12 bits): per-millisecond sequence
//! ```
//!
//! IDs are serialized in JSON as strings so that JavaScript clients cannot lose precision.
//! The generator lives in the server; this module only defines the layout.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The Hermes epoch, 2026-01-01T00:00:00Z, in Unix milliseconds.
pub const HERMES_EPOCH_MS: u64 = 1_767_225_600_000;

const TIMESTAMP_BITS: u32 = 42;
const WORKER_BITS: u32 = 10;
const SEQUENCE_BITS: u32 = 12;

const WORKER_SHIFT: u32 = SEQUENCE_BITS;
const TIMESTAMP_SHIFT: u32 = SEQUENCE_BITS + WORKER_BITS;

/// Largest value of each part.
pub const MAX_TIMESTAMP: u64 = (1 << TIMESTAMP_BITS) - 1;
pub const MAX_WORKER: u16 = (1 << WORKER_BITS) - 1;
pub const MAX_SEQUENCE: u16 = (1 << SEQUENCE_BITS) - 1;

/// A 64-bit snowflake ID. Ordering follows creation time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(pub u64);

impl Id {
    /// Builds an ID from its parts. Each part is masked to its bit width.
    pub const fn from_parts(ms_since_epoch: u64, worker: u16, sequence: u16) -> Self {
        Id(((ms_since_epoch & MAX_TIMESTAMP) << TIMESTAMP_SHIFT)
            | (((worker & MAX_WORKER) as u64) << WORKER_SHIFT)
            | (sequence & MAX_SEQUENCE) as u64)
    }

    /// The smallest ID that can have been created at `unix_ms`.
    /// Useful as a pagination bound ("messages after this time").
    pub const fn min_at_unix_ms(unix_ms: u64) -> Self {
        Self::from_parts(unix_ms.saturating_sub(HERMES_EPOCH_MS), 0, 0)
    }

    /// Milliseconds since the Hermes epoch.
    pub const fn ms_since_epoch(self) -> u64 {
        self.0 >> TIMESTAMP_SHIFT
    }

    /// Creation time in Unix milliseconds.
    pub const fn timestamp_ms(self) -> u64 {
        self.ms_since_epoch() + HERMES_EPOCH_MS
    }

    pub const fn worker(self) -> u16 {
        ((self.0 >> WORKER_SHIFT) as u16) & MAX_WORKER
    }

    pub const fn sequence(self) -> u16 {
        (self.0 as u16) & MAX_SEQUENCE
    }

    /// The value as stored in SQLite (`INTEGER`). Snowflakes never use bit 63 within
    /// the next ~139 years, so the conversion is lossless in practice; it is a plain bit cast.
    pub const fn to_i64(self) -> i64 {
        self.0 as i64
    }

    pub const fn from_i64(value: i64) -> Self {
        Id(value as u64)
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for Id {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Id)
    }
}

impl From<u64> for Id {
    fn from(value: u64) -> Self {
        Id(value)
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct IdVisitor;

        impl Visitor<'_> for IdVisitor {
            type Value = Id;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a snowflake ID as a decimal string")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Id, E> {
                // `u64::from_str` accepts a leading '+'; IDs must be plain digits.
                if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(E::invalid_value(de::Unexpected::Str(v), &self));
                }
                v.parse()
                    .map_err(|_| E::invalid_value(de::Unexpected::Str(v), &self))
            }
        }

        deserializer.deserialize_str(IdVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_2026_01_01() {
        // 56 years from 1970 with 14 leap days (1972..=2024).
        let days = 56 * 365 + 14;
        assert_eq!(HERMES_EPOCH_MS, days * 86_400 * 1000);
    }

    #[test]
    fn parts_round_trip() {
        let id = Id::from_parts(123_456_789, 3, 42);
        assert_eq!(id.ms_since_epoch(), 123_456_789);
        assert_eq!(id.worker(), 3);
        assert_eq!(id.sequence(), 42);
        assert_eq!(id.timestamp_ms(), HERMES_EPOCH_MS + 123_456_789);
    }

    #[test]
    fn bit_layout() {
        assert_eq!(Id::from_parts(1, 0, 0).0, 1 << 22);
        assert_eq!(Id::from_parts(0, 1, 0).0, 1 << 12);
        assert_eq!(Id::from_parts(0, 0, 1).0, 1);
        assert_eq!(
            Id::from_parts(MAX_TIMESTAMP, MAX_WORKER, MAX_SEQUENCE).0,
            u64::MAX
        );
    }

    #[test]
    fn parts_are_masked() {
        let id = Id::from_parts(0, MAX_WORKER + 1, MAX_SEQUENCE + 1);
        assert_eq!(id, Id(0));
    }

    #[test]
    fn ordering_follows_time_then_sequence() {
        let a = Id::from_parts(10, 0, 4095);
        let b = Id::from_parts(11, 0, 0);
        let c = Id::from_parts(11, 0, 1);
        assert!(a < b && b < c);
    }

    #[test]
    fn min_at_unix_ms() {
        let t = HERMES_EPOCH_MS + 5000;
        let id = Id::min_at_unix_ms(t);
        assert_eq!(id.timestamp_ms(), t);
        assert!(id <= Id::from_parts(5000, 0, 0));
        assert_eq!(Id::min_at_unix_ms(0), Id(0));
    }

    #[test]
    fn i64_round_trip() {
        let id = Id::from_parts(MAX_TIMESTAMP / 2, 0, 7);
        assert!(id.to_i64() > 0);
        assert_eq!(Id::from_i64(id.to_i64()), id);
    }

    #[test]
    fn json_is_a_string() {
        let id = Id(u64::MAX);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"18446744073709551615\"");
        assert_eq!(serde_json::from_str::<Id>(&json).unwrap(), id);
    }

    #[test]
    fn json_rejects_non_strings_and_garbage() {
        for bad in [
            "123", "\"\"", "\"+1\"", "\"-1\"", "\"12a\"", "\" 1\"", "null",
        ] {
            assert!(serde_json::from_str::<Id>(bad).is_err(), "accepted {bad}");
        }
        assert!(serde_json::from_str::<Id>("\"18446744073709551616\"").is_err());
    }
}
