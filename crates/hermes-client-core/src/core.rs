//! `Core`: the UI-facing API (PLAN.md section 14.2). It owns the instances (REST client, gateway
//! task, outbox), the store, the config and the known hosts, and reports everything through one
//! event stream.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};
use std::task::{Context, Poll};
use std::time::Duration;

use hermes_proto::api::{HistoryQuery, InstanceResponse, LoginRequest, RegisterRequest};
use hermes_proto::gateway::{ServerEvent, SpaceSnapshot};
use hermes_proto::id::Id;
use hermes_proto::models::{Attachment, Invite, Message, PresenceStatus};
use hermes_proto::{ErrorCode, PROTOCOL_VERSION};
use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;

use crate::address::{AddressError, Fingerprint, HostPort};
use crate::config::{self, ClientConfig, ConfigError, ConfigPaths, InstanceEntry};
use crate::gateway::{self, Command, ConnStatus, Gateway, GatewayEvent};
use crate::known_hosts::KnownHosts;
use crate::notify::{self, Notification};
use crate::rest::{self, Rest};
use crate::store::{
    Changed, HistoryAnchor, InstanceStore, LocalId, PAGE, PendingMessage, PendingState,
    SNAPSHOT_MEMBER_CAP,
};
use crate::tls::{Recorder, Trust};
use crate::tokens::TokenStore;
use crate::voice::{Voice, VoiceStatus};
use crate::volumes::UserVolumes;
use hermes_audio::Io;

/// `host`, plus `:port` when not 7700 (the `id` of `[[instances]]`).
pub type InstanceId = String;

/// Unsent messages kept while offline (section 14.3).
pub const MAX_QUEUED: usize = 50;
const READ_DEBOUNCE: Duration = Duration::from_secs(1);
const TYPING_THROTTLE: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq)]
pub enum CoreEvent {
    InstanceStatus {
        inst: InstanceId,
        status: ConnStatus,
    },
    StoreChanged {
        inst: InstanceId,
        what: Changed,
    },
    Notification {
        inst: InstanceId,
        notification: Notification,
    },
    IncomingCall {
        inst: InstanceId,
        channel: Id,
        caller: Id,
    },
    /// The ringing for a call stopped (the caller left, or you joined from elsewhere).
    CallRingStopped {
        inst: InstanceId,
        channel: Id,
    },
    /// `config.toml` changed on disk and was reloaded.
    ConfigReloaded {
        warnings: Vec<String>,
    },
    /// Something the user should see (failed background action).
    Error(String),
    VoiceStatus(VoiceStatus),
    /// Someone in your voice room (you included) started or stopped speaking.
    Speaking {
        inst: InstanceId,
        user: Id,
        speaking: bool,
    },
    /// Microphone level in dBFS, 20 times a second while [`Core::set_level_meter`] is on.
    InputLevel(f32),
    /// Who shares their screen in your voice room changed, or your own share started or
    /// stopped ([`Core::screen_sharers`], [`Core::sharing_screen`]).
    ScreenShares,
    /// Someone needs a keyframe of your screen share: encode the next frame as one.
    KeyframeRequested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    Address(AddressError),
    Rest(rest::Error),
    Config(String),
    UnknownInstance(InstanceId),
    /// The server speaks another protocol version.
    UnsupportedProtocol {
        server: u32,
    },
    /// Not CA-valid and not pinned: show the fingerprint and ask (trust on first use).
    UntrustedCertificate(Fingerprint),
    /// The pinned fingerprint changed: refuse, with an explicit "trust the new one" action.
    CertificateChanged {
        pinned: Fingerprint,
        now: Fingerprint,
    },
    Io(String),
    /// Joining or keeping voice failed.
    Voice(String),
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoreError::Address(e) => e.fmt(f),
            CoreError::Rest(e) => e.fmt(f),
            CoreError::Config(e) => f.write_str(e),
            CoreError::UnknownInstance(id) => write!(f, "no instance {id}"),
            CoreError::UnsupportedProtocol { server } => write!(
                f,
                "this server uses protocol version {server}, this client supports {PROTOCOL_VERSION}; \
                 update the {} side",
                if *server > PROTOCOL_VERSION {
                    "client"
                } else {
                    "server"
                }
            ),
            CoreError::UntrustedCertificate(fp) => write!(
                f,
                "the server's certificate is self-signed; its fingerprint is {}",
                fp.colon_hex()
            ),
            CoreError::CertificateChanged { pinned, now } => write!(
                f,
                "the server's certificate changed (was {}, now {}). If the owner re-generated \
                 it, trust the new one; otherwise someone may be intercepting the connection",
                pinned.colon_hex(),
                now.colon_hex()
            ),
            CoreError::Io(e) | CoreError::Voice(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for CoreError {}

impl From<rest::Error> for CoreError {
    fn from(e: rest::Error) -> Self {
        CoreError::Rest(e)
    }
}

impl From<AddressError> for CoreError {
    fn from(e: AddressError) -> Self {
        CoreError::Address(e)
    }
}

impl From<ConfigError> for CoreError {
    fn from(e: ConfigError) -> Self {
        CoreError::Config(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;

/// How the server certificate relates to what this client trusts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertStatus {
    /// Valid for normal CA validation.
    Trusted,
    /// Matches the pinned fingerprint.
    Pinned,
    /// Self-signed and not pinned yet: ask the user.
    Unknown,
    /// Different from the pinned fingerprint.
    Changed { pinned: Fingerprint },
}

/// Result of probing an address before adding it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub host: HostPort,
    pub info: InstanceResponse,
    pub fingerprint: Fingerprint,
    pub cert: CertStatus,
}

/// A message to send.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    pub content: String,
    pub reply_to: Option<Id>,
    pub attachments: Vec<Attachment>,
}

/// All instance stores. Read with [`Core::store`].
#[derive(Debug, Default)]
pub struct Store {
    pub instances: HashMap<InstanceId, InstanceStore>,
}

impl Store {
    pub fn instance(&self, inst: &str) -> Option<&InstanceStore> {
        self.instances.get(inst)
    }
}

/// Options for [`Core::new`].
pub struct CoreOptions {
    /// Sent in `identify` (`hermes-gui`, `hermes-tui`).
    pub client_name: String,
    /// `None`: nothing is read from or written to disk (tests).
    pub paths: Option<ConfigPaths>,
    pub tokens: TokenStore,
    pub known_hosts: KnownHosts,
    pub user_volumes: UserVolumes,
    /// The audio devices, or a virtual device (voicebot, tests).
    pub audio: Io,
}

impl fmt::Debug for CoreOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoreOptions")
            .field("client_name", &self.client_name)
            .field("paths", &self.paths)
            .finish_non_exhaustive()
    }
}

impl CoreOptions {
    /// Real files in `paths`, tokens in the OS keyring.
    pub fn with_paths(
        client_name: &str,
        paths: ConfigPaths,
    ) -> std::result::Result<Self, ConfigError> {
        Ok(CoreOptions {
            client_name: client_name.to_owned(),
            known_hosts: KnownHosts::load(&paths.known_hosts())?,
            tokens: TokenStore::Keyring {
                fallback: paths.tokens(),
            },
            user_volumes: UserVolumes::load(&paths.user_volumes()),
            audio: Io::Devices,
            paths: Some(paths),
        })
    }

    /// Everything in memory.
    pub fn in_memory(client_name: &str) -> Self {
        CoreOptions {
            client_name: client_name.to_owned(),
            paths: None,
            tokens: TokenStore::memory(),
            known_hosts: KnownHosts::in_memory(),
            user_volumes: UserVolumes::in_memory(),
            audio: Io::Devices,
        }
    }
}

/// The event stream of a [`Core`].
#[derive(Debug)]
pub struct CoreEvents(mpsc::UnboundedReceiver<CoreEvent>);

impl CoreEvents {
    pub async fn recv(&mut self) -> Option<CoreEvent> {
        self.0.recv().await
    }

    pub fn try_recv(&mut self) -> Option<CoreEvent> {
        self.0.try_recv().ok()
    }
}

impl futures_util::Stream for CoreEvents {
    type Item = CoreEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<CoreEvent>> {
        self.0.poll_recv(cx)
    }
}

#[derive(Debug, Default)]
struct Outbox {
    queue: Mutex<VecDeque<(Id, LocalId)>>,
    wake: Notify,
}

#[derive(Debug)]
struct InstanceHandle {
    username: String,
    rest: Rest,
    gateway: Gateway,
    status: Mutex<ConnStatus>,
    connected: tokio::sync::watch::Sender<bool>,
    outbox: Outbox,
    /// channel → newest message to ack, waiting for the debounce.
    read_marks: Mutex<HashMap<Id, Id>>,
    typing_sent: Mutex<HashMap<Id, Instant>>,
}

#[derive(Debug, Default)]
struct Focus {
    app_focused: bool,
    visible: HashSet<(InstanceId, Id)>,
}

pub(crate) struct Inner {
    client_name: String,
    paths: Option<ConfigPaths>,
    pub(crate) config: RwLock<ClientConfig>,
    known_hosts: Mutex<KnownHosts>,
    tokens: TokenStore,
    store: RwLock<Store>,
    instances: Mutex<HashMap<InstanceId, Arc<InstanceHandle>>>,
    events: mpsc::UnboundedSender<CoreEvent>,
    next_local: AtomicU64,
    focus: Mutex<Focus>,
    presence: Mutex<PresenceStatus>,
    watcher: Mutex<Option<::notify::RecommendedWatcher>>,
    pub(crate) voice: Voice,
}

/// The client core. Cheap to clone. Must be created inside a tokio runtime.
#[derive(Clone)]
pub struct Core {
    pub(crate) inner: Arc<Inner>,
}

impl fmt::Debug for Core {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Core").finish_non_exhaustive()
    }
}

/// Unix ms now.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A random UUID v4 string, used as the message nonce.
pub fn uuid_v4() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

impl Core {
    pub fn new(config: ClientConfig, options: CoreOptions) -> (Core, CoreEvents) {
        // rustls needs a process-wide provider for reqwest's internals.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (tx, rx) = mpsc::unbounded_channel();
        let (audio_tx, audio_rx) = mpsc::unbounded_channel();
        let voice = Voice::new(
            options.audio,
            crate::voice::audio_settings(&config.voice),
            options.user_volumes,
            audio_tx,
        );
        let core = Core {
            inner: Arc::new(Inner {
                client_name: options.client_name,
                paths: options.paths,
                config: RwLock::new(config),
                known_hosts: Mutex::new(options.known_hosts),
                tokens: options.tokens,
                store: RwLock::new(Store::default()),
                instances: Mutex::new(HashMap::new()),
                events: tx,
                next_local: AtomicU64::new(1),
                focus: Mutex::new(Focus {
                    app_focused: true,
                    visible: HashSet::new(),
                }),
                presence: Mutex::new(PresenceStatus::Online),
                watcher: Mutex::new(None),
                voice,
            }),
        };
        core.spawn_ticker();
        core.spawn_audio_events(audio_rx);
        (core, CoreEvents(rx))
    }

    /// Engine events (speaking, levels, devices) into core events. Holds the core weakly.
    fn spawn_audio_events(&self, mut rx: mpsc::UnboundedReceiver<hermes_audio::AudioEvent>) {
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                Core { inner }.audio_event(event);
            }
        });
    }

    pub(crate) fn emit(&self, event: CoreEvent) {
        let _ = self.inner.events.send(event);
    }

    fn changed(&self, inst: &str, what: impl IntoIterator<Item = Changed>) {
        for what in what {
            self.emit(CoreEvent::StoreChanged {
                inst: inst.to_owned(),
                what,
            });
        }
    }

    /// Read access to every instance's state. Do not hold it across an `.await`.
    pub fn store(&self) -> RwLockReadGuard<'_, Store> {
        self.inner.store.read().expect("store lock")
    }

    fn with_store<T>(&self, inst: &str, f: impl FnOnce(&mut InstanceStore) -> T) -> Option<T> {
        let mut store = self.inner.store.write().expect("store lock");
        store.instances.get_mut(inst).map(f)
    }

    pub fn config(&self) -> ClientConfig {
        self.inner.config.read().expect("config lock").clone()
    }

    pub fn paths(&self) -> Option<&ConfigPaths> {
        self.inner.paths.as_ref()
    }

    pub(crate) fn save_config(&self) -> Result<()> {
        if let Some(paths) = &self.inner.paths {
            let config = self.config();
            config::save(&paths.config(), &config)?;
        }
        Ok(())
    }

    /// Changes one setting (`:set buffer.group_minutes 10`) and writes `config.toml`.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let new = config::set_value(&self.config(), key, value)?;
        *self.inner.config.write().expect("config lock") = new;
        self.save_config()
    }

    /// Starts watching `config.toml` for changes (hot reload).
    pub fn watch_config(&self) -> Result<()> {
        let Some(paths) = &self.inner.paths else {
            return Ok(());
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let watcher =
            config::watch(paths.config(), tx).map_err(|e| CoreError::Io(e.to_string()))?;
        *self.inner.watcher.lock().expect("watcher lock") = Some(watcher);
        let core = self.clone();
        tokio::spawn(async move {
            while let Some(result) = rx.recv().await {
                match result {
                    Ok(loaded) => {
                        *core.inner.config.write().expect("config lock") = loaded.config;
                        core.apply_audio_settings();
                        core.emit(CoreEvent::ConfigReloaded {
                            warnings: loaded.warnings,
                        });
                    }
                    Err(err) => core.emit(CoreEvent::Error(format!("config not reloaded: {err}"))),
                }
            }
        });
        Ok(())
    }

    // ---- instances ------------------------------------------------------------------------

    pub fn instance_ids(&self) -> Vec<InstanceId> {
        self.config().instances.into_iter().map(|i| i.id).collect()
    }

    pub fn status(&self, inst: &str) -> ConnStatus {
        self.handle(inst).map_or(ConnStatus::LoggedOut, |h| {
            h.status.lock().expect("status").clone()
        })
    }

    fn handle(&self, inst: &str) -> Option<Arc<InstanceHandle>> {
        self.inner
            .instances
            .lock()
            .expect("instances lock")
            .get(inst)
            .cloned()
    }

    fn handle_or_err(&self, inst: &str) -> Result<Arc<InstanceHandle>> {
        self.handle(inst)
            .ok_or_else(|| CoreError::UnknownInstance(inst.to_owned()))
    }

    /// The signed-in REST client of an instance, for every endpoint without its own method.
    pub fn rest(&self, inst: &str) -> Result<Rest> {
        Ok(self.handle_or_err(inst)?.rest.clone())
    }

    fn trust(&self, host: &HostPort) -> Trust {
        match self
            .inner
            .known_hosts
            .lock()
            .expect("known hosts")
            .get(host)
        {
            Some(fp) => Trust::Pinned(fp),
            None => Trust::Ca,
        }
    }

    /// Connects every saved instance that has `autoconnect` and a stored token. Instances
    /// without a token report `LoggedOut`.
    pub fn start(&self) {
        for entry in self.config().instances {
            if !entry.autoconnect {
                continue;
            }
            match self.inner.tokens.get(&entry.id, &entry.username) {
                Some(token) => {
                    if let Err(err) = self.connect(&entry, &token) {
                        self.emit(CoreEvent::Error(format!("{}: {err}", entry.id)));
                    }
                }
                None => self.emit(CoreEvent::InstanceStatus {
                    inst: entry.id.clone(),
                    status: ConnStatus::LoggedOut,
                }),
            }
        }
    }

    /// `GET /instance` plus the certificate, before adding an instance.
    pub async fn probe_instance(&self, host: &HostPort) -> Result<Probe> {
        let recorder = Recorder::new();
        let config = recorder
            .client_config()
            .map_err(|e| CoreError::Io(e.to_string()))?;
        let rest = Rest::with_tls((*config).clone(), host)?;
        let info = rest.instance().await?;
        let seen = recorder
            .seen()
            .ok_or_else(|| CoreError::Io("no certificate seen".into()))?;
        let pinned = self
            .inner
            .known_hosts
            .lock()
            .expect("known hosts")
            .get(host);
        let cert = match pinned {
            Some(p) if p == seen.fingerprint => CertStatus::Pinned,
            Some(pinned) => CertStatus::Changed { pinned },
            None if seen.ca_valid => CertStatus::Trusted,
            None => CertStatus::Unknown,
        };
        if info.protocol_version != PROTOCOL_VERSION {
            return Err(CoreError::UnsupportedProtocol {
                server: info.protocol_version,
            });
        }
        Ok(Probe {
            host: host.clone(),
            info,
            fingerprint: seen.fingerprint,
            cert,
        })
    }

    /// Pins a fingerprint for a host (trust on first use, an invite's `#fp=`, or "I re-generated
    /// the certificate").
    pub fn trust_fingerprint(&self, host: &HostPort, fingerprint: Fingerprint) -> Result<()> {
        self.inner
            .known_hosts
            .lock()
            .expect("known hosts")
            .trust(host, fingerprint)?;
        Ok(())
    }

    /// An invite link's `#fp=` (PLAN.md section 13): pinned only for a host this client has
    /// never trusted. A host that is already pinned keeps its pin, and an instance that is
    /// already added (trusted through its CA certificate when it has no pin) is never switched
    /// to a pin by a link: a pin replaces CA validation, so a crafted link could otherwise
    /// hand someone else's certificate the trust of a real server. Returns whether it pinned.
    pub fn trust_invite_fingerprint(
        &self,
        host: &HostPort,
        fingerprint: Fingerprint,
    ) -> Result<bool> {
        let added = self
            .config()
            .instances
            .iter()
            .any(|i| i.url.authority() == host.authority());
        let mut known = self.inner.known_hosts.lock().expect("known hosts");
        if added || known.get(host).is_some() {
            return Ok(false);
        }
        known.trust(host, fingerprint)?;
        Ok(true)
    }

    /// Fails unless the host's certificate is trusted (CA-valid or pinned).
    async fn check_trusted(&self, host: &HostPort) -> Result<()> {
        let probe = self.probe_instance(host).await?;
        match probe.cert {
            CertStatus::Trusted | CertStatus::Pinned => Ok(()),
            CertStatus::Unknown => Err(CoreError::UntrustedCertificate(probe.fingerprint)),
            CertStatus::Changed { pinned } => Err(CoreError::CertificateChanged {
                pinned,
                now: probe.fingerprint,
            }),
        }
    }

    fn device_name(&self) -> String {
        format!("{} on {}", self.inner.client_name, std::env::consts::OS)
    }

    pub async fn login(
        &self,
        host: &HostPort,
        username: &str,
        password: &str,
    ) -> Result<InstanceId> {
        self.check_trusted(host).await?;
        let rest = Rest::new(host, self.trust(host))?;
        let res = rest
            .login(&LoginRequest {
                username: username.into(),
                password: password.into(),
                device_name: self.device_name(),
            })
            .await?;
        self.signed_in(host, &res.user.user.username, &res.token)
    }

    pub async fn register(
        &self,
        host: &HostPort,
        username: &str,
        password: &str,
        invite: Option<&str>,
    ) -> Result<InstanceId> {
        self.check_trusted(host).await?;
        let rest = Rest::new(host, self.trust(host))?;
        let res = rest
            .register(&RegisterRequest {
                username: username.into(),
                password: password.into(),
                invite_code: invite.map(str::to_owned),
                device_name: self.device_name(),
            })
            .await?;
        self.signed_in(host, &res.user.user.username, &res.token)
    }

    /// `GET /invites/{code}` without signing in: what an invite leads to (space name, icon,
    /// member count), shown before the account step.
    pub async fn invite_preview(&self, host: &HostPort, code: &str) -> Result<Invite> {
        self.check_trusted(host).await?;
        let rest = Rest::new(host, self.trust(host))?;
        Ok(rest.invite_preview(code).await?)
    }

    fn signed_in(&self, host: &HostPort, username: &str, token: &str) -> Result<InstanceId> {
        let id = host.display();
        self.inner
            .tokens
            .set(&id, username, token)
            .map_err(|e| CoreError::Io(format!("could not store the token: {e}")))?;
        let entry = InstanceEntry {
            id: id.clone(),
            url: host.clone(),
            username: username.to_owned(),
            autoconnect: true,
        };
        {
            let mut config = self.inner.config.write().expect("config lock");
            config.instances.retain(|i| i.id != id);
            config.instances.push(entry.clone());
        }
        self.save_config()?;
        self.connect(&entry, token)?;
        Ok(id)
    }

    /// Starts (or restarts) the gateway of an instance.
    fn connect(&self, entry: &InstanceEntry, token: &str) -> Result<()> {
        let trust = self.trust(&entry.url);
        let rest = Rest::new(&entry.url, trust)?.with_token(token);
        let (tx, rx) = mpsc::unbounded_channel();
        let gateway = Gateway::spawn(
            entry.url.clone(),
            trust,
            token.to_owned(),
            gateway::client_info(&self.inner.client_name),
            tx,
        );
        let presence = *self.inner.presence.lock().expect("presence");
        if presence != PresenceStatus::Online {
            gateway.send(Command::Presence(presence));
        }
        let (connected, _) = tokio::sync::watch::channel(false);
        let handle = Arc::new(InstanceHandle {
            username: entry.username.clone(),
            rest,
            gateway,
            status: Mutex::new(ConnStatus::Connecting),
            connected,
            outbox: Outbox::default(),
            read_marks: Mutex::new(HashMap::new()),
            typing_sent: Mutex::new(HashMap::new()),
        });
        self.inner
            .instances
            .lock()
            .expect("instances lock")
            .insert(entry.id.clone(), handle.clone());
        self.inner
            .store
            .write()
            .expect("store lock")
            .instances
            .entry(entry.id.clone())
            .or_default();
        tokio::spawn(self.clone().pump(entry.id.clone(), handle.clone(), rx));
        tokio::spawn(self.clone().outbox(entry.id.clone(), handle));
        Ok(())
    }

    /// Signs out (revoking the session on the server), forgets the token and disconnects. The
    /// instance stays in the config so signing in again is quick.
    pub async fn logout(&self, inst: &str) -> Result<()> {
        let handle = self.handle_or_err(inst)?;
        let _ = handle.rest.logout().await;
        self.disconnect(inst, &handle);
        Ok(())
    }

    fn disconnect(&self, inst: &str, handle: &InstanceHandle) {
        self.leave_voice_on(inst);
        let _ = self.inner.tokens.delete(inst, &handle.username);
        self.inner
            .instances
            .lock()
            .expect("instances lock")
            .remove(inst);
        self.inner
            .store
            .write()
            .expect("store lock")
            .instances
            .remove(inst);
        self.emit(CoreEvent::InstanceStatus {
            inst: inst.to_owned(),
            status: ConnStatus::LoggedOut,
        });
        self.changed(inst, [Changed::All]);
    }

    /// Logs out and removes the instance from the config.
    pub async fn remove_instance(&self, inst: &str) -> Result<()> {
        if self.handle(inst).is_some() {
            self.logout(inst).await?;
        }
        self.inner
            .config
            .write()
            .expect("config lock")
            .instances
            .retain(|i| i.id != inst);
        self.save_config()
    }

    /// Goes offline (keeping the session to resume) or back online.
    pub fn set_online(&self, inst: &str, online: bool) -> Result<()> {
        let handle = self.handle_or_err(inst)?;
        handle.gateway.send(if online {
            Command::Online
        } else {
            Command::Offline
        });
        Ok(())
    }

    /// Changes my presence on every instance.
    pub fn set_presence(&self, status: PresenceStatus) {
        *self.inner.presence.lock().expect("presence") = status;
        for handle in self.inner.instances.lock().expect("instances").values() {
            handle.gateway.send(Command::Presence(status));
        }
    }

    pub fn presence(&self) -> PresenceStatus {
        *self.inner.presence.lock().expect("presence")
    }

    /// The web invite link for `code`, with `#fp=` when the instance's certificate is pinned.
    pub fn invite_link(&self, inst: &str, code: &str) -> Option<String> {
        let config = self.config();
        let host = &config.instance(inst)?.url;
        let fragment = self
            .inner
            .known_hosts
            .lock()
            .expect("known hosts")
            .get(host)
            .map_or_else(String::new, |fp| format!("#fp={fp}"));
        Some(format!("{}/invite/{code}{fragment}", host.url()))
    }

    /// Joins a space through an invite code on a signed-in instance.
    pub async fn join_invite(&self, inst: &str, code: &str) -> Result<SpaceSnapshot> {
        let rest = self.rest(inst)?;
        let snapshot = rest.join_invite(code).await?;
        let space = snapshot.space.id;
        let big = snapshot.members.len() >= SNAPSHOT_MEMBER_CAP;
        let changed = self.with_store(inst, |s| {
            s.apply(
                ServerEvent::SpaceCreate(Box::new(snapshot.clone())),
                now_ms(),
            )
        });
        self.changed(inst, changed.unwrap_or_default());
        if big {
            self.spawn_members(inst, space);
        }
        Ok(snapshot)
    }

    // ---- gateway events -------------------------------------------------------------------

    async fn pump(
        self,
        inst: InstanceId,
        handle: Arc<InstanceHandle>,
        mut rx: mpsc::UnboundedReceiver<GatewayEvent>,
    ) {
        while let Some(event) = rx.recv().await {
            // A newer connection replaced this one (signed in again).
            if !self.handle(&inst).is_some_and(|h| Arc::ptr_eq(&h, &handle)) {
                return;
            }
            match event {
                GatewayEvent::Status(status) => {
                    *handle.status.lock().expect("status") = status.clone();
                    let _ = handle.connected.send(status == ConnStatus::Connected);
                    if status == ConnStatus::Connected {
                        handle.outbox.wake.notify_one();
                    }
                    if status == ConnStatus::LoggedOut {
                        let _ = self.inner.tokens.delete(&inst, &handle.username);
                    }
                    self.emit(CoreEvent::InstanceStatus {
                        inst: inst.clone(),
                        status,
                    });
                }
                GatewayEvent::Ready(ready) => {
                    let spaces: Vec<Id> = ready
                        .spaces
                        .iter()
                        .filter(|s| s.members.len() >= SNAPSHOT_MEMBER_CAP)
                        .map(|s| s.space.id)
                        .collect();
                    self.with_store(&inst, |s| s.apply_ready(*ready));
                    self.changed(&inst, [Changed::All]);
                    for space in spaces {
                        self.spawn_members(&inst, space);
                    }
                    // Reload what is on screen; other windows load when opened.
                    let visible: Vec<Id> = self.visible_channels(&inst);
                    for channel in visible {
                        let core = self.clone();
                        let inst = inst.clone();
                        tokio::spawn(async move {
                            let _ = core
                                .load_history(&inst, channel, HistoryAnchor::Latest)
                                .await;
                        });
                    }
                }
                GatewayEvent::Resumed => {}
                GatewayEvent::Dispatch(event) => self.dispatch(&inst, *event),
            }
        }
    }

    fn dispatch(&self, inst: &str, event: ServerEvent) {
        let now = now_ms();
        match &event {
            ServerEvent::CallRing {
                channel_id,
                caller_id,
            } => self.emit(CoreEvent::IncomingCall {
                inst: inst.to_owned(),
                channel: *channel_id,
                caller: *caller_id,
            }),
            ServerEvent::CallRingStop { channel_id } => self.emit(CoreEvent::CallRingStopped {
                inst: inst.to_owned(),
                channel: *channel_id,
            }),
            ServerEvent::SpaceCreate(snapshot) if snapshot.members.len() >= SNAPSHOT_MEMBER_CAP => {
                self.spawn_members(inst, snapshot.space.id);
            }
            _ => {}
        }
        let message = match &event {
            ServerEvent::MessageCreate(m) => Some(m.clone()),
            _ => None,
        };
        // A message request notifies once, when it first arrives.
        let request = match &event {
            ServerEvent::DmCreate(dm) if dm.request => self
                .with_store(inst, |s| !s.dms.get(&dm.id).is_some_and(|d| d.request))
                .unwrap_or(false)
                .then(|| dm.clone()),
            _ => None,
        };
        let Some(changed) = self.with_store(inst, |s| s.apply(event, now)) else {
            return;
        };
        self.changed(inst, changed);
        if let Some(message) = message {
            self.maybe_notify(inst, &message, now);
        }
        if let Some(dm) = request
            && self.config().notifications.enabled
        {
            let notification = {
                let store = self.store();
                store
                    .instance(inst)
                    .and_then(|s| notify::request(s, &dm, self.presence()))
            };
            if let Some(notification) = notification {
                self.emit(CoreEvent::Notification {
                    inst: inst.to_owned(),
                    notification,
                });
            }
        }
    }

    fn maybe_notify(&self, inst: &str, message: &Message, now: i64) {
        if !self.config().notifications.enabled {
            return;
        }
        let focused = {
            let focus = self.inner.focus.lock().expect("focus");
            focus.app_focused
                && focus
                    .visible
                    .contains(&(inst.to_owned(), message.channel_id))
        };
        let presence = self.presence();
        let store = self.store();
        let Some(s) = store.instance(inst) else {
            return;
        };
        if let Some(notification) = notify::check(s, message, focused, presence, now) {
            drop(store);
            self.emit(CoreEvent::Notification {
                inst: inst.to_owned(),
                notification,
            });
        }
    }

    fn spawn_members(&self, inst: &str, space: Id) {
        let Some(handle) = self.handle(inst) else {
            return;
        };
        let core = self.clone();
        let inst = inst.to_owned();
        tokio::spawn(async move {
            match handle.rest.all_members(space).await {
                Ok(members) => {
                    if core.with_store(&inst, |s| s.set_members(space, members)) == Some(true) {
                        core.changed(&inst, [Changed::Members(space)]);
                    }
                }
                Err(err) => tracing::warn!("loading members of {space}: {err}"),
            }
        });
    }

    // ---- focus ----------------------------------------------------------------------------

    /// The channels shown in visible panes, for notifications and window trimming.
    pub fn set_visible_channels(&self, channels: impl IntoIterator<Item = (InstanceId, Id)>) {
        let now = now_ms();
        let visible: HashSet<_> = channels.into_iter().collect();
        {
            let mut store = self.inner.store.write().expect("store lock");
            for (inst, channel) in &visible {
                if let Some(s) = store.instances.get_mut(inst) {
                    s.window_mut(*channel).last_viewed = now;
                }
            }
        }
        self.inner.focus.lock().expect("focus").visible = visible;
    }

    pub fn set_app_focused(&self, focused: bool) {
        self.inner.focus.lock().expect("focus").app_focused = focused;
    }

    fn visible_channels(&self, inst: &str) -> Vec<Id> {
        self.inner
            .focus
            .lock()
            .expect("focus")
            .visible
            .iter()
            .filter(|(i, _)| i == inst)
            .map(|(_, c)| *c)
            .collect()
    }

    /// Opens a channel: loads the latest page if nothing is loaded yet.
    pub async fn open_channel(&self, inst: &str, channel: Id) -> Result<()> {
        let loaded = self
            .with_store(inst, |s| {
                let w = s.window_mut(channel);
                w.last_viewed = now_ms();
                w.loaded
            })
            .ok_or_else(|| CoreError::UnknownInstance(inst.to_owned()))?;
        if !loaded {
            self.load_history(inst, channel, HistoryAnchor::Latest)
                .await?;
        }
        Ok(())
    }

    // ---- messages -------------------------------------------------------------------------

    pub async fn load_history(&self, inst: &str, channel: Id, anchor: HistoryAnchor) -> Result<()> {
        let rest = self.rest(inst)?;
        let mut query = HistoryQuery {
            limit: Some(PAGE),
            ..HistoryQuery::default()
        };
        match anchor {
            HistoryAnchor::Latest => {}
            HistoryAnchor::Before(id) => query.before = Some(id),
            HistoryAnchor::After(id) => query.after = Some(id),
            HistoryAnchor::Around(id) => query.around = Some(id),
        }
        let page = rest.messages(channel, &query).await?;
        self.with_store(inst, |s| s.merge_history(channel, anchor, page, PAGE));
        self.changed(inst, [Changed::Messages(channel)]);
        Ok(())
    }

    /// Sends a message optimistically: it shows up as pending right away and is replaced by the
    /// real one when the server accepts it. While disconnected, up to 50 messages queue up.
    pub fn send_message(&self, inst: &str, channel: Id, draft: Draft) -> Result<LocalId> {
        let handle = self.handle_or_err(inst)?;
        let local_id = self.inner.next_local.fetch_add(1, Ordering::Relaxed);
        let queued = handle.outbox.queue.lock().expect("outbox").len();
        let state = if queued >= MAX_QUEUED {
            PendingState::Failed("Too many unsent messages".into())
        } else if *handle.connected.borrow() {
            PendingState::Sending
        } else {
            PendingState::Queued
        };
        let failed = matches!(state, PendingState::Failed(_));
        let pending = PendingMessage {
            local_id,
            nonce: uuid_v4(),
            channel_id: channel,
            content: draft.content,
            reply_to: draft.reply_to,
            attachments: draft.attachments,
            state,
            created_at: now_ms(),
        };
        self.with_store(inst, |s| s.add_pending(pending));
        self.changed(inst, [Changed::Messages(channel)]);
        if !failed {
            handle
                .outbox
                .queue
                .lock()
                .expect("outbox")
                .push_back((channel, local_id));
            handle.outbox.wake.notify_one();
        }
        Ok(local_id)
    }

    /// Sends a failed message again.
    pub fn retry_message(&self, inst: &str, channel: Id, local_id: LocalId) -> Result<()> {
        let handle = self.handle_or_err(inst)?;
        let found = self.with_store(inst, |s| {
            s.pending_mut(channel, local_id)
                .map(|p| p.state = PendingState::Queued)
                .is_some()
        });
        if found == Some(true) {
            handle
                .outbox
                .queue
                .lock()
                .expect("outbox")
                .push_back((channel, local_id));
            handle.outbox.wake.notify_one();
            self.changed(inst, [Changed::Messages(channel)]);
        }
        Ok(())
    }

    /// Deletes a pending (failed or queued) message without sending it.
    pub fn discard_message(&self, inst: &str, channel: Id, local_id: LocalId) {
        if let Some(handle) = self.handle(inst) {
            handle
                .outbox
                .queue
                .lock()
                .expect("outbox")
                .retain(|&(_, id)| id != local_id);
        }
        self.with_store(inst, |s| s.remove_pending(channel, local_id));
        self.changed(inst, [Changed::Messages(channel)]);
    }

    fn set_pending_state(&self, inst: &str, channel: Id, local_id: LocalId, state: PendingState) {
        self.with_store(inst, |s| {
            if let Some(p) = s.pending_mut(channel, local_id) {
                p.state = state;
            }
        });
        self.changed(inst, [Changed::Messages(channel)]);
    }

    /// Sends queued messages one at a time, in order, while connected.
    async fn outbox(self, inst: InstanceId, handle: Arc<InstanceHandle>) {
        let mut connected = handle.connected.subscribe();
        loop {
            // Stop when the instance was removed or replaced.
            if !self.handle(&inst).is_some_and(|h| Arc::ptr_eq(&h, &handle)) {
                return;
            }
            if !*connected.borrow_and_update() {
                tokio::select! {
                    r = connected.changed() => if r.is_err() { return },
                    () = tokio::time::sleep(Duration::from_secs(30)) => {}
                }
                continue;
            }
            let next = handle.outbox.queue.lock().expect("outbox").pop_front();
            let Some((channel, local_id)) = next else {
                tokio::select! {
                    () = handle.outbox.wake.notified() => {}
                    () = tokio::time::sleep(Duration::from_secs(30)) => {}
                }
                continue;
            };
            let request = self.with_store(&inst, |s| {
                s.pending_mut(channel, local_id).map(|p| {
                    p.state = PendingState::Sending;
                    hermes_proto::api::SendMessageRequest {
                        content: p.content.clone(),
                        nonce: p.nonce.clone(),
                        reply_to_id: p.reply_to,
                        attachment_ids: p.attachments.iter().map(|a| a.id).collect(),
                    }
                })
            });
            let Some(Some(request)) = request else {
                continue; // discarded meanwhile
            };
            self.changed(&inst, [Changed::Messages(channel)]);
            match handle.rest.send_message(channel, &request).await {
                Ok(message) => {
                    let id = message.id;
                    self.with_store(&inst, |s| s.resolve_pending(local_id, message));
                    self.changed(
                        &inst,
                        [Changed::Messages(channel), Changed::ReadState(channel)],
                    );
                    self.mark_read(&inst, channel, id);
                }
                Err(rest::Error::Api(err)) if err.code == ErrorCode::RateLimited => {
                    handle
                        .outbox
                        .queue
                        .lock()
                        .expect("outbox")
                        .push_front((channel, local_id));
                    let wait = err.retry_after_ms.unwrap_or(1000).clamp(100, 60_000);
                    tokio::time::sleep(Duration::from_millis(wait)).await;
                }
                Err(err) if err.is_network() => {
                    handle
                        .outbox
                        .queue
                        .lock()
                        .expect("outbox")
                        .push_front((channel, local_id));
                    self.set_pending_state(&inst, channel, local_id, PendingState::Queued);
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(err) => {
                    self.set_pending_state(
                        &inst,
                        channel,
                        local_id,
                        PendingState::Failed(err.to_string()),
                    );
                }
            }
        }
    }

    pub async fn edit_message(
        &self,
        inst: &str,
        channel: Id,
        message: Id,
        content: &str,
    ) -> Result<()> {
        let updated = self
            .rest(inst)?
            .edit_message(channel, message, content)
            .await?;
        let changed = self.with_store(inst, |s| {
            s.apply(ServerEvent::MessageUpdate(updated), now_ms())
        });
        self.changed(inst, changed.unwrap_or_default());
        Ok(())
    }

    pub async fn delete_message(&self, inst: &str, channel: Id, message: Id) -> Result<()> {
        self.rest(inst)?.delete_message(channel, message).await?;
        let changed = self.with_store(inst, |s| {
            s.apply(
                ServerEvent::MessageDelete {
                    id: message,
                    channel_id: channel,
                },
                now_ms(),
            )
        });
        self.changed(inst, changed.unwrap_or_default());
        Ok(())
    }

    pub async fn react(&self, inst: &str, channel: Id, message: Id, emoji: &str) -> Result<()> {
        Ok(self.rest(inst)?.react(channel, message, emoji).await?)
    }

    pub async fn unreact(&self, inst: &str, channel: Id, message: Id, emoji: &str) -> Result<()> {
        Ok(self.rest(inst)?.unreact(channel, message, emoji).await?)
    }

    /// Marks a channel read up to `message`: shown at once, sent to the server after 1 s
    /// (only the newest mark of a burst is sent).
    pub fn mark_read(&self, inst: &str, channel: Id, message: Id) {
        let Some(handle) = self.handle(inst) else {
            return;
        };
        let already = self
            .with_store(inst, |s| {
                let read = s.read_states.get(&channel);
                if read.is_some_and(|r| r.last_read_id >= message && r.mention_count == 0) {
                    return true;
                }
                s.read_states.insert(
                    channel,
                    hermes_proto::models::ReadState {
                        channel_id: channel,
                        last_read_id: message,
                        mention_count: 0,
                    },
                );
                false
            })
            .unwrap_or(true);
        if already {
            return;
        }
        self.changed(inst, [Changed::ReadState(channel)]);
        let first = handle
            .read_marks
            .lock()
            .expect("read marks")
            .insert(channel, message)
            .is_none();
        if first {
            tokio::spawn(async move {
                tokio::time::sleep(READ_DEBOUNCE).await;
                let mark = handle
                    .read_marks
                    .lock()
                    .expect("read marks")
                    .remove(&channel);
                if let Some(message) = mark
                    && let Err(err) = handle.rest.ack(channel, message).await
                {
                    tracing::info!("ack {channel}: {err}");
                }
            });
        }
    }

    /// Tells others I am typing (at most every 8 s per channel).
    pub fn typing(&self, inst: &str, channel: Id) {
        let Some(handle) = self.handle(inst) else {
            return;
        };
        {
            let mut sent = handle.typing_sent.lock().expect("typing");
            let now = Instant::now();
            if sent
                .get(&channel)
                .is_some_and(|t| now.duration_since(*t) < TYPING_THROTTLE)
            {
                return;
            }
            sent.insert(channel, now);
        }
        tokio::spawn(async move {
            let _ = handle.rest.typing(channel).await;
        });
    }

    /// Uploads a file; attach the result to a [`Draft`].
    pub async fn upload(&self, inst: &str, path: &Path) -> Result<Attachment> {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| CoreError::Io(format!("{}: {e}", path.display())))?;
        let filename = path
            .file_name()
            .map_or_else(|| "file".to_owned(), |n| n.to_string_lossy().into_owned());
        let content_type = content_type(&filename);
        Ok(self
            .rest(inst)?
            .upload(&filename, content_type, bytes)
            .await?)
    }

    /// Expires typing indicators every second and trims idle message windows every minute.
    fn spawn_ticker(&self) {
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            let mut n = 0u64;
            loop {
                tick.tick().await;
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                let core = Core { inner };
                let now = now_ms();
                n += 1;
                let mut changes = Vec::new();
                {
                    let visible: Vec<(InstanceId, Id)> = core
                        .inner
                        .focus
                        .lock()
                        .expect("focus")
                        .visible
                        .iter()
                        .cloned()
                        .collect();
                    let mut store = core.inner.store.write().expect("store lock");
                    for (inst, s) in &mut store.instances {
                        for channel in s.expire_typing(now) {
                            changes.push((inst.clone(), Changed::Typing(channel)));
                        }
                        if n.is_multiple_of(60) {
                            let keep: Vec<Id> = visible
                                .iter()
                                .filter(|(i, _)| i == inst)
                                .map(|(_, c)| *c)
                                .collect();
                            for channel in s.trim_windows(now, &keep) {
                                changes.push((inst.clone(), Changed::Messages(channel)));
                            }
                        }
                    }
                }
                for (inst, what) in changes {
                    core.changed(&inst, [what]);
                }
            }
        });
    }
}

/// A content type from the file extension (the server detects images itself).
pub fn content_type(filename: &str) -> &'static str {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "txt" | "log" | "md" => "text/plain",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "mp3" => "audio/mpeg",
        "ogg" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_are_v4() {
        let a = uuid_v4();
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(a, uuid_v4());
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type("Cat.PNG"), "image/png");
        assert_eq!(content_type("notes"), "application/octet-stream");
    }
}
