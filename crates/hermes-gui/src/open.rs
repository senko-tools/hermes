//! Opening links in the browser and saving downloads. Nothing here goes through a shell: on
//! Windows `cmd /C start` would re-parse `&` and similar characters in a URL or filename.

use std::path::{Path, PathBuf};

/// Opens an `http(s)` URL with the system's default browser.
pub fn url(url: &str) -> std::io::Result<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(std::io::Error::other(
            "only http and https links are opened",
        ));
    }
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler").arg(url);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

/// A filename that is safe to create on any platform: no directory parts, no characters Windows
/// forbids, not a reserved device name, not empty.
pub fn safe_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut out: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    out = out.trim_matches(|c: char| c == ' ' || c == '.').to_owned();
    if out.is_empty() {
        out = "download".into();
    }
    let stem = out
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        out.insert(0, '_');
    }
    out
}

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
    fn filenames_are_made_safe() {
        assert_eq!(safe_filename("cat.png"), "cat.png");
        assert_eq!(safe_filename("..\\..\\evil.exe"), "evil.exe");
        assert_eq!(safe_filename("a/b/c.txt"), "c.txt");
        assert_eq!(
            safe_filename("x&calc.exe"),
            "x&calc.exe",
            "never passed to a shell"
        );
        assert_eq!(safe_filename("what?<>|.txt"), "what____.txt");
        assert_eq!(safe_filename("CON.txt"), "_CON.txt");
        assert_eq!(safe_filename("com1"), "_com1");
        assert_eq!(safe_filename(" .. "), "download");
    }

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
    }
}
