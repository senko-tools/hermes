//! `user_volumes.toml`: per-user voice volumes, by instance (PLAN.md section 17).
//!
//! ```toml
//! ["hermes.senko.tools"]
//! "1234567890" = 1.5
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hermes_proto::id::Id;

use crate::config::{ConfigError, write_atomic};

pub const USER_VOLUMES_FILE: &str = "user_volumes.toml";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserVolumes {
    /// `None`: kept in memory only (tests).
    path: Option<PathBuf>,
    volumes: BTreeMap<String, BTreeMap<String, f32>>,
}

impl UserVolumes {
    pub fn in_memory() -> Self {
        UserVolumes::default()
    }

    /// Loads `path`; a missing or broken file is an empty list (volumes are not worth failing
    /// over).
    pub fn load(path: &Path) -> Self {
        let volumes = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default();
        UserVolumes {
            path: Some(path.to_owned()),
            volumes,
        }
    }

    /// 0–2; 1 when not set.
    pub fn get(&self, inst: &str, user: Id) -> f32 {
        self.volumes
            .get(inst)
            .and_then(|users| users.get(&user.to_string()))
            .copied()
            .unwrap_or(1.0)
    }

    /// Sets (1.0 removes the entry) and saves.
    pub fn set(&mut self, inst: &str, user: Id, volume: f32) -> Result<(), ConfigError> {
        let volume = volume.clamp(0.0, hermes_audio::mixer::MAX_USER_VOLUME);
        let users = self.volumes.entry(inst.to_owned()).or_default();
        if (volume - 1.0).abs() < f32::EPSILON {
            users.remove(&user.to_string());
        } else {
            users.insert(user.to_string(), volume);
        }
        self.volumes.retain(|_, users| !users.is_empty());
        self.save()
    }

    fn save(&self) -> Result<(), ConfigError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = toml::to_string(&self.volumes).expect("volumes serialize");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io(parent.to_owned(), e))?;
        }
        let text = format!("# Per-user voice volumes, managed by Hermes.\n{text}");
        write_atomic(path, text.as_bytes()).map_err(|e| ConfigError::Io(path.to_owned(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volumes_round_trip() {
        let dir = std::env::temp_dir().join(format!("hermes-volumes-{}", std::process::id()));
        let path = dir.join(USER_VOLUMES_FILE);
        let mut volumes = UserVolumes::load(&path);
        assert_eq!(volumes.get("a", Id(1)), 1.0);
        volumes.set("a", Id(1), 1.5).unwrap();
        volumes.set("a", Id(2), 9.0).unwrap();
        let loaded = UserVolumes::load(&path);
        assert_eq!(loaded.get("a", Id(1)), 1.5);
        assert_eq!(loaded.get("a", Id(2)), 2.0);
        assert_eq!(loaded.get("b", Id(1)), 1.0);
        volumes.set("a", Id(1), 1.0).unwrap();
        volumes.set("a", Id(2), 1.0).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().lines().count() <= 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
