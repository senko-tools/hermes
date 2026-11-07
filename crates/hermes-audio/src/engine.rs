//! The audio engine: one worker thread that owns the device streams (or a virtual device), the
//! capture and playback pipelines, and reports packets to send, speaking changes, the input
//! level and device changes through a callback.
//!
//! Devices are opened only when needed: the input while transmitting, metering or testing the
//! microphone; the output while transmitting, testing, or a sound plays. A stream error that
//! means the device is gone rebuilds the stream on the default device (PLAN.md 12.5).

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ringbuf::traits::{Consumer, Observer, Producer};

use crate::capture::Capture;
use crate::devices::{self, Input, Output};
use crate::playback::Playback;
use crate::sounds::Sound;
use crate::{AudioSettings, FRAME, SAMPLE_RATE};

/// Participant ID used for the microphone test loopback.
const MIC_TEST: u16 = u16::MAX;
/// The output ring is topped up to this much audio.
const OUTPUT_TARGET: Duration = Duration::from_millis(30);
/// The output closes after this long with nothing to play.
const OUTPUT_IDLE: Duration = Duration::from_secs(3);
/// How long to wait before retrying a device that could not be opened.
const RETRY: Duration = Duration::from_secs(2);
/// The shortest time between input level reports while metering: less than the 20 ms tick, so every
/// tick reports (one frame of audio, 50 times a second).
const LEVEL_INTERVAL: Duration = Duration::from_millis(15);
const TICK: Duration = Duration::from_millis(20);

/// A device replacement for headless use (the voicebot, tests): 48 kHz mono in 20 ms frames,
/// driven in real time by the engine.
pub trait VirtualIo: Send {
    /// Fills one frame of microphone input.
    fn read(&mut self, frame: &mut [f32; FRAME]);
    /// Receives one frame of mixed output.
    fn write(&mut self, frame: &[f32; FRAME]);
}

pub enum Io {
    Devices,
    Virtual(Box<dyn VirtualIo>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum AudioEvent {
    /// A datagram to send to the voice server.
    Packet(Vec<u8>),
    /// `participant: None` is yourself (the gate).
    Speaking {
        participant: Option<u16>,
        speaking: bool,
    },
    /// dBFS of the processed microphone signal, 50 times a second while metering.
    InputLevel(f32),
    /// A device was lost (or the chosen one is missing) and another one is used.
    DeviceChanged { input: bool, name: String },
    /// No device could be opened.
    DeviceError { input: bool, message: String },
}

enum Command {
    Settings(Box<AudioSettings>),
    Transmit(bool),
    Bitrate(u32),
    SelfMute(bool),
    SelfDeaf(bool),
    Ptt(bool),
    Receive(u16, Vec<u8>),
    ReceiveShare(u16, Vec<u8>),
    ShareVolume(u16, f32),
    RemoveShare(u16),
    RemoveParticipant(u16),
    ClearParticipants,
    Volume(u16, f32),
    LocalMute(u16, bool),
    Sound(Sound),
    StopSounds,
    MicTest(bool),
    Meter(bool),
}

/// Handle to the worker thread; dropping it stops the thread and closes the devices.
pub struct Engine {
    tx: Option<Sender<Command>>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start(
        io: Io,
        settings: AudioSettings,
        events: impl Fn(AudioEvent) + Send + 'static,
    ) -> Engine {
        let (tx, rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("hermes-audio".into())
            .spawn(move || Worker::new(io, settings, Box::new(events)).run(&rx))
            .expect("can spawn the audio thread");
        Engine {
            tx: Some(tx),
            thread: Some(thread),
        }
    }

    fn send(&self, command: Command) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(command);
        }
    }

    pub fn set_settings(&self, settings: AudioSettings) {
        self.send(Command::Settings(Box::new(settings)));
    }

    /// Capture and send while in a voice room.
    pub fn set_transmitting(&self, on: bool) {
        self.send(Command::Transmit(on));
    }

    pub fn set_bitrate(&self, bitrate: u32) {
        self.send(Command::Bitrate(bitrate));
    }

    pub fn set_self_mute(&self, on: bool) {
        self.send(Command::SelfMute(on));
    }

    /// Deafened: nobody is played (sounds still are).
    pub fn set_self_deaf(&self, on: bool) {
        self.send(Command::SelfDeaf(on));
    }

    pub fn set_ptt(&self, pressed: bool) {
        self.send(Command::Ptt(pressed));
    }

    /// A datagram received from the voice server (its sender ID is the participant).
    pub fn receive(&self, participant: u16, datagram: Vec<u8>) {
        self.send(Command::Receive(participant, datagram));
    }

    /// A share audio datagram of `participant`'s screen share.
    pub fn receive_share(&self, participant: u16, datagram: Vec<u8>) {
        self.send(Command::ReceiveShare(participant, datagram));
    }

    /// The volume of `participant`'s screen share sound, 0–2 (200 %).
    pub fn set_share_volume(&self, participant: u16, volume: f32) {
        self.send(Command::ShareVolume(participant, volume));
    }

    /// Stops playing `participant`'s screen share sound (no longer watched, or stopped).
    pub fn remove_share(&self, participant: u16) {
        self.send(Command::RemoveShare(participant));
    }

    pub fn remove_participant(&self, participant: u16) {
        self.send(Command::RemoveParticipant(participant));
    }

    pub fn clear_participants(&self) {
        self.send(Command::ClearParticipants);
    }

    /// 0–2 (200 %).
    pub fn set_volume(&self, participant: u16, volume: f32) {
        self.send(Command::Volume(participant, volume));
    }

    pub fn set_local_mute(&self, participant: u16, muted: bool) {
        self.send(Command::LocalMute(participant, muted));
    }

    pub fn play_sound(&self, sound: Sound) {
        self.send(Command::Sound(sound));
    }

    pub fn stop_sounds(&self) {
        self.send(Command::StopSounds);
    }

    /// Plays your processed microphone back to you.
    pub fn set_mic_test(&self, on: bool) {
        self.send(Command::MicTest(on));
    }

    /// Reports [`AudioEvent::InputLevel`] 50 times a second (opens the microphone).
    pub fn set_level_meter(&self, on: bool) {
        self.send(Command::Meter(on));
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Where audio goes in and out. There is one per engine, so the size difference does not matter.
#[allow(clippy::large_enum_variant)]
enum Backend {
    Devices {
        input: Option<(Input, Capture)>,
        output: Option<(Output, Playback)>,
        input_retry: Option<Instant>,
        output_retry: Option<Instant>,
        input_buf: Vec<f32>,
    },
    Virtual {
        io: Box<dyn VirtualIo>,
        capture: Capture,
        playback: Playback,
        frame: [f32; FRAME],
    },
}

struct Worker {
    settings: AudioSettings,
    events: Box<dyn Fn(AudioEvent) + Send>,
    backend: Backend,
    transmitting: bool,
    mic_test: bool,
    meter: bool,
    self_mute: bool,
    self_deaf: bool,
    ptt: bool,
    bitrate: u32,
    volumes: HashMap<u16, f32>,
    share_volumes: HashMap<u16, f32>,
    local_mutes: HashMap<u16, bool>,
    speaking: HashMap<Option<u16>, bool>,
    /// Sounds requested while the output was closed.
    pending_sounds: Vec<Sound>,
    last_level: Instant,
    output_idle_since: Option<Instant>,
    started: Instant,
}

impl Worker {
    fn new(io: Io, settings: AudioSettings, events: Box<dyn Fn(AudioEvent) + Send>) -> Self {
        let bitrate = 64_000;
        let backend = match io {
            Io::Devices => Backend::Devices {
                input: None,
                output: None,
                input_retry: None,
                output_retry: None,
                input_buf: vec![0.0; 48_000],
            },
            Io::Virtual(io) => Backend::Virtual {
                io,
                capture: Capture::new(SAMPLE_RATE, 1, &settings, bitrate),
                playback: Playback::new(SAMPLE_RATE),
                frame: [0.0; FRAME],
            },
        };
        let mut worker = Worker {
            settings,
            events,
            backend,
            transmitting: false,
            mic_test: false,
            meter: false,
            self_mute: false,
            self_deaf: false,
            ptt: false,
            bitrate,
            volumes: HashMap::new(),
            share_volumes: HashMap::new(),
            local_mutes: HashMap::new(),
            speaking: HashMap::new(),
            pending_sounds: Vec::new(),
            last_level: Instant::now(),
            output_idle_since: None,
            started: Instant::now(),
        };
        worker.configure();
        worker
    }

    fn now_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }

    fn run(mut self, rx: &Receiver<Command>) {
        let mut next_tick = Instant::now();
        loop {
            // Devices are polled every 5 ms; the virtual device runs in 20 ms steps.
            let wait = match self.backend {
                Backend::Devices { .. } => Duration::from_millis(5),
                Backend::Virtual { .. } => next_tick.saturating_duration_since(Instant::now()),
            };
            match rx.recv_timeout(wait) {
                Ok(command) => {
                    self.handle(command);
                    while let Ok(command) = rx.try_recv() {
                        self.handle(command);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            match self.backend {
                Backend::Devices { .. } => self.step_devices(),
                Backend::Virtual { .. } => {
                    let now = Instant::now();
                    if now >= next_tick {
                        self.step_virtual();
                        next_tick += TICK;
                        // Far behind (the machine was suspended): do not catch up.
                        if now > next_tick + Duration::from_millis(200) {
                            next_tick = now + TICK;
                        }
                    }
                }
            }
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Settings(settings) => {
                let devices_changed = settings.input_device != self.settings.input_device
                    || settings.output_device != self.settings.output_device;
                self.settings = *settings;
                if devices_changed && let Backend::Devices { input, output, .. } = &mut self.backend
                {
                    *input = None;
                    *output = None;
                }
                self.configure();
            }
            Command::Transmit(on) => {
                self.transmitting = on;
                if !on {
                    self.with_playback(Playback::clear);
                }
            }
            Command::Bitrate(bitrate) => {
                self.bitrate = bitrate;
                self.configure();
            }
            Command::SelfMute(on) => {
                self.self_mute = on;
                self.configure();
            }
            Command::SelfDeaf(on) => {
                self.self_deaf = on;
                self.configure();
            }
            Command::Ptt(pressed) => {
                self.ptt = pressed;
                self.with_capture(|c| c.gate.set_ptt(pressed));
            }
            Command::Receive(participant, datagram) => {
                let now = self.now_ms();
                let volume = self.volumes.get(&participant).copied().unwrap_or(1.0);
                let muted = self.local_mutes.get(&participant).copied().unwrap_or(false);
                self.with_playback(|p| {
                    p.push(participant, &datagram, now, volume);
                    if muted {
                        p.set_muted(participant, true);
                    }
                });
            }
            Command::ReceiveShare(participant, datagram) => {
                let now = self.now_ms();
                let volume = self.share_volumes.get(&participant).copied().unwrap_or(1.0);
                self.with_playback(|p| p.push_share(participant, &datagram, now, volume));
            }
            Command::ShareVolume(participant, volume) => {
                self.share_volumes.insert(participant, volume);
                self.with_playback(|p| p.set_share_volume(participant, volume));
            }
            Command::RemoveShare(participant) => {
                self.with_playback(|p| p.remove_share(participant));
            }
            Command::RemoveParticipant(participant) => {
                self.with_playback(|p| p.remove(participant));
                if self.speaking.remove(&Some(participant)) == Some(true) {
                    (self.events)(AudioEvent::Speaking {
                        participant: Some(participant),
                        speaking: false,
                    });
                }
            }
            Command::ClearParticipants => {
                self.with_playback(Playback::clear);
            }
            Command::Volume(participant, volume) => {
                self.volumes.insert(participant, volume);
                self.with_playback(|p| p.set_volume(participant, volume));
            }
            Command::LocalMute(participant, muted) => {
                self.local_mutes.insert(participant, muted);
                self.with_playback(|p| p.set_muted(participant, muted));
            }
            Command::Sound(sound) => {
                let mut played = false;
                self.with_playback(|p| {
                    p.play_sound(sound);
                    played = true;
                });
                if !played && self.settings.sounds {
                    self.pending_sounds.push(sound);
                }
            }
            Command::StopSounds => {
                self.pending_sounds.clear();
                self.with_playback(Playback::stop_sounds);
            }
            Command::MicTest(on) => {
                self.mic_test = on;
                if !on {
                    self.with_playback(|p| p.remove(MIC_TEST));
                }
            }
            Command::Meter(on) => self.meter = on,
        }
    }

    /// Pushes settings and flags into the pipelines.
    fn configure(&mut self) {
        let (settings, bitrate, mute, deaf, ptt) = (
            self.settings.clone(),
            self.bitrate,
            self.self_mute,
            self.self_deaf,
            self.ptt,
        );
        self.with_capture(|c| {
            c.apply(&settings);
            c.set_bitrate(bitrate);
            c.gate.muted = mute;
            c.gate.set_ptt(ptt);
        });
        self.with_playback(|p| configure_playback(p, &settings, deaf));
    }

    fn with_capture(&mut self, f: impl FnOnce(&mut Capture)) {
        match &mut self.backend {
            Backend::Devices { input, .. } => {
                if let Some((_, capture)) = input {
                    f(capture);
                }
            }
            Backend::Virtual { capture, .. } => f(capture),
        }
    }

    fn with_playback(&mut self, f: impl FnOnce(&mut Playback)) {
        match &mut self.backend {
            Backend::Devices { output, .. } => {
                if let Some((_, playback)) = output {
                    f(playback);
                }
            }
            Backend::Virtual { playback, .. } => f(playback),
        }
    }

    fn wants_input(&self) -> bool {
        self.transmitting || self.mic_test || self.meter
    }

    /// Sends what the capture produced: to the server while transmitting, to yourself while
    /// testing.
    fn captured(
        events: &dyn Fn(AudioEvent),
        transmitting: bool,
        mic_test: bool,
        datagram: &[u8],
        loopback: &mut Vec<Vec<u8>>,
    ) {
        if transmitting {
            events(AudioEvent::Packet(datagram.to_vec()));
        }
        if mic_test {
            loopback.push(datagram.to_vec());
        }
    }

    fn step_virtual(&mut self) {
        let now = self.now_ms();
        let (transmitting, mic_test) = (self.transmitting, self.mic_test);
        let Backend::Virtual {
            io,
            capture,
            playback,
            frame,
        } = &mut self.backend
        else {
            return;
        };
        let mut loopback = Vec::new();
        if transmitting || mic_test || self.meter {
            io.read(frame);
            capture.push(frame.as_slice(), |d| {
                Self::captured(&*self.events, transmitting, mic_test, d, &mut loopback);
            });
        }
        for datagram in loopback {
            playback.push(MIC_TEST, &datagram, now, 1.0);
        }
        io.write(playback.render_48k());
        self.report(now);
    }

    fn step_devices(&mut self) {
        let now = self.now_ms();
        self.ensure_devices();
        let (transmitting, mic_test) = (self.transmitting, self.mic_test);
        let Backend::Devices {
            input,
            output,
            input_buf,
            ..
        } = &mut self.backend
        else {
            return;
        };
        let mut loopback = Vec::new();
        if let Some((device, capture)) = input {
            let channels = device.stream.channels;
            loop {
                let n = device.ring.pop_slice(input_buf);
                let n = n - n % channels;
                if n == 0 {
                    break;
                }
                capture.push(&input_buf[..n], |d| {
                    Self::captured(&*self.events, transmitting, mic_test, d, &mut loopback);
                });
            }
        }
        if let Some((device, playback)) = output {
            for datagram in loopback {
                playback.push(MIC_TEST, &datagram, now, 1.0);
            }
            // Stereo samples: two per frame.
            let target = (device.stream.rate as f64 * OUTPUT_TARGET.as_secs_f64()) as usize * 2;
            while device.ring.occupied_len() < target {
                let samples = playback.render();
                device.ring.push_slice(samples);
            }
        }
        self.report(now);
    }

    /// Opens, closes and rebuilds device streams as needed.
    fn ensure_devices(&mut self) {
        let wants_input = self.wants_input();
        let mut wants_output =
            self.transmitting || self.mic_test || !self.pending_sounds.is_empty();
        let settings = self.settings.clone();
        let (bitrate, mute, deaf, ptt) = (self.bitrate, self.self_mute, self.self_deaf, self.ptt);
        let Backend::Devices {
            input,
            output,
            input_retry,
            output_retry,
            ..
        } = &mut self.backend
        else {
            return;
        };
        let events = &*self.events;

        // Input.
        if input
            .as_ref()
            .is_some_and(|(i, _)| i.stream.failed.load(Ordering::Relaxed))
        {
            *input = None;
            *input_retry = None;
        }
        if !wants_input {
            *input = None;
        } else if input.is_none() && input_retry.is_none_or(|t| t.elapsed() >= RETRY) {
            match open_with_fallback::<Input>(settings.input_device.as_deref(), true, events) {
                Some(device) => {
                    let mut capture = Capture::new(
                        device.stream.rate,
                        device.stream.channels,
                        &settings,
                        bitrate,
                    );
                    capture.gate.muted = mute;
                    capture.gate.set_ptt(ptt);
                    *input = Some((device, capture));
                    *input_retry = None;
                }
                None => *input_retry = Some(Instant::now()),
            }
        }

        // Output.
        if let Some((device, playback)) = output.as_ref() {
            if device.stream.failed.load(Ordering::Relaxed) {
                *output = None;
                *output_retry = None;
            } else if playback.is_active() {
                wants_output = true;
            }
        }
        if wants_output {
            self.output_idle_since = None;
        } else if output.is_some() {
            let since = *self.output_idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() < OUTPUT_IDLE {
                wants_output = true;
            } else {
                *output = None;
                self.output_idle_since = None;
            }
        }
        if wants_output && output.is_none() && output_retry.is_none_or(|t| t.elapsed() >= RETRY) {
            match open_with_fallback::<Output>(settings.output_device.as_deref(), false, events) {
                Some(device) => {
                    let mut playback = Playback::new(device.stream.rate);
                    configure_playback(&mut playback, &settings, deaf);
                    for (&participant, &volume) in &self.volumes {
                        playback.set_volume(participant, volume);
                    }
                    for sound in self.pending_sounds.drain(..) {
                        playback.play_sound(sound);
                    }
                    *output = Some((device, playback));
                    *output_retry = None;
                }
                None => {
                    // Sounds are not worth keeping while no output exists.
                    self.pending_sounds.clear();
                    *output_retry = Some(Instant::now());
                }
            }
        }
    }

    /// Speaking changes and the input level.
    fn report(&mut self, now: f64) {
        let (own, level, remote): (bool, f32, Vec<(u16, bool)>) = match &self.backend {
            Backend::Devices { input, output, .. } => (
                input.as_ref().is_some_and(|(_, c)| c.is_open()),
                input.as_ref().map_or(-100.0, |(_, c)| c.level_db()),
                output
                    .as_ref()
                    .map_or_else(Vec::new, |(_, p)| speaking(p, now)),
            ),
            Backend::Virtual {
                capture, playback, ..
            } => (
                capture.is_open(),
                capture.level_db(),
                speaking(playback, now),
            ),
        };
        let own = own && self.transmitting && !self.self_mute;
        let changes: Vec<(Option<u16>, bool)> = std::iter::once((None, own))
            .chain(remote.into_iter().map(|(p, s)| (Some(p), s)))
            .filter(|(p, s)| self.speaking.get(p).copied().unwrap_or(false) != *s)
            .collect();
        for (participant, speaking) in changes {
            self.speaking.insert(participant, speaking);
            (self.events)(AudioEvent::Speaking {
                participant,
                speaking,
            });
        }
        if self.meter && self.last_level.elapsed() >= LEVEL_INTERVAL {
            self.last_level = Instant::now();
            (self.events)(AudioEvent::InputLevel(level));
        }
    }
}

fn speaking(playback: &Playback, now: f64) -> Vec<(u16, bool)> {
    playback
        .participants()
        .filter(|&p| p != MIC_TEST)
        .map(|p| (p, playback.speaking(p, now)))
        .collect()
}

fn configure_playback(playback: &mut Playback, settings: &AudioSettings, deaf: bool) {
    playback.output_volume = settings.output_volume.clamp(0.0, 2.0);
    playback.sounds_enabled = settings.sounds;
    playback.sounds_volume = settings.sounds_volume;
    playback.deafened = deaf;
}

/// Opens the named device; if that fails, the default one (reporting the change); if that
/// fails too, reports the error and returns `None`.
fn open_with_fallback<T: Opened>(
    name: Option<&str>,
    input: bool,
    events: &dyn Fn(AudioEvent),
) -> Option<T> {
    let named = name.filter(|n| !n.is_empty() && *n != "default");
    let first = devices::find(named, input).and_then(|d| T::open(&d));
    let err = match first {
        Ok(device) => return Some(device),
        Err(err) => err,
    };
    if named.is_some()
        && let Ok(device) = devices::find(None, input).and_then(|d| T::open(&d))
    {
        tracing::warn!("{err}; using the default device");
        events(AudioEvent::DeviceChanged {
            input,
            name: device.name().to_owned(),
        });
        return Some(device);
    }
    tracing::warn!("{err}");
    events(AudioEvent::DeviceError {
        input,
        message: err,
    });
    None
}

trait Opened: Sized {
    fn open(device: &cpal::Device) -> Result<Self, String>;
    fn name(&self) -> &str;
}

impl Opened for Input {
    fn open(device: &cpal::Device) -> Result<Self, String> {
        devices::open_input(device)
    }
    fn name(&self) -> &str {
        &self.stream.name
    }
}

impl Opened for Output {
    fn open(device: &cpal::Device) -> Result<Self, String> {
        devices::open_output(device)
    }
    fn name(&self) -> &str {
        &self.stream.name
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::dsp;

    /// A tone in, a recorder out.
    struct Loop {
        phase: usize,
        recorded: Arc<Mutex<Vec<f32>>>,
    }

    impl VirtualIo for Loop {
        fn read(&mut self, frame: &mut [f32; FRAME]) {
            for s in frame.iter_mut() {
                *s =
                    0.3 * (self.phase as f32 * 2.0 * std::f32::consts::PI * 440.0 / 48_000.0).sin();
                self.phase += 1;
            }
        }
        fn write(&mut self, frame: &[f32; FRAME]) {
            self.recorded.lock().unwrap().extend_from_slice(frame);
        }
    }

    fn settings() -> AudioSettings {
        AudioSettings {
            noise_suppression: false,
            ..AudioSettings::default()
        }
    }

    #[test]
    fn virtual_engine_sends_receives_and_reports() {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let engine = {
            let events = events.clone();
            Engine::start(
                Io::Virtual(Box::new(Loop {
                    phase: 0,
                    recorded: recorded.clone(),
                })),
                settings(),
                move |e| events.lock().unwrap().push(e),
            )
        };
        engine.set_transmitting(true);
        engine.set_level_meter(true);
        thread::sleep(Duration::from_millis(500));
        let packets: Vec<Vec<u8>> = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AudioEvent::Packet(p) => Some(p.clone()),
                _ => None,
            })
            .collect();
        assert!(packets.len() >= 15, "{}", packets.len());
        {
            let events = events.lock().unwrap();
            assert!(events.contains(&AudioEvent::Speaking {
                participant: None,
                speaking: true
            }));
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, AudioEvent::InputLevel(l) if *l > -20.0))
            );
        }

        // Feed the packets back as participant 3, paced like the network: they are played and
        // 3 is speaking.
        let before = recorded.lock().unwrap().len();
        for p in &packets {
            engine.receive(3, p.clone());
            thread::sleep(TICK);
        }
        let out = recorded.lock().unwrap()[before..].to_vec();
        assert!(dsp::rms(&out[out.len() / 2..]) > 0.1, "{}", dsp::rms(&out));
        assert!(events.lock().unwrap().contains(&AudioEvent::Speaking {
            participant: Some(3),
            speaking: true
        }));

        // Muted: no more packets.
        engine.set_self_mute(true);
        thread::sleep(Duration::from_millis(100));
        let count = |events: &[AudioEvent]| {
            events
                .iter()
                .filter(|e| matches!(e, AudioEvent::Packet(_)))
                .count()
        };
        let before = count(&events.lock().unwrap());
        thread::sleep(Duration::from_millis(200));
        assert_eq!(count(&events.lock().unwrap()), before);
        drop(engine);
    }

    #[test]
    fn mic_test_plays_yourself_back() {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let engine = Engine::start(
            Io::Virtual(Box::new(Loop {
                phase: 0,
                recorded: recorded.clone(),
            })),
            settings(),
            |_| {},
        );
        engine.set_mic_test(true);
        thread::sleep(Duration::from_millis(500));
        let out = recorded.lock().unwrap().clone();
        assert!(dsp::rms(&out[out.len() / 2..]) > 0.1);
    }
}
