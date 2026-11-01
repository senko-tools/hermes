//! Session token storage (PLAN.md section 13): the OS credential store through `keyring`, with a
//! user-only file as the fallback when no credential store is available.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::config::write_atomic;

/// Keyring service name; the account is `<username>@<instance id>`.
const SERVICE: &str = "hermes";

#[derive(Debug)]
pub enum TokenStore {
    /// OS credential store, falling back to the file when it fails.
    Keyring { fallback: PathBuf },
    /// Only the file (`tokens.toml`, user-only permissions).
    File(PathBuf),
    /// In memory, for tests.
    Memory(Mutex<BTreeMap<String, String>>),
}

fn account(instance: &str, username: &str) -> String {
    format!("{username}@{instance}")
}

impl TokenStore {
    pub fn memory() -> Self {
        TokenStore::Memory(Mutex::new(BTreeMap::new()))
    }

    pub fn get(&self, instance: &str, username: &str) -> Option<String> {
        let account = account(instance, username);
        match self {
            TokenStore::Keyring { fallback } => keyring::Entry::new(SERVICE, &account)
                .and_then(|e| e.get_password())
                .ok()
                .or_else(|| read_file(fallback).remove(&account)),
            TokenStore::File(path) => read_file(path).remove(&account),
            TokenStore::Memory(map) => map.lock().expect("token lock").get(&account).cloned(),
        }
    }

    pub fn set(&self, instance: &str, username: &str, token: &str) -> std::io::Result<()> {
        let account = account(instance, username);
        match self {
            TokenStore::Keyring { fallback } => {
                match keyring::Entry::new(SERVICE, &account).and_then(|e| e.set_password(token)) {
                    Ok(()) => {
                        // Never leave an older copy in the file.
                        remove_from_file(fallback, &account)
                    }
                    Err(err) => {
                        tracing::warn!(
                            "credential store unavailable ({err}); using the token file"
                        );
                        write_to_file(fallback, &account, token)
                    }
                }
            }
            TokenStore::File(path) => write_to_file(path, &account, token),
            TokenStore::Memory(map) => {
                map.lock()
                    .expect("token lock")
                    .insert(account, token.to_owned());
                Ok(())
            }
        }
    }

    pub fn delete(&self, instance: &str, username: &str) -> std::io::Result<()> {
        let account = account(instance, username);
        match self {
            TokenStore::Keyring { fallback } => {
                if let Ok(entry) = keyring::Entry::new(SERVICE, &account) {
                    let _ = entry.delete_credential();
                }
                remove_from_file(fallback, &account)
            }
            TokenStore::File(path) => remove_from_file(path, &account),
            TokenStore::Memory(map) => {
                map.lock().expect("token lock").remove(&account);
                Ok(())
            }
        }
    }
}

fn read_file(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_file(path: &Path, tokens: &BTreeMap<String, String>) -> std::io::Result<()> {
    if tokens.is_empty() {
        return match std::fs::remove_file(path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string(tokens).map_err(std::io::Error::other)?;
    write_atomic(path, text.as_bytes())?;
    restrict(path)
}

fn write_to_file(path: &Path, account: &str, token: &str) -> std::io::Result<()> {
    let mut tokens = read_file(path);
    tokens.insert(account.to_owned(), token.to_owned());
    write_file(path, &tokens)
}

fn remove_from_file(path: &Path, account: &str) -> std::io::Result<()> {
    let mut tokens = read_file(path);
    if tokens.remove(account).is_some() {
        write_file(path, &tokens)?;
    }
    Ok(())
}

/// Owner-only permissions. On Windows the per-user config directory is already private.
#[cfg(unix)]
fn restrict(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store() {
        let dir = std::env::temp_dir().join(format!("hermes-cc-tokens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = TokenStore::File(dir.join("tokens.toml"));
        assert_eq!(store.get("example.com", "alice"), None);
        store.set("example.com", "alice", "hms_a").unwrap();
        store.set("example.com:9000", "alice", "hms_b").unwrap();
        assert_eq!(store.get("example.com", "alice").as_deref(), Some("hms_a"));
        assert_eq!(
            store.get("example.com:9000", "alice").as_deref(),
            Some("hms_b")
        );
        store.delete("example.com", "alice").unwrap();
        assert_eq!(store.get("example.com", "alice"), None);
        store.delete("example.com:9000", "alice").unwrap();
        assert!(!dir.join("tokens.toml").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn memory_store() {
        let store = TokenStore::memory();
        store.set("a", "b", "t").unwrap();
        assert_eq!(store.get("a", "b").as_deref(), Some("t"));
        store.delete("a", "b").unwrap();
        assert_eq!(store.get("a", "b"), None);
    }
}
