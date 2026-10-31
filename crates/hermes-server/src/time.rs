//! Wall-clock helpers. Timestamps in the database and on the wire are Unix milliseconds.

use std::time::{SystemTime, UNIX_EPOCH};

pub const SECOND_MS: i64 = 1000;
pub const HOUR_MS: i64 = 60 * 60 * SECOND_MS;

/// The current Unix time in milliseconds (0 if the clock is before 1970).
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
