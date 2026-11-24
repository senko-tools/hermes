//! A share's sound on Linux: everything apps play, except Hermes itself, through PipeWire.
//!
//! PipeWire (also behind PulseAudio's API on today's desktops) shows every app's playback as a
//! node of its own. Hermes makes a capture stream that the session manager leaves alone, and
//! links every app's playback node to it, except its own (the call would go back to the people
//! in it). Apps that start playing later are linked when they appear. This is the same as
//! Windows' process loopback that leaves Hermes out.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use hermes_client_core::audio::codec::ShareEncoder;
use hermes_client_core::audio::dsp;
use pipewire as pw;
use pw::spa;

use super::{Framer, Send};

/// Captures until dropped.
pub struct ShareSound {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ShareSound {
    /// Starts capturing; `Err` without PipeWire.
    pub fn start(send: Send) -> Result<ShareSound, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel();
        let stopping = stop.clone();
        let thread = std::thread::Builder::new()
            .name("share-sound".into())
            .spawn(move || {
                if let Err(err) = run(send, &stopping, &ready_tx) {
                    let _ = ready_tx.send(Err(err));
                }
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(ShareSound {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(err)) => Err(err),
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                Err("the sound did not start".into())
            }
        }
    }
}

impl Drop for ShareSound {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Which side a channel goes to: left, right or both.
fn sides(channel: &str) -> &'static [&'static str] {
    match channel {
        "FL" | "SL" | "RL" | "FLC" | "TFL" | "TRL" => &["FL"],
        "FR" | "SR" | "RR" | "FRC" | "TFR" | "TRR" => &["FR"],
        // Mono, centre, the subwoofer and unknown ones: both.
        _ => &["FL", "FR"],
    }
}

/// What the registry has shown so far, and the links made from it.
#[derive(Default)]
struct Graph {
    /// Our capture stream's node (known once it is set up).
    own_node: Option<u32>,
    /// Input ports: (port, node, channel). Ours are among them, maybe before `own_node`.
    inputs: Vec<(u32, u32, String)>,
    /// PipeWire clients of this process.
    own_clients: HashSet<u32>,
    /// Playback nodes of other apps.
    apps: HashSet<u32>,
    /// Output ports: (port, node, channel).
    outputs: Vec<(u32, u32, String)>,
    /// Links we made, by (output port, input port).
    links: HashMap<(u32, u32), pw::link::Link>,
}

impl Graph {
    /// Links every app output port that is not linked yet.
    fn link_all(&mut self, core: &pw::core::Core) {
        let Some(own) = self.own_node else {
            return;
        };
        let own_ports: HashMap<&str, u32> = self
            .inputs
            .iter()
            .filter(|(_, node, _)| *node == own)
            .map(|(port, _, channel)| (channel.as_str(), *port))
            .collect();
        if own_ports.is_empty() {
            return;
        }
        for (port, node, channel) in &self.outputs {
            if !self.apps.contains(node) {
                continue;
            }
            for side in sides(channel) {
                let Some(&input) = own_ports.get(*side) else {
                    continue;
                };
                if self.links.contains_key(&(*port, input)) {
                    continue;
                }
                let props = pw::properties::properties! {
                    "link.output.node" => node.to_string(),
                    "link.output.port" => port.to_string(),
                    "link.input.node" => own.to_string(),
                    "link.input.port" => input.to_string(),
                    "object.linger" => "false",
                };
                match core.create_object::<pw::link::Link>("link-factory", &props) {
                    Ok(link) => {
                        self.links.insert((*port, input), link);
                    }
                    Err(err) => tracing::debug!("share sound: no link {port} -> {input}: {err}"),
                }
            }
        }
    }

    fn removed(&mut self, id: u32) {
        self.apps.remove(&id);
        self.own_clients.remove(&id);
        self.outputs
            .retain(|(port, node, _)| *port != id && *node != id);
        self.inputs
            .retain(|(port, node, _)| *port != id && *node != id);
        self.links
            .retain(|(output, input), _| *output != id && *input != id);
    }
}

/// The stream's side: frames of samples to Opus packets.
struct Audio {
    framer: Framer,
    encoder: ShareEncoder,
    send: Send,
    samples: Vec<f32>,
}

fn run(
    send: Send,
    stop: &Arc<AtomicBool>,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    use pw::properties::properties;

    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context
        .connect_rc(None)
        .map_err(|e| format!("no PipeWire: {e}"))?;
    let registry = core.get_registry_rc().map_err(|e| e.to_string())?;
    let me = std::process::id().to_string();
    let graph = Rc::new(RefCell::new(Graph::default()));

    let stream = pw::stream::StreamBox::new(
        &core,
        "Hermes screen share sound",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Production",
            *pw::keys::NODE_NAME => "hermes-share-sound",
            // Hermes links it itself; the session manager must not add the microphone.
            "node.autoconnect" => "false",
            "node.dont-reconnect" => "true",
        },
    )
    .map_err(|e| e.to_string())?;

    let ready_once = RefCell::new(Some(ready.clone()));
    let own_graph = graph.clone();
    let own_core = core.clone();
    let _stream_listener = stream
        .add_local_listener_with_user_data(Audio {
            framer: Framer::default(),
            encoder: ShareEncoder::default(),
            send,
            samples: Vec::new(),
        })
        .state_changed(move |stream, _, old, new| {
            tracing::debug!("share sound stream: {old:?} -> {new:?}");
            // Set up: its node and ports exist now, and apps can be linked to it.
            let id = stream.node_id();
            let mut graph = own_graph.borrow_mut();
            if id != u32::MAX && graph.own_node != Some(id) {
                graph.own_node = Some(id);
                graph.link_all(&own_core);
            }
            drop(graph);
            if matches!(
                new,
                pw::stream::StreamState::Paused | pw::stream::StreamState::Streaming
            ) && let Some(ready) = ready_once.borrow_mut().take()
            {
                let _ = ready.send(Ok(()));
            }
            if let pw::stream::StreamState::Error(err) = new
                && let Some(ready) = ready_once.borrow_mut().take()
            {
                let _ = ready.send(Err(err));
            }
        })
        .process(|stream, audio| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let bytes = data.chunk().size() as usize;
            let offset = data.chunk().offset() as usize;
            let Some(raw) = data.data() else {
                return;
            };
            let Some(raw) = raw.get(offset..offset + bytes) else {
                return;
            };
            audio.samples.clear();
            audio.samples.extend(
                raw.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b)),
            );
            let Audio {
                framer,
                encoder,
                send,
                samples,
            } = audio;
            framer.push(samples, |frame| {
                if let Some(opus) = encoder.encode(frame) {
                    send(opus, dsp::rms(frame));
                }
            });
        })
        .register()
        .map_err(|e| e.to_string())?;

    let _registry_listener = registry
        .add_listener_local()
        .global({
            let graph = graph.clone();
            let core = core.clone();
            let me = me.clone();
            move |global| {
                let Some(props) = global.props else {
                    return;
                };
                let mut graph = graph.borrow_mut();
                match global.type_ {
                    // Clients come before their nodes, and carry the process id that the
                    // registry leaves out of a node's own properties.
                    pw::types::ObjectType::Client => {
                        if props.get("application.process.id") == Some(me.as_str()) {
                            graph.own_clients.insert(global.id);
                        }
                        return;
                    }
                    pw::types::ObjectType::Node => {
                        let playback = props.get("media.class") == Some("Stream/Output/Audio");
                        let client = props.get("client.id").and_then(|c| c.parse::<u32>().ok());
                        // Our own sounds and the call (cpal names its PulseAudio client after
                        // the process).
                        let pulse_name = format!("cpal-pulseaudio-{me}");
                        let ours = props.get("application.process.id") == Some(me.as_str())
                            || client.is_some_and(|c| graph.own_clients.contains(&c))
                            || props.get("node.name") == Some(pulse_name.as_str())
                            || props.get("application.name") == Some(pulse_name.as_str());
                        if playback && !ours {
                            graph.apps.insert(global.id);
                        }
                    }
                    pw::types::ObjectType::Port => {
                        let node = props.get("node.id").and_then(|n| n.parse::<u32>().ok());
                        let channel = props.get("audio.channel").unwrap_or("MONO").to_owned();
                        let Some(node) = node else {
                            return;
                        };
                        let monitor = props.get("port.monitor") == Some("true");
                        match props.get("port.direction") {
                            Some("in") => graph.inputs.push((global.id, node, channel)),
                            Some("out") if !monitor => {
                                graph.outputs.push((global.id, node, channel));
                            }
                            _ => return,
                        }
                    }
                    _ => return,
                }
                graph.link_all(&core);
            }
        })
        .global_remove({
            let graph = graph.clone();
            move |id| graph.borrow_mut().removed(id)
        })
        .register();

    // 48 kHz stereo float, the format a share's sound is sent in: PipeWire converts to it.
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(48_000);
    info.set_channels(2);
    let mut position = [0; spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    let format = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map_err(|e| format!("{e:?}"))?
    .0
    .into_inner();
    let mut params = [spa::pod::Pod::from_bytes(&format).ok_or("bad format")?];
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| e.to_string())?;

    // Stops when asked.
    let tick = {
        let quit = mainloop.clone();
        let stop = stop.clone();
        move |_| {
            if stop.load(Ordering::Relaxed) {
                quit.quit();
            }
        }
    };
    let timer = mainloop.loop_().add_timer(tick);
    let _ = timer.update_timer(
        Some(Duration::from_millis(20)),
        Some(Duration::from_millis(100)),
    );
    mainloop.run();
    graph.borrow_mut().links.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sides;

    #[test]
    fn channels_go_to_their_side() {
        assert_eq!(sides("FL"), ["FL"]);
        assert_eq!(sides("RR"), ["FR"]);
        assert_eq!(sides("MONO"), ["FL", "FR"]);
        assert_eq!(sides("LFE"), ["FL", "FR"]);
    }
}
