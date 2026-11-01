//! Crash logs (PLAN.md section 20, Phase 8): a panic writes `crash-<time>.log` with the message,
//! where it happened and a backtrace into the logs folder, so users can attach it to a bug
//! report. The previous panic hook still runs (it restores the terminal in the TUI).

use std::io::Write;
use std::path::{Path, PathBuf};

/// Installs the hook; `app` names the program in the log (`hermes`, `hermes-tui`).
pub fn install(logs_dir: PathBuf, app: &'static str) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".to_owned());
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let text = report(app, &message, &location, &backtrace.to_string());
        let written = write(&logs_dir, &text);
        // First the previous hook: in the TUI it gives the terminal back.
        previous(info);
        match written {
            Ok(path) => eprintln!("{app} crashed; details were saved to {}", path.display()),
            Err(err) => eprintln!("{app} crashed and the crash log could not be written: {err}"),
        }
    }));
}

/// The text of a crash log.
pub fn report(app: &str, message: &str, location: &str, backtrace: &str) -> String {
    format!(
        "{app} {} crashed\nsystem: {} {}\ntime (unix ms): {}\npanic: {message}\nat: {location}\n\n\
         backtrace:\n{backtrace}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        crate::core::now_ms(),
    )
}

fn write(dir: &Path, text: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("crash-{}.log", crate::core::now_ms()));
    let mut file = std::fs::File::create(&path)?;
    file.write_all(text.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_name_the_panic() {
        let text = report("hermes", "boom", "src/x.rs:3", "frames");
        assert!(text.starts_with("hermes "));
        assert!(text.contains("panic: boom\nat: src/x.rs:3"));
        assert!(text.contains("backtrace:\nframes"));
        let dir = std::env::temp_dir().join(format!("hermes-crash-test-{}", std::process::id()));
        let path = write(&dir, &text).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let _ = std::fs::remove_dir_all(dir);
    }
}
