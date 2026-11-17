//! Saving downloaded attachments safely. Filenames come from other users (through the server),
//! so they are cleaned before touching the disk, and only files that open in a viewer are ever
//! opened automatically.

/// Characters that reorder or hide text (bidirectional controls, zero-width marks). In a
/// filename they make `invoice<U+202E>fdp.exe` look like `invoiceexe.pdf`.
fn is_hidden_format(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
    )
}

/// The longest name [`safe_filename`] gives, in bytes. File systems take 255, and Windows wants
/// the whole path under 260 characters; an encrypted file's name is not limited by the server.
const MAX_NAME_BYTES: usize = 150;
/// The longest extension kept whole when a name is shortened.
const MAX_EXTENSION_BYTES: usize = 16;

/// `name` cut to [`MAX_NAME_BYTES`], keeping its extension.
fn shorten(name: &str) -> String {
    if name.len() <= MAX_NAME_BYTES {
        return name.to_owned();
    }
    let cut = |text: &str, room: usize| {
        let mut end = room.min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].trim_end_matches([' ', '.']).to_owned()
    };
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= MAX_EXTENSION_BYTES => {
            format!("{}.{ext}", cut(stem, MAX_NAME_BYTES - ext.len() - 1))
        }
        // No extension worth keeping. The `_` keeps the cut from ending in one it never had
        // (`a.exeeee…` must not become `a.exe`).
        _ => format!("{}_", cut(name, MAX_NAME_BYTES - 1)),
    }
}

/// A filename that is safe to create on any platform: no directory parts, no characters Windows
/// forbids, no characters that disguise the name, not a reserved device name, not empty.
pub fn safe_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut out: String = base
        .chars()
        .map(|c| {
            if c.is_control()
                || is_hidden_format(c)
                || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    out = shorten(out.trim_matches(|c: char| c == ' ' || c == '.'));
    if out.is_empty() {
        out = "download".into();
    }
    // Windows ignores trailing spaces and everything after the first dot when it matches
    // device names ("CON .txt", "COM¹.log").
    let stem = out
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|n| {
            let mut chars = n.chars();
            matches!(
                (chars.next(), chars.next()),
                (Some('0'..='9' | '¹' | '²' | '³'), None)
            )
        })
    };
    let reserved = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || numbered("COM")
        || numbered("LPT");
    if reserved {
        out.insert(0, '_');
    }
    out
}

/// Extensions of files that open in a viewer (pictures, sound, video, PDF, plain text) and do
/// not run code through their default application. Only these are opened automatically; every
/// other file is saved and left alone, so a new kind of dangerous file is never opened by
/// mistake (a list of dangerous extensions is never complete).
const VIEWER_EXTENSIONS: [&str; 20] = [
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "avif", "mp3", "ogg", "opus", "wav", "flac", "m4a",
    "aac", "mp4", "webm", "mkv", "mov", "pdf", "txt",
];

/// Whether a downloaded file may be opened with the system's default application.
pub fn opens_in_viewer(name: &str) -> bool {
    let name = safe_filename(name).to_ascii_lowercase();
    name.rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && VIEWER_EXTENSIONS.contains(&ext))
}

/// Marks a downloaded file as coming from the internet (the `Zone.Identifier` stream Windows
/// browsers write), so SmartScreen and Office's Protected View still check it if it is opened
/// by hand later. Elsewhere, and if the volume has no streams (FAT), it does nothing.
pub fn mark_downloaded(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        if let Err(err) = std::fs::write(&stream, "[ZoneTransfer]\r\nZoneId=3\r\n") {
            tracing::debug!("could not mark {} as downloaded: {err}", path.display());
        }
    }
    #[cfg(not(windows))]
    let _ = path;
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
        assert_eq!(safe_filename("CON .txt"), "_CON .txt");
        assert_eq!(safe_filename("conout$.log"), "_conout$.log");
        assert_eq!(safe_filename("COM¹.txt"), "_COM¹.txt");
        assert_eq!(safe_filename("COM10.txt"), "COM10.txt");
        assert_eq!(safe_filename("console.txt"), "console.txt");
        assert_eq!(safe_filename(" .. "), "download");
        assert_eq!(
            safe_filename("invoice\u{202E}fdp.exe"),
            "invoice_fdp.exe",
            "a right-to-left override cannot disguise the extension"
        );
        assert_eq!(safe_filename("a\u{200B}.png"), "a_.png");
        assert_eq!(safe_filename("tab\tname.txt"), "tab_name.txt");
    }

    #[test]
    fn long_names_are_cut_keeping_the_extension() {
        let long = format!("{}.png", "ü".repeat(200));
        let safe = safe_filename(&long);
        assert!(safe.len() <= MAX_NAME_BYTES, "{}", safe.len());
        assert!(safe.ends_with("ü.png"));
        assert!(opens_in_viewer(&long));

        // An extension too long to keep is not turned into a shorter one.
        let tricky = format!("{}.exe{}", "a".repeat(140), "e".repeat(100));
        let safe = safe_filename(&tricky);
        assert!(safe.len() <= MAX_NAME_BYTES);
        assert!(!safe.ends_with(".exe"), "{safe}");
        assert!(!opens_in_viewer(&tricky));
        let tricky = format!("{}.png{}", "a".repeat(146), "x".repeat(100));
        assert!(!opens_in_viewer(&tricky), "not a picture");

        // Nothing changes for names that fit.
        let fits = format!("{}.txt", "a".repeat(MAX_NAME_BYTES - 4));
        assert_eq!(safe_filename(&fits), fits);
    }

    #[test]
    fn only_viewer_files_open() {
        for name in [
            "cat.PNG",
            "song.mp3",
            "clip.webm",
            "report.pdf",
            "notes.txt",
        ] {
            assert!(opens_in_viewer(name), "{name}");
        }
        for name in [
            "x&calc.exe",
            "SETUP.MSI",
            "run.bat",
            "link.lnk",
            "a.ps1",
            "trick.exe. ",
            "help.chm",
            "disk.iso",
            "script.py",
            "addin.xll",
            "macro.docm",
            "lib.library-ms",
            "page.html",
            "image.svg",
            "table.csv",
            "photo.png.exe",
            "notes.txt\u{202E}exe.scr",
            "archive.zip",
            ".png",
            "png",
        ] {
            assert!(!opens_in_viewer(name), "{name}");
        }
    }

    #[test]
    fn marking_never_fails() {
        let dir = std::env::temp_dir().join(format!("hermes-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.txt");
        std::fs::write(&file, "x").unwrap();
        mark_downloaded(&file);
        assert_eq!(std::fs::read(&file).unwrap(), b"x");
        #[cfg(windows)]
        {
            let mut stream = file.as_os_str().to_owned();
            stream.push(":Zone.Identifier");
            if let Ok(text) = std::fs::read_to_string(&stream) {
                assert!(text.contains("ZoneId=3"));
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
