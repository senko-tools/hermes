//! `known_hosts.toml`: pinned certificate fingerprints of self-signed instances (PLAN.md
//! section 13).
//!
//! ```toml
//! [hosts]
//! "hermes.example.org:7700" = "sha256:3fa9…"
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::address::{Fingerprint, HostPort};
use crate::config::{ConfigError, write_atomic};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownHosts {
    /// `None`: kept in memory only (tests).
    path: Option<PathBuf>,
    hosts: BTreeMap<String, Fingerprint>,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    hosts: BTreeMap<String, Fingerprint>,
}

impl KnownHosts {
    pub fn in_memory() -> Self {
        KnownHosts::default()
    }

    /// Loads `path`; a missing file is an empty list.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let hosts = match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str::<File>(&text)
                    .map_err(|e| ConfigError::Parse(path.to_owned(), e.to_string()))?
                    .hosts
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(err) => return Err(ConfigError::Io(path.to_owned(), err)),
        };
        Ok(KnownHosts {
            path: Some(path.to_owned()),
            hosts,
        })
    }

    pub fn get(&self, host: &HostPort) -> Option<Fingerprint> {
        self.hosts.get(&host.authority()).copied()
    }

    /// Pins `fingerprint` for `host` (replacing an older pin) and saves the file.
    pub fn trust(&mut self, host: &HostPort, fingerprint: Fingerprint) -> Result<(), ConfigError> {
        self.hosts.insert(host.authority(), fingerprint);
        self.save()
    }

    pub fn forget(&mut self, host: &HostPort) -> Result<(), ConfigError> {
        if self.hosts.remove(&host.authority()).is_some() {
            self.save()?;
        }
        Ok(())
    }

    fn save(&self) -> Result<(), ConfigError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = toml::to_string(&File {
            hosts: self.hosts.clone(),
        })
        .expect("known hosts serialize");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io(parent.to_owned(), e))?;
        }
        let text = format!("# Pinned certificate fingerprints, managed by Hermes.\n{text}");
        write_atomic(path, text.as_bytes()).map_err(|e| ConfigError::Io(path.to_owned(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_saves_and_loads() {
        let dir = std::env::temp_dir().join(format!("hermes-cc-kh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("known_hosts.toml");
        let host = HostPort::parse("[::1]:9000").unwrap();
        let mut known = KnownHosts::load(&path).unwrap();
        assert_eq!(known.get(&host), None);
        known.trust(&host, Fingerprint([7; 32])).unwrap();
        let loaded = KnownHosts::load(&path).unwrap();
        assert_eq!(loaded.get(&host), Some(Fingerprint([7; 32])));
        known.forget(&host).unwrap();
        assert_eq!(KnownHosts::load(&path).unwrap().get(&host), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
