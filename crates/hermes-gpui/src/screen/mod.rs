//! Screen sharing in the voice room (docs/DECISIONS.md 2026-11-02): capturing and encoding my
//! screen, and decoding the shares I watch, each on its own thread so the window never waits
//! for them. `hermes-client-core` carries the encoded frames; only people in the call get them.
//!
//! gpui's Windows renderer has no video surface, so a decoded picture becomes a BGRA image on the
//! decoder's thread (scaled down when the tile is small), and the shell frees the image it
//! replaces. See docs/DECISIONS.md for the numbers.

pub mod capture;
pub mod codec;
#[cfg_attr(not(windows), allow(dead_code))]
mod cursor;
#[cfg(windows)]
mod cursor_gpu;
#[cfg(windows)]
mod dxgi;
#[cfg(windows)]
mod gpu_convert;
#[cfg(windows)]
mod mf_decode;
#[cfg(windows)]
mod mf_encode;
mod scale;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use gpui_kit::{AppContext as _, Context, RenderImage, Window};
use hermes_client_core::proto::id::Id;
use hermes_client_core::voice::RemoteFrame;
use hermes_client_core::{Core, InstanceId};
use image::Frame;

use crate::shell::Shell;

/// Frames waiting for the decoder; more are dropped (the decoder then asks for a keyframe).
const DECODE_QUEUE: usize = 8;

/// A sharer is asked for a keyframe at most this often by this window (the server limits it too,
/// but each request is a control message).
const KEYFRAME_REQUEST_GAP: std::time::Duration = std::time::Duration::from_millis(500);

/// How often the decoder's numbers are logged.
const STATS_EVERY: std::time::Duration = std::time::Duration::from_secs(10);

/// The latest picture of a share I watch, ready to draw.
pub struct Share {
    pub image: Arc<RenderImage>,
}

/// What the decoder did with one sharer's frames since the last log line.
#[derive(Default)]
struct DecodeStats {
    frames: u32,
    pictures: u32,
    /// Decoded, but the window was still busy with the picture before.
    unshown: u32,
    errors: u32,
    /// Time frames waited between arriving and being decoded, decoding took, and turning the
    /// picture into an image took.
    waited_us: u64,
    decode_us: u64,
    convert_us: u64,
    slowest_decode_us: u64,
}

impl DecodeStats {
    fn log(&self, user: Id, secs: f64) {
        let per = |us: u64, n: u32| us as f64 / 1000.0 / f64::from(n.max(1));
        tracing::info!(
            "screen share of {user} (decoding): {:.1} fps in, {:.1} pictures/s out, {} not shown \
             (window busy), {} errors, {:.1} ms waiting, {:.1} ms to decode (slowest {:.1} ms), \
             {:.1} ms to convert",
            f64::from(self.frames) / secs,
            f64::from(self.pictures) / secs,
            self.unshown,
            self.errors,
            per(self.waited_us, self.frames),
            per(self.decode_us, self.frames),
            self.slowest_decode_us as f64 / 1000.0,
            per(self.convert_us, self.pictures),
        );
    }
}

/// The newest decoded picture of each share, waiting for the window. The decoder overwrites a
/// picture the window has not taken yet, so a busy window gets the newest one at its own pace
/// instead of a queue of old ones (one update of the window per batch, not per picture).
#[derive(Default)]
pub struct Latest {
    pictures: Mutex<HashMap<(InstanceId, Id), Arc<RenderImage>>>,
    ready: tokio::sync::Notify,
    /// The window's side ended: the decoder stops too.
    closed: AtomicBool,
    /// The largest width and height a share shows at now, in device pixels.
    show_width: AtomicU32,
    show_height: AtomicU32,
}

impl Latest {
    /// Puts a picture in; `false` if it replaced one the window never took.
    fn put(&self, key: (InstanceId, Id), image: Arc<RenderImage>) -> bool {
        let replaced = self
            .pictures
            .lock()
            .expect("latest pictures")
            .insert(key, image)
            .is_some();
        self.ready.notify_one();
        !replaced
    }

    fn take(&self) -> Vec<((InstanceId, Id), Arc<RenderImage>)> {
        self.pictures
            .lock()
            .expect("latest pictures")
            .drain()
            .collect()
    }

    /// Tells the decoder how large shares show, so it does not convert pixels nobody sees.
    pub fn set_shown_size(&self, width: u32, height: u32) {
        self.show_width.store(width, Ordering::Relaxed);
        self.show_height.store(height, Ordering::Relaxed);
    }
}

/// How much to skip when converting: the largest whole step that still gives at least the shown
/// size (1 = every pixel).
fn skip_for(picture: (u32, u32), shown: (u32, u32)) -> u32 {
    if shown.0 == 0 || shown.1 == 0 {
        return 1;
    }
    (picture.0 / shown.0).min(picture.1 / shown.1).clamp(1, 4)
}

/// Limited-range BT.601, as OpenH264 encodes, to BGRA. `skip` takes every n-th pixel. Fixed point
/// (16.16); the loops are plain so the compiler can vectorize them.
fn to_bgra(p: &codec::Picture, skip: u32) -> (u32, u32, Vec<u8>) {
    let (w, h) = (p.width as usize, p.height as usize);
    let step = skip.max(1) as usize;
    let (out_w, out_h) = (w.div_ceil(step), h.div_ceil(step));
    let mut out = vec![255u8; out_w * out_h * 4];
    let (y_plane, u_plane, v_plane) = (p.y(), p.u(), p.v());
    let (cw, _) = p.chroma_size();
    let cw = cw as usize;
    for (row, line) in out.chunks_exact_mut(out_w * 4).enumerate() {
        let sy = row * step;
        let y_row = &y_plane[sy * w..sy * w + w];
        let c_row = sy / 2;
        for (col, px) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let sx = col * step;
            let cx = sx / 2;
            let (u, v) = if p.nv12 {
                let at = c_row * cw * 2 + cx * 2;
                (i32::from(u_plane[at]), i32::from(u_plane[at + 1]))
            } else {
                let at = c_row * cw + cx;
                (i32::from(u_plane[at]), i32::from(v_plane[at]))
            };
            let y = (i32::from(y_row[sx]) - 16).max(0) * 76309;
            let (u, v) = (u - 128, v - 128);
            let r = (y + 104597 * v) >> 16;
            let g = (y - 25675 * u - 53279 * v) >> 16;
            let b = (y + 132201 * u) >> 16;
            px[0] = b.clamp(0, 255) as u8;
            px[1] = g.clamp(0, 255) as u8;
            px[2] = r.clamp(0, 255) as u8;
        }
    }
    (out_w as u32, out_h as u32, out)
}

fn to_image(p: &codec::Picture, skip: u32) -> Option<Arc<RenderImage>> {
    let (width, height, pixels) = to_bgra(p, skip);
    let buffer = image::RgbaImage::from_raw(width, height, pixels)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(buffer)])))
}

impl Shell {
    /// Starts the decoder thread, which lives as long as the window, and takes its pictures.
    pub fn start_screen_decoder(&mut self, cx: &mut Context<Self>) {
        let (sink, frames) = tokio::sync::mpsc::channel::<RemoteFrame>(DECODE_QUEUE);
        self.core.set_screen_sink(sink);
        let latest = Arc::new(Latest::default());
        self.screen_latest = Some(latest.clone());
        let decoder_core = self.core.clone();
        let decoder_latest = latest.clone();
        let spawned = std::thread::Builder::new()
            .name("hermes-screen-decode".into())
            .spawn(move || decode_loop(&decoder_core, frames, &decoder_latest));
        if let Err(err) = spawned {
            tracing::warn!("screen share decoder: {err}");
            return;
        }
        let handle = self.window;
        cx.spawn(async move |this, cx| {
            loop {
                latest.ready.notified().await;
                let batch = latest.take();
                if batch.is_empty() {
                    continue;
                }
                let result = cx.update_window(handle, |_, window, cx| {
                    this.update(cx, |this, cx| this.take_pictures(batch, window, cx))
                });
                if !matches!(result, Ok(Ok(()))) {
                    latest.closed.store(true, Ordering::Relaxed);
                    return;
                }
            }
        })
        .detach();
    }

    /// New pictures: draws them, and frees the images they replace from the GPU.
    fn take_pictures(
        &mut self,
        batch: Vec<((InstanceId, Id), Arc<RenderImage>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (key, image) in batch {
            // One decoded just before I stopped watching.
            if !self.watching_shares.contains(&key) {
                let _ = window.drop_image(image);
                continue;
            }
            if let Some(old) = self.screens.insert(key, Share { image }) {
                let _ = window.drop_image(old.image);
            }
        }
        cx.notify();
    }

    /// Watches the screen shares I chose in my voice room while its page shows, and nothing
    /// otherwise; ends my capture once my share has ended.
    pub fn sync_screens(&mut self, window: &mut Window) {
        // A share that ended (or a call I left) is no longer chosen.
        let sharers = self.core.screen_sharers();
        self.watching_shares.retain(|share| sharers.contains(share));
        let watched: Vec<Id> = match self.core.voice_channel() {
            Some((inst, channel))
                if self.window_visible
                    && self.page == crate::shell::Page::Voice(inst.clone(), channel) =>
            {
                sharers
                    .into_iter()
                    .filter(|share| share.0 == inst && self.watching_shares.contains(share))
                    .map(|(_, user)| user)
                    .collect()
            }
            _ => Vec::new(),
        };
        if watched != self.watched_now {
            self.core.watch_screens(&watched);
            self.watched_now.clone_from(&watched);
        }
        let gone: Vec<(InstanceId, Id)> = self
            .screens
            .keys()
            .filter(|(_, user)| !watched.contains(user))
            .cloned()
            .collect();
        for key in gone {
            if let Some(share) = self.screens.remove(&key) {
                let _ = window.drop_image(share.image);
            }
        }
        if self
            .enlarged_share
            .as_ref()
            .is_some_and(|(_, user)| !watched.contains(user))
        {
            self.enlarged_share = None;
        }
        if self.capture.is_some() && !self.core.sharing_screen() {
            self.capture = None;
        }
    }

    pub fn watch_share(&mut self, share: (InstanceId, Id), watch: bool) {
        if watch {
            self.watching_shares.insert(share);
        } else {
            if self.enlarged_share.as_ref() == Some(&share) {
                self.enlarged_share = None;
            }
            self.watching_shares.remove(&share);
        }
    }

    /// Starts sharing with the chosen settings (`Err`: why not).
    pub fn start_share(&mut self, cx: &mut Context<Self>) {
        if self.capture.is_some() {
            return;
        }
        if let Err(err) = self.core.start_screen_share() {
            self.error(err.to_string(), cx);
            return;
        }
        let v = &self.config.voice;
        let screen = v.share_screen.clone();
        let quality = capture::Quality::from_config(&v.share_resolution, v.share_fps);
        let preference = capture::Preference::from_config(&v.share_preference);
        match capture::Capture::start(self.core.clone(), &screen, quality, preference) {
            Ok(capture) => self.capture = Some(capture),
            Err(err) => {
                self.core.stop_screen_share();
                self.error(format!("Could not share the screen: {err}"), cx);
            }
        }
        cx.notify();
    }

    pub fn stop_share(&mut self, cx: &mut Context<Self>) {
        if self.capture.take().is_some() {
            self.core.stop_screen_share();
        }
        cx.notify();
    }
}

/// Decodes frames per sharer until the sink closes. A broken picture resets that sharer's
/// decoder and asks them for a keyframe; pictures the window has no time for are dropped.
fn decode_loop(core: &Core, mut frames: tokio::sync::mpsc::Receiver<RemoteFrame>, latest: &Latest) {
    let mut decoders: HashMap<(InstanceId, Id), codec::Decoder> = HashMap::new();
    let mut stats: HashMap<Id, DecodeStats> = HashMap::new();
    let mut requested: HashMap<Id, std::time::Instant> = HashMap::new();
    let mut want_keyframe = |user: Id| {
        let due = requested
            .get(&user)
            .is_none_or(|t| t.elapsed() >= KEYFRAME_REQUEST_GAP);
        if due {
            requested.insert(user, std::time::Instant::now());
            core.request_keyframe(user);
        }
    };
    let mut stats_since = std::time::Instant::now();
    while let Some(frame) = frames.blocking_recv() {
        if latest.closed.load(Ordering::Relaxed) {
            return;
        }
        if stats_since.elapsed() >= STATS_EVERY {
            let secs = stats_since.elapsed().as_secs_f64();
            for (user, s) in stats.drain() {
                s.log(user, secs);
            }
            stats_since = std::time::Instant::now();
        }
        let begin = std::time::Instant::now();
        let s = stats.entry(frame.user).or_default();
        s.frames += 1;
        s.waited_us += micros(begin.duration_since(frame.received));
        let key = (frame.inst.clone(), frame.user);
        if !decoders.contains_key(&key) {
            // A fresh decoder can only start at a keyframe.
            if !frame.header.keyframe {
                want_keyframe(frame.user);
                continue;
            }
            match codec::Decoder::new() {
                Ok(decoder) => {
                    tracing::info!(
                        "screen share of {}: decoding with {}",
                        frame.user,
                        decoder.kind()
                    );
                    decoders.insert(key.clone(), decoder);
                }
                Err(err) => {
                    tracing::warn!("screen share decoder: {err}");
                    continue;
                }
            }
        }
        let Some(decoder) = decoders.get_mut(&key) else {
            continue;
        };
        let decoded = decoder.decode(&frame.data);
        let took = micros(begin.elapsed());
        let s = stats.entry(frame.user).or_default();
        s.decode_us += took;
        s.slowest_decode_us = s.slowest_decode_us.max(took);
        match decoded {
            Ok(Some(picture)) => {
                s.pictures += 1;
                let convert = std::time::Instant::now();
                let shown = (
                    latest.show_width.load(Ordering::Relaxed),
                    latest.show_height.load(Ordering::Relaxed),
                );
                let skip = skip_for((picture.width, picture.height), shown);
                if let Some(image) = to_image(&picture, skip) {
                    s.convert_us += micros(convert.elapsed());
                    if !latest.put(key, image) {
                        s.unshown += 1;
                    }
                }
            }
            Ok(None) => {}
            Err(err) => {
                s.errors += 1;
                tracing::debug!("screen share of {}: {err}", frame.user);
                decoders.remove(&key);
                want_keyframe(frame.user);
            }
        }
    }
}

fn micros(d: std::time::Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(y: u8, u: u8, v: u8, nv12: bool) -> codec::Picture {
        let (w, h) = (4u32, 4u32);
        let mut planes = vec![y; (w * h) as usize];
        if nv12 {
            for _ in 0..(w / 2 * h / 2) {
                planes.push(u);
                planes.push(v);
            }
        } else {
            planes.extend(std::iter::repeat_n(u, (w / 2 * h / 2) as usize));
            planes.extend(std::iter::repeat_n(v, (w / 2 * h / 2) as usize));
        }
        codec::Picture {
            width: w,
            height: h,
            nv12,
            planes,
        }
    }

    #[test]
    fn colors_convert_the_same_from_both_layouts() {
        // Limited-range white, black and a red.
        for (y, u, v, bgr) in [
            (235, 128, 128, [255, 255, 255]),
            (16, 128, 128, [0, 0, 0]),
            (81, 90, 240, [0, 0, 255]),
        ] {
            for nv12 in [false, true] {
                let (w, h, out) = to_bgra(&picture(y, u, v, nv12), 1);
                assert_eq!((w, h), (4, 4));
                for (got, want) in out[..3].iter().zip(bgr) {
                    assert!(
                        (i32::from(*got) - want).abs() <= 2,
                        "{y} {u} {v}: {:?} is not {bgr:?}",
                        &out[..3]
                    );
                }
                assert_eq!(out[3], 255, "opaque");
            }
        }
    }

    #[test]
    fn skipping_halves_the_size() {
        let (w, h, out) = to_bgra(&picture(100, 128, 128, false), 2);
        assert_eq!((w, h), (2, 2));
        assert_eq!(out.len(), 16);
    }

    #[test]
    fn small_tiles_skip_pixels_but_never_show_less_than_they_need() {
        assert_eq!(skip_for((1920, 1080), (0, 0)), 1, "not measured yet");
        assert_eq!(skip_for((1920, 1080), (1920, 1080)), 1);
        assert_eq!(skip_for((1920, 1080), (1000, 600)), 1);
        assert_eq!(skip_for((1920, 1080), (900, 500)), 2);
        assert_eq!(
            skip_for((3840, 2160), (100, 100)),
            4,
            "at most every fourth"
        );
    }
}
