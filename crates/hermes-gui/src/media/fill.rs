//! Videos that came without their size, or without a poster sharp enough for their card (#76):
//! the sender had no FFmpeg, sent from the terminal client, or sent before posters existed, and
//! the server's poster is at most 400 px. When such a card is on screen, this client looks at
//! the video itself (`super::inspect`): only its start, its index and one picture are read.
//!
//! Two at a time, only for cards on screen, once per video. What was learnt is kept in memory
//! and, for files that are not end-to-end encrypted, on disk, so a restart or scrolling back
//! does not look again.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::Context;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Attachment;

use super::cards::MediaKey;
use crate::images::Img;
use crate::shell::Shell;

/// Videos looked at at once.
const AT_ONCE: usize = 2;
/// Videos whose findings stay in memory.
const KEPT: usize = 96;
/// The most the posters on disk may take; the oldest go first.
const DISK_BYTES: u64 = 64 << 20;
/// An end-to-end encrypted file is only read whole: looked at up to this size.
const WHOLE_MAX_BYTES: u64 = 16 << 20;
/// A poster this much smaller than its card is redone.
const SHARP_ENOUGH: f32 = 0.8;
/// The first line of a poster file on disk.
const MAGIC: &str = "hermes-poster-1";

/// What was learnt of one video.
pub struct Filled {
    /// Its size as shown.
    pub size: (u32, u32),
    pub duration_ms: Option<u64>,
    /// A JPEG of it, sized for its card.
    poster: Option<Arc<Vec<u8>>>,
    /// The poster's longest side.
    poster_side: u32,
}

/// A card on screen that wants its video looked at.
struct Want {
    key: MediaKey,
    attachment: Attachment,
    /// The card's largest box in device pixels: the poster is made to fit it.
    bound: (u32, u32),
}

#[derive(Default)]
pub struct Fill {
    done: HashMap<MediaKey, Filled>,
    order: VecDeque<MediaKey>,
    running: HashSet<MediaKey>,
    failed: HashSet<MediaKey>,
    /// Posters being decoded for drawing.
    decoding: HashSet<MediaKey>,
    /// Noted while the cards are drawn; read after the frame.
    wanted: RefCell<Vec<Want>>,
    /// FFmpeg cannot run here: nothing is looked at.
    no_ffmpeg: bool,
}

impl Fill {
    /// What was learnt of a video, if anything.
    pub fn get(&self, key: &MediaKey) -> Option<&Filled> {
        self.done.get(key)
    }

    /// Whether a video is being looked at now.
    pub fn running(&self, key: &MediaKey) -> bool {
        self.running.contains(key)
    }

    fn keep(&mut self, key: MediaKey, filled: Filled) {
        if self.done.insert(key.clone(), filled).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > KEPT {
            if let Some(old) = self.order.pop_front() {
                self.done.remove(&old);
            }
        }
    }
}

/// The longest side of a picture.
fn longest((w, h): (u32, u32)) -> u32 {
    w.max(h)
}

/// The size of a video's picture fitting `bound`, as a float, never larger than the video.
fn fitted((w, h): (u32, u32), (bw, bh): (u32, u32)) -> (f32, f32) {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let scale = (bw as f32 / w).min(bh as f32 / h).min(1.);
    (w * scale, h * scale)
}

impl Shell {
    /// Called as a video card that has not been played is drawn: notes it for looking at
    /// when its size is unknown or its poster is missing or too small for `bound` (the card's
    /// box in device pixels).
    pub(super) fn note_video_card(&self, key: &MediaKey, a: &Attachment, bound: (u32, u32)) {
        let fill = &self.media.fill;
        if fill.no_ffmpeg || fill.running.contains(key) || fill.failed.contains(key) {
            return;
        }
        let shown = self
            .images
            .thumbs
            .get(key)
            .and_then(|img| match img {
                Img::Ready(p) => Some(p.still.size(0)),
                _ => None,
            })
            .map(|size| longest((size.width.0 as u32, size.height.0 as u32)));
        if let Some(done) = fill.done.get(key) {
            // Looked at already: its poster is drawn unless a sharper one is.
            if done.poster.is_some()
                && !fill.decoding.contains(key)
                && shown.is_none_or(|side| side + 1 < done.poster_side)
            {
                self.want(key, a, bound);
            }
            return;
        }
        let size = a.width.zip(a.height);
        let wanted = match (size, self.images.thumbs.get(key)) {
            (None, _) => true,
            // Its poster is on its way.
            (_, Some(Img::Loading)) => false,
            (_, Some(Img::Failed)) => true,
            (Some(_), None) => !crate::channel::has_poster(a),
            (Some(size), Some(Img::Ready(_))) => {
                let (w, h) = fitted(size, bound);
                shown.is_some_and(|side| (side as f32) < w.max(h) * SHARP_ENOUGH)
            }
        };
        if wanted {
            self.want(key, a, bound);
        }
    }

    fn want(&self, key: &MediaKey, a: &Attachment, bound: (u32, u32)) {
        self.media.fill.wanted.borrow_mut().push(Want {
            key: key.clone(),
            attachment: a.clone(),
            bound,
        });
    }

    /// After a frame: draws the posters learnt for the cards on it, and starts looking at the
    /// videos on it that want it, a few at a time.
    pub fn start_video_fills(&mut self, cx: &mut Context<Self>) {
        let wanted = std::mem::take(&mut *self.media.fill.wanted.borrow_mut());
        for want in wanted {
            if self.media.fill.done.contains_key(&want.key) {
                self.show_filled_poster(want.key, cx);
                continue;
            }
            // Noted twice in one frame, or already started by an earlier note.
            let fill = &self.media.fill;
            if fill.running.len() >= AT_ONCE
                || fill.running.contains(&want.key)
                || fill.failed.contains(&want.key)
            {
                continue;
            }
            self.look_at_video(want, cx);
        }
    }

    /// Decodes a learnt poster and puts it in the card.
    fn show_filled_poster(&mut self, key: MediaKey, cx: &mut Context<Self>) {
        let Some(bytes) = self
            .media
            .fill
            .done
            .get(&key)
            .and_then(|d| d.poster.clone())
        else {
            return;
        };
        if !self.media.fill.decoding.insert(key.clone()) {
            return;
        }
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(move || crate::images::decode_preview(&bytes, None))
                    .await
                    .map_err(|e| e.to_string())?
            },
            move |this, result, _, cx| {
                this.media.fill.decoding.remove(&key);
                match result {
                    Ok(picture) => this.images.put_thumbnail(key, picture),
                    Err(err) => {
                        tracing::warn!("video poster {}: {err}", key.1);
                        this.media.fill.failed.insert(key);
                    }
                }
                cx.notify();
            },
        );
    }

    fn look_at_video(&mut self, want: Want, cx: &mut Context<Self>) {
        let Want {
            key,
            attachment,
            bound,
        } = want;
        let remote = match self.core.remote_file(&key.0, &attachment) {
            Ok(file) => file.map(Arc::new),
            Err(err) => {
                tracing::debug!("video {}: {err}", attachment.id);
                None
            }
        };
        // Read whole only when it is small enough to be worth it.
        if remote.is_none() && attachment.size > WHOLE_MAX_BYTES {
            self.media.fill.failed.insert(key);
            return;
        }
        self.media.fill.running.insert(key.clone());
        // An end-to-end encrypted video's picture is never written to disk.
        let file = (attachment.sealed.is_none())
            .then(|| {
                self.core
                    .paths()
                    .map(|p| poster_file(&p.dir, &key.0, attachment.id))
            })
            .flatten();
        let core = self.core.clone();
        let runtime = self.runtime.clone();
        let inst = key.0.clone();
        cx.notify();
        self.run(
            cx,
            async move {
                if let Some(path) = file.clone()
                    && let Ok(Some(found)) =
                        tokio::task::spawn_blocking(move || read_poster(&path)).await
                {
                    return Ok(found);
                }
                let source = match remote {
                    Some(file) => super::serve::Source::Remote(file, runtime),
                    None => super::serve::Source::Memory(Arc::new(
                        core.download(&inst, &attachment)
                            .await
                            .map_err(|e| e.to_string())?,
                    )),
                };
                tokio::task::spawn_blocking(move || {
                    let seen = super::inspect(source, bound)?;
                    if let Some(path) = &file {
                        write_poster(path, &seen);
                    }
                    Ok(seen)
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |this, result: Result<super::Inspected, String>, _, cx| {
                this.media.fill.running.remove(&key);
                match result {
                    Ok(seen) => {
                        let poster_side = seen
                            .poster
                            .as_ref()
                            .and_then(|p| image::load_from_memory(p).ok())
                            .map_or(0, |p| longest((p.width(), p.height())));
                        this.media.fill.keep(
                            key.clone(),
                            Filled {
                                size: seen.size,
                                duration_ms: seen.duration_ms,
                                poster: seen.poster.map(Arc::new),
                                poster_side,
                            },
                        );
                        this.show_filled_poster(key, cx);
                    }
                    Err(err) => {
                        tracing::info!("video {} not looked at: {err}", key.1);
                        if err.contains("FFmpeg is missing") {
                            this.media.fill.no_ffmpeg = true;
                        }
                        this.media.fill.failed.insert(key);
                    }
                }
                cx.notify();
            },
        );
    }
}

/// Where the poster of attachment `id` on `inst` is kept.
fn poster_file(dir: &Path, inst: &str, id: Id) -> PathBuf {
    let host: String = inst
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    dir.join("posters").join(format!("{host}-{id}.jpg"))
}

/// A kept poster: [`MAGIC`] with the size and length on the first line, then the JPEG.
fn read_poster(path: &Path) -> Option<super::Inspected> {
    let bytes = std::fs::read(path).ok()?;
    let end = bytes.iter().position(|b| *b == b'\n')?;
    let head = std::str::from_utf8(&bytes[..end]).ok()?;
    let mut parts = head.split(' ');
    if parts.next()? != MAGIC {
        return None;
    }
    let width: u32 = parts.next()?.parse().ok()?;
    let height: u32 = parts.next()?.parse().ok()?;
    let duration_ms: u64 = parts.next()?.parse().ok()?;
    let jpeg = bytes[end + 1..].to_vec();
    Some(super::Inspected {
        size: (width, height),
        duration_ms: (duration_ms > 0).then_some(duration_ms),
        poster: (!jpeg.is_empty()).then_some(jpeg),
    })
}

/// Keeps a poster on disk, and drops the oldest kept ones past [`DISK_BYTES`].
fn write_poster(path: &Path, seen: &super::Inspected) {
    let Some(dir) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let mut bytes = format!(
        "{MAGIC} {} {} {}\n",
        seen.size.0,
        seen.size.1,
        seen.duration_ms.unwrap_or(0)
    )
    .into_bytes();
    bytes.extend_from_slice(seen.poster.as_deref().unwrap_or_default());
    if let Err(err) = std::fs::write(path, bytes) {
        tracing::debug!("poster not kept: {err}");
        return;
    }
    trim_posters(dir, DISK_BYTES);
}

/// Removes the oldest files in `dir` until they take at most `limit` bytes.
fn trim_posters(dir: &Path, limit: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((meta.modified().ok()?, meta.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.0);
    for (_, len, path) in files {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_poster_reads_back() {
        let dir = std::env::temp_dir().join(format!("hermes-posters-{}", std::process::id()));
        let path = poster_file(&dir, "chat.example.org:8443", 42.into());
        let seen = super::super::Inspected {
            size: (1080, 1920),
            duration_ms: Some(12_500),
            poster: Some(vec![1, 2, 3, b'\n', 4]),
        };
        write_poster(&path, &seen);
        let back = read_poster(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back.size, (1080, 1920));
        assert_eq!(back.duration_ms, Some(12_500));
        assert_eq!(back.poster, seen.poster);
        assert!(path.ends_with("chat_example_org_8443-42.jpg"));
    }

    #[test]
    fn old_posters_go_first() {
        let dir = std::env::temp_dir().join(format!("hermes-trim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for n in 0..3 {
            std::fs::write(dir.join(format!("{n}.jpg")), [0u8; 100]).unwrap();
            // Distinct times, oldest first.
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        trim_posters(&dir, 250);
        let left: Vec<_> = (0..3)
            .map(|n| dir.join(format!("{n}.jpg")).exists())
            .collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(left, [false, true, true]);
    }

    #[test]
    fn posters_fit_without_growing() {
        assert_eq!(fitted((1920, 1080), (800, 600)), (800., 450.));
        assert_eq!(fitted((320, 180), (800, 600)), (320., 180.));
    }
}
