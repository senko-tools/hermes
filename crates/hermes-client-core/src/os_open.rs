//! Handing links, folders and files to the system: the browser, the file manager, the
//! default application. Nothing here goes through a shell: on Windows `cmd /C start` would
//! re-parse `&` and similar characters in a URL or filename. On Windows no program is started
//! by name either (`rundll32`, `explorer`): Windows looks for it in the application's own folder
//! first, so a file saved next to a portable `hermes.exe` (in Downloads, say) could stand in
//! for it. `ShellExecuteW` is called instead.

use std::ffi::OsStr;
use std::path::Path;

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

/// Opens a downloaded file with its default application. Callers check
/// [`crate::files::opens_in_viewer`] first: a program or script is never opened this way.
pub fn file(file: &Path) -> std::io::Result<()> {
    if !file.is_file() {
        return Err(std::io::Error::other("not a file"));
    }
    launch(file.as_os_str())
}

/// Opens a folder in the system's file manager.
pub fn path(dir: &Path) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Err(std::io::Error::other("not a folder"));
    }
    launch(dir.as_os_str())
}

/// `ShellExecuteW(open)`: the default browser for a URL, the file manager for a folder.
///
/// It runs on a thread of its own. Called on the UI thread from a click, ShellExecute pumps
/// window messages while it talks to Explorer (always for a folder), gpui re-enters its own
/// event handling while the click still holds the app, and the app panicked ("RefCell already
/// borrowed", 2026-11-18, Open themes folder). Its own thread also gets COM set up the way
/// ShellExecute asks for. A failure is logged, since the click has returned by then.
#[cfg(windows)]
fn launch(target: &OsStr) -> std::io::Result<()> {
    let wide: Vec<u16> = {
        use std::os::windows::ffi::OsStrExt;
        target.encode_wide().chain([0]).collect()
    };
    std::thread::Builder::new()
        .name("shell-open".into())
        .spawn(move || {
            if let Err(err) = shell_execute(&wide) {
                tracing::warn!("{err}");
            }
        })
        .map(|_| ())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn shell_execute(wide: &[u16]) -> std::io::Result<()> {
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{PCWSTR, w};

    // SAFETY: called once on this fresh thread, balanced by CoUninitialize below when it worked.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
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
    if com.is_ok() {
        // SAFETY: pairs the successful CoInitializeEx on this thread.
        unsafe { CoUninitialize() };
    }
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
    let mut child = std::process::Command::new(opener)
        .arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    // Waited for elsewhere, so it does not linger as a zombie until Hermes quits. `xdg-open` can
    // stay as long as the app it started (its generic fallback runs the browser in the
    // foreground), so not on the caller's thread.
    let _ = std::thread::Builder::new()
        .name("open-wait".into())
        .spawn(move || {
            if let Ok(status) = child.wait()
                && !status.success()
            {
                tracing::warn!("{opener} failed: {status}");
            }
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
