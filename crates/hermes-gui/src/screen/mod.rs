//! Screen sharing in the voice room (docs/DECISIONS.md 2026-11-02): capturing and encoding my
//! screen, and decoding the shares I watch, each on its own thread so the window never waits
//! for them. `hermes-client-core` carries the encoded frames; only people in the call get them.

pub mod capture;
pub mod codec;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use hermes_client_core::proto::id::Id;
use hermes_client_core::voice::RemoteFrame;
use hermes_client_core::{Core, InstanceId};
use iced::Subscription;
use iced::futures::SinkExt;
use iced::widget::image::Handle;

use crate::message::Message;

/// Frames waiting for the decoder; more are dropped (the decoder then asks for a keyframe).
const DECODE_QUEUE: usize = 8;

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
        while let Some((inst, user, (width, height, rgba))) = decoded.recv().await {
            let handle = Handle::from_rgba(width, height, rgba);
            if output
                .send(Message::ScreenImage(inst, user, handle))
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
    while let Some(frame) = frames.blocking_recv() {
        let key = (frame.inst.clone(), frame.user);
        if !decoders.contains_key(&key) {
            // A fresh decoder can only start at a keyframe.
            if !frame.header.keyframe {
                core.request_keyframe(frame.user);
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
        match decoder.decode(&frame.data) {
            Ok(Some(picture)) => {
                if pictures.try_send((key.0, key.1, picture)).is_err() && pictures.is_closed() {
                    return;
                }
            }
            Ok(None) => {}
            Err(err) => {
                tracing::debug!("screen share of {}: {err}", frame.user);
                decoders.remove(&key);
                core.request_keyframe(frame.user);
            }
        }
    }
}
