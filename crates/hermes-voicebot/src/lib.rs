//! Hermes voicebot (PLAN.md sections 5.2 and 19): a headless client that signs in, joins a voice
//! room through `hermes-client-core` and plays a WAV file into it while recording what it hears.
//! The audio runs through the real pipelines on a virtual device.
//!
//! - [`Bot`]: sign in, join, play, record
//! - [`signal`]: test signals and the burst analysis used to measure loss and latency
//! - [`wav`]: WAV reading and writing

pub mod signal;
pub mod wav;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hermes_client_core::address::{Fingerprint, HostPort};
use hermes_client_core::audio::{FRAME, Io, SAMPLE_RATE, VirtualIo, dsp::MonoResampler};
use hermes_client_core::config::ClientConfig;
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::id::Id;
use hermes_client_core::{
    CertStatus, Core, CoreEvent, CoreEvents, CoreOptions, InstanceId, VoiceStatus,
};

/// What a bot plays into its microphone.
#[derive(Debug, Default)]
struct Source {
    samples: Vec<f32>,
    pos: usize,
    looped: bool,
    /// When the first frame was taken (= when sending started).
    started: Option<Instant>,
}

/// What a bot heard.
#[derive(Debug, Default)]
struct Recording {
    samples: Vec<f32>,
    /// When the first frame was written; frames follow every 20 ms.
    started: Option<Instant>,
}

struct BotIo {
    source: Arc<Mutex<Source>>,
    recording: Arc<Mutex<Recording>>,
}

impl VirtualIo for BotIo {
    fn read(&mut self, frame: &mut [f32; FRAME]) {
        let mut source = self.source.lock().expect("source");
        source.started.get_or_insert_with(Instant::now);
        for s in frame.iter_mut() {
            if source.pos >= source.samples.len() && source.looped && !source.samples.is_empty() {
                source.pos = 0;
            }
            *s = source.samples.get(source.pos).copied().unwrap_or(0.0);
            source.pos += 1;
        }
    }

    fn write(&mut self, frame: &[f32; FRAME]) {
        let mut recording = self.recording.lock().expect("recording");
        recording.started.get_or_insert_with(Instant::now);
        recording.samples.extend_from_slice(frame);
    }
}

/// How a bot signs in.
#[derive(Debug, Clone)]
pub struct Login {
    pub server: HostPort,
    /// Pin this certificate; without it a self-signed certificate is trusted on first use.
    pub fingerprint: Option<Fingerprint>,
    pub username: String,
    pub password: String,
    /// Create the account (with this invite code, if any) instead of signing in.
    pub register: Option<Option<String>>,
}

pub struct Bot {
    pub core: Core,
    pub events: CoreEvents,
    pub inst: InstanceId,
    source: Arc<Mutex<Source>>,
    recording: Arc<Mutex<Recording>>,
}

/// Resamples `samples` (mono) from `rate` to 48 kHz.
pub fn to_48k(samples: &[f32], rate: u32) -> Vec<f32> {
    let mut input = samples.to_vec();
    let mut out = Vec::with_capacity(samples.len() * SAMPLE_RATE as usize / rate.max(1) as usize);
    let mut resampler = MonoResampler::new(rate, SAMPLE_RATE);
    // Flush the resampler's last partial chunk with silence.
    input.extend(std::iter::repeat_n(0.0, rate as usize / 50));
    resampler.process(&mut input, &mut out);
    out
}

impl Bot {
    /// Signs in (or registers) and waits until the gateway is connected. `noise_suppression`
    /// should be off for synthetic test tones, which RNNoise treats as noise.
    pub async fn connect(login: &Login, noise_suppression: bool) -> Result<Bot, String> {
        let source = Arc::new(Mutex::new(Source::default()));
        let recording = Arc::new(Mutex::new(Recording::default()));
        let mut options = CoreOptions::in_memory("hermes-voicebot");
        options.audio = Io::Virtual(Box::new(BotIo {
            source: source.clone(),
            recording: recording.clone(),
        }));
        let mut config = ClientConfig::default();
        config.voice.noise_suppression = noise_suppression;
        // UI sounds would end up in the recording.
        config.voice.sounds = false;
        let (core, mut events) = Core::new(config, options);

        let fingerprint = match login.fingerprint {
            Some(fp) => Some(fp),
            None => {
                let probe = core
                    .probe_instance(&login.server)
                    .await
                    .map_err(|e| format!("cannot reach {}: {e}", login.server))?;
                (probe.cert != CertStatus::Trusted).then(|| {
                    eprintln!(
                        "trusting the certificate of {} on first use: {}",
                        login.server,
                        probe.fingerprint.colon_hex()
                    );
                    probe.fingerprint
                })
            }
        };
        if let Some(fp) = fingerprint {
            core.trust_fingerprint(&login.server, fp)
                .map_err(|e| e.to_string())?;
        }
        let inst = match &login.register {
            Some(invite) => {
                core.register(
                    &login.server,
                    &login.username,
                    &login.password,
                    invite.as_deref(),
                )
                .await
            }
            None => {
                core.login(&login.server, &login.username, &login.password)
                    .await
            }
        }
        .map_err(|e| format!("cannot sign in as {}: {e}", login.username))?;
        wait(&mut events, Duration::from_secs(15), |e| {
            matches!(
                e,
                CoreEvent::InstanceStatus {
                    status: ConnStatus::Connected,
                    ..
                }
            )
        })
        .await
        .ok_or("the gateway did not connect")?;
        Ok(Bot {
            core,
            events,
            inst,
            source,
            recording,
        })
    }

    /// Finds a voice channel by ID or name (case-insensitive), in any space, or a DM by ID.
    pub fn find_channel(&self, wanted: &str) -> Option<Id> {
        let store = self.core.store();
        let s = store.instance(&self.inst)?;
        if let Ok(id) = wanted.parse::<Id>()
            && s.channel(id).is_some_and(|c| c.kind.has_voice())
        {
            return Some(id);
        }
        let wanted = wanted.trim_start_matches('#').to_lowercase();
        s.spaces
            .values()
            .flat_map(|space| space.channels.values())
            .find(|c| {
                c.kind.has_voice()
                    && c.name.as_deref().map(str::to_lowercase) == Some(wanted.clone())
            })
            .map(|c| c.id)
    }

    /// Sets what to play (48 kHz mono), from the start.
    pub fn play(&self, samples: Vec<f32>, looped: bool) {
        let mut source = self.source.lock().expect("source");
        source.samples = samples;
        source.pos = 0;
        source.looped = looped;
    }

    /// Whether everything was played (never for a loop).
    pub fn finished_playing(&self) -> bool {
        let source = self.source.lock().expect("source");
        !source.looped && source.pos >= source.samples.len()
    }

    pub async fn join(&self, channel: &str) -> Result<Id, String> {
        let id = self
            .find_channel(channel)
            .ok_or_else(|| format!("no voice channel {channel:?}"))?;
        self.core
            .join_voice(&self.inst, id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    pub async fn leave(&self) {
        self.core.leave_voice().await;
    }

    pub fn status(&self) -> VoiceStatus {
        self.core.voice_status()
    }

    /// Everything heard so far (48 kHz mono) and when it started.
    pub fn recording(&self) -> (Vec<f32>, Option<Instant>) {
        let recording = self.recording.lock().expect("recording");
        (recording.samples.clone(), recording.started)
    }

    /// When the bot started sending (its first microphone frame).
    pub fn source_start(&self) -> Option<Instant> {
        self.source.lock().expect("source").started
    }

    /// Forgets what was recorded so far.
    pub fn clear_recording(&self) {
        *self.recording.lock().expect("recording") = Recording::default();
    }
}

/// The next event matching `f` within `timeout`.
pub async fn wait(
    events: &mut CoreEvents,
    timeout: Duration,
    mut f: impl FnMut(&CoreEvent) -> bool,
) -> Option<CoreEvent> {
    tokio::time::timeout(timeout, async {
        while let Some(event) = events.recv().await {
            if f(&event) {
                return Some(event);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}
