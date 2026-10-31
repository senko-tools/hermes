//! Local time for message timestamps without a date/time crate: the UTC offset is read once at
//! startup (`date +%z` on unix, `GetTimeZoneInformation` on Windows), and `[buffer]
//! timestamp_format` supports `%H %M %S %d %m %Y %y %%`.

/// The local UTC offset in seconds (0 if it cannot be determined).
pub fn local_offset_secs() -> i64 {
    platform_offset().unwrap_or(0)
}

#[cfg(unix)]
fn platform_offset() -> Option<i64> {
    let out = std::process::Command::new("date")
        .arg("+%z")
        .output()
        .ok()?;
    parse_offset(String::from_utf8_lossy(&out.stdout).trim())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn platform_offset() -> Option<i64> {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut info = TIME_ZONE_INFORMATION::default();
    // SAFETY: `info` is a valid, writable TIME_ZONE_INFORMATION for the call's duration.
    let id = unsafe { GetTimeZoneInformation(&mut info) };
    if id == u32::MAX {
        return None;
    }
    // 2 = TIME_ZONE_ID_DAYLIGHT. Bias: UTC = local + bias (minutes).
    let extra = if id == 2 {
        info.DaylightBias
    } else {
        info.StandardBias
    };
    Some(-i64::from(info.Bias + extra) * 60)
}

#[cfg(not(any(unix, windows)))]
fn platform_offset() -> Option<i64> {
    None
}

/// Parses `+0200` / `-0530` (the output of `date +%z`).
#[cfg_attr(not(unix), allow(dead_code))]
pub fn parse_offset(s: &str) -> Option<i64> {
    let (sign, digits) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i64 = digits[..2].parse().ok()?;
    let minutes: i64 = digits[2..].parse().ok()?;
    Some(sign * (hours * 3600 + minutes * 60))
}

/// `(year, month, day)` from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Local calendar day of a Unix-ms timestamp, for day separators.
pub fn day(unix_ms: i64, offset_secs: i64) -> i64 {
    (unix_ms / 1000 + offset_secs).div_euclid(86_400)
}

pub fn format(unix_ms: i64, pattern: &str, offset_secs: i64) -> String {
    let secs = unix_ms.div_euclid(1000) + offset_secs;
    let (y, mo, d) = civil(secs.div_euclid(86_400));
    let in_day = secs.rem_euclid(86_400);
    let (h, mi, s) = (in_day / 3600, in_day % 3600 / 60, in_day % 60);
    let mut out = String::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('H') => out.push_str(&format!("{h:02}")),
            Some('M') => out.push_str(&format!("{mi:02}")),
            Some('S') => out.push_str(&format!("{s:02}")),
            Some('d') => out.push_str(&format!("{d:02}")),
            Some('m') => out.push_str(&format!("{mo:02}")),
            Some('Y') => out.push_str(&y.to_string()),
            Some('y') => out.push_str(&format!("{:02}", y.rem_euclid(100))),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        // 2026-10-31 13:57:05 UTC
        let ms = 1_793_455_025_000;
        assert_eq!(format(ms, "%Y-%m-%d %H:%M:%S", 0), "2026-10-31 13:57:05");
        assert_eq!(format(ms, "%H:%M", 2 * 3600), "15:57");
        assert_eq!(format(ms, "%d.%m.%y %%", -14 * 3600), "30.10.26 %");
        assert_eq!(format(0, "%Y-%m-%d", 0), "1970-01-01");
        assert_eq!(day(ms, 0) + 1, day(ms + 11 * 3600 * 1000, 0));
    }

    #[test]
    fn offsets() {
        assert_eq!(parse_offset("+0200"), Some(7200));
        assert_eq!(parse_offset("-0530"), Some(-19800));
        assert_eq!(parse_offset("0200"), None);
        assert_eq!(parse_offset("+2"), None);
    }
}
