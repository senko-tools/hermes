//! `read_marks.toml`: what I have read, kept on this device while "Sync what I have read" is off
//! (the server is not told then, so without this every channel would look unread after a
//! restart). Channel → newest read message, by instance.
//!
//! ```toml
//! ["hermes.senko.tools"]
//! "1234567890" = "1234567999"
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hermes_proto::id::Id;

use crate::config::{ConfigError, write_atomic};

pub const READ_MARKS_FILE: &str = "read_marks.toml";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LocalReadMarks {
    /// `None`: kept in memory only (tests).
    path: Option<PathBuf>,
    marks: BTreeMap<String, BTreeMap<String, String>>,
}

impl LocalReadMarks {
    pub fn in_memory() -> Self {
        LocalReadMarks::default()
    }

    /// Loads `path`; a missing or broken file has no marks.
    pub fn load(path: &Path) -> Self {
        let marks = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default();
        LocalReadMarks {
            path: Some(path.to_owned()),
            marks,
        }
    }

    /// The marks of an instance, `(channel, last read message)`.
    pub fn of(&self, inst: &str) -> Vec<(Id, Id)> {
        self.marks
            .get(inst)
            .into_iter()
            .flatten()
            .filter_map(|(channel, message)| Some((channel.parse().ok()?, message.parse().ok()?)))
            .collect()
    }

    /// Sets a mark (it may move backwards: marked unread). `false` when it was already so.
    pub fn set(&mut self, inst: &str, channel: Id, message: Id) -> bool {
        let message = message.to_string();
        let channels = self.marks.entry(inst.to_owned()).or_default();
        if channels.get(&channel.to_string()) == Some(&message) {
            return false;
        }
        channels.insert(channel.to_string(), message);
        true
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = toml::to_string(&self.marks).expect("read marks serialize");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io(parent.to_owned(), e))?;
        }
        let text = format!("# What has been read on this device, managed by Hermes.\n{text}");
        write_atomic(path, text.as_bytes()).map_err(|e| ConfigError::Io(path.to_owned(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_round_trip() {
        let dir = std::env::temp_dir().join(format!("hermes-read-marks-{}", std::process::id()));
        let path = dir.join(READ_MARKS_FILE);
        let mut marks = LocalReadMarks::load(&path);
        assert!(marks.of("a").is_empty());
        assert!(marks.set("a", Id(10), Id(100)));
        assert!(!marks.set("a", Id(10), Id(100)), "unchanged");
        assert!(marks.set("a", Id(11), Id(5)));
        marks.save().unwrap();
        let loaded = LocalReadMarks::load(&path);
        let mut of = loaded.of("a");
        of.sort();
        assert_eq!(of, vec![(Id(10), Id(100)), (Id(11), Id(5))]);
        assert!(loaded.of("b").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
