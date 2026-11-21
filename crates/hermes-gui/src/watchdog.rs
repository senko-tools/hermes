//! Freeze watchdog: a thread that notices when the UI thread stops handling messages (a deadlock
//! or an endless loop leaves no crash log) and writes `logs/freeze-YYYY-MM-DD-HHMMSS.log`, plus a
//! minidump of every thread's stack on Windows (`.dmp`, for WinDbg or Visual Studio). The UI
//! beats at least every [`BEAT_EVERY`] through a subscription; [`STUCK_AFTER`] without a beat
//! counts as frozen. A system sleep is not: the watchdog's own clock jumps then and it starts over.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How often the UI beats while idle.
pub const BEAT_EVERY: Duration = Duration::from_secs(2);
/// No beat for this long is a freeze.
const STUCK_AFTER: Duration = Duration::from_secs(10);
/// How often the watchdog looks.
const CHECK_EVERY: Duration = Duration::from_secs(1);
/// At most this many reports per run (a slow machine must not fill the disk).
const MAX_REPORTS: u32 = 3;

/// Milliseconds since [`START`] of the last beat.
static LAST_BEAT: AtomicU64 = AtomicU64::new(0);
static START: OnceLock<Instant> = OnceLock::new();
/// What the UI thread is doing: [`IDLE`], [`UPDATE`] or [`VIEW`].
static PHASE: AtomicU8 = AtomicU8::new(IDLE);

const IDLE: u8 = 0;
const UPDATE: u8 = 1;
const VIEW: u8 = 2;

fn elapsed_ms() -> u64 {
    let start = START.get_or_init(Instant::now);
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The UI thread is alive (called for every message).
pub fn beat() {
    LAST_BEAT.store(elapsed_ms(), Ordering::Relaxed);
}

/// Marks the UI thread as inside `update` until the guard drops.
pub fn in_update() -> PhaseGuard {
    PhaseGuard(PHASE.swap(UPDATE, Ordering::Relaxed))
}

/// Marks the UI thread as inside `view` until the guard drops.
pub fn in_view() -> PhaseGuard {
    PhaseGuard(PHASE.swap(VIEW, Ordering::Relaxed))
}

/// Restores the phase before it (`update` can call itself).
pub struct PhaseGuard(u8);

impl Drop for PhaseGuard {
    fn drop(&mut self) {
        PHASE.store(self.0, Ordering::Relaxed);
    }
}

/// Starts the watchdog; reports go to `logs_dir`.
pub fn start(logs_dir: PathBuf) {
    beat();
    let spawned = std::thread::Builder::new()
        .name("hermes-watchdog".into())
        .spawn(move || watch(&logs_dir));
    if let Err(err) = spawned {
        tracing::warn!("could not start the freeze watchdog: {err}");
    }
}

fn watch(logs_dir: &Path) {
    let mut reports = 0;
    // The beat a report was written for: one report per freeze.
    let mut reported: Option<u64> = None;
    let mut last_check = elapsed_ms();
    loop {
        std::thread::sleep(CHECK_EVERY);
        let now = elapsed_ms();
        let slept = now.saturating_sub(last_check);
        last_check = now;
        let beat = LAST_BEAT.load(Ordering::Relaxed);
        if slept > CHECK_EVERY.as_millis() as u64 * 5 {
            // The computer slept (or this thread was starved): the UI had no chance to beat.
            LAST_BEAT.store(now, Ordering::Relaxed);
            continue;
        }
        if let Some(at) = reported
            && at != beat
        {
            let secs = (beat.saturating_sub(at)) / 1000;
            tracing::warn!("the UI responds again after about {secs} s");
            reported = None;
        }
        let stuck = now.saturating_sub(beat);
        if stuck < STUCK_AFTER.as_millis() as u64 || reported.is_some() || reports >= MAX_REPORTS {
            continue;
        }
        reported = Some(beat);
        reports += 1;
        let phase = match PHASE.load(Ordering::Relaxed) {
            UPDATE => "handling a message (update)",
            VIEW => "building the window (view)",
            _ => "outside update and view (drawing, or the window system)",
        };
        report(logs_dir, stuck / 1000, phase);
    }
}

fn report(dir: &Path, secs: u64, phase: &str) {
    use hermes_client_core::{logs, now_ms, time};

    let _ = std::fs::create_dir_all(dir);
    let (now, offset) = (now_ms(), time::local_offset_secs());
    let dump = write_dump(&logs::report_path(dir, "freeze", "dmp", now, offset));
    write_report(dir, secs, phase, &dump, now, offset);
}

/// Writes the text of a freeze report, naming the dump.
fn write_report(
    dir: &Path,
    secs: u64,
    phase: &str,
    dump: &std::io::Result<Option<PathBuf>>,
    now: i64,
    offset: i64,
) {
    use hermes_client_core::{logs, time};

    let dump_line = match dump {
        Ok(Some(path)) => format!("minidump: {}", path.display()),
        Ok(None) => "minidump: not available on this system".to_owned(),
        Err(err) => format!("minidump: could not be written: {err}"),
    };
    let text = format!(
        "hermes {} froze\nsystem: {} {}\ntime: {}\nthe window has not responded for {secs} s\n\
         the UI thread was {phase}\n{dump_line}\n\n\
         Hermes keeps running; voice may still work. Attach this file and the .dmp to a bug \
         report (the .dmp can contain parts of memory such as your session token: sign out and \
         in again after sharing it).\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        time::format(now, "%Y-%m-%d %H:%M:%S", offset),
    );
    let path = logs::report_path(dir, "freeze", "log", now, offset);
    match std::fs::write(&path, text) {
        Ok(()) => tracing::error!(
            "the UI has not responded for {secs} s ({phase}); details in {}",
            path.display()
        ),
        Err(err) => tracing::error!("the UI has not responded for {secs} s ({phase}): {err}"),
    }
}

/// Writes a minidump of this process (every thread's stack) to `path`.
#[cfg(windows)]
fn write_dump(path: &Path) -> std::io::Result<Option<PathBuf>> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Diagnostics::Debug::{
        MiniDumpNormal, MiniDumpWithThreadInfo, MiniDumpWriteDump,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId};

    let file = std::fs::File::create(path)?;
    // SAFETY: the process handle is the current-process pseudo handle and the file handle
    // belongs to `file`, which outlives the call; no optional parameters are passed.
    #[allow(unsafe_code)]
    let written = unsafe {
        MiniDumpWriteDump(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            HANDLE(file.as_raw_handle()),
            MiniDumpNormal | MiniDumpWithThreadInfo,
            None,
            None,
            None,
        )
    };
    drop(file);
    match written {
        Ok(()) => Ok(Some(path.to_owned())),
        Err(err) => {
            let _ = std::fs::remove_file(path);
            Err(std::io::Error::other(err.to_string()))
        }
    }
}

#[cfg(not(windows))]
fn write_dump(_path: &Path) -> std::io::Result<Option<PathBuf>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_name_the_phase_and_the_dump() {
        let dir = std::env::temp_dir().join(format!("hermes-watchdog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dump = Ok(Some(dir.join("freeze.dmp")));
        write_report(
            &dir,
            12,
            "handling a message (update)",
            &dump,
            hermes_client_core::now_ms(),
            0,
        );
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let log = names
            .iter()
            .find(|n| n.ends_with(".log"))
            .expect("a report");
        assert!(log.starts_with("freeze-"));
        let text = std::fs::read_to_string(dir.join(log)).unwrap();
        assert!(text.contains("has not responded for 12 s"), "{text}");
        assert!(text.contains("handling a message (update)"), "{text}");
        assert!(text.contains("freeze.dmp"), "{text}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A dump suspends every thread of the process, and that can deadlock while other threads
    /// run (it froze about one test run in four), so this only runs on request and alone:
    /// `cargo test -p hermes-gui -- --ignored a_real_dump --test-threads=1`.
    #[test]
    #[ignore = "suspends the whole process: run it alone"]
    fn a_real_dump_is_written() {
        let dir = std::env::temp_dir().join(format!("hermes-dump-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        report(&dir, 12, "handling a message (update)");
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        if cfg!(windows) {
            assert!(names.iter().any(|n| n.ends_with(".dmp")), "{names:?}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
