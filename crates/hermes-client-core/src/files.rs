//! Saving downloaded attachments safely. Filenames come from other users (through the server),
//! so they are cleaned before touching the disk, and programs are never opened automatically.

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

/// Extensions that run code when "opened" on Windows (or are shortcuts to something that does).
const PROGRAM_EXTENSIONS: [&str; 30] = [
    "exe",
    "com",
    "bat",
    "cmd",
    "scr",
    "pif",
    "msi",
    "msp",
    "msc",
    "cpl",
    "dll",
    "lnk",
    "url",
    "ps1",
    "psm1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "hta",
    "jar",
    "reg",
    "inf",
    "scf",
    "appref-ms",
    "application",
    "gadget",
    "sh",
];

/// Whether opening the file with the system's default application would run a program. Such
/// files are saved but never opened automatically.
pub fn is_program(name: &str) -> bool {
    let name = name.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| PROGRAM_EXTENSIONS.contains(&ext))
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
    fn programs_are_recognized() {
        for name in [
            "x&calc.exe",
            "SETUP.MSI",
            "run.bat",
            "link.lnk",
            "a.ps1",
            "trick.exe. ",
        ] {
            assert!(is_program(name), "{name}");
        }
        for name in ["cat.png", "notes.txt", "exe", "archive.zip", "report.pdf"] {
            assert!(!is_program(name), "{name}");
        }
    }
}
