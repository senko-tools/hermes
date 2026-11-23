//! Log files (PLAN.md section 13: rolling client logs, 7 days). The running client writes
//! `logs/latest.log`; at the next start, or when the day changes while it runs, that file moves
//! to `logs/logs_old/<app>-YYYY-MM-DD.log`. Old logs are deleted after `log_days` (7 days unless
//! set in Settings > Diagnostics), crash and freeze reports in `logs/` after 3 days.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::time;

/// Days rotated logs are kept unless the config says otherwise.
pub const DEFAULT_LOG_DAYS: u32 = 7;
/// The `log_days` a config may set.
pub const LOG_DAYS: std::ops::RangeInclusive<u32> = 1..=90;
/// Crash and freeze reports are kept this long.
pub const KEEP_REPORTS: Duration = Duration::from_secs(3 * 86_400);
/// The folder for rotated logs, inside the logs folder.
pub const OLD_DIR: &str = "logs_old";

/// The current log of `app` (`hermes` → `latest.log`, `hermes-tui` → `latest-tui.log`).
pub fn latest_name(app: &str) -> String {
    match app.strip_prefix("hermes-") {
        Some(rest) => format!("latest-{rest}.log"),
        None => "latest.log".to_owned(),
    }
}

/// The open `latest.log`; moves itself into `logs_old` when the local day changes.
pub struct DailyLog {
    dir: PathBuf,
    app: String,
    offset_secs: i64,
    day: i64,
    file: File,
}

impl DailyLog {
    /// Moves the previous run's log (and a `hermes.log` from older versions) into `logs_old`,
    /// deletes expired logs and reports, and opens a new `latest.log`.
    /// Rotated logs older than `log_days` are deleted (see [`LOG_DAYS`]).
    pub fn open(dir: &Path, app: &str, log_days: u32) -> io::Result<DailyLog> {
        Self::open_at(
            dir,
            app,
            crate::core::now_ms(),
            time::local_offset_secs(),
            log_days,
        )
    }

    fn open_at(
        dir: &Path,
        app: &str,
        now_ms: i64,
        offset_secs: i64,
        log_days: u32,
    ) -> io::Result<DailyLog> {
        fs::create_dir_all(dir.join(OLD_DIR))?;
        let latest = dir.join(latest_name(app));
        for previous in [latest.clone(), dir.join(format!("{app}.log"))] {
            if previous.is_file() {
                let written = modified_ms(&previous).unwrap_or(now_ms);
                archive(
                    dir,
                    app,
                    &previous,
                    time::day(written, offset_secs),
                    offset_secs,
                )?;
            }
        }
        clean(dir, SystemTime::now(), keep_old(log_days));
        Ok(DailyLog {
            file: create(&latest)?,
            dir: dir.to_owned(),
            app: app.to_owned(),
            offset_secs,
            day: time::day(now_ms, offset_secs),
        })
    }

    /// Starts a new `latest.log` if `now_ms` is on a later day than the open one.
    fn roll(&mut self, now_ms: i64) -> io::Result<()> {
        let today = time::day(now_ms, self.offset_secs);
        if today == self.day {
            return Ok(());
        }
        let latest = self.dir.join(latest_name(&self.app));
        self.file.flush()?;
        // std opens files with delete sharing on Windows, so the open file can be moved.
        archive(&self.dir, &self.app, &latest, self.day, self.offset_secs)?;
        self.file = create(&latest)?;
        self.day = today;
        Ok(())
    }
}

impl Write for DailyLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // A failed roll keeps writing to the old file rather than losing the line.
        let _ = self.roll(crate::core::now_ms());
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn create(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
}

fn modified_ms(path: &Path) -> Option<i64> {
    let since = fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?;
    i64::try_from(since.as_millis()).ok()
}

/// Moves `file` to `logs_old/<app>-<date of day>.log` (`-2`, `-3`, … when taken); an empty file
/// is just removed.
fn archive(dir: &Path, app: &str, file: &Path, day: i64, offset_secs: i64) -> io::Result<()> {
    if fs::metadata(file).is_ok_and(|m| m.len() == 0) {
        return fs::remove_file(file);
    }
    let date = time::format(
        day * 86_400_000 - offset_secs * 1000,
        "%Y-%m-%d",
        offset_secs,
    );
    let old = dir.join(OLD_DIR);
    let mut target = old.join(format!("{app}-{date}.log"));
    let mut n = 2;
    while target.exists() {
        target = old.join(format!("{app}-{date}-{n}.log"));
        n += 1;
    }
    fs::rename(file, target)
}

/// How long rotated logs are kept for a `log_days` setting, clamped to [`LOG_DAYS`].
pub fn keep_old(log_days: u32) -> Duration {
    let days = log_days.clamp(*LOG_DAYS.start(), *LOG_DAYS.end());
    Duration::from_secs(u64::from(days) * 86_400)
}

/// Deletes rotated logs older than `keep_old` and crash/freeze reports older than
/// [`KEEP_REPORTS`]. Best effort: a file that cannot be removed stays.
fn clean(dir: &Path, now: SystemTime, keep_old: Duration) {
    let expired = |path: &Path, keep: Duration| {
        fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > keep)
    };
    let remove = |folder: &Path, keep: Duration, wanted: &dyn Fn(&str) -> bool| {
        let Ok(entries) = fs::read_dir(folder) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_file() && wanted(&name) && expired(&path, keep) {
                let _ = fs::remove_file(&path);
            }
        }
    };
    remove(&dir.join(OLD_DIR), keep_old, &|name| name.ends_with(".log"));
    remove(dir, KEEP_REPORTS, &|name| {
        (name.starts_with("crash-") || name.starts_with("freeze-"))
            && (name.ends_with(".log") || name.ends_with(".dmp"))
    });
}

/// A free path `dir/<prefix>-YYYY-MM-DD-HHMMSS.<ext>` for a report written now (local time).
pub fn report_path(dir: &Path, prefix: &str, ext: &str, now_ms: i64, offset_secs: i64) -> PathBuf {
    let stamp = time::format(now_ms, "%Y-%m-%d-%H%M%S", offset_secs);
    let mut path = dir.join(format!("{prefix}-{stamp}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{prefix}-{stamp}-{n}.{ext}"));
        n += 1;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_MS: i64 = 86_400_000;
    // 2026-10-31 13:57:05 UTC
    const NOW: i64 = 1_793_455_025_000;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hermes-logs-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn names_per_app() {
        assert_eq!(latest_name("hermes"), "latest.log");
        assert_eq!(latest_name("hermes-tui"), "latest-tui.log");
    }

    #[test]
    fn previous_runs_move_to_logs_old_by_date() {
        let dir = dir("rotate");
        let today = time::format(crate::core::now_ms(), "%Y-%m-%d", 0);
        fs::write(dir.join("latest.log"), "run 1\n").unwrap();
        fs::write(dir.join("hermes.log"), "from an older version\n").unwrap();
        fs::write(dir.join("crash-1.log"), "kept: new").unwrap();
        let mut log = DailyLog::open_at(&dir, "hermes", NOW, 0, DEFAULT_LOG_DAYS).unwrap();
        writeln!(log, "run 2").unwrap();
        drop(log);
        assert_eq!(
            names(&dir.join(OLD_DIR)),
            [
                format!("hermes-{today}-2.log"),
                format!("hermes-{today}.log")
            ]
        );
        assert_eq!(
            fs::read_to_string(dir.join("latest.log")).unwrap(),
            "run 2\n"
        );
        assert!(dir.join("crash-1.log").exists());

        // An empty log is not kept.
        let dir2 = self::dir("empty");
        fs::write(dir2.join("latest.log"), "").unwrap();
        drop(DailyLog::open_at(&dir2, "hermes", NOW, 0, DEFAULT_LOG_DAYS).unwrap());
        assert!(names(&dir2.join(OLD_DIR)).is_empty());
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(dir2);
    }

    #[test]
    fn a_new_day_starts_a_new_file() {
        let dir = dir("roll");
        // `write` rolls by the real clock, so this starts now.
        let now = crate::core::now_ms();
        let offset = 2 * 3600;
        let today = time::format(now, "%Y-%m-%d", offset);
        let mut log = DailyLog::open_at(&dir, "hermes-tui", now, offset, DEFAULT_LOG_DAYS).unwrap();
        writeln!(log, "before midnight").unwrap();
        log.roll(now + DAY_MS).unwrap();
        log.file
            .write_all(
                b"after midnight
",
            )
            .unwrap();
        drop(log);
        let old = format!("hermes-tui-{today}.log");
        assert_eq!(names(&dir.join(OLD_DIR)), std::slice::from_ref(&old));
        assert_eq!(
            fs::read_to_string(dir.join(OLD_DIR).join(old)).unwrap(),
            "before midnight
"
        );
        assert_eq!(
            fs::read_to_string(dir.join("latest-tui.log")).unwrap(),
            "after midnight
"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn expired_files_are_deleted() {
        let dir = dir("clean");
        fs::create_dir_all(dir.join(OLD_DIR)).unwrap();
        fs::write(dir.join(OLD_DIR).join("hermes-2026-01-01.log"), "x").unwrap();
        fs::write(dir.join("crash-2026-01-01-000000.log"), "x").unwrap();
        fs::write(dir.join("notes.txt"), "not ours").unwrap();
        let week = keep_old(DEFAULT_LOG_DAYS);
        clean(
            &dir,
            SystemTime::now() + Duration::from_secs(2 * 86_400),
            week,
        );
        assert!(dir.join("crash-2026-01-01-000000.log").exists());
        assert!(!names(&dir.join(OLD_DIR)).is_empty());
        clean(
            &dir,
            SystemTime::now() + Duration::from_secs(4 * 86_400),
            week,
        );
        assert!(
            !dir.join("crash-2026-01-01-000000.log").exists(),
            "3 days for reports"
        );
        // A longer setting keeps them.
        clean(
            &dir,
            SystemTime::now() + Duration::from_secs(8 * 86_400),
            keep_old(14),
        );
        assert!(!names(&dir.join(OLD_DIR)).is_empty(), "14 days set");
        clean(
            &dir,
            SystemTime::now() + Duration::from_secs(8 * 86_400),
            week,
        );
        assert!(names(&dir.join(OLD_DIR)).is_empty(), "7 days for old logs");
        assert!(dir.join("notes.txt").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn log_days_are_clamped() {
        assert_eq!(keep_old(0), Duration::from_secs(86_400));
        assert_eq!(keep_old(7), Duration::from_secs(7 * 86_400));
        assert_eq!(keep_old(1000), Duration::from_secs(90 * 86_400));
    }

    #[test]
    fn report_paths_are_dated_and_free() {
        let dir = dir("reports");
        let first = report_path(&dir, "crash", "log", NOW, 0);
        assert_eq!(first, dir.join("crash-2026-10-31-135705.log"));
        fs::write(&first, "x").unwrap();
        assert_eq!(
            report_path(&dir, "crash", "log", NOW, 0),
            dir.join("crash-2026-10-31-135705-2.log")
        );
        assert_eq!(
            report_path(&dir, "freeze", "dmp", NOW + DAY_MS, 3600),
            dir.join("freeze-2026-11-01-145705.dmp")
        );
        let _ = fs::remove_dir_all(dir);
    }
}
