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
    /// A credential store that cannot be read, for tests.
    #[cfg(test)]
    Failing,
}

/// Secrets went to the file because there is no credential store.
static FALLBACK_USED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the user still has to be told that secrets are in a file.
pub(crate) fn take_fallback_warning() -> bool {
    FALLBACK_USED.swap(false, std::sync::atomic::Ordering::Relaxed)
}

fn account(instance: &str, username: &str) -> String {
    format!("{username}@{instance}")
}

impl TokenStore {
    pub fn memory() -> Self {
        TokenStore::Memory(Mutex::new(BTreeMap::new()))
    }

    pub fn get(&self, instance: &str, username: &str) -> Option<String> {
        self.read(instance, username).ok().flatten()
    }

    /// What is kept under this name. `Ok(None)` means nothing is; an error means the credential
    /// store could not say, so something may well be there. What cannot be replaced must not be
    /// made anew on an error.
    pub fn read(&self, instance: &str, username: &str) -> Result<Option<String>, String> {
        let account = account(instance, username);
        match self {
            TokenStore::Keyring { fallback } => {
                match keyring::Entry::new(SERVICE, &account).and_then(|e| e.get_password()) {
                    Ok(secret) => Ok(Some(secret)),
                    Err(err) => match (read_file(fallback).remove(&account), err) {
                        (Some(secret), _) => Ok(Some(secret)),
                        (None, keyring::Error::NoEntry) => Ok(None),
                        (None, err) => Err(err.to_string()),
                    },
                }
            }
            TokenStore::File(path) => Ok(read_file(path).remove(&account)),
            TokenStore::Memory(map) => Ok(map
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&account)
                .cloned()),
            #[cfg(test)]
            TokenStore::Failing => Err("the credential store is locked".to_owned()),
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
                        FALLBACK_USED.store(true, std::sync::atomic::Ordering::Relaxed);
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
            #[cfg(test)]
            TokenStore::Failing => Err(std::io::Error::other("locked")),
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
                map.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&account);
                Ok(())
            }
            #[cfg(test)]
            TokenStore::Failing => Err(std::io::Error::other("locked")),
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
    )?;
    #[cfg(windows)]
    restrict(path)?;
    Ok(())
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

/// Only the current user, also where the folder is not private (portable mode). On Unix the
/// file is written owner-only from its first byte.
#[cfg(windows)]
fn restrict(path: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(user) = std::env::var("USERNAME") else {
        return Ok(());
    };
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("{user}:F"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
    if !status.is_ok_and(|s| s.success()) {
        tracing::warn!("could not restrict {} to this user", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_that_cannot_be_read_is_not_an_empty_one() {
        let store = TokenStore::Failing;
        assert!(store.read("example.com", "alice").is_err());
        let store = TokenStore::memory();
        assert_eq!(store.read("example.com", "alice"), Ok(None));
    }

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
                store.read(&format!("host{i}"), "alice"),
                Ok(Some(expected)),
                "no write is lost"
            );
        }
        assert_eq!(store.read("nowhere", "alice"), Ok(None));
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
