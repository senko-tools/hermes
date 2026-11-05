//! Screen sharing in the voice room (docs/DECISIONS.md 2026-11-02): capturing and encoding my
//! screen, and decoding the shares I watch, each on its own thread so the window never waits
//! for them. `hermes-client-core` carries the encoded frames; only people in the call get them.
//! Decoded pictures are drawn on the GPU ([`view`]).

pub mod capture;
pub mod codec;
#[cfg(windows)]
mod dxgi;
#[cfg(windows)]
mod gpu_convert;
#[cfg(windows)]
mod mf_encode;
mod scale;
pub mod view;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use hermes_client_core::proto::id::Id;
use hermes_client_core::voice::RemoteFrame;
use hermes_client_core::{Core, InstanceId};
use iced::Subscription;
use iced::futures::SinkExt;

use crate::message::Message;

/// Frames waiting for the decoder; more are dropped (the decoder then asks for a keyframe).
const DECODE_QUEUE: usize = 8;

/// A sharer is asked for a keyframe at most this often by this window (the server limits it too,
/// but each request is a control message).
const KEYFRAME_REQUEST_GAP: std::time::Duration = std::time::Duration::from_millis(500);

/// How often the decoder's numbers are logged.
const STATS_EVERY: std::time::Duration = std::time::Duration::from_secs(10);

/// What the decoder did with one sharer's frames since the last log line.
#[derive(Default)]
struct DecodeStats {
    frames: u32,
    pictures: u32,
    /// Decoded, but the window was still busy with the picture before.
    unshown: u32,
    errors: u32,
    /// Time frames waited between arriving and being decoded, and decoding took.
    waited_us: u64,
    decode_us: u64,
    slowest_decode_us: u64,
    /// Arrival time minus the sharer's timestamp (both in ms): its spread shows jitter, its
    /// trend shows a growing delay (a queue somewhere filling up).
    offset_min: Option<i64>,
    offset_max: Option<i64>,
    offset_first: Option<i64>,
    offset_last: Option<i64>,
}

impl DecodeStats {
    fn log(&self, user: Id, secs: f64) {
        let per = |us: u64, n: u32| us as f64 / 1000.0 / f64::from(n.max(1));
        let spread = self
            .offset_max
            .zip(self.offset_min)
            .map_or(0, |(a, b)| a - b);
        let drift = self
            .offset_last
            .zip(self.offset_first)
            .map_or(0, |(a, b)| a - b);
        tracing::info!(
            "screen share of {user} (decoding): {:.1} fps in, {:.1} pictures/s out, {} not shown              (window busy), {} errors, {:.1} ms waiting, {:.1} ms to decode (slowest {:.1} ms),              arrival jitter {spread} ms, delay drift {drift:+} ms",
            f64::from(self.frames) / secs,
            f64::from(self.pictures) / secs,
            self.unshown,
            self.errors,
            per(self.waited_us, self.frames),
            per(self.decode_us, self.frames),
            self.slowest_decode_us as f64 / 1000.0,
        );
    }
}

/// The decoder, as a subscription that runs once per window.
#[derive(Clone)]
struct Frames(Core);

impl Hash for Frames {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "screen-frames".hash(state);
    }
}

/// Decoded pictures of the shares I watch, as [`Message::ScreenImage`].
pub fn subscription(core: &Core) -> Subscription<Message> {
    Subscription::run_with(Frames(core.clone()), |frames| decoded(frames.0.clone()))
}

fn decoded(core: Core) -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(4, async move |mut output| {
        let (sink, frames) = tokio::sync::mpsc::channel::<RemoteFrame>(DECODE_QUEUE);
        core.set_screen_sink(sink);
        let (pictures, mut decoded) = tokio::sync::mpsc::channel(2);
        let decoder_core = core.clone();
        let spawned = std::thread::Builder::new()
            .name("hermes-screen-decode".into())
            .spawn(move || decode_loop(&decoder_core, frames, &pictures));
        if let Err(err) = spawned {
            tracing::warn!("screen share decoder: {err}");
            return;
        }
        while let Some((inst, user, picture)) = decoded.recv().await {
            if output
                .send(Message::ScreenImage(inst, user, Arc::new(picture)))
                .await
                .is_err()
            {
                return;
            }
        }
    })
}

type Decoded = (InstanceId, Id, codec::Picture);

/// Decodes frames per sharer until the sink closes. A broken picture resets that sharer's
/// decoder and asks them for a keyframe; pictures the window has no time for are dropped.
fn decode_loop(
    core: &Core,
    mut frames: tokio::sync::mpsc::Receiver<RemoteFrame>,
    pictures: &tokio::sync::mpsc::Sender<Decoded>,
) {
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
    let started = std::time::Instant::now();
    while let Some(frame) = frames.blocking_recv() {
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
        let arrived =
            i64::try_from(frame.received.duration_since(started).as_millis()).unwrap_or(i64::MAX);
        let offset = arrived - i64::from(frame.header.timestamp);
        s.offset_min = Some(s.offset_min.map_or(offset, |m| m.min(offset)));
        s.offset_max = Some(s.offset_max.map_or(offset, |m| m.max(offset)));
        s.offset_first.get_or_insert(offset);
        s.offset_last = Some(offset);
        let key = (frame.inst.clone(), frame.user);
        if !decoders.contains_key(&key) {
            // A fresh decoder can only start at a keyframe.
            if !frame.header.keyframe {
                want_keyframe(frame.user);
                continue;
            }
            match codec::Decoder::new() {
                Ok(decoder) => {
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
                if pictures.try_send((key.0, key.1, picture)).is_err() {
                    if pictures.is_closed() {
                        return;
                    }
                    s.unshown += 1;
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
