//! Session token storage (PLAN.md section 13): the OS credential store through `keyring`, with a
//! user-only file as the fallback when no credential store is available.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use crate::config::{WriteOptions, write_atomic_with};

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

/// What [`TokenStore::lookup`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Found(String),
    /// Stored nowhere.
    Missing,
    /// Not in the file, and the credential store did not answer (locked, or not running yet), so
    /// it may still be kept there.
    Unavailable,
}

fn account(instance: &str, username: &str) -> String {
    format!("{username}@{instance}")
}

/// Whether the credential store said it has no such entry, or that there is none at all (the file
/// is all there is then), rather than failing to answer.
fn certainly_missing(err: &keyring::Error) -> bool {
    matches!(
        err,
        keyring::Error::NoEntry | keyring::Error::NoDefaultStore
    )
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

    /// Like [`Self::get`], but tells "stored nowhere" apart from "the credential store did not
    /// answer", for a secret that must not be replaced just because it could not be read.
    pub fn lookup(&self, instance: &str, username: &str) -> Lookup {
        let account = account(instance, username);
        let found = |value: Option<String>| value.map_or(Lookup::Missing, Lookup::Found);
        match self {
            TokenStore::Keyring { fallback } => {
                match keyring::Entry::new(SERVICE, &account).and_then(|e| e.get_password()) {
                    Ok(value) => Lookup::Found(value),
                    Err(err) => match read_file(fallback).remove(&account) {
                        Some(value) => Lookup::Found(value),
                        None if certainly_missing(&err) => Lookup::Missing,
                        None => {
                            tracing::warn!("the credential store did not answer: {err}");
                            Lookup::Unavailable
                        }
                    },
                }
            }
            TokenStore::File(path) => found(read_file(path).remove(&account)),
            TokenStore::Memory(map) => {
                found(map.lock().expect("token lock").get(&account).cloned())
            }
        }
    }

    /// Keeps a secret in the file only, never in the credential store: for one made while the
    /// credential store did not answer, which must not replace what it may hold there.
    /// [`Self::get`] still prefers the credential store's copy.
    pub fn set_in_file(&self, instance: &str, username: &str, token: &str) -> std::io::Result<()> {
        match self {
            TokenStore::Keyring { fallback } | TokenStore::File(fallback) => {
                write_to_file(fallback, &account(instance, username), token)
            }
            TokenStore::Memory(_) => self.set(instance, username, token),
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

/// Every change of the file reads, changes and writes it whole: two at once (signing in to two
/// instances, a device key made meanwhile) would lose one of them.
static FILE_LOCK: Mutex<()> = Mutex::new(());

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
    // Private from its first byte: a file restricted only after its rename could be read by others
    // in between.
    write_atomic_with(
        path,
        text.as_bytes(),
        WriteOptions {
            durable: true,
            private: true,
        },
    )
}

fn write_to_file(path: &Path, account: &str, token: &str) -> std::io::Result<()> {
    let _lock = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let mut tokens = read_file(path);
    tokens.insert(account.to_owned(), token.to_owned());
    write_file(path, &tokens)
}

fn remove_from_file(path: &Path, account: &str) -> std::io::Result<()> {
    let _lock = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let mut tokens = read_file(path);
    if tokens.remove(account).is_some() {
        write_file(path, &tokens)?;
    }
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
    fn the_file_is_private_and_takes_writers_at_once() {
        let dir = std::env::temp_dir().join(format!("hermes-cc-tokens2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("tokens.toml");
        let store = std::sync::Arc::new(TokenStore::File(path.clone()));
        let writers: Vec<_> = (0..8)
            .map(|i| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store
                        .set(&format!("host{i}"), "alice", &format!("hms_{i}"))
                        .unwrap();
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        for i in 0..8 {
            let expected = format!("hms_{i}");
            assert_eq!(
                store.lookup(&format!("host{i}"), "alice"),
                Lookup::Found(expected),
                "no write is lost"
            );
        }
        assert_eq!(store.lookup("nowhere", "alice"), Lookup::Missing);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // Nothing half-written is left next to it.
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["tokens.toml"]);
        store.set_in_file("host0", "alice", "hms_file").unwrap();
        assert_eq!(store.get("host0", "alice").as_deref(), Some("hms_file"));
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
