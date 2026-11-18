//! Updating the desktop app in place, with as little downloaded as possible.
//!
//! A release publishes `update-manifest.json`, signed with Ed25519 (`.sig` next to it). It lists
//! every file of the app with its size and SHA-256, where to get each one whole (compressed),
//! and patches from the files of recent releases (zstd with the old file as a reference). The
//! client hashes what it has, and fetches nothing for files that did not change, a patch where
//! it has a file a patch starts from, and the whole file otherwise. Everything is checked
//! against the signed hashes before it is used.
//!
//! Installing renames the files in use to `*.old` (Windows allows that for a running program),
//! moves the new ones in and starts the new version. The new version clears `*.old` once it is
//! up; if it does not come up, the old files are moved back.

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// The manifest's name in a release, and its signature's.
pub const MANIFEST: &str = "update-manifest.json";
pub const SIGNATURE: &str = "update-manifest.json.sig";

/// Where a release's files are downloaded from: `{base}/v{version}/{asset}`.
pub const DOWNLOADS: &str = "https://github.com/senko-tools/hermes/releases/download";

/// The key releases are signed with (base64), built in by the release workflow. Without it the
/// app cannot update itself and only says that a new version exists.
pub fn public_key() -> Option<VerifyingKey> {
    let key = option_env!("HERMES_UPDATE_PUBLIC_KEY")?;
    let bytes: [u8; 32] = base64::engine::general_purpose::STANDARD
        .decode(key.trim())
        .ok()?
        .try_into()
        .ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

/// One file of the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Relative to the app's folder, with `/`.
    pub path: String,
    pub size: u64,
    /// SHA-256, lower-case hex.
    pub sha256: String,
    /// The release asset holding the whole file, zstd-compressed, and its size.
    pub full: String,
    pub full_size: u64,
}

/// A patch that makes `path` from the file whose SHA-256 is `from`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchEntry {
    pub path: String,
    pub from: String,
    pub asset: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Without the `v`.
    pub version: String,
    pub files: Vec<FileEntry>,
    #[serde(default)]
    pub patches: Vec<PatchEntry>,
}

/// Reads a manifest after checking its signature against `key`.
pub fn verify(json: &[u8], signature: &[u8], key: &VerifyingKey) -> Result<Manifest, String> {
    let signature = base64::engine::general_purpose::STANDARD
        .decode(String::from_utf8_lossy(signature).trim())
        .map_err(|_| "the update's signature is not readable".to_owned())?;
    let signature = Signature::from_slice(&signature)
        .map_err(|_| "the update's signature is not readable".to_owned())?;
    key.verify(json, &signature)
        .map_err(|_| "the update is not signed by Hermes' release key".to_owned())?;
    let manifest: Manifest =
        serde_json::from_slice(json).map_err(|e| format!("the update list is broken: {e}"))?;
    for f in &manifest.files {
        safe_path(&f.path)?;
    }
    Ok(manifest)
}

/// A manifest path that stays inside the app's folder.
fn safe_path(path: &str) -> Result<&str, String> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..");
    if bad {
        return Err(format!("the update names a file it may not: {path}"));
    }
    Ok(path)
}

/// SHA-256 of bytes, lower-case hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of a file, or `None` if it is not there.
pub fn file_sha256(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hex(&hasher.finalize()))
}

/// How one file gets up to date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum How {
    /// A patch from the file that is there now.
    Patch { asset: String, size: u64 },
    /// The whole file.
    Full { asset: String, size: u64 },
}

impl How {
    pub fn size(&self) -> u64 {
        match self {
            How::Patch { size, .. } | How::Full { size, .. } => *size,
        }
    }
}

/// What an update needs: per file that changed, how to get it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub version: String,
    pub steps: Vec<(FileEntry, How)>,
}

impl Plan {
    /// Bytes to download.
    pub fn total(&self) -> u64 {
        self.steps.iter().map(|(_, how)| how.size()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Compares the manifest with the files in `dir` (`hashes` gives a file's SHA-256, `None` if it
/// is missing). Files already right are left out.
pub fn plan(manifest: &Manifest, have: impl Fn(&str) -> Option<String>) -> Plan {
    let mut patches: BTreeMap<(&str, &str), &PatchEntry> = BTreeMap::new();
    for p in &manifest.patches {
        patches.insert((p.path.as_str(), p.from.as_str()), p);
    }
    let steps = manifest
        .files
        .iter()
        .filter_map(|f| {
            let current = have(&f.path);
            if current.as_deref() == Some(f.sha256.as_str()) {
                return None;
            }
            let how = current
                .as_deref()
                .and_then(|from| patches.get(&(f.path.as_str(), from)))
                .filter(|p| p.size < f.full_size)
                .map_or_else(
                    || How::Full {
                        asset: f.full.clone(),
                        size: f.full_size,
                    },
                    |p| How::Patch {
                        asset: p.asset.clone(),
                        size: p.size,
                    },
                );
            Some((f.clone(), how))
        })
        .collect();
    Plan {
        version: manifest.version.clone(),
        steps,
    }
}

/// The largest window a patch may need (a file up to 1 GiB).
const MAX_WINDOW_LOG: u32 = 30;

fn window_log(len: usize) -> u32 {
    (usize::BITS - len.max(1 << 20).leading_zeros()).clamp(20, MAX_WINDOW_LOG)
}

/// A patch that makes `new` out of `old` (zstd, `old` as the reference).
pub fn make_patch(old: &[u8], new: &[u8], level: i32) -> std::io::Result<Vec<u8>> {
    use zstd::stream::raw::{CParameter, Encoder};
    let mut encoder = Encoder::with_ref_prefix(level, old)?;
    encoder.set_parameter(CParameter::WindowLog(window_log(old.len().max(new.len()))))?;
    encoder.set_parameter(CParameter::EnableLongDistanceMatching(true))?;
    let mut writer = zstd::stream::zio::Writer::new(Vec::new(), encoder);
    writer.write_all(new)?;
    writer.finish()?;
    Ok(writer.into_inner().0)
}

/// The file a patch makes from `old`.
pub fn apply_patch(old: &[u8], patch: &[u8]) -> std::io::Result<Vec<u8>> {
    use zstd::stream::raw::{DParameter, Decoder};
    let mut decoder = Decoder::with_ref_prefix(old)?;
    decoder.set_parameter(DParameter::WindowLogMax(MAX_WINDOW_LOG))?;
    let mut reader = zstd::stream::zio::Reader::new(patch, decoder);
    let mut out = Vec::new();
    reader.read_to_end(&mut out)?;
    Ok(out)
}

/// A whole file, compressed for download.
pub fn compress_full(bytes: &[u8], level: i32) -> std::io::Result<Vec<u8>> {
    zstd::stream::encode_all(bytes, level)
}

fn decompress_full(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut decoder = zstd::stream::read::Decoder::new(bytes)?;
    decoder.window_log_max(MAX_WINDOW_LOG)?;
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

/// What the updater is doing, for its window.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    Checking,
    /// `file` of the update, bytes done of all bytes.
    Downloading {
        version: String,
        done: u64,
        total: u64,
    },
    Patching {
        file: String,
    },
    Verifying,
    Installing,
    Restarting,
    UpToDate,
    Failed(String),
}

/// Where downloads of `version` wait until installed.
pub fn staging_dir(version: &str) -> Option<PathBuf> {
    let base = directories::ProjectDirs::from("tools", "senko", "Hermes")?;
    Some(base.cache_dir().join("update").join(version))
}

/// The folder the running app is in.
pub fn install_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

fn http() -> Result<reqwest::Client, String> {
    let config =
        crate::tls::client_config(crate::tls::Trust::Ca, true).map_err(|e| e.to_string())?;
    reqwest::Client::builder()
        .use_preconfigured_tls((*config).clone())
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("hermes-client/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// Whether this build listens to `HERMES_UPDATE_URL` and `HERMES_UPDATE_KEY`: debug builds, and
/// release builds made with `HERMES_UPDATE_TESTING=1` set at build time (to try updating on one
/// PC, see docs/desktop-client.md). Official releases never set it, so their updates only come
/// from GitHub and only with the built-in key.
pub const TESTING: bool = cfg!(debug_assertions) || option_env!("HERMES_UPDATE_TESTING").is_some();

/// The test server's address, when testing.
fn test_url() -> Option<String> {
    if !TESTING {
        return None;
    }
    let url = std::env::var("HERMES_UPDATE_URL").ok()?;
    Some(url.trim_end_matches('/').to_owned())
}

/// Where a release's files are: the test server when testing, else GitHub.
fn base_url(version: &str) -> String {
    if let Some(url) = test_url() {
        return url;
    }
    format!("{DOWNLOADS}/v{version}")
}

/// When testing: the version the test server offers, checked like a real update. `None` when
/// not testing, so the update check asks GitHub as usual.
pub async fn test_check(current: &str) -> Option<Result<Option<crate::update::Update>, String>> {
    let base = test_url()?;
    Some(
        async {
            let key = key().ok_or("HERMES_UPDATE_KEY is not a base64 public key")?;
            let http = http()?;
            let json = get_all(&http, &format!("{base}/{MANIFEST}")).await?;
            let signature = get_all(&http, &format!("{base}/{SIGNATURE}")).await?;
            let manifest = verify(&json, &signature, &key)?;
            let newer = match (
                crate::update::Version::parse(&manifest.version),
                crate::update::Version::parse(current),
            ) {
                (Some(offered), Some(current)) => offered > current,
                _ => false,
            };
            tracing::info!(
                "update test server {base} offers {} (newer: {newer})",
                manifest.version
            );
            Ok(newer.then(|| crate::update::Update {
                version: manifest.version,
                url: base.clone(),
            }))
        }
        .await,
    )
}

/// The key to check with: `HERMES_UPDATE_KEY` (base64) when [`TESTING`], else the built-in one.
fn key() -> Option<VerifyingKey> {
    if TESTING && let Ok(text) = std::env::var("HERMES_UPDATE_KEY") {
        let bytes: [u8; 32] = base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .ok()?
            .try_into()
            .ok()?;
        return VerifyingKey::from_bytes(&bytes).ok();
    }
    public_key()
}

/// Whether this build can update itself (it has a release key, and knows its folder).
pub fn available() -> bool {
    key().is_some() && install_dir().is_some()
}

async fn get_all(http: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let res = http.get(url).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("{url} answered {}", res.status()));
    }
    Ok(res.bytes().await.map_err(|e| e.to_string())?.to_vec())
}

/// Fetches and checks `version`'s manifest, and works out what this install needs.
pub async fn prepare(version: &str) -> Result<Plan, String> {
    let key = key().ok_or("this build has no release key")?;
    let dir = install_dir().ok_or("the app's folder is unknown")?;
    let http = http()?;
    let base = base_url(version);
    let json = get_all(&http, &format!("{base}/{MANIFEST}")).await?;
    let signature = get_all(&http, &format!("{base}/{SIGNATURE}")).await?;
    let manifest = verify(&json, &signature, &key)?;
    tokio::task::spawn_blocking(move || plan(&manifest, |path| file_sha256(&dir.join(path))))
        .await
        .map_err(|e| e.to_string())
}

/// Downloads what `plan` needs into its staging folder and rebuilds every changed file there,
/// checked against the manifest. `progress` hears about every stage. Downloads that were cut
/// off go on from where they stopped.
pub async fn download(
    plan: &Plan,
    progress: impl Fn(Stage) + Send + Sync,
) -> Result<PathBuf, String> {
    let staging = staging_dir(&plan.version).ok_or("no place to keep the download")?;
    let install = install_dir().ok_or("the app's folder is unknown")?;
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let http = http()?;
    let base = base_url(&plan.version);
    let total = plan.total();
    let mut done = 0u64;
    let report = |done: u64| {
        progress(Stage::Downloading {
            version: plan.version.clone(),
            done,
            total,
        });
    };
    report(0);
    for (file, how) in &plan.steps {
        let asset = match how {
            How::Patch { asset, .. } | How::Full { asset, .. } => asset,
        };
        let part = staging.join(format!("{asset}.part"));
        let have = std::fs::metadata(&part).map_or(0, |m| m.len());
        let start = done;
        let mut got = have.min(how.size());
        report(start + got);
        if got < how.size() {
            let mut req = http.get(format!("{base}/{asset}"));
            if got > 0 {
                req = req.header("Range", format!("bytes={got}-"));
            }
            let mut res = req.send().await.map_err(|e| e.to_string())?;
            let resumed = res.status() == reqwest::StatusCode::PARTIAL_CONTENT;
            if !res.status().is_success() {
                return Err(format!("{asset}: the server answered {}", res.status()));
            }
            let mut out = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(resumed)
                .truncate(!resumed)
                .open(&part)
                .map_err(|e| e.to_string())?;
            if !resumed {
                got = 0;
            }
            while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
                out.write_all(&chunk).map_err(|e| e.to_string())?;
                got += chunk.len() as u64;
                report(start + got.min(how.size()));
            }
        }
        done = start + how.size();
        progress(Stage::Patching {
            file: file.path.clone(),
        });
        let bytes = std::fs::read(&part).map_err(|e| e.to_string())?;
        let built = match how {
            How::Patch { .. } => {
                std::fs::read(install.join(&file.path)).and_then(|old| apply_patch(&old, &bytes))
            }
            How::Full { .. } => decompress_full(&bytes),
        };
        let ok = built.as_ref().is_ok_and(|b| sha256_hex(b) == file.sha256);
        let _ = std::fs::remove_file(&part);
        if !ok {
            return Err(format!(
                "{} did not come out as the release has it; try again",
                file.path
            ));
        }
        let target = staging.join(&file.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&target, built.unwrap_or_default()).map_err(|e| e.to_string())?;
    }
    progress(Stage::Verifying);
    for (file, _) in &plan.steps {
        if file_sha256(&staging.join(&file.path)).as_deref() != Some(file.sha256.as_str()) {
            return Err(format!("{} changed after it was downloaded", file.path));
        }
    }
    Ok(staging)
}

/// The marker the new version deletes once it is up.
const STARTED_MARKER: &str = ".update-pending";

/// Moves the staged files of `plan` into the app's folder, keeping the old ones as `*.old`.
/// Undone if anything fails on the way.
pub fn install(plan: &Plan, staging: &Path) -> Result<(), String> {
    let dir = install_dir().ok_or("the app's folder is unknown")?;
    let mut moved: Vec<PathBuf> = Vec::new();
    let undo = |moved: &[PathBuf]| {
        for target in moved.iter().rev() {
            let _ = std::fs::remove_file(target);
            let _ = std::fs::rename(old_name(target), target);
        }
    };
    for (file, _) in &plan.steps {
        let target = dir.join(&file.path);
        let old = old_name(&target);
        let _ = std::fs::remove_file(&old);
        if target.exists()
            && let Err(err) = std::fs::rename(&target, &old)
        {
            undo(&moved);
            return Err(format!("{} is in use ({err})", file.path));
        }
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let staged = staging.join(&file.path);
        if let Err(err) = std::fs::rename(&staged, &target)
            .or_else(|_| std::fs::copy(&staged, &target).map(|_| ()))
        {
            moved.push(target);
            undo(&moved);
            return Err(format!("{} could not be put in place ({err})", file.path));
        }
        moved.push(target);
    }
    let _ = std::fs::write(dir.join(STARTED_MARKER), &plan.version);
    let _ = std::fs::remove_dir_all(staging);
    Ok(())
}

fn old_name(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

/// Every `*.old` the last update left in `dir`.
fn old_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "old") {
                out.push(path);
            }
        }
    }
    out
}

/// The new version is up: the old files and the marker go. The watcher still runs from
/// `hermes.exe.old` for up to half a second after the marker goes, and Windows does not delete a
/// running program, so what is still in use is tried again for a little while.
pub fn finish_started() {
    let Some(dir) = install_dir() else { return };
    if std::fs::remove_file(dir.join(STARTED_MARKER)).is_ok() {
        tracing::info!("update installed");
    }
    let mut left: Vec<PathBuf> = old_files(&dir)
        .into_iter()
        .filter(|old| std::fs::remove_file(old).is_err())
        .collect();
    if left.is_empty() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("update-cleanup".into())
        .spawn(move || {
            for _ in 0..40 {
                std::thread::sleep(Duration::from_millis(250));
                left.retain(|old| std::fs::remove_file(old).is_err() && old.exists());
                if left.is_empty() {
                    return;
                }
            }
            tracing::info!(
                "could not remove {} old file(s) of the last update",
                left.len()
            );
        });
}

/// Whether the new version has said it is up.
pub fn new_version_started() -> bool {
    install_dir().is_none_or(|dir| !dir.join(STARTED_MARKER).exists())
}

/// The new version did not come up: the old files go back.
pub fn roll_back() {
    let Some(dir) = install_dir() else { return };
    for old in old_files(&dir) {
        let target = old.with_extension("");
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::rename(&old, &target);
    }
    let _ = std::fs::remove_file(dir.join(STARTED_MARKER));
}

/// Run by the old program (as `hermes.exe.old --update-watch`): starts the new version and
/// waits for it to say it is up. If it ends or hangs before that, the old files go back and the
/// old version starts again.
pub const WATCH_FLAG: &str = "--update-watch";
/// Given to the new version by the watcher: the old one may still be closing.
pub const AFTER_FLAG: &str = "--after-update";

/// How long a new version gets to come up.
const START_WITHIN: Duration = Duration::from_secs(60);

pub fn watch(args: &[std::ffi::OsString]) {
    let Some(dir) = install_dir() else { return };
    let Ok(me) = std::env::current_exe() else {
        return;
    };
    // `hermes.exe.old` starts `hermes.exe`.
    let new_exe = me.with_extension("");
    let new_exe = if new_exe.extension().is_some() {
        new_exe
    } else {
        dir.join(me.file_stem().unwrap_or_default())
    };
    let Ok(mut child) = std::process::Command::new(&new_exe)
        .arg(AFTER_FLAG)
        .args(args)
        .spawn()
    else {
        roll_back();
        let _ = std::process::Command::new(&new_exe).args(args).spawn();
        return;
    };
    let started = std::time::Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(500));
        if new_version_started() {
            return;
        }
        let ended = matches!(child.try_wait(), Ok(Some(_)));
        if ended || started.elapsed() >= START_WITHIN {
            let _ = child.kill();
            let _ = child.wait();
            roll_back();
            let _ = std::process::Command::new(&new_exe).args(args).spawn();
            return;
        }
    }
}

/// After [`install`]: starts the watcher, which starts the new version. The caller quits next.
pub fn restart(args: &[std::ffi::OsString]) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let watcher = old_name(&me);
    // The program itself did not change: a copy of it watches (and is cleared up the same way).
    if !watcher.exists() {
        std::fs::copy(&me, &watcher).map_err(|e| format!("could not restart: {e}"))?;
    }
    std::process::Command::new(watcher)
        .arg(WATCH_FLAG)
        .args(args)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not restart: {e}"))
}

/// `340 KB / 2.1 MB` as `0.3 / 2.1 MB`: both numbers in the total's unit, so they read
/// together.
pub fn human_progress(done: u64, total: u64) -> String {
    const UNITS: [(&str, f64); 4] = [
        ("GB", 1024.0 * 1024.0 * 1024.0),
        ("MB", 1024.0 * 1024.0),
        ("KB", 1024.0),
        ("B", 1.0),
    ];
    let (unit, size) = UNITS
        .into_iter()
        .find(|(_, size)| total as f64 >= *size)
        .unwrap_or(("B", 1.0));
    let (d, t) = (done.min(total) as f64 / size, total as f64 / size);
    match unit {
        "B" => format!("{} / {} B", done.min(total), total),
        "KB" => format!("{d:.0} / {t:.0} KB"),
        _ if t >= 100.0 => format!("{d:.0} / {t:.0} {unit}"),
        _ if t >= 10.0 => format!("{d:.1} / {t:.1} {unit}"),
        _ => format!("{d:.2} / {t:.2} {unit}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer as _, SigningKey};

    fn entry(path: &str, data: &[u8]) -> FileEntry {
        FileEntry {
            path: path.into(),
            size: data.len() as u64,
            sha256: sha256_hex(data),
            full: format!("{path}.zst"),
            full_size: 1000,
        }
    }

    #[test]
    fn plans_only_what_changed_and_prefers_patches() {
        let (a_old, a_new, b, c) = (b"a1".as_slice(), b"a2".as_slice(), b"b".as_slice(), b"c");
        let manifest = Manifest {
            version: "1.3.0".into(),
            files: vec![
                entry("hermes.exe", a_new),
                entry("b.dll", b),
                entry("c.txt", c),
            ],
            patches: vec![PatchEntry {
                path: "hermes.exe".into(),
                from: sha256_hex(a_old),
                asset: "hermes.exe.from-x.patch".into(),
                size: 40,
            }],
        };
        let have = |path: &str| match path {
            "hermes.exe" => Some(sha256_hex(a_old)),
            "b.dll" => Some(sha256_hex(b)),
            _ => None,
        };
        let plan = plan(&manifest, have);
        assert_eq!(plan.steps.len(), 2, "b.dll is already right");
        assert_eq!(
            plan.steps[0].1,
            How::Patch {
                asset: "hermes.exe.from-x.patch".into(),
                size: 40
            }
        );
        assert!(
            matches!(plan.steps[1].1, How::Full { .. }),
            "c.txt is missing"
        );
        assert_eq!(plan.total(), 1040);
    }

    #[test]
    fn patches_rebuild_the_new_file() {
        let old: Vec<u8> = (0..200_000u32).flat_map(u32::to_le_bytes).collect();
        let mut new = old.clone();
        new[400_000..400_100].fill(7);
        new.extend_from_slice(b"tail");
        let patch = make_patch(&old, &new, 19).unwrap();
        assert!(patch.len() < new.len() / 50, "patch {} bytes", patch.len());
        assert_eq!(apply_patch(&old, &patch).unwrap(), new);
        // From another file it makes something else (the hash check then refuses it).
        let other = vec![3u8; old.len()];
        assert!(apply_patch(&other, &patch).map_or(true, |out| out != new));
        let full = compress_full(&new, 3).unwrap();
        assert_eq!(decompress_full(&full).unwrap(), new);
    }

    #[test]
    fn manifests_need_the_release_signature() {
        let signing = SigningKey::from_bytes(&[7; 32]);
        let manifest = Manifest {
            version: "1.3.0".into(),
            files: vec![entry("hermes.exe", b"x")],
            patches: vec![],
        };
        let json = serde_json::to_vec(&manifest).unwrap();
        let sig = base64::engine::general_purpose::STANDARD.encode(signing.sign(&json).to_bytes());
        let key = signing.verifying_key();
        assert_eq!(verify(&json, sig.as_bytes(), &key).unwrap(), manifest);
        let mut tampered = json.clone();
        tampered[5] ^= 1;
        assert!(verify(&tampered, sig.as_bytes(), &key).is_err());
        let other = SigningKey::from_bytes(&[8; 32]).verifying_key();
        assert!(verify(&json, sig.as_bytes(), &other).is_err());

        let evil = Manifest {
            files: vec![entry("../evil.exe", b"x")],
            ..manifest
        };
        let json = serde_json::to_vec(&evil).unwrap();
        let sig = base64::engine::general_purpose::STANDARD.encode(signing.sign(&json).to_bytes());
        assert!(
            verify(&json, sig.as_bytes(), &key).is_err(),
            "paths stay inside"
        );
    }

    #[test]
    fn progress_reads_in_the_totals_unit() {
        assert_eq!(human_progress(340 * 1024, 2_202_009), "0.33 / 2.10 MB");
        assert_eq!(human_progress(12 * 1024, 840 * 1024), "12 / 840 KB");
        assert_eq!(human_progress(500, 900), "500 / 900 B");
        assert_eq!(
            human_progress(1_342_177_280, 1_503_238_553),
            "1.25 / 1.40 GB"
        );
        assert_eq!(human_progress(5 << 20, 45 << 20), "5.0 / 45.0 MB");
        assert_eq!(human_progress(9, 5), "5 / 5 B", "never past the total");
    }
}
