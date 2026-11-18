//! Makes a release's update files (see `hermes_client_core::updater`):
//!
//! ```text
//! make-update keygen
//!     prints a new signing key (keep it secret: the UPDATE_SIGNING_KEY secret of the release
//!     workflow) and its public half (HERMES_UPDATE_PUBLIC_KEY, built into the app)
//! make-update build --dir <app folder> --version <1.3.0> --out <folder> [--previous <folder>]...
//!     with UPDATE_SIGNING_KEY set: every file of the app compressed, patches from the same files
//!     in each previous release's folder, and the signed update-manifest.json
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey};
use hermes_client_core::updater::{
    FileEntry, MANIFEST, Manifest, PatchEntry, SIGNATURE, compress_full, make_patch, sha256_hex,
};

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
    path.replace('/', "__")
}

fn build(args: &[String]) -> Result<(), String> {
    let mut dir = None;
    let mut version = None;
    let mut out = None;
    let mut previous: Vec<PathBuf> = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--dir" => dir = Some(PathBuf::from(value()?)),
            "--version" => version = Some(value()?.trim_start_matches('v').to_owned()),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--previous" => previous.push(PathBuf::from(value()?)),
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

    let mut manifest = Manifest {
        version,
        files: Vec::new(),
        patches: Vec::new(),
    };
    for path in files(&dir)? {
        let bytes = std::fs::read(dir.join(&path)).map_err(|e| format!("{path}: {e}"))?;
        let sha = sha256_hex(&bytes);
        let full = format!("{}.zst", asset_name(&path));
        let packed = compress_full(&bytes, LEVEL).map_err(|e| e.to_string())?;
        std::fs::write(out.join(&full), &packed).map_err(|e| e.to_string())?;
        for prev in &previous {
            let Ok(old) = std::fs::read(prev.join(&path)) else {
                continue;
            };
            let from = sha256_hex(&old);
            if from == sha
                || manifest
                    .patches
                    .iter()
                    .any(|p| p.path == path && p.from == from)
            {
                continue;
            }
            let patch = make_patch(&old, &bytes, LEVEL).map_err(|e| e.to_string())?;
            if patch.len() >= packed.len() {
                continue;
            }
            let asset = format!("{}.from-{}.patch", asset_name(&path), &from[..12]);
            std::fs::write(out.join(&asset), &patch).map_err(|e| e.to_string())?;
            eprintln!(
                "{path}: patch from {} is {} bytes",
                &from[..12],
                patch.len()
            );
            manifest.patches.push(PatchEntry {
                path: path.clone(),
                from,
                asset,
                size: patch.len() as u64,
            });
        }
        eprintln!("{path}: {} bytes, {} compressed", bytes.len(), packed.len());
        manifest.files.push(FileEntry {
            path,
            size: bytes.len() as u64,
            sha256: sha,
            full,
            full_size: packed.len() as u64,
        });
    }
    let json = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    let signature = base64::engine::general_purpose::STANDARD.encode(key.sign(&json).to_bytes());
    std::fs::write(out.join(MANIFEST), &json).map_err(|e| e.to_string())?;
    std::fs::write(out.join(SIGNATURE), signature).map_err(|e| e.to_string())?;
    eprintln!(
        "{} files, {} patches",
        manifest.files.len(),
        manifest.patches.len()
    );
    Ok(())
}
