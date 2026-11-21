//! Where downloads and pasted pictures go. Opening links and folders is
//! `hermes_client_core::os_open`.

use std::path::{Path, PathBuf};

pub use hermes_client_core::os_open::{path, url};

pub use hermes_client_core::files::safe_filename;

/// Where a pasted picture waits while it uploads.
pub fn paste_dir() -> PathBuf {
    std::env::temp_dir().join("hermes-gui")
}

/// Removes pasted pictures of earlier runs: nothing is using them, and a picture meant for an
/// encrypted chat should not stay readable on the disk.
pub fn clear_pasted() {
    let _ = std::fs::remove_dir_all(paste_dir());
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
}
