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
pub mod popout;
mod scale;
pub mod sound;
#[cfg(target_os = "linux")]
mod x11;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use gpui_kit::{AppContext as _, Context, RenderImage, Window};
use hermes_client_core::proto::id::Id;
use hermes_client_core::voice::RemoteFrame;
use hermes_client_core::{Core, InstanceId};
use image::Frame;

use crate::shell::Shell;

/// Frames waiting for the decoder (about a quarter of a second at 60 fps); past that the frame is
/// lost, and `hermes-client-core` holds that share back until a keyframe it asks for.
const DECODE_QUEUE: usize = 16;

/// A sharer is asked for a keyframe at most this often by this window (the server limits it too,
/// but each request is a control message).
const KEYFRAME_REQUEST_GAP: std::time::Duration = std::time::Duration::from_millis(500);

/// How often the decoder's numbers are logged.
const STATS_EVERY: std::time::Duration = std::time::Duration::from_secs(10);

/// A sharer's decoder (on Windows a Direct3D device and Media Foundation decoder of its own) is
/// dropped after this long without a frame: they stopped sharing, or I stopped watching. A new
/// one starts at their next keyframe. Well over the 5 s within which a sharer repeats even a still
/// screen. (Checked every [`STATS_EVERY`] as frames come in, so the last decoders stay until
/// somebody's share is watched again.)
const DECODER_IDLE: std::time::Duration = std::time::Duration::from_secs(30);

/// The latest picture of a share I watch, ready to draw.
pub struct Share {
    pub image: Arc<RenderImage>,
}

/// What the decoder did with one sharer's frames since the last log line.
#[derive(Default)]
struct DecodeStats {
    frames: u32,
    pictures: u32,
    /// Decoded, but never shown: a newer frame was already waiting, or the window was still busy
    /// with the picture before.
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
             (too late), {} errors, {:.1} ms waiting, {:.1} ms to decode (slowest {:.1} ms), \
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

/// One limited-range BT.601 pixel, as OpenH264 encodes, in 16.16 fixed point: `y` is already scaled
/// and `(r, g, b)` are the chroma terms, which two neighbouring pixels share.
#[inline(always)]
fn put_pixel(px: &mut [u8], y: i32, (r, g, b): (i32, i32, i32)) {
    px[0] = ((y + b) >> 16).clamp(0, 255) as u8;
    px[1] = ((y + g) >> 16).clamp(0, 255) as u8;
    px[2] = ((y + r) >> 16).clamp(0, 255) as u8;
    px[3] = 255;
}

#[inline(always)]
fn luma(y: u8) -> i32 {
    (i32::from(y) - 16).max(0) * 76309
}

#[inline(always)]
fn chroma(u: u8, v: u8) -> (i32, i32, i32) {
    chroma_terms(i32::from(u), i32::from(v))
}

#[inline(always)]
fn chroma_terms(u: i32, v: i32) -> (i32, i32, i32) {
    let (u, v) = (u - 128, v - 128);
    (104597 * v, -25675 * u - 53279 * v, 132201 * u)
}

/// A picture to BGRA. `skip` shrinks it n times, averaging each n x n block.
fn to_bgra(p: &codec::Picture, skip: u32) -> (u32, u32, Vec<u8>) {
    if skip <= 1 {
        full_size_to_bgra(p)
    } else {
        skipping_to_bgra(p, skip)
    }
}

/// Threads that convert one picture together: a 1080p picture took about 10 ms on one, most of
/// the 16.7 ms a picture has at 60 fps, so pictures that came in a burst were dropped.
fn convert_threads(rows: usize) -> usize {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    cores
        .saturating_sub(1)
        .clamp(1, 4)
        .min(rows.div_ceil(64))
        .max(1)
}

/// Fills `out` (rows of `row_bytes`) in bands on several threads: `fill(first_row, band)`.
fn in_bands(out: &mut [u8], row_bytes: usize, fill: impl Fn(usize, &mut [u8]) + Sync) {
    let rows = out.len() / row_bytes.max(1);
    let threads = convert_threads(rows);
    if threads <= 1 {
        fill(0, out);
        return;
    }
    let band_rows = rows.div_ceil(threads);
    std::thread::scope(|scope| {
        let mut bands = out.chunks_mut(band_rows * row_bytes).enumerate();
        // The calling thread does the first band itself.
        let first = bands.next();
        for (n, band) in bands {
            let fill = &fill;
            scope.spawn(move || fill(n * band_rows, band));
        }
        if let Some((_, band)) = first {
            fill(0, band);
        }
    });
}

/// Every pixel, the case that has to be fast: a window-sized share at 60 fps, converted in bands
/// on several threads.
fn full_size_to_bgra(p: &codec::Picture) -> (u32, u32, Vec<u8>) {
    let (w, h) = (p.width as usize, p.height as usize);
    // Zeroed memory comes from the system as it is first written, by the threads in parallel;
    // filling it first took longer than converting. Every byte is written below.
    let mut out = vec![0u8; w * h * 4];
    in_bands(&mut out, w * 4, |first, band| {
        full_size_rows(p, first, band)
    });
    (p.width, p.height, out)
}

/// Rows `first..` of [`full_size_to_bgra`] into `band`. Colour comes at half the size (one
/// sample per 2x2 pixels) and is blended between neighbouring samples instead of repeated over
/// each 2x2 block, which drew coloured text and edges with blocky fringes. Each sample is taken
/// as the middle of its block (what averaging senders make), so a pixel takes 3/4 of its own
/// sample and 1/4 of the next one towards it, across and down.
fn full_size_rows(p: &codec::Picture, first: usize, band: &mut [u8]) {
    let w = p.width as usize;
    let (y_plane, u_plane, v_plane) = (p.y(), p.u(), p.v());
    let cw = ((p.width / 2) as usize).max(1);
    let last_c_row = ((p.height / 2) as usize).saturating_sub(1);
    let sample = |row: usize, i: usize| -> (i32, i32) {
        if p.nv12 {
            let at = row * cw * 2 + i * 2;
            (i32::from(u_plane[at]), i32::from(u_plane[at + 1]))
        } else {
            let at = row * cw + i;
            (i32::from(u_plane[at]), i32::from(v_plane[at]))
        }
    };
    // The colour terms of this row's samples, blended down from the two nearest sample rows.
    let mut terms = vec![(0, 0, 0); cw];
    for (row, line) in band
        .chunks_exact_mut(w * 4)
        .enumerate()
        .map(|(n, line)| (first + n, line))
    {
        let y_row = &y_plane[row * w..row * w + w];
        let near = (row / 2).min(last_c_row);
        let far = if row % 2 == 0 {
            near.saturating_sub(1)
        } else {
            (near + 1).min(last_c_row)
        };
        for (i, term) in terms.iter_mut().enumerate() {
            let ((u0, v0), (u1, v1)) = (sample(near, i), sample(far, i));
            *term = chroma_terms((3 * u0 + u1 + 2) >> 2, (3 * v0 + v1 + 2) >> 2);
        }
        for (x, (px, y)) in line
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(y_row)
            .enumerate()
        {
            let i = (x / 2).min(cw - 1);
            let other = if x % 2 == 0 {
                i.saturating_sub(1)
            } else {
                (i + 1).min(cw - 1)
            };
            let (a, b) = (terms[i], terms[other]);
            let term = (
                (3 * a.0 + b.0) >> 2,
                (3 * a.1 + b.1) >> 2,
                (3 * a.2 + b.2) >> 2,
            );
            put_pixel(px, luma(*y), term);
        }
    }
}

/// Each `skip` x `skip` block of the picture as one pixel (its average), for tiles much smaller
/// than the picture: taking one pixel of each block would make thin text shimmer as it moves.
fn skipping_to_bgra(p: &codec::Picture, skip: u32) -> (u32, u32, Vec<u8>) {
    let (w, h) = (p.width as usize, p.height as usize);
    let step = skip.max(1) as usize;
    let (out_w, out_h) = (w.div_ceil(step), h.div_ceil(step));
    let mut out = vec![0u8; out_w * out_h * 4];
    in_bands(&mut out, out_w * 4, |first, band| {
        averaged_rows(p, step, out_w, first, band);
    });
    (out_w as u32, out_h as u32, out)
}

/// Output rows `first..` of [`skipping_to_bgra`] into `band`.
fn averaged_rows(p: &codec::Picture, step: usize, out_w: usize, first: usize, band: &mut [u8]) {
    let (w, h) = (p.width as usize, p.height as usize);
    let (y_plane, u_plane, v_plane) = (p.y(), p.u(), p.v());
    let (cw, ch) = ((p.width / 2) as usize, (p.height / 2) as usize);
    let average = |sum: u32, n: usize| {
        let n = n.max(1) as u32;
        ((sum + n / 2) / n) as u8
    };
    for (row, line) in band
        .chunks_exact_mut(out_w * 4)
        .enumerate()
        .map(|(n, line)| (first + n, line))
    {
        let rows = row * step..((row + 1) * step).min(h);
        let c_rows = chroma_span(&rows, ch);
        for (col, px) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let cols = col * step..((col + 1) * step).min(w);
            let c_cols = chroma_span(&cols, cw);
            let y_sum: u32 = rows
                .clone()
                .flat_map(|y| &y_plane[y * w + cols.start..y * w + cols.end])
                .map(|&v| u32::from(v))
                .sum();
            let (mut u_sum, mut v_sum) = (0u32, 0u32);
            for cy in c_rows.clone() {
                for cx in c_cols.clone() {
                    let (u, v) = if p.nv12 {
                        let at = cy * cw * 2 + cx * 2;
                        (u_plane[at], u_plane[at + 1])
                    } else {
                        let at = cy * cw + cx;
                        (u_plane[at], v_plane[at])
                    };
                    u_sum += u32::from(u);
                    v_sum += u32::from(v);
                }
            }
            let c_n = c_rows.len() * c_cols.len();
            let terms = chroma(average(u_sum, c_n), average(v_sum, c_n));
            put_pixel(px, luma(average(y_sum, rows.len() * cols.len())), terms);
        }
    }
}

/// The chroma samples (one per two pixels, `len` of them) that cover `pixels`; an odd last pixel
/// takes the last sample.
fn chroma_span(pixels: &std::ops::Range<usize>, len: usize) -> std::ops::Range<usize> {
    let last = len.saturating_sub(1);
    let start = (pixels.start / 2).min(last);
    let end = (pixels.end.saturating_sub(1) / 2).min(last) + 1;
    start..end
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
        // A share in its own window stays watched whatever this window shows.
        if self
            .popped_out
            .as_ref()
            .is_some_and(|(key, _)| !sharers.contains(key))
        {
            self.popped_out = None;
        }
        let popped = self.popped_out.as_ref().map(|(key, _)| key.clone());
        let mut watched: Vec<Id> = match self.core.voice_channel() {
            Some((inst, channel))
                if self.window_visible
                    && self.page == crate::shell::Page::Voice(inst.clone(), channel) =>
            {
                sharers
                    .iter()
                    .filter(|share| share.0 == inst && self.watching_shares.contains(share))
                    .map(|(_, user)| *user)
                    .collect()
            }
            _ => Vec::new(),
        };
        if let (Some((inst, user)), Some((call, _))) = (&popped, self.core.voice_channel())
            && *inst == call
            && !watched.contains(user)
        {
            watched.push(*user);
        }
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
            self.share_sound = None;
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
        #[cfg(target_os = "linux")]
        if crate::desktop::wayland() {
            self.start_portal_share(cx);
            return;
        }
        if let Err(err) = self.core.start_screen_share() {
            self.error(err.to_string(), cx);
            return;
        }
        let v = &self.config.voice;
        let screen = v.share_screen.clone();
        let quality =
            capture::Quality::from_config(v.share_resolution, v.share_fps, v.share_max_mbps);
        let preference = capture::Preference::from_config(v.share_preference);
        match capture::Capture::start(self.core.clone(), &screen, quality, preference) {
            Ok(capture) => {
                self.capture = Some(capture);
                self.start_share_sound(cx);
            }
            Err(err) => {
                self.core.stop_screen_share();
                self.error(format!("Could not share the screen: {err}"), cx);
            }
        }
        cx.notify();
    }

    /// Wayland: the desktop's dialog picks what to share (or shares the last choice again,
    /// with its token), then the share starts as elsewhere.
    #[cfg(target_os = "linux")]
    fn start_portal_share(&mut self, cx: &mut Context<Self>) {
        if self.picking_share {
            return;
        }
        self.picking_share = true;
        let token = Some(self.config.voice.share_restore_token.clone()).filter(|t| !t.is_empty());
        self.run(cx, capture::portal::pick(token), |this, picked, _, cx| {
            this.picking_share = false;
            let picked = match picked {
                Ok(picked) => picked,
                Err(capture::portal::PickError::Cancelled) => return,
                Err(capture::portal::PickError::Failed(err)) => {
                    // A token the desktop no longer knows asks again next time.
                    this.set_config(|c| c.voice.share_restore_token.clear(), cx);
                    this.error(format!("Could not share the screen: {err}"), cx);
                    return;
                }
            };
            let token = picked.token.clone().unwrap_or_default();
            if token != this.config.voice.share_restore_token {
                this.set_config(|c| c.voice.share_restore_token = token, cx);
            }
            // Started another way while the dialog was open, or left the call.
            if this.capture.is_some() || this.core.voice_channel().is_none() {
                return;
            }
            if let Err(err) = this.core.start_screen_share() {
                this.error(err.to_string(), cx);
                return;
            }
            let v = &this.config.voice;
            let quality =
                capture::Quality::from_config(v.share_resolution, v.share_fps, v.share_max_mbps);
            let preference = capture::Preference::from_config(v.share_preference);
            match capture::Capture::start_portal(this.core.clone(), picked, quality, preference) {
                Ok(capture) => {
                    this.capture = Some(capture);
                    this.start_share_sound(cx);
                }
                Err(err) => {
                    this.core.stop_screen_share();
                    this.error(format!("Could not share the screen: {err}"), cx);
                }
            }
            cx.notify();
        });
    }

    /// Sends what the computer plays along with the share, when that is wanted and the voice
    /// server forwards it. Without it the share goes on silently.
    fn start_share_sound(&mut self, cx: &mut Context<Self>) {
        if !self.config.voice.share_sound || !self.core.share_audio_supported() {
            return;
        }
        let core = self.core.clone();
        let send: sound::Send = Box::new(move |opus, level| core.send_share_sound(opus, level));
        match sound::ShareSound::start(send) {
            Ok(sound) => self.share_sound = Some(sound),
            Err(err) => self.info(format!("Sharing without sound: {err}"), cx),
        }
    }

    pub fn stop_share(&mut self, cx: &mut Context<Self>) {
        self.share_sound = None;
        if self.capture.take().is_some() {
            self.core.stop_screen_share();
        }
        cx.notify();
    }
}

/// Decodes frames per sharer until the sink closes. A broken picture resets that sharer's
/// decoder and asks them for a keyframe; pictures the window has no time for are dropped.
fn decode_loop(core: &Core, mut frames: tokio::sync::mpsc::Receiver<RemoteFrame>, latest: &Latest) {
    let mut decoders: HashMap<(InstanceId, Id), (codec::Decoder, std::time::Instant)> =
        HashMap::new();
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
            drop_idle(&mut decoders, stats_since);
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
                    decoders.insert(key.clone(), (decoder, begin));
                }
                Err(err) => {
                    tracing::warn!("screen share decoder: {err}");
                    continue;
                }
            }
        }
        let Some((decoder, last_frame)) = decoders.get_mut(&key) else {
            continue;
        };
        *last_frame = begin;
        let decoded = decoder.decode(&frame.data);
        let took = micros(begin.elapsed());
        let s = stats.entry(frame.user).or_default();
        s.decode_us += took;
        s.slowest_decode_us = s.slowest_decode_us.max(took);
        match decoded {
            // Every frame is decoded, for the frames after it, but colours are worked out only
            // for the newest: falling behind then costs pictures, never frames. One frame
            // waiting is normal (frames arrive in bursts) and is not falling behind.
            Ok(Some(_)) if frames.len() >= 2 => {
                s.pictures += 1;
                s.unshown += 1;
            }
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

/// Drops the decoders that got no frame for [`DECODER_IDLE`].
fn drop_idle<K, D>(decoders: &mut HashMap<K, (D, std::time::Instant)>, now: std::time::Instant) {
    decoders.retain(|_, (_, last_frame)| now.duration_since(*last_frame) < DECODER_IDLE);
}

fn micros(d: std::time::Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

/// The hardware H.264 encoders screen sharing can use; empty means it encodes in software.
pub fn hardware_encoders() -> Result<Vec<String>, String> {
    #[cfg(windows)]
    {
        mf_encode::hardware_encoder_names()
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lists this machine's hardware H.264 encoders, as Diagnostics does.
    #[test]
    #[ignore = "needs a GPU with an H.264 encoder"]
    fn hardware_encoders_are_listed() {
        let names = hardware_encoders().expect("the search runs");
        eprintln!("hardware encoders: {names:?}");
        assert!(!names.is_empty());
    }

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

    /// A picture with every sample different, in either layout.
    fn gradient(w: u32, h: u32, nv12: bool) -> codec::Picture {
        let mut planes: Vec<u8> = (0..w * h).map(|i| (i % 251) as u8).collect();
        let chroma = (w / 2 * h / 2) as usize;
        planes.extend((0..chroma * 2).map(|i| (i * 7 % 253) as u8));
        codec::Picture {
            width: w,
            height: h,
            nv12,
            planes,
        }
    }

    #[test]
    fn the_fast_path_draws_what_the_plain_one_does() {
        // With even colour the blending changes nothing: the same pixels as the plain path,
        // for sizes that fit a sample per two pixels exactly and an odd one that does not.
        for (w, h) in [(8, 6), (16, 2), (7, 5)] {
            for nv12 in [false, true] {
                let mut picture = gradient(w, h, nv12);
                let luma = (w * h) as usize;
                let (cw, ch) = picture.chroma_size();
                let samples = (cw * ch) as usize;
                for (n, c) in picture.planes[luma..].iter_mut().enumerate() {
                    let is_v = if nv12 { n % 2 == 1 } else { n >= samples };
                    *c = if is_v { 90 } else { 170 };
                }
                // The skipping path visits every pixel with a step of one.
                let plain = skipping_to_bgra(&picture, 1);
                assert_eq!(full_size_to_bgra(&picture), plain, "{w}x{h} nv12={nv12}");
            }
        }
    }

    #[test]
    fn colour_is_blended_between_samples() {
        // Grey luma; colour samples alternate red-ish and blue-ish across.
        let (w, h) = (8u32, 4u32);
        let mut planes = vec![128u8; (w * h) as usize];
        for _ in 0..h / 2 {
            for i in 0..w / 2 {
                let (u, v) = if i % 2 == 0 { (100, 200) } else { (200, 100) };
                planes.extend([u, v]);
            }
        }
        let picture = codec::Picture {
            width: w,
            height: h,
            nv12: true,
            planes,
        };
        let (_, _, out) = full_size_to_bgra(&picture);
        let red = |x: usize| i32::from(out[x * 4 + 2]);
        // Pixel 0 has no sample to its left: its own. Pixels 1 and 2 lie between the first two
        // samples, each a quarter of the way towards the other.
        let (r0, r1, r2) = (red(0), red(1), red(2));
        assert!(r0 > r1 && r1 > r2, "{r0} {r1} {r2}");
        assert!(((r0 - r1) * 2 - (r1 - r2)).abs() <= 2, "{r0} {r1} {r2}");
    }

    #[test]
    fn pictures_converted_in_bands_match_one_pass() {
        // Tall enough for several bands (where the machine has the cores), with an odd height.
        for nv12 in [false, true] {
            let picture = gradient(64, 302, nv12);
            let mut whole = vec![255u8; 64 * 302 * 4];
            full_size_rows(&picture, 0, &mut whole);
            assert_eq!(full_size_to_bgra(&picture).2, whole, "nv12={nv12}");

            let (out_w, out_h) = (32usize, 151usize);
            let mut averaged = vec![255u8; out_w * out_h * 4];
            averaged_rows(&picture, 2, out_w, 0, &mut averaged);
            assert_eq!(skipping_to_bgra(&picture, 2).2, averaged, "nv12={nv12}");
        }
    }

    /// How long a 1080p picture takes in one pass and in bands (run with `--release`).
    #[test]
    #[ignore = "a timing, not a check"]
    fn conversion_timing() {
        let picture = gradient(1920, 1080, true);
        let time = |f: &dyn Fn()| {
            let start = std::time::Instant::now();
            for _ in 0..50 {
                f();
            }
            start.elapsed().as_secs_f64() * 1000.0 / 50.0
        };
        let one = time(&|| {
            let mut out = vec![255u8; 1920 * 1080 * 4];
            full_size_rows(&picture, 0, &mut out);
            std::hint::black_box(out);
        });
        let banded = time(&|| {
            std::hint::black_box(full_size_to_bgra(&picture));
        });
        let alloc = time(&|| {
            std::hint::black_box(vec![0u8; 1920 * 1080 * 4]);
        });
        let mut reused = vec![255u8; 1920 * 1080 * 4];
        let start = std::time::Instant::now();
        for _ in 0..50 {
            in_bands(&mut reused, 1920 * 4, |first, band| {
                full_size_rows(&picture, first, band);
            });
        }
        let warm = start.elapsed().as_secs_f64() * 1000.0 / 50.0;
        eprintln!("1080p: {one:.2} ms in one pass, {banded:.2} ms in bands");
        eprintln!("allocating {alloc:.2} ms, bands into a reused buffer {warm:.2} ms");
    }

    #[test]
    fn small_tiles_average_each_block() {
        // One-pixel stripes of black and white, like thin text, at any skip become an even grey
        // instead of whichever stripe the skip lands on.
        for nv12 in [false, true] {
            let (w, h) = (8u32, 4u32);
            let mut planes: Vec<u8> = (0..w * h)
                .map(|i| if i % 2 == 0 { 16 } else { 235 })
                .collect();
            planes.extend(std::iter::repeat_n(128, (w / 2 * h / 2 * 2) as usize));
            let picture = codec::Picture {
                width: w,
                height: h,
                nv12,
                planes,
            };
            let (out_w, out_h, out) = skipping_to_bgra(&picture, 2);
            assert_eq!((out_w, out_h), (4, 2));
            for px in out.as_chunks::<4>().0 {
                assert!(px[..3].iter().all(|c| (120..=136).contains(c)), "{px:?}");
            }
        }
        // Sizes that do not divide by the skip keep their partial last blocks.
        for (w, h, skip) in [(7, 5, 3), (8, 6, 4), (16, 2, 5)] {
            let (out_w, out_h, out) = skipping_to_bgra(&gradient(w, h, true), skip);
            assert_eq!((out_w, out_h), (w.div_ceil(skip), h.div_ceil(skip)));
            assert_eq!(out.len(), (out_w * out_h * 4) as usize);
        }
    }

    #[test]
    fn idle_decoders_are_dropped() {
        let start = std::time::Instant::now();
        let mut decoders = HashMap::from([("gone", ((), start)), ("live", ((), start))]);
        drop_idle(&mut decoders, start + DECODER_IDLE / 2);
        assert_eq!(decoders.len(), 2, "nothing is idle yet");
        decoders.get_mut("live").unwrap().1 = start + DECODER_IDLE / 2;
        drop_idle(&mut decoders, start + DECODER_IDLE);
        assert_eq!(decoders.keys().collect::<Vec<_>>(), [&"live"]);
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
