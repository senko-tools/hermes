//! Makes a release's update files (see `hermes_client_core::updater`):
//!
//! ```text
//! make-update keygen
//!     prints a new signing key (keep it secret: the UPDATE_SIGNING_KEY secret of the release
//!     workflow) and its public half (HERMES_UPDATE_PUBLIC_KEY, built into the app)
//! make-update build --dir <app folder> --version <1.3.0> --out <folder> [--previous <folder>]...
//!                   [--platform windows|linux]
//!     with UPDATE_SIGNING_KEY set: every file of the app compressed, patches from the same files
//!     in each previous release's folder, and the signed update-manifest.json (Linux:
//!     update-manifest-linux.json, its assets named linux-*, beside Windows' in one release)
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey};
use hermes_client_core::updater::{
    FileEntry, MANIFEST, MANIFEST_LINUX, Manifest, PatchEntry, SIGNATURE, SIGNATURE_LINUX,
    compress_full, make_patch, sha256_hex,
};

/// What every asset name starts with: `linux-` for Linux's update files.
static PREFIX: OnceLock<&'static str> = OnceLock::new();

const LEVEL: i32 = 19;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("keygen") => keygen(),
        Some("build") => build(&args[1..]),
        _ => Err(
            "usage: make-update keygen | build --dir D --version V --out O [--previous P]..."
                .into(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("make-update: {err}");
            ExitCode::FAILURE
        }
    }
}

fn keygen() -> Result<(), String> {
    let mut seed = [0u8; 32];
    rand::fill(&mut seed);
    let key = SigningKey::from_bytes(&seed);
    let b64 = base64::engine::general_purpose::STANDARD;
    println!("UPDATE_SIGNING_KEY={}", b64.encode(seed));
    println!(
        "HERMES_UPDATE_PUBLIC_KEY={}",
        b64.encode(key.verifying_key().to_bytes())
    );
    Ok(())
}

/// Every file under `dir`, as `/`-separated relative paths, sorted.
fn files(dir: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(dir).map_err(|e| e.to_string())?;
                let parts: Vec<String> = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                out.push(parts.join("/"));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// A release asset's name for a file path (assets are one flat list).
fn asset_name(path: &str) -> String {
    format!("{}{}", PREFIX.get().unwrap_or(&""), path.replace('/', "__"))
}

fn build(args: &[String]) -> Result<(), String> {
    let mut dir = None;
    let mut version = None;
    let mut out = None;
    let mut previous: Vec<PathBuf> = Vec::new();
    let mut linux = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--dir" => dir = Some(PathBuf::from(value()?)),
            "--version" => version = Some(value()?.trim_start_matches('v').to_owned()),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--previous" => previous.push(PathBuf::from(value()?)),
            "--platform" => match value()?.as_str() {
                "windows" => linux = false,
                "linux" => linux = true,
                other => return Err(format!("unknown platform {other}")),
            },
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let (dir, version, out) = (
        dir.ok_or("--dir is missing")?,
        version.ok_or("--version is missing")?,
        out.ok_or("--out is missing")?,
    );
    let seed = std::env::var("UPDATE_SIGNING_KEY").map_err(|_| "UPDATE_SIGNING_KEY is not set")?;
    let seed: [u8; 32] = base64::engine::general_purpose::STANDARD
        .decode(seed.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or("UPDATE_SIGNING_KEY is not a base64 key of 32 bytes")?;
    let key = SigningKey::from_bytes(&seed);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;

    if linux {
        let _ = PREFIX.set("linux-");
    }
    let (manifest_name, signature_name) = if linux {
        (MANIFEST_LINUX, SIGNATURE_LINUX)
    } else {
        (MANIFEST, SIGNATURE)
    };
    let manifest = make_manifest(&dir, version, &previous, &out)?;
    let json = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    let signature = base64::engine::general_purpose::STANDARD.encode(key.sign(&json).to_bytes());
    std::fs::write(out.join(manifest_name), &json).map_err(|e| e.to_string())?;
    std::fs::write(out.join(signature_name), signature).map_err(|e| e.to_string())?;
    eprintln!(
        "{} files, {} patches",
        manifest.files.len(),
        manifest.patches.len()
    );
    Ok(())
}

/// One file of the app: its bytes and the previous versions to patch from (distinct, and
/// different from it), in `--previous` order.
struct Source {
    path: String,
    bytes: Vec<u8>,
    sha: String,
    olds: Vec<(String, Vec<u8>)>,
}

/// A piece of compression work: a file's full `.zst`, or a patch to it from an old version.
enum Job {
    Full(usize),
    Patch(usize, usize),
}

/// Compresses every file of `dir` and makes its patches into `out`. The work runs on all cores,
/// but the manifest lists everything in the order a serial run would (files sorted, patches in
/// `--previous` order), so the result does not depend on timing.
fn make_manifest(
    dir: &Path,
    version: String,
    previous: &[PathBuf],
    out: &Path,
) -> Result<Manifest, String> {
    let mut sources = Vec::new();
    for path in files(dir)? {
        let bytes = std::fs::read(dir.join(&path)).map_err(|e| format!("{path}: {e}"))?;
        let sha = sha256_hex(&bytes);
        let mut olds: Vec<(String, Vec<u8>)> = Vec::new();
        for prev in previous {
            let Ok(old) = std::fs::read(prev.join(&path)) else {
                continue;
            };
            let from = sha256_hex(&old);
            if from != sha && olds.iter().all(|(f, _)| *f != from) {
                olds.push((from, old));
            }
        }
        sources.push(Source {
            path,
            bytes,
            sha,
            olds,
        });
    }

    // Biggest first, so the long jobs do not start last.
    let mut jobs: Vec<Job> = Vec::new();
    for (i, s) in sources.iter().enumerate() {
        jobs.push(Job::Full(i));
        jobs.extend((0..s.olds.len()).map(|j| Job::Patch(i, j)));
    }
    jobs.sort_by_key(|job| {
        let (Job::Full(i) | Job::Patch(i, _)) = *job;
        std::cmp::Reverse(sources[i].bytes.len())
    });

    let fulls: Vec<OnceLock<Vec<u8>>> = sources.iter().map(|_| OnceLock::new()).collect();
    let patches: Vec<Vec<OnceLock<Vec<u8>>>> = sources
        .iter()
        .map(|s| s.olds.iter().map(|_| OnceLock::new()).collect())
        .collect();
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(jobs.len())
        .max(1);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| -> std::io::Result<()> {
                    while let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                        match *job {
                            Job::Full(i) => {
                                let _ = fulls[i].set(compress_full(&sources[i].bytes, LEVEL)?);
                            }
                            Job::Patch(i, j) => {
                                let (old, new) = (&sources[i].olds[j].1, &sources[i].bytes);
                                let _ = patches[i][j].set(make_patch(old, new, LEVEL)?);
                            }
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        workers.into_iter().try_for_each(|w| {
            w.join()
                .map_err(|_| "a compression thread panicked".to_owned())?
                .map_err(|e| e.to_string())
        })
    })?;

    let mut manifest = Manifest {
        version,
        files: Vec::new(),
        patches: Vec::new(),
    };
    for (i, s) in sources.into_iter().enumerate() {
        let packed = fulls[i].get().ok_or("a file was not compressed")?;
        let full = format!("{}.zst", asset_name(&s.path));
        std::fs::write(out.join(&full), packed).map_err(|e| e.to_string())?;
        for (j, (from, _)) in s.olds.into_iter().enumerate() {
            let patch = patches[i][j].get().ok_or("a patch was not made")?;
            if patch.len() >= packed.len() {
                continue;
            }
            let asset = format!("{}.from-{}.patch", asset_name(&s.path), &from[..12]);
            std::fs::write(out.join(&asset), patch).map_err(|e| e.to_string())?;
            eprintln!(
                "{}: patch from {} is {} bytes",
                s.path,
                &from[..12],
                patch.len()
            );
            manifest.patches.push(PatchEntry {
                path: s.path.clone(),
                from,
                asset,
                size: patch.len() as u64,
            });
        }
        eprintln!(
            "{}: {} bytes, {} compressed",
            s.path,
            s.bytes.len(),
            packed.len()
        );
        manifest.files.push(FileEntry {
            path: s.path,
            size: s.bytes.len() as u64,
            sha256: s.sha,
            full,
            full_size: packed.len() as u64,
        });
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the system temp directory.
    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hermes-make-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, path: &str, bytes: &[u8]) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    }

    /// `n` numbered lines, every 97th of them altered when `changed`: compresses and patches well.
    fn text(n: usize, changed: bool) -> Vec<u8> {
        (0..n)
            .map(|i| {
                if changed && i % 97 == 0 {
                    format!("line {i} changed\n")
                } else {
                    format!("line {i} of the app\n")
                }
            })
            .collect::<String>()
            .into_bytes()
    }

    #[test]
    fn manifest_keeps_the_serial_order() {
        let root = temp_dir("order");
        let (app, old1, old2, out) = (
            root.join("app"),
            root.join("old1"),
            root.join("old2"),
            root.join("out"),
        );
        std::fs::create_dir_all(&out).unwrap();
        write(&app, "hermes.exe", &text(20_000, false));
        write(&app, "sub/b.dll", &text(5_000, false));
        write(&app, "a.txt", b"same in every version");
        write(&old1, "hermes.exe", &text(20_000, true));
        write(&old1, "a.txt", b"same in every version");
        // The same old hermes.exe again: still one patch for it.
        write(&old2, "hermes.exe", &text(20_000, true));
        write(&old2, "sub/b.dll", &text(5_000, true));

        let previous = [old1, old2];
        let manifest = make_manifest(&app, "1.0.0".into(), &previous, &out).unwrap();

        let files: Vec<&str> = manifest.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(files, ["a.txt", "hermes.exe", "sub/b.dll"]);
        let patches: Vec<&str> = manifest.patches.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(patches, ["hermes.exe", "sub/b.dll"]);
        for p in &manifest.patches {
            assert_eq!(std::fs::metadata(out.join(&p.asset)).unwrap().len(), p.size);
        }
        for f in &manifest.files {
            let packed = std::fs::read(out.join(&f.full)).unwrap();
            assert_eq!(packed.len() as u64, f.full_size);
            let original = std::fs::read(app.join(&f.path)).unwrap();
            assert_eq!(zstd::stream::decode_all(&packed[..]).unwrap(), original);
        }
        let again = make_manifest(&app, "1.0.0".into(), &previous, &out).unwrap();
        assert_eq!(
            serde_json::to_string(&manifest).unwrap(),
            serde_json::to_string(&again).unwrap()
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
