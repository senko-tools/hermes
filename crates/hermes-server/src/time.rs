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

/// `YYYY-MM-DD` (UTC) of a Unix millisecond time.
pub fn format_date(unix_ms: i64) -> String {
    // Howard Hinnant's days-to-civil algorithm.
    let days = unix_ms.div_euclid(24 * HOUR_MS);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(951_782_400_000), "2000-02-29");
        assert_eq!(format_date(1_793_491_200_000), "2026-11-01");
    }
}
