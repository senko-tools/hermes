//! Opening links in the browser and saving downloads. Nothing here goes through a shell: on
//! Windows `cmd /C start` would re-parse `&` and similar characters in a URL or filename. On
//! Windows no program is started by name either (`rundll32`, `explorer`): Windows looks for it
//! in the application's own folder first, so a file saved next to a portable `hermes.exe` (in
//! Downloads, say) could stand in for it. `ShellExecuteW` is called instead.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Whether `url` is a web link that is safe to hand to the system: `http(s)://`, then no
/// whitespace, control characters, quotes, backslashes or other characters a URL never has
/// unencoded (a launcher could read them as something else).
pub fn is_web_link(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    rest.is_some_and(|rest| {
        !rest.is_empty()
            && !rest.chars().any(|c| {
                c.is_whitespace()
                    || c.is_control()
                    || matches!(
                        c,
                        '"' | '\'' | '<' | '>' | '\\' | '^' | '`' | '{' | '|' | '}'
                    )
            })
    })
}

/// Opens an `http(s)` URL with the system's default browser.
pub fn url(url: &str) -> std::io::Result<()> {
    if !is_web_link(url) {
        return Err(std::io::Error::other(
            "only plain http and https links are opened",
        ));
    }
    launch(OsStr::new(url))
}

/// Opens a folder in the system's file manager.
pub fn path(dir: &Path) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Err(std::io::Error::other("not a folder"));
    }
    launch(dir.as_os_str())
}

/// `ShellExecuteW(open)`: the default browser for a URL, the file manager for a folder.
#[cfg(windows)]
#[allow(unsafe_code)]
fn launch(target: &OsStr) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{PCWSTR, w};

    let wide: Vec<u16> = target.encode_wide().chain([0]).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call; the other
    // pointers are static strings or null.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    if result.0 as usize > 32 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "could not open it (error {})",
            result.0 as usize
        )))
    }
}

/// `open` (macOS) or `xdg-open`, found on `PATH`.
#[cfg(not(windows))]
fn launch(target: &OsStr) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

pub use hermes_client_core::files::safe_filename;

/// The Downloads folder (or the home folder, or the current one).
pub fn downloads_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|d| {
            d.download_dir()
                .map(Path::to_path_buf)
                .or_else(|| Some(d.home_dir().to_path_buf()))
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Writes `bytes` to `<dir>/<safe name>`, adding ` (1)`, ` (2)`, … instead of overwriting.
pub fn save_new(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;

    std::fs::create_dir_all(dir)?;
    let name = safe_filename(name);
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    for n in 0..1000 {
        let candidate = if n == 0 {
            dir.join(&name)
        } else {
            dir.join(format!("{stem} ({n}){ext}"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                file.write_all(bytes)?;
                drop(file);
                hermes_client_core::files::mark_downloaded(&candidate);
                return Ok(candidate);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(std::io::Error::other("too many files with that name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("hermes-gui-{}-save", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = save_new(&dir, "f.txt", b"one").unwrap();
        let b = save_new(&dir, "f.txt", b"two").unwrap();
        assert_eq!(a.file_name().unwrap(), "f.txt");
        assert_eq!(b.file_name().unwrap(), "f (1).txt");
        assert_eq!(std::fs::read(&a).unwrap(), b"one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_web_links_open() {
        assert!(url("file:///etc/passwd").is_err());
        assert!(url("javascript:alert(1)").is_err());
        assert!(is_web_link("https://example.com/a_(b)?q=1&r=%20#x"));
        assert!(is_web_link("https://例え.jp/パス"));
        for bad in [
            "https://",
            "https://a.b/\"\\..\\calc.exe",
            "https://a.b/ c",
            "https://a.b/\u{7}",
            "https://a.b/`x`",
            "https://a.b\\..\\x",
            "HTTPS://a.b",
        ] {
            assert!(!is_web_link(bad), "{bad:?}");
            assert!(url(bad).is_err(), "{bad:?}");
        }
    }
}
