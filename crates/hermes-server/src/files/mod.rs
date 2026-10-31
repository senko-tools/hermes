//! Uploaded files on disk (PLAN.md sections 9.3 and 13): `<data_dir>/files/ab/cd/<id>`, sharded by
//! the first bytes of the SHA-256 hex, plus `<id>.thumb.webp` next to images.
//!
//! Uploads are streamed to `files/tmp/` while hashing, then probed: only PNG, JPEG, GIF and WebP
//! detected from the bytes count as images (dimensions + thumbnail); the client's
//! `Content-Type` never makes something an image. Originals are never modified.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use hermes_proto::id::Id;
use image::{ImageFormat, ImageReader, Limits};
use tokio::sync::Semaphore;

use crate::db;
use crate::state::AppState;

/// Subdirectory of `storage.data_dir` holding the uploads.
pub const FILES_DIR: &str = "files";
/// Unfinished uploads; emptied on start.
const TMP_DIR: &str = "tmp";
/// Thumbnails fit in this square (PLAN.md section 10).
pub const THUMB_SIZE: u32 = 400;
/// Uploads attached to nothing are deleted after this long.
pub const UNATTACHED_TTL_MS: i64 = 24 * 60 * 60 * 1000;
/// Images larger than this in either dimension are stored as plain files.
const MAX_IMAGE_SIDE: u32 = 16_384;
/// Memory one image decode may allocate.
const MAX_DECODE_ALLOC: u64 = 256 * 1024 * 1024;
/// Image decodes running at the same time (they are memory hungry).
const PARALLEL_DECODES: usize = 2;

#[derive(Debug)]
pub struct Files {
    root: PathBuf,
    /// Bytes of all stored originals, for `storage.max_total_storage_gb`.
    used: AtomicU64,
    decodes: Semaphore,
}

impl Files {
    /// Uses `<data_dir>/files`, removing leftovers of interrupted uploads.
    pub fn open(data_dir: &Path, used: u64) -> io::Result<Self> {
        let root = data_dir.join(FILES_DIR);
        let tmp = root.join(TMP_DIR);
        match std::fs::remove_dir_all(&tmp) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        std::fs::create_dir_all(&tmp)?;
        Ok(Self {
            root,
            used: AtomicU64::new(used),
            decodes: Semaphore::new(PARALLEL_DECODES),
        })
    }

    /// Where the original of upload `id` with content hash `sha256` lives.
    pub fn path(&self, sha256: &[u8; 32], id: Id) -> PathBuf {
        let hex = |b: u8| format!("{b:02x}");
        self.root
            .join(hex(sha256[0]))
            .join(hex(sha256[1]))
            .join(id.to_string())
    }

    pub fn thumb_path(&self, sha256: &[u8; 32], id: Id) -> PathBuf {
        self.path(sha256, id).with_extension("thumb.webp")
    }

    /// A temporary path for upload `id` while it is being received.
    pub fn tmp_path(&self, id: Id, suffix: &str) -> PathBuf {
        self.root.join(TMP_DIR).join(format!("{id}{suffix}"))
    }

    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }

    /// Counts `size` more bytes unless that would pass `quota` (0 = unlimited).
    pub fn reserve(&self, size: u64, quota: u64) -> bool {
        self.used
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                let new = used.saturating_add(size);
                (quota == 0 || new <= quota).then_some(new)
            })
            .is_ok()
    }

    pub fn release(&self, size: u64) {
        self.used
            .update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.saturating_sub(size)
            });
    }

    /// [`probe`] on a blocking thread, with at most [`PARALLEL_DECODES`] at a time.
    pub async fn probe(&self, file: PathBuf, thumb: Option<PathBuf>) -> Option<ImageInfo> {
        let _permit = self.decodes.acquire().await.ok()?;
        tokio::task::spawn_blocking(move || probe(&file, thumb.as_deref()))
            .await
            .ok()
            .flatten()
    }

    /// Removes an upload's files; missing files are fine.
    pub fn remove(&self, sha256: &[u8; 32], id: Id) {
        for path in [self.path(sha256, id), self.thumb_path(sha256, id)] {
            if let Err(err) = std::fs::remove_file(&path)
                && err.kind() != io::ErrorKind::NotFound
            {
                tracing::warn!("could not delete {}: {err}", path.display());
            }
        }
    }
}

/// What [`probe`] found out about an image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub content_type: &'static str,
    pub width: u32,
    pub height: u32,
    pub has_thumb: bool,
}

/// `Some` if `file` is a PNG, JPEG, GIF or WebP image (detected from its bytes) that decodes
/// within the limits. With `thumb`, a WebP thumbnail fitting in [`THUMB_SIZE`]² is written there;
/// an image that fails to decode then counts as no image at all.
pub fn probe(file: &Path, thumb: Option<&Path>) -> Option<ImageInfo> {
    let open = || -> Option<ImageReader<std::io::BufReader<std::fs::File>>> {
        let mut reader = ImageReader::open(file).ok()?.with_guessed_format().ok()?;
        let mut limits = Limits::default();
        limits.max_image_width = Some(MAX_IMAGE_SIDE);
        limits.max_image_height = Some(MAX_IMAGE_SIDE);
        limits.max_alloc = Some(MAX_DECODE_ALLOC);
        reader.limits(limits);
        Some(reader)
    };
    let reader = open()?;
    let content_type = match reader.format()? {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Gif => "image/gif",
        ImageFormat::WebP => "image/webp",
        _ => return None,
    };
    let (width, height) = reader.into_dimensions().ok()?;
    if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
        return None;
    }
    let Some(thumb) = thumb else {
        return Some(ImageInfo {
            content_type,
            width,
            height,
            has_thumb: false,
        });
    };
    // GIFs and animated WebPs decode to their first frame.
    let image = open()?.decode().ok()?;
    let image = if width > THUMB_SIZE || height > THUMB_SIZE {
        image.thumbnail(THUMB_SIZE, THUMB_SIZE)
    } else {
        image
    };
    let image = image::DynamicImage::ImageRgba8(image.to_rgba8());
    let has_thumb = match image.save_with_format(thumb, ImageFormat::WebP) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!("could not write thumbnail {}: {err}", thumb.display());
            let _ = std::fs::remove_file(thumb);
            false
        }
    };
    Some(ImageInfo {
        content_type,
        width,
        height,
        has_thumb,
    })
}

/// Deletes uploads that were not attached within [`UNATTACHED_TTL_MS`] (avatars and icons stay).
/// Returns how many were deleted.
pub async fn cleanup(state: &AppState, now: i64) -> Result<usize, sqlx::Error> {
    let expired =
        db::attachments::expired_unattached(&state.db.read, now - UNATTACHED_TTL_MS).await?;
    let mut deleted = 0;
    for (id, sha256, size) in expired {
        if db::attachments::delete_if_unused(&state.db.write, id).await? {
            state.files.remove(&sha256, id);
            state.files.release(size);
            deleted += 1;
        }
    }
    Ok(deleted)
}

/// Runs [`cleanup`] every hour, forever.
pub async fn cleanup_task(state: std::sync::Arc<AppState>) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
    loop {
        interval.tick().await;
        match cleanup(&state, crate::time::now_ms()).await {
            Ok(0) => {}
            Ok(n) => tracing::info!("deleted {n} unattached uploads"),
            Err(err) => tracing::error!("could not delete unattached uploads: {err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::TempDir;

    fn png(dir: &Path, name: &str, width: u32, height: u32) -> PathBuf {
        let path = dir.join(name);
        image::RgbImage::from_pixel(width, height, image::Rgb([200, 30, 30]))
            .save_with_format(&path, ImageFormat::Png)
            .unwrap();
        path
    }

    #[test]
    fn paths_are_sharded_by_hash() {
        let dir = TempDir::new("files-paths");
        let files = Files::open(dir.path(), 0).unwrap();
        let mut sha = [0u8; 32];
        sha[0] = 0xab;
        sha[1] = 0x0c;
        let path = files.path(&sha, Id(42));
        assert!(path.ends_with(Path::new("files").join("ab").join("0c").join("42")));
        assert!(files.thumb_path(&sha, Id(42)).ends_with("42.thumb.webp"));
    }

    #[test]
    fn open_clears_unfinished_uploads() {
        let dir = TempDir::new("files-tmp");
        let files = Files::open(dir.path(), 0).unwrap();
        std::fs::write(files.tmp_path(Id(1), ".part"), b"x").unwrap();
        let files = Files::open(dir.path(), 0).unwrap();
        assert!(!files.tmp_path(Id(1), ".part").exists());
        assert!(files.tmp_path(Id(1), ".part").parent().unwrap().is_dir());
    }

    #[test]
    fn quota_reservation() {
        let dir = TempDir::new("files-quota");
        let files = Files::open(dir.path(), 10).unwrap();
        assert!(files.reserve(5, 20));
        assert!(!files.reserve(6, 20));
        assert!(files.reserve(1_000_000, 0));
        files.release(1_000_000);
        assert_eq!(files.used(), 15);
        files.release(100);
        assert_eq!(files.used(), 0);
    }

    #[test]
    fn probe_large_png_makes_a_bounded_thumbnail() {
        let dir = TempDir::new("files-probe-large");
        let file = png(dir.path(), "big.png", 1000, 500);
        let thumb = dir.path().join("big.thumb.webp");
        let info = probe(&file, Some(&thumb)).unwrap();
        assert_eq!(
            info,
            ImageInfo {
                content_type: "image/png",
                width: 1000,
                height: 500,
                has_thumb: true,
            }
        );
        let t = image::open(&thumb).unwrap();
        assert_eq!((t.width(), t.height()), (400, 200));
        assert_eq!(
            image::guess_format(&std::fs::read(&thumb).unwrap()).unwrap(),
            ImageFormat::WebP
        );
    }

    #[test]
    fn probe_small_image_is_not_upscaled() {
        let dir = TempDir::new("files-probe-small");
        let file = png(dir.path(), "small.png", 20, 10);
        let thumb = dir.path().join("small.thumb.webp");
        probe(&file, Some(&thumb)).unwrap();
        let t = image::open(&thumb).unwrap();
        assert_eq!((t.width(), t.height()), (20, 10));
    }

    #[test]
    fn probe_without_thumbnails_still_reads_dimensions() {
        let dir = TempDir::new("files-probe-nothumb");
        let file = png(dir.path(), "a.png", 30, 40);
        let info = probe(&file, None).unwrap();
        assert_eq!((info.width, info.height, info.has_thumb), (30, 40, false));
    }

    #[test]
    fn probe_rejects_non_images_and_broken_images() {
        let dir = TempDir::new("files-probe-reject");
        let text = dir.path().join("a.txt");
        std::fs::write(&text, "hello, not an image").unwrap();
        assert_eq!(probe(&text, None), None);

        let svg = dir.path().join("a.svg");
        std::fs::write(&svg, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#).unwrap();
        assert_eq!(probe(&svg, None), None);

        // A valid PNG header with the pixel data cut off.
        let file = png(dir.path(), "cut.png", 300, 300);
        let bytes = std::fs::read(&file).unwrap();
        std::fs::write(&file, &bytes[..bytes.len() / 2]).unwrap();
        let thumb = dir.path().join("cut.thumb.webp");
        assert_eq!(probe(&file, Some(&thumb)), None);
        assert!(!thumb.exists());
    }
}
