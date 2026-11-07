//! Voice (PLAN.md sections 12 and 14): the QUIC connection to an instance's voice server,
//! joining, switching, leaving and reconnecting, speaking detection, per-user volume, the audio
//! engine (`hermes-audio`), and screen share transport (encoded frames in and out; capturing
//! and coding is the UI's job).
//!
//! One voice session at a time. It owns one QUIC connection, reused when switching rooms on the
//! same instance. A lost connection is retried with a fresh `/voice/join` for the same channel,
//! up to [`RECONNECT_ATTEMPTS`] times with backoff.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hermes_audio::{AudioEvent, AudioSettings, Engine, InputMode, Io, Sound};
use hermes_proto::api::{VoiceJoinRequest, VoiceJoinResponse, VoiceSelfStateRequest};
use hermes_proto::id::Id;
use hermes_proto::models::ChannelKind;
use hermes_proto::permissions::Permissions;
use hermes_proto::voice::{
    self, AudioHeader, AudioKind, ClientControl, KickReason, Participant, ServerControl,
    VideoHeader,
};
use quinn::crypto::rustls::QuicClientConfig;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::address::Fingerprint;
use crate::config;
use crate::core::{Core, CoreError, CoreEvent, InstanceId, Result, now_ms};
use crate::volumes::UserVolumes;

pub const RECONNECT_ATTEMPTS: u32 = 5;
/// How long `join_voice` waits for the server's `ready`.
const READY_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PING_INTERVAL: Duration = Duration::from_secs(2);
/// Own screen share frames waiting to be sent; more are dropped (the next frame should then
/// be a keyframe).
const SCREEN_QUEUE: usize = 8;
/// A screen share frame must arrive completely within this time.
const FRAME_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the screen share transport's numbers are logged.
const SCREEN_STATS_EVERY: Duration = Duration::from_secs(10);

/// One encoded frame of your own screen share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenFrame {
    pub keyframe: bool,
    pub width: u16,
    pub height: u16,
    /// Milliseconds since the share started.
    pub timestamp: u32,
    /// An H.264 access unit (Annex B).
    pub data: Vec<u8>,
}

/// A frame of someone else's screen share, for the UI's decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFrame {
    pub inst: InstanceId,
    pub user: Id,
    pub header: VideoHeader,
    /// The H.264 access unit.
    pub data: Vec<u8>,
    /// When the frame had arrived completely (to measure how long it waits for the decoder).
    pub received: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VoiceStatus {
    Disconnected,
    Connecting,
    Connected { ping_ms: u32, loss_pct: f32 },
    Reconnecting { attempt: u32 },
}

/// The voice part of [`Core`].
pub(crate) struct Voice {
    engine: Engine,
    /// The connection captured packets go to.
    link: Arc<Mutex<Option<quinn::Connection>>>,
    /// Where own screen share frames go (the current connection's writer).
    screen_out: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    /// Where others' screen share frames go (set by the UI).
    screen_sink: Arc<Mutex<Option<mpsc::Sender<RemoteFrame>>>>,
    state: Mutex<State>,
    volumes: Mutex<UserVolumes>,
    /// `PATCH /voice/@me` requests go out one at a time (see `apply_self_state`).
    rest_state: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Default)]
struct State {
    session: Option<Session>,
    next_generation: u64,
    self_mute: bool,
    self_deaf: bool,
    server_mute: bool,
    server_deaf: bool,
    /// inst → user → locally muted.
    local_mutes: HashMap<(InstanceId, Id), bool>,
    /// Who in the room is speaking right now (you included).
    speaking: HashSet<(InstanceId, Id)>,
}

struct Session {
    inst: InstanceId,
    channel: Id,
    generation: u64,
    control: mpsc::UnboundedSender<ClientControl>,
    participants: HashMap<u16, Id>,
    me: Option<Id>,
    /// Others in the room sharing their screen.
    sharing: HashSet<u16>,
    /// Shares this client receives.
    watching: HashSet<u16>,
    /// The server forwards share audio (`ready.share_audio`).
    share_audio: bool,
    /// My own screen share runs.
    my_share: bool,
    /// How many watch my share (`share_watchers`); `None` until the server says, which an older
    /// server never does: then frames are always sent.
    my_watchers: Option<u16>,
    status: VoiceStatus,
    connected_once: bool,
    /// A `join_voice` waiting for `ready` (or an error).
    pending: Option<oneshot::Sender<std::result::Result<(), String>>>,
    task: Option<JoinHandle<()>>,
}

impl Voice {
    /// Starts the audio engine; its events go to `events` (packets go straight to the
    /// connection).
    pub(crate) fn new(
        io: Io,
        settings: AudioSettings,
        volumes: UserVolumes,
        events: mpsc::UnboundedSender<AudioEvent>,
    ) -> Self {
        let link: Arc<Mutex<Option<quinn::Connection>>> = Arc::default();
        let engine = {
            let link = link.clone();
            Engine::start(io, settings, move |event| match event {
                AudioEvent::Packet(datagram) => {
                    let conn = link.lock().expect("voice link").clone();
                    if let Some(conn) = conn {
                        // Never blocks; a full buffer drops the packet.
                        let _ = conn.send_datagram(datagram.into());
                    }
                }
                other => {
                    let _ = events.send(other);
                }
            })
        };
        Voice {
            engine,
            link,
            screen_out: Mutex::default(),
            screen_sink: Arc::default(),
            state: Mutex::default(),
            volumes: Mutex::new(volumes),
            rest_state: Arc::default(),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("voice state")
    }
}

/// `[voice]` of the config as engine settings.
pub fn audio_settings(v: &config::Voice) -> AudioSettings {
    let device = |name: &str| (!name.is_empty() && name != "default").then(|| name.to_owned());
    AudioSettings {
        input_device: device(&v.input_device),
        output_device: device(&v.output_device),
        input_volume: v.input_volume as f32,
        output_volume: v.output_volume as f32,
        mode: if v.mode == "push_to_talk" {
            InputMode::PushToTalk
        } else {
            InputMode::VoiceActivity
        },
        ptt_release_delay: Duration::from_millis(u64::from(v.ptt_release_delay_ms)),
        vad_sensitivity_db: v.vad_sensitivity as f32,
        noise_suppression: v.noise_suppression,
        auto_gain: v.auto_gain,
        sounds: v.sounds,
        sounds_volume: v.sounds_volume as f32,
    }
}

/// Writes engine settings back into `[voice]` (keeping the keys the engine does not have).
fn store_settings(v: &mut config::Voice, s: &AudioSettings) {
    v.input_device = s.input_device.clone().unwrap_or_else(|| "default".into());
    v.output_device = s.output_device.clone().unwrap_or_else(|| "default".into());
    v.input_volume = f64::from(s.input_volume);
    v.output_volume = f64::from(s.output_volume);
    v.mode = match s.mode {
        InputMode::PushToTalk => "push_to_talk",
        InputMode::VoiceActivity => "voice_activity",
    }
    .into();
    v.ptt_release_delay_ms = u32::try_from(s.ptt_release_delay.as_millis()).unwrap_or(u32::MAX);
    v.vad_sensitivity = s.vad_sensitivity_db.round() as i32;
    v.noise_suppression = s.noise_suppression;
    v.auto_gain = s.auto_gain;
    v.sounds = s.sounds;
    v.sounds_volume = f64::from(s.sounds_volume);
}

/// The message shown when the server ends your voice session.
fn kick_message(reason: KickReason) -> &'static str {
    match reason {
        KickReason::JoinedElsewhere => "You joined voice somewhere else",
        KickReason::Disconnected => "You were disconnected from voice",
        KickReason::ChannelDeleted => "The voice channel was deleted",
        KickReason::Removed => "You can no longer access that voice channel",
    }
}

/// How a connection ended.
enum End {
    /// `leave_voice` (or the session was replaced).
    Left,
    Kicked(KickReason),
    /// Could not connect or authenticate.
    Failed(String),
    /// An established connection broke.
    Lost(String),
}

impl Core {
    fn voice(&self) -> &Voice {
        &self.inner.voice
    }

    // ---- settings and devices ---------------------------------------------------------------

    pub fn audio_settings(&self) -> AudioSettings {
        audio_settings(&self.config().voice)
    }

    /// Applies new audio settings and saves them to `[voice]` in `config.toml`.
    pub fn set_audio_settings(&self, settings: AudioSettings) -> Result<()> {
        {
            let mut config = self.inner.config.write().expect("config lock");
            store_settings(&mut config.voice, &settings);
        }
        self.apply_audio_settings();
        self.save_config()
    }

    /// Hands `[voice]` (forcing push-to-talk where voice activity is not allowed) to the engine.
    pub(crate) fn apply_audio_settings(&self) {
        let mut settings = self.audio_settings();
        if self.voice_requires_ptt() {
            settings.mode = InputMode::PushToTalk;
        }
        self.voice().engine.set_settings(settings);
    }

    /// Whether the current voice channel requires push-to-talk (no `USE_VOICE_ACTIVITY`).
    pub fn voice_requires_ptt(&self) -> bool {
        let Some((inst, channel)) = self.voice_channel() else {
            return false;
        };
        let store = self.store();
        store.instance(&inst).is_some_and(|s| {
            !s.channel_permissions(channel)
                .contains(Permissions::USE_VOICE_ACTIVITY)
        })
    }

    pub fn list_audio_devices(&self) -> hermes_audio::AudioDevices {
        hermes_audio::devices::list()
    }

    /// Plays your processed microphone back to you ("mic test").
    pub fn set_mic_test(&self, on: bool) {
        self.voice().engine.set_mic_test(on);
    }

    /// Emits [`CoreEvent::InputLevel`] 20 times a second while on (opens the microphone).
    pub fn set_level_meter(&self, on: bool) {
        self.voice().engine.set_level_meter(on);
    }

    /// Plays a UI sound (incoming call ring, ping) unless `voice.sounds` is off.
    pub fn play_sound(&self, sound: Sound) {
        self.voice().engine.play_sound(sound);
    }

    pub fn stop_sounds(&self) {
        self.voice().engine.stop_sounds();
    }

    // ---- per-user volume ----------------------------------------------------------------------

    pub fn user_volume(&self, inst: &str, user: Id) -> f32 {
        self.voice()
            .volumes
            .lock()
            .expect("volumes")
            .get(inst, user)
    }

    /// 0–2 (200 %), saved in `user_volumes.toml`.
    pub fn set_user_volume(&self, inst: &str, user: Id, volume: f32) -> Result<()> {
        self.voice()
            .volumes
            .lock()
            .expect("volumes")
            .set(inst, user, volume)?;
        if let Some(pid) = self.participant_of(inst, user) {
            self.voice().engine.set_volume(pid, volume);
        }
        Ok(())
    }

    pub fn user_muted(&self, inst: &str, user: Id) -> bool {
        self.voice()
            .state()
            .local_mutes
            .get(&(inst.to_owned(), user))
            .copied()
            .unwrap_or(false)
    }

    /// Mutes a user for yourself only (not saved).
    pub fn set_user_muted(&self, inst: &str, user: Id, muted: bool) {
        self.voice()
            .state()
            .local_mutes
            .insert((inst.to_owned(), user), muted);
        if let Some(pid) = self.participant_of(inst, user) {
            self.voice().engine.set_local_mute(pid, muted);
        }
    }

    fn participant_of(&self, inst: &str, user: Id) -> Option<u16> {
        let state = self.voice().state();
        let session = state.session.as_ref().filter(|s| s.inst == inst)?;
        session
            .participants
            .iter()
            .find(|(_, u)| **u == user)
            .map(|(pid, _)| *pid)
    }

    // ---- state ----------------------------------------------------------------------------

    /// The instance and channel of the voice session, if any.
    pub fn voice_channel(&self) -> Option<(InstanceId, Id)> {
        let state = self.voice().state();
        state.session.as_ref().map(|s| (s.inst.clone(), s.channel))
    }

    pub fn voice_status(&self) -> VoiceStatus {
        let state = self.voice().state();
        state
            .session
            .as_ref()
            .map_or(VoiceStatus::Disconnected, |s| s.status.clone())
    }

    /// `user` is speaking in your voice room (your own state comes from the local gate).
    pub fn is_speaking(&self, inst: &str, user: Id) -> bool {
        self.voice()
            .state()
            .speaking
            .contains(&(inst.to_owned(), user))
    }

    fn set_speaking(&self, inst: InstanceId, user: Id, speaking: bool) {
        {
            let mut state = self.voice().state();
            if speaking {
                state.speaking.insert((inst.clone(), user));
            } else {
                state.speaking.remove(&(inst.clone(), user));
            }
        }
        self.emit(CoreEvent::Speaking {
            inst,
            user,
            speaking,
        });
    }

    pub fn self_mute(&self) -> bool {
        self.voice().state().self_mute
    }

    pub fn self_deaf(&self) -> bool {
        self.voice().state().self_deaf
    }

    pub fn set_self_mute(&self, on: bool) {
        let changed = {
            let mut state = self.voice().state();
            std::mem::replace(&mut state.self_mute, on) != on
        };
        if changed {
            self.play_sound(if on { Sound::Mute } else { Sound::Unmute });
            self.apply_self_state();
        }
    }

    /// Deafening also mutes you (undeafening restores your own mute setting).
    pub fn set_self_deaf(&self, on: bool) {
        let changed = {
            let mut state = self.voice().state();
            std::mem::replace(&mut state.self_deaf, on) != on
        };
        if changed {
            self.play_sound(if on { Sound::Mute } else { Sound::Unmute });
            self.apply_self_state();
        }
    }

    /// Push-to-talk key state (only used in push-to-talk mode).
    pub fn set_ptt(&self, pressed: bool) {
        self.voice().engine.set_ptt(pressed);
    }

    /// What the server should see: deafened implies muted.
    fn self_flags(state: &State) -> (bool, bool) {
        (state.self_mute || state.self_deaf, state.self_deaf)
    }

    /// Pushes mute/deafen to the engine, the control stream and `PATCH /voice/@me`.
    fn apply_self_state(&self) {
        let (mute, deaf, server, session) = {
            let state = self.voice().state();
            let (mute, deaf) = Self::self_flags(&state);
            let session = state
                .session
                .as_ref()
                .map(|s| (s.inst.clone(), s.control.clone()));
            (mute, deaf, (state.server_mute, state.server_deaf), session)
        };
        let engine = &self.voice().engine;
        engine.set_self_mute(mute || server.0);
        engine.set_self_deaf(deaf || server.1);
        let Some((inst, control)) = session else {
            return;
        };
        let _ = control.send(ClientControl::SetState {
            self_mute: mute,
            self_deaf: deaf,
        });
        if let Ok(rest) = self.rest(&inst) {
            // Quick toggles spawn several requests that could arrive out of order; one at a
            // time, each sending the state as it is then, the last one is always the newest.
            let core = self.clone();
            let lock = self.voice().rest_state.clone();
            tokio::spawn(async move {
                let _turn = lock.lock().await;
                let (mute, deaf) = {
                    let state = core.voice().state();
                    if state.session.as_ref().is_none_or(|s| s.inst != inst) {
                        return;
                    }
                    Self::self_flags(&state)
                };
                let req = VoiceSelfStateRequest {
                    self_mute: Some(mute),
                    self_deaf: Some(deaf),
                };
                let _ = rest.voice_self_state(&req).await;
            });
        }
    }

    // ---- join / leave -------------------------------------------------------------------------

    /// Joins (or switches to) the voice room of a voice channel, DM or group DM. Returns once
    /// the server has answered `ready`.
    pub async fn join_voice(&self, inst: &str, channel: Id) -> Result<()> {
        let rest = self.rest(inst)?;
        let (mute, deaf) = Self::self_flags(&self.voice().state());
        let join = rest
            .voice_join(&VoiceJoinRequest {
                channel_id: channel,
                self_mute: mute,
                self_deaf: deaf,
            })
            .await?;
        let (tx, rx) = oneshot::channel();
        let switched = {
            let mut state = self.voice().state();
            match state.session.as_mut() {
                Some(s) if s.inst == inst && s.connected_once => {
                    s.pending = Some(tx);
                    let _ = s.control.send(ClientControl::Switch { token: join.token });
                    None
                }
                _ => Some((tx, join)),
            }
        };
        if let Some((tx, join)) = switched {
            self.leave_voice().await;
            self.start_session(inst, channel, join, tx);
        }
        match tokio::time::timeout(READY_TIMEOUT, rx).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(message))) => {
                self.end_failed_join(inst, channel);
                Err(CoreError::Voice(message))
            }
            Ok(Err(_)) => Err(CoreError::Voice("Voice connection closed".into())),
            Err(_) => {
                self.end_failed_join(inst, channel);
                Err(CoreError::Voice(
                    "Voice connection timed out (is UDP blocked?)".into(),
                ))
            }
        }
    }

    /// After a failed first join, drop the half-open session.
    fn end_failed_join(&self, inst: &str, channel: Id) {
        let session = {
            let mut state = self.voice().state();
            let failed = state
                .session
                .as_ref()
                .is_some_and(|s| s.inst == inst && s.channel == channel && !s.connected_once);
            if failed { state.session.take() } else { None }
        };
        if let Some(session) = session {
            self.close_session(session, false);
        }
    }

    /// Joins a DM's call and rings the other recipients (PLAN.md 12.6 "Start call").
    pub async fn start_call(&self, inst: &str, channel: Id) -> Result<()> {
        self.join_voice(inst, channel).await?;
        self.rest(inst)?.ring(channel, None).await?;
        Ok(())
    }

    /// What the UIs' "join" does: joins the room, except that a DM room nobody else is in
    /// starts a call (rings the others).
    pub async fn join_or_call(&self, inst: &str, channel: Id) -> Result<()> {
        let (dm, others) = {
            let store = self.store();
            let s = store.instance(inst);
            // A DM created a moment ago may not be in the store yet; space channels always are.
            let dm = s.is_none_or(|s| {
                s.space_of(channel).is_none()
                    || s.channel(channel)
                        .is_some_and(|c| matches!(c.kind, ChannelKind::Dm | ChannelKind::GroupDm))
            });
            let others = s.is_some_and(|s| {
                let me = s.me_id();
                s.voice_states
                    .values()
                    .any(|v| v.channel_id == channel && Some(v.user_id) != me)
            });
            (dm, others)
        };
        if dm && !others {
            self.start_call(inst, channel).await
        } else {
            self.join_voice(inst, channel).await
        }
    }

    /// Leaves voice (no-op when not in voice).
    pub async fn leave_voice(&self) {
        let session = self.voice().state().session.take();
        if let Some(session) = session {
            let _ = session.control.send(ClientControl::Leave {});
            self.close_session(session, true);
        }
    }

    /// Stops a session's audio and tells the UI. The driver task ends on its own when the
    /// control sender is dropped.
    fn close_session(&self, session: Session, sound: bool) {
        let engine = &self.voice().engine;
        engine.set_transmitting(false);
        engine.clear_participants();
        {
            let mut state = self.voice().state();
            state.server_mute = false;
            state.server_deaf = false;
            state.speaking.clear();
        }
        if sound && session.connected_once {
            engine.play_sound(Sound::Leave);
        }
        drop(session.control);
        if let Some(task) = session.task {
            // Give it a moment to send `leave`, then stop it.
            tokio::spawn(async move {
                let abort = task.abort_handle();
                if tokio::time::timeout(Duration::from_secs(2), task)
                    .await
                    .is_err()
                {
                    abort.abort();
                }
            });
        }
        self.emit(CoreEvent::VoiceStatus(VoiceStatus::Disconnected));
        self.apply_audio_settings();
    }

    /// Leaves voice if it is on `inst` (signing out, removing the instance).
    pub(crate) fn leave_voice_on(&self, inst: &str) {
        let session = {
            let mut state = self.voice().state();
            if state.session.as_ref().is_some_and(|s| s.inst == inst) {
                state.session.take()
            } else {
                None
            }
        };
        if let Some(session) = session {
            let _ = session.control.send(ClientControl::Leave {});
            self.close_session(session, true);
        }
    }

    /// Breaks the QUIC connection as if the network dropped (tests of the reconnect logic).
    #[doc(hidden)]
    pub fn drop_voice_connection(&self) {
        if let Some(conn) = self.voice().link.lock().expect("voice link").as_ref() {
            conn.close(1u32.into(), b"test");
        }
    }

    fn start_session(
        &self,
        inst: &str,
        channel: Id,
        join: VoiceJoinResponse,
        pending: oneshot::Sender<std::result::Result<(), String>>,
    ) {
        let (control, control_rx) = mpsc::unbounded_channel();
        let generation = {
            let mut state = self.voice().state();
            state.next_generation += 1;
            let generation = state.next_generation;
            state.session = Some(Session {
                inst: inst.to_owned(),
                channel,
                generation,
                control,
                participants: HashMap::new(),
                me: None,
                sharing: HashSet::new(),
                watching: HashSet::new(),
                share_audio: false,
                my_share: false,
                my_watchers: None,
                status: VoiceStatus::Connecting,
                connected_once: false,
                pending: Some(pending),
                task: None,
            });
            generation
        };
        self.emit(CoreEvent::VoiceStatus(VoiceStatus::Connecting));
        let task = tokio::spawn(self.clone().drive(
            inst.to_owned(),
            channel,
            join,
            generation,
            control_rx,
        ));
        if let Some(s) = self.voice().state().session.as_mut()
            && s.generation == generation
        {
            s.task = Some(task);
        }
    }

    /// Runs the session's connection, reconnecting after losses.
    async fn drive(
        self,
        inst: InstanceId,
        mut channel: Id,
        mut join: VoiceJoinResponse,
        generation: u64,
        mut control: mpsc::UnboundedReceiver<ClientControl>,
    ) {
        loop {
            let end = self
                .connection(&mut channel, &join, generation, &mut control)
                .await;
            let connected_once = self.with_session(generation, |s| s.connected_once);
            match end {
                End::Left => return,
                End::Kicked(reason) => {
                    self.session_ended(generation, Some(kick_message(reason).to_owned()));
                    return;
                }
                End::Failed(message) if connected_once != Some(true) => {
                    self.fail_pending(generation, message);
                    return;
                }
                End::Failed(message) | End::Lost(message) => {
                    tracing::info!("voice connection lost: {message}");
                    match self.rejoin(&inst, channel, generation).await {
                        Some(next) => join = next,
                        None => {
                            self.session_ended(
                                generation,
                                Some("Voice connection lost (is UDP blocked?)".into()),
                            );
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Fresh `/voice/join` tokens for `channel`, with backoff. `None` when giving up.
    async fn rejoin(&self, inst: &str, channel: Id, generation: u64) -> Option<VoiceJoinResponse> {
        *self.voice().link.lock().expect("voice link") = None;
        for attempt in 1..=RECONNECT_ATTEMPTS {
            self.set_status(generation, VoiceStatus::Reconnecting { attempt })?;
            let delay = Duration::from_millis(500 * (1 << (attempt - 1)));
            tokio::time::sleep(delay).await;
            self.with_session(generation, |_| ())?;
            let Ok(rest) = self.rest(inst) else {
                return None;
            };
            let (mute, deaf) = Self::self_flags(&self.voice().state());
            match rest
                .voice_join(&VoiceJoinRequest {
                    channel_id: channel,
                    self_mute: mute,
                    self_deaf: deaf,
                })
                .await
            {
                Ok(join) => return Some(join),
                // The server answered "no" (permission, full, disabled): stop trying.
                Err(err) if !err.is_network() => return None,
                Err(_) => {}
            }
        }
        None
    }

    /// One QUIC connection: connect, authenticate, then serve until it ends.
    async fn connection(
        &self,
        channel: &mut Id,
        join: &VoiceJoinResponse,
        generation: u64,
        control: &mut mpsc::UnboundedReceiver<ClientControl>,
    ) -> End {
        let conn = match connect(join).await {
            Ok(conn) => conn,
            Err(err) => return End::Failed(err),
        };
        let (mut send, recv) = match conn.open_bi().await {
            Ok(streams) => streams,
            Err(err) => return End::Failed(format!("Voice connection failed: {err}")),
        };
        let (mute, deaf) = Self::self_flags(&self.voice().state());
        let auth = ClientControl::Auth {
            token: join.token.clone(),
            self_mute: mute,
            self_deaf: deaf,
        };
        if write(&mut send, &auth).await.is_err() {
            return End::Failed("Voice connection failed".into());
        }
        *self.voice().link.lock().expect("voice link") = Some(conn.clone());
        let (frames_tx, mut frames) = mpsc::channel(32);
        let reader = tokio::spawn(read_frames(recv, frames_tx));
        let (screen_out, screen_frames) = mpsc::channel(SCREEN_QUEUE);
        *self.voice().screen_out.lock().expect("screen out") = Some(screen_out);
        let screen_writer = tokio::spawn(write_screen(conn.clone(), screen_frames));
        let screen_reader = tokio::spawn(self.clone().read_screen(conn.clone(), generation));
        let mut ping = tokio::time::interval(PING_INTERVAL);
        let mut authed = false;

        let end = loop {
            tokio::select! {
                msg = control.recv() => match msg {
                    None => break End::Left,
                    Some(msg) => {
                        let leaving = matches!(msg, ClientControl::Leave {});
                        let _ = write(&mut send, &msg).await;
                        if leaving {
                            let _ = send.finish();
                            break End::Left;
                        }
                    }
                },
                frame = frames.recv() => {
                    let Some(msg) = frame else {
                        break End::Lost("control stream closed".into());
                    };
                    match msg {
                        ServerControl::Ready { participant_id, room_id, bitrate, participants, sharing, share_audio } => {
                            authed = true;
                            *channel = room_id;
                            self.on_ready(generation, participant_id, room_id, bitrate, participants);
                            // A new room: nothing is watched and my share (if any) has ended.
                            self.with_session(generation, |s| {
                                s.sharing = sharing.into_iter().collect();
                                s.share_audio = share_audio;
                                s.watching.clear();
                                s.my_share = false;
                                s.my_watchers = None;
                            });
                            self.emit(CoreEvent::ScreenShares);
                        }
                        ServerControl::ParticipantJoined(p) => self.on_joined(generation, p),
                        ServerControl::ShareStarted { participant_id } => {
                            self.with_session(generation, |s| s.sharing.insert(participant_id));
                            self.emit(CoreEvent::ScreenShares);
                        }
                        ServerControl::ShareStopped { participant_id } => {
                            self.with_session(generation, |s| {
                                s.sharing.remove(&participant_id);
                                s.watching.remove(&participant_id);
                            });
                            self.voice().engine.remove_share(participant_id);
                            self.emit(CoreEvent::ScreenShares);
                        }
                        ServerControl::KeyframeRequested {} => self.emit(CoreEvent::KeyframeRequested),
                        ServerControl::ShareWatchers { count } => {
                            self.with_session(generation, |s| {
                                if s.my_share {
                                    s.my_watchers = Some(count);
                                }
                            });
                            self.emit(CoreEvent::ScreenShares);
                        }
                        ServerControl::ParticipantLeft { participant_id } => {
                            self.on_left(generation, participant_id);
                        }
                        ServerControl::ServerState { server_mute, server_deaf } => {
                            {
                                let mut state = self.voice().state();
                                state.server_mute = server_mute;
                                state.server_deaf = server_deaf;
                            }
                            self.apply_self_state();
                        }
                        ServerControl::BitrateChanged { bitrate } => {
                            self.voice().engine.set_bitrate(bitrate);
                        }
                        ServerControl::Moved { token } => {
                            let _ = write(&mut send, &ClientControl::Switch { token }).await;
                        }
                        ServerControl::Kicked { reason } => break End::Kicked(reason),
                        ServerControl::Pong { t } => {
                            let ping_ms = u32::try_from((now_ms() as u64).saturating_sub(t)).unwrap_or(u32::MAX);
                            let stats = conn.stats();
                            let sent = stats.path.sent_packets.max(1);
                            let loss_pct = stats.path.lost_packets as f32 * 100.0 / sent as f32;
                            if self.with_session(generation, |s| s.connected_once) == Some(true) {
                                let _ = self.set_status(generation, VoiceStatus::Connected { ping_ms, loss_pct });
                            }
                        }
                        ServerControl::Error { message, .. } => {
                            if !authed {
                                break End::Failed(message);
                            }
                            self.fail_pending(generation, message);
                        }
                    }
                }
                datagram = conn.read_datagram() => match datagram {
                    Ok(data) => match AudioHeader::decode_any(&data) {
                        Ok((AudioKind::Voice, header, _)) => {
                            self.voice().engine.receive(header.sender_id, data.to_vec());
                        }
                        // Only from shares I watch (the server sends no others).
                        Ok((AudioKind::Share, header, _)) => {
                            let watched = self
                                .voice()
                                .state()
                                .session
                                .as_ref()
                                .is_some_and(|s| s.watching.contains(&header.sender_id));
                            if watched {
                                self.voice().engine.receive_share(header.sender_id, data.to_vec());
                            }
                        }
                        Err(_) => {}
                    },
                    Err(err) => break End::Lost(err.to_string()),
                },
                _ = ping.tick() => {
                    let _ = write(&mut send, &ClientControl::Ping { t: now_ms() as u64 }).await;
                }
            }
        };
        reader.abort();
        screen_writer.abort();
        screen_reader.abort();
        *self.voice().screen_out.lock().expect("screen out") = None;
        {
            let mut link = self.voice().link.lock().expect("voice link");
            if link
                .as_ref()
                .is_some_and(|c| c.stable_id() == conn.stable_id())
            {
                *link = None;
            }
        }
        if matches!(end, End::Left | End::Kicked(_)) {
            // Let `leave` reach the server before closing.
            let _ = tokio::time::timeout(Duration::from_millis(500), send.stopped()).await;
            conn.close(0u32.into(), b"bye");
        }
        end
    }

    fn with_session<T>(&self, generation: u64, f: impl FnOnce(&mut Session) -> T) -> Option<T> {
        let mut state = self.voice().state();
        state
            .session
            .as_mut()
            .filter(|s| s.generation == generation)
            .map(f)
    }

    fn set_status(&self, generation: u64, status: VoiceStatus) -> Option<()> {
        self.with_session(generation, |s| s.status = status.clone())?;
        self.emit(CoreEvent::VoiceStatus(status));
        Some(())
    }

    fn on_ready(
        &self,
        generation: u64,
        participant_id: u16,
        room_id: Id,
        bitrate: u32,
        participants: Vec<Participant>,
    ) {
        let me = self.with_session(generation, |s| {
            s.channel = room_id;
            s.participants = participants
                .iter()
                .map(|p| (p.participant_id, p.user_id))
                .collect();
            s.connected_once = true;
            if let Some(pending) = s.pending.take() {
                let _ = pending.send(Ok(()));
            }
            s.inst.clone()
        });
        let Some(inst) = me else {
            return;
        };
        let me_id = self.store().instance(&inst).and_then(|s| s.me_id());
        self.with_session(generation, |s| s.me = me_id);
        let _ = participant_id;
        let engine = &self.voice().engine;
        engine.clear_participants();
        for p in &participants {
            self.configure_participant(&inst, p);
        }
        engine.set_bitrate(bitrate);
        self.apply_audio_settings();
        self.apply_self_state();
        engine.set_transmitting(true);
        engine.play_sound(Sound::Join);
        let _ = self.set_status(
            generation,
            VoiceStatus::Connected {
                ping_ms: 0,
                loss_pct: 0.0,
            },
        );
    }

    fn configure_participant(&self, inst: &str, p: &Participant) {
        let engine = &self.voice().engine;
        engine.set_volume(p.participant_id, self.user_volume(inst, p.user_id));
        engine.set_local_mute(p.participant_id, self.user_muted(inst, p.user_id));
    }

    fn on_joined(&self, generation: u64, p: Participant) {
        let Some(inst) = self.with_session(generation, |s| {
            s.participants.insert(p.participant_id, p.user_id);
            s.inst.clone()
        }) else {
            return;
        };
        self.configure_participant(&inst, &p);
        self.voice().engine.play_sound(Sound::Join);
    }

    fn on_left(&self, generation: u64, participant_id: u16) {
        let left = self.with_session(generation, |s| {
            let shared = s.sharing.remove(&participant_id);
            s.watching.remove(&participant_id);
            (
                shared,
                s.participants
                    .remove(&participant_id)
                    .map(|user| (s.inst.clone(), user)),
            )
        });
        let left = left.map(|(shared, left)| {
            if shared {
                self.emit(CoreEvent::ScreenShares);
            }
            left
        });
        self.voice().engine.remove_participant(participant_id);
        if let Some(Some((inst, user))) = left {
            self.set_speaking(inst, user, false);
            self.voice().engine.play_sound(Sound::Leave);
        }
    }

    /// Fails a waiting `join_voice` with `message`.
    fn fail_pending(&self, generation: u64, message: String) {
        self.with_session(generation, |s| {
            if let Some(pending) = s.pending.take() {
                let _ = pending.send(Err(message));
            }
        });
    }

    /// The server or the network ended the session.
    fn session_ended(&self, generation: u64, message: Option<String>) {
        let session = {
            let mut state = self.voice().state();
            if state
                .session
                .as_ref()
                .is_some_and(|s| s.generation == generation)
            {
                state.session.take()
            } else {
                None
            }
        };
        if let Some(mut session) = session {
            session.task = None;
            self.close_session(session, true);
            if let Some(message) = message {
                self.emit(CoreEvent::Error(message));
            }
        }
    }

    /// Handles a non-packet engine event (on the core's runtime).
    pub(crate) fn audio_event(&self, event: AudioEvent) {
        match event {
            AudioEvent::Packet(_) => {}
            AudioEvent::Speaking {
                participant,
                speaking,
            } => {
                let who = {
                    let state = self.voice().state();
                    state.session.as_ref().and_then(|s| {
                        let user = match participant {
                            Some(pid) => s.participants.get(&pid).copied(),
                            None => s.me,
                        };
                        user.map(|u| (s.inst.clone(), u))
                    })
                };
                if let Some((inst, user)) = who {
                    self.set_speaking(inst, user, speaking);
                }
            }
            AudioEvent::InputLevel(db) => self.emit(CoreEvent::InputLevel(db)),
            AudioEvent::DeviceChanged { input, name } => {
                let what = if input { "Microphone" } else { "Speakers" };
                self.emit(CoreEvent::Error(format!(
                    "{what} unavailable; now using {name}"
                )));
            }
            AudioEvent::DeviceError { input, message } => {
                let what = if input { "microphone" } else { "speakers" };
                self.emit(CoreEvent::Error(format!(
                    "Cannot open the {what}: {message}"
                )));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Screen shares
// ---------------------------------------------------------------------------------------------

impl Core {
    /// Starts sharing my screen in my voice room; frames go in with [`Core::send_screen_frame`].
    /// The server refuses without `STREAM` (the error arrives as [`CoreEvent::Error`]).
    pub fn start_screen_share(&self) -> Result<()> {
        let control = {
            let mut state = self.voice().state();
            let session = state
                .session
                .as_mut()
                .filter(|s| s.connected_once)
                .ok_or_else(|| CoreError::Io("Join a voice channel first".into()))?;
            session.my_share = true;
            session.my_watchers = None;
            session.control.clone()
        };
        let _ = control.send(ClientControl::ShareStart {});
        self.emit(CoreEvent::ScreenShares);
        Ok(())
    }

    pub fn stop_screen_share(&self) {
        let control = {
            let mut state = self.voice().state();
            state.session.as_mut().filter(|s| s.my_share).map(|s| {
                s.my_share = false;
                s.my_watchers = None;
                s.control.clone()
            })
        };
        if let Some(control) = control {
            let _ = control.send(ClientControl::ShareStop {});
            self.emit(CoreEvent::ScreenShares);
        }
    }

    /// Whether my own screen share runs.
    pub fn sharing_screen(&self) -> bool {
        self.voice()
            .state()
            .session
            .as_ref()
            .is_some_and(|s| s.my_share)
    }

    /// How many watch my screen share; `None` while not sharing or when the server does not
    /// tell (an older one: assume someone does).
    pub fn share_watchers(&self) -> Option<u16> {
        self.voice()
            .state()
            .session
            .as_ref()
            .filter(|s| s.my_share)
            .and_then(|s| s.my_watchers)
    }

    /// The others in my voice room who share their screen.
    pub fn screen_sharers(&self) -> Vec<(InstanceId, Id)> {
        let state = self.voice().state();
        let Some(s) = state.session.as_ref() else {
            return Vec::new();
        };
        let mut users: Vec<(InstanceId, Id)> = s
            .sharing
            .iter()
            .filter_map(|pid| s.participants.get(pid))
            .map(|&user| (s.inst.clone(), user))
            .collect();
        users.sort();
        users
    }

    /// Receives exactly these users' screen shares (others in my room); the rest are stopped.
    pub fn watch_screens(&self, users: &[Id]) {
        let mut state = self.voice().state();
        let Some(s) = state.session.as_mut() else {
            return;
        };
        let wanted: HashSet<u16> = s
            .participants
            .iter()
            .filter(|(pid, user)| users.contains(user) && s.sharing.contains(pid))
            .map(|(&pid, _)| pid)
            .collect();
        let volumes = self.voice().volumes.lock().expect("volumes");
        for &pid in wanted.difference(&s.watching) {
            let _ = s.control.send(ClientControl::Watch {
                participant_id: pid,
                watching: true,
            });
            if let Some(&user) = s.participants.get(&pid) {
                let volume = volumes.get_share(&s.inst, user);
                self.voice().engine.set_share_volume(pid, volume);
            }
        }
        for &pid in s.watching.difference(&wanted) {
            let _ = s.control.send(ClientControl::Watch {
                participant_id: pid,
                watching: false,
            });
            self.voice().engine.remove_share(pid);
        }
        s.watching = wanted;
    }

    /// Whether the voice server forwards screen share sound (newer servers do).
    pub fn share_audio_supported(&self) -> bool {
        self.voice()
            .state()
            .session
            .as_ref()
            .is_some_and(|s| s.share_audio)
    }

    /// The volume of `user`'s screen share sound, 0–2.
    pub fn share_volume(&self, inst: &str, user: Id) -> f32 {
        self.voice()
            .volumes
            .lock()
            .expect("volumes")
            .get_share(inst, user)
    }

    /// 0–2 (200 %), saved in `user_volumes.toml`.
    pub fn set_share_volume(&self, inst: &str, user: Id, volume: f32) -> Result<()> {
        self.voice()
            .volumes
            .lock()
            .expect("volumes")
            .set_share(inst, user, volume)?;
        if let Some(pid) = self.participant_of(inst, user) {
            self.voice().engine.set_share_volume(pid, volume);
        }
        Ok(())
    }

    /// The picture of `user`'s share broke (a lost frame): ask them for a keyframe.
    pub fn request_keyframe(&self, user: Id) {
        let state = self.voice().state();
        let Some(s) = state.session.as_ref() else {
            return;
        };
        if let Some((&pid, _)) = s.participants.iter().find(|(_, u)| **u == user) {
            let _ = s.control.send(ClientControl::RequestKeyframe {
                participant_id: pid,
            });
        }
    }

    /// Queues one frame of my screen share. `false` when it was dropped (no share, or the
    /// connection is behind): the next frame should be a keyframe.
    pub fn send_screen_frame(&self, frame: ScreenFrame) -> bool {
        if !self.sharing_screen()
            || frame.data.len() + voice::VIDEO_HEADER_LEN > voice::MAX_VIDEO_FRAME
        {
            return false;
        }
        let Some(out) = self.voice().screen_out.lock().expect("screen out").clone() else {
            return false;
        };
        let header = VideoHeader {
            keyframe: frame.keyframe,
            sender_id: 0,
            width: frame.width,
            height: frame.height,
            timestamp: frame.timestamp,
        };
        let mut data = Vec::with_capacity(voice::VIDEO_HEADER_LEN + frame.data.len());
        data.extend_from_slice(&header.encode());
        data.extend_from_slice(&frame.data);
        out.try_send(data).is_ok()
    }

    /// Where others' screen share frames go (the UI's decoder). Frames are dropped while it is
    /// full.
    pub fn set_screen_sink(&self, sink: mpsc::Sender<RemoteFrame>) {
        *self.voice().screen_sink.lock().expect("screen sink") = Some(sink);
    }

    /// Reads screen share frames from the server, one stream at a time (in order), and hands
    /// those of watched sharers to the sink.
    async fn read_screen(self, conn: quinn::Connection, generation: u64) {
        let mut stats = TransportStats::new("received");
        loop {
            let Ok(mut stream) = conn.accept_uni().await else {
                return;
            };
            let begin = Instant::now();
            let read =
                tokio::time::timeout(FRAME_TIMEOUT, stream.read_to_end(voice::MAX_VIDEO_FRAME))
                    .await;
            let Ok(Ok(data)) = read else {
                stats.failed += 1;
                continue;
            };
            let Ok(header) = VideoHeader::decode(&data) else {
                stats.failed += 1;
                continue;
            };
            stats.frame(data.len(), header.keyframe, begin.elapsed());
            let who = self.with_session(generation, |s| {
                s.watching
                    .contains(&header.sender_id)
                    .then(|| s.participants.get(&header.sender_id).copied())
                    .flatten()
                    .map(|user| (s.inst.clone(), user))
            });
            let Some(Some((inst, user))) = who else {
                stats.skipped += 1;
                continue;
            };
            let sink = self
                .voice()
                .screen_sink
                .lock()
                .expect("screen sink")
                .clone();
            if let Some(sink) = sink {
                let frame = RemoteFrame {
                    inst,
                    user,
                    header,
                    data: data[voice::VIDEO_HEADER_LEN..].to_vec(),
                    received: Instant::now(),
                };
                if sink.try_send(frame).is_err() {
                    stats.skipped += 1;
                }
            }
            stats.maybe_log();
        }
    }
}

/// The screen share transport's numbers in one direction, logged every [`SCREEN_STATS_EVERY`]
/// while frames flow: together with the server's and the encoder's lines they show where a
/// share loses time.
struct TransportStats {
    what: &'static str,
    since: Instant,
    frames: u32,
    keyframes: u32,
    bytes: u64,
    /// Time from a frame's stream being opened (or accepted) to its last byte.
    total: Duration,
    slowest: Duration,
    /// Received but not handed on (not watched, or the decoder's queue was full).
    skipped: u32,
    /// Streams that failed, timed out or carried an invalid header.
    failed: u32,
}

impl TransportStats {
    fn new(what: &'static str) -> Self {
        TransportStats {
            what,
            since: Instant::now(),
            frames: 0,
            keyframes: 0,
            bytes: 0,
            total: Duration::ZERO,
            slowest: Duration::ZERO,
            skipped: 0,
            failed: 0,
        }
    }

    fn frame(&mut self, bytes: usize, keyframe: bool, took: Duration) {
        self.frames += 1;
        self.keyframes += u32::from(keyframe);
        self.bytes += bytes as u64;
        self.total += took;
        self.slowest = self.slowest.max(took);
    }

    fn maybe_log(&mut self) {
        let elapsed = self.since.elapsed();
        if elapsed < SCREEN_STATS_EVERY {
            return;
        }
        let secs = elapsed.as_secs_f64();
        tracing::info!(
            "screen share transport ({}): {:.1} fps ({} keyframes), {:.0} kbit/s, {:.1} ms per \
             frame on the wire (slowest {:.1} ms), {} not handed on, {} failed",
            self.what,
            f64::from(self.frames) / secs,
            self.keyframes,
            self.bytes as f64 * 8.0 / 1000.0 / secs,
            self.total.as_secs_f64() * 1000.0 / f64::from(self.frames.max(1)),
            self.slowest.as_secs_f64() * 1000.0,
            self.skipped,
            self.failed,
        );
        *self = TransportStats::new(self.what);
    }
}

/// Writes my screen share frames, one unidirectional stream each, in order.
async fn write_screen(conn: quinn::Connection, mut frames: mpsc::Receiver<Vec<u8>>) {
    let mut stats = TransportStats::new("sent");
    while let Some(frame) = frames.recv().await {
        let begin = Instant::now();
        let Ok(mut stream) = conn.open_uni().await else {
            return;
        };
        if stream.write_all(&frame).await.is_ok() {
            let _ = stream.finish();
            let keyframe = VideoHeader::decode(&frame).is_ok_and(|h| h.keyframe);
            stats.frame(frame.len(), keyframe, begin.elapsed());
        } else {
            stats.failed += 1;
        }
        stats.maybe_log();
    }
}

/// Resolves the endpoint of a `/voice/join` answer and connects with the pinned certificate.
async fn connect(join: &VoiceJoinResponse) -> std::result::Result<quinn::Connection, String> {
    let failed = |e: &dyn std::fmt::Display| format!("Voice connection failed: {e}");
    let fingerprint = Fingerprint::parse(&join.quic.cert_sha256).map_err(|e| failed(&e))?;
    let crypto = crate::tls::voice_config(fingerprint).map_err(|e| failed(&e))?;
    let crypto = QuicClientConfig::try_from(crypto).map_err(|e| failed(&e))?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(transport_config()));
    let host = join.quic.host.as_str();
    let addr: SocketAddr = tokio::net::lookup_host((host, join.quic.port))
        .await
        .map_err(|e| failed(&e))?
        .next()
        .ok_or_else(|| failed(&"no address"))?;
    let bind: SocketAddr = if addr.is_ipv6() {
        "[::]:0".parse().expect("valid")
    } else {
        "0.0.0.0:0".parse().expect("valid")
    };
    let mut endpoint = quinn::Endpoint::client(bind).map_err(|e| failed(&e))?;
    endpoint.set_default_client_config(config);
    let server_name = if host.parse::<std::net::IpAddr>().is_ok() || host.is_empty() {
        "hermes-voice"
    } else {
        host
    };
    let connecting = endpoint
        .connect(addr, server_name)
        .map_err(|e| failed(&e))?;
    match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
        Ok(Ok(conn)) => Ok(conn),
        Ok(Err(err)) => Err(failed(&err)),
        Err(_) => Err("Voice connection timed out (is UDP blocked?)".into()),
    }
}

fn transport_config() -> quinn::TransportConfig {
    let mut transport = quinn::TransportConfig::default();
    if let Ok(idle) = quinn::IdleTimeout::try_from(Duration::from_millis(voice::IDLE_TIMEOUT_MS)) {
        transport.max_idle_timeout(Some(idle));
    }
    transport.keep_alive_interval(Some(Duration::from_millis(voice::KEEP_ALIVE_MS)));
    // One stream per screen share frame; the server writes them one at a time.
    transport.max_concurrent_uni_streams(32u32.into());
    transport.stream_receive_window(voice::VIDEO_STREAM_WINDOW.into());
    transport.receive_window(voice::VIDEO_CONNECTION_WINDOW.into());
    transport.send_window(u64::from(voice::VIDEO_CONNECTION_WINDOW));
    transport
}

async fn write(send: &mut quinn::SendStream, msg: &ClientControl) -> std::result::Result<(), ()> {
    send.write_all(&voice::encode_frame(msg))
        .await
        .map_err(|_| ())
}

/// Reads control frames until the stream ends or a frame is invalid.
async fn read_frames(mut recv: quinn::RecvStream, frames: mpsc::Sender<ServerControl>) {
    loop {
        let mut prefix = [0u8; 4];
        if recv.read_exact(&mut prefix).await.is_err() {
            return;
        }
        let Some(len) = voice::frame_len(prefix) else {
            return;
        };
        let mut body = vec![0u8; len];
        if recv.read_exact(&mut body).await.is_err() {
            return;
        }
        // Frames this client does not know (newer servers) are skipped.
        if let Ok(msg) = serde_json::from_slice(&body)
            && frames.send(msg).await.is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_map_both_ways() {
        let mut v = config::Voice::default();
        let s = audio_settings(&v);
        assert_eq!(s.input_device, None);
        assert_eq!(s.mode, InputMode::VoiceActivity);
        assert_eq!(s.vad_sensitivity_db, -50.0);
        assert_eq!(s.ptt_release_delay, Duration::from_millis(150));
        let changed = AudioSettings {
            input_device: Some("Yeti X".into()),
            mode: InputMode::PushToTalk,
            sounds_volume: 0.25,
            ..s
        };
        store_settings(&mut v, &changed);
        assert_eq!(v.input_device, "Yeti X");
        assert_eq!(v.output_device, "default");
        assert_eq!(v.mode, "push_to_talk");
        assert_eq!(v.sounds_volume, 0.25);
        assert_eq!(audio_settings(&v), changed);
    }
}
