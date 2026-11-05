//! End-to-end: the voice UI against a real `hermes-server` with its QUIC voice endpoint, on
//! virtual audio devices: joining puts you in the sidebar and the voice panel, the mute keybind,
//! the per-user volume menu, a DM call rings until the caller leaves, leaving, and the Voice &
//! Audio settings.

use std::sync::Arc;
use std::time::Duration;

use hermes_client_core::address::{Fingerprint, HostPort, InviteLink};
use hermes_client_core::audio::{FRAME, Io, VirtualIo};
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::api::CreateInviteRequest;
use hermes_client_core::proto::id::Id;
use hermes_client_core::{Core, CoreEvent, CoreEvents, CoreOptions, InstanceId};
use hermes_server::config::{Config, RegistrationMode, TlsMode};
use hermes_server::state::AppState;
use iced::keyboard::{Key, Modifiers, key::NativeCode, key::Physical};
use tokio::sync::Notify;

use crate::app::App;
use crate::buffer::{Buffer, settings};
use crate::message::Message;
use crate::sidebar::tree::Row;
use crate::sidebar::voice_panel;
use crate::voice::{self, VoiceMsg};

const WAIT: Duration = Duration::from_secs(15);

struct Server {
    host: HostPort,
    setup_code: String,
    fingerprint: Fingerprint,
    shutdown: Arc<Notify>,
    dir: std::path::PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.notify_one();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn start(name: &str) -> Server {
    let dir = std::env::temp_dir().join(format!("hermes-gui-voice-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut config = Config::default();
    config.instance.name = "Voice".into();
    config.storage.data_dir = dir.join("data");
    config.tls.mode = TlsMode::SelfSigned;
    config.registration.mode = RegistrationMode::Open;
    config.limits.requests_per_second = 100_000;
    config.limits.login_attempts_per_minute = 100_000;
    let tls = hermes_server::tls::load(&config).unwrap();
    let fp = *tls.fingerprint().unwrap();
    let (quic, cert_sha256) =
        hermes_server::voice::endpoint::bind(&config, "127.0.0.1:0".parse().unwrap()).unwrap();
    let mut state = AppState::load(config).await.unwrap();
    state.fingerprint = Some(fp);
    state
        .voice
        .set_endpoint(hermes_server::voice::EndpointInfo {
            cert_sha256,
            port: quic.local_addr().unwrap().port(),
        });
    let links = hermes_server::setup::first_start(&state, Some(&fp))
        .await
        .unwrap()
        .unwrap();
    let link = InviteLink::parse(&links.web).unwrap();
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = tcp.local_addr().unwrap().port();
    let shutdown = Arc::new(Notify::new());
    let signal = {
        let shutdown = shutdown.clone();
        async move { shutdown.notified().await }
    };
    tokio::spawn(hermes_server::server::run(
        Arc::new(state),
        tcp,
        tls,
        Some(quic),
        signal,
    ));
    Server {
        host: HostPort::parse(&format!("127.0.0.1:{port}")).unwrap(),
        setup_code: link.code,
        fingerprint: link.fingerprint.unwrap(),
        shutdown,
        dir,
    }
}

/// A silent microphone and speakers nobody listens to.
struct Silence;

impl VirtualIo for Silence {
    fn read(&mut self, frame: &mut [f32; FRAME]) {
        frame.fill(0.0);
    }

    fn write(&mut self, _frame: &[f32; FRAME]) {}
}

struct Client {
    app: App,
    events: CoreEvents,
}

impl Client {
    fn new() -> Client {
        let mut options = CoreOptions::in_memory("hermes-gui");
        options.audio = Io::Virtual(Box::new(Silence));
        let mut config = hermes_client_core::config::ClientConfig::default();
        config.voice.sounds = false;
        let (core, events) = Core::new(config, options);
        Client {
            app: App::new(core, None, None),
            events,
        }
    }

    fn core(&self) -> Core {
        self.app.core.clone()
    }

    /// Hands the core's events to the app until `f` holds.
    async fn until(&mut self, what: &str, f: impl Fn(&App) -> bool) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            while let Some(event) = self.events.try_recv() {
                let _ = self.app.update(Message::Core(event));
            }
            if f(&self.app) {
                return;
            }
            assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn sign_up(&mut self, server: &Server, name: &str, invite: &str) -> InstanceId {
        let core = self.core();
        core.trust_fingerprint(&server.host, server.fingerprint)
            .unwrap();
        let inst = core
            .register(&server.host, name, "correct horse", Some(invite))
            .await
            .unwrap();
        let wanted = inst.clone();
        self.until("connected", move |app| {
            app.core.status(&wanted) == ConnStatus::Connected
                && app
                    .core
                    .store()
                    .instance(&wanted)
                    .is_some_and(|s| !s.spaces.is_empty())
        })
        .await;
        inst
    }

    /// Runs the join the app would start for `VoiceMsg::Join`.
    async fn join(&mut self, inst: &InstanceId, channel: Id) {
        let _ = self
            .app
            .update(Message::Voice(VoiceMsg::Join(inst.clone(), channel)));
        let result = voice::join(self.core(), inst.clone(), channel).await;
        assert!(result.is_ok(), "{result:?}");
        let _ = self.app.update(Message::Voice(VoiceMsg::Joined(
            inst.clone(),
            channel,
            result,
        )));
    }

    fn voice_rows(&self) -> Vec<(String, bool)> {
        let rows = crate::sidebar::tree::rows(
            &self.app.core.store(),
            &self
                .app
                .core
                .instance_ids()
                .into_iter()
                .map(|i| (i, ConnStatus::Connected))
                .collect::<Vec<_>>(),
            &self.app.collapsed,
            None,
            crate::sidebar::tree::Tab::Spaces,
            &|_, _| false,
        );
        rows.into_iter()
            .filter_map(|row| match row {
                Row::VoiceUser { name, muted, .. } => Some((name, muted)),
                _ => None,
            })
            .collect()
    }
}

fn voice_channel(core: &Core, inst: &str) -> Id {
    let store = core.store();
    let space = store
        .instance(inst)
        .unwrap()
        .spaces
        .values()
        .next()
        .unwrap();
    space
        .channels
        .values()
        .find(|c| c.kind == hermes_client_core::proto::models::ChannelKind::Voice)
        .unwrap()
        .id
}

#[tokio::test]
async fn voice_panel_mute_volume_calls_and_leaving() {
    let server = start("ui").await;
    let mut alice = Client::new();
    let inst = alice.sign_up(&server, "alice", &server.setup_code).await;
    let room = voice_channel(&alice.core(), &inst);
    let space = *alice
        .core()
        .store()
        .instance(&inst)
        .unwrap()
        .spaces
        .keys()
        .next()
        .unwrap();
    let invite = alice
        .core()
        .rest(&inst)
        .unwrap()
        .create_invite(space, &CreateInviteRequest::default())
        .await
        .unwrap();
    let mut bob = Client::new();
    bob.sign_up(&server, "bob", &invite.code).await;
    let bob_id = bob.core().store().instance(&inst).unwrap().me_id().unwrap();

    // Not in voice: no panel.
    assert!(voice_panel::panel(&alice.app.core, false).is_none());

    // Alice joins: she is listed under the channel (bob sees her without joining).
    alice.join(&inst, room).await;
    let panel = voice_panel::panel(&alice.app.core, false).unwrap();
    assert!(panel.place.contains(" · "), "{panel:?}");
    assert!(!panel.muted);
    bob.until("alice in the sidebar", |app| {
        app.core.store().instance(&inst).is_some_and(|s| {
            s.voice_states
                .values()
                .any(|v| v.channel_id == room && Some(v.user_id) != s.me_id())
        })
    })
    .await;
    assert_eq!(bob.voice_rows(), [("alice".to_owned(), false)]);

    // The mute keybind (Ctrl+Shift+M); bob sees the muted flag.
    let _ = alice.app.update(Message::Key(
        Key::Character("m".into()),
        Physical::Unidentified(NativeCode::Unidentified),
        Modifiers::CTRL | Modifiers::SHIFT,
        false,
    ));
    assert!(alice.app.core.self_mute());
    assert!(voice_panel::panel(&alice.app.core, false).unwrap().muted);
    bob.until("alice muted", |app| {
        app.core
            .store()
            .instance(&inst)
            .is_some_and(|s| s.voice_states.values().any(|v| v.self_mute))
    })
    .await;
    assert_eq!(bob.voice_rows(), [("alice".to_owned(), true)]);
    let _ = alice.app.update(Message::Voice(VoiceMsg::ToggleMute));
    assert!(!alice.app.core.self_mute());

    // Bob calls alice in a DM: her banner shows until he leaves.
    let alice_id = alice
        .core()
        .store()
        .instance(&inst)
        .unwrap()
        .me_id()
        .unwrap();
    let dm = bob
        .core()
        .rest(&inst)
        .unwrap()
        .create_dm(vec![alice_id])
        .await
        .unwrap()
        .id;
    bob.join(&inst, dm).await;
    alice
        .until("incoming call", |app| {
            app.call
                .as_ref()
                .is_some_and(|c| c.channel == dm && c.caller == bob_id)
        })
        .await;
    let _ = bob.app.update(Message::Voice(VoiceMsg::Leave));
    bob.core().leave_voice().await;
    alice
        .until("ringing stopped", |app| app.call.is_none())
        .await;

    // Bob joins alice's room; alice turns him up in the volume menu.
    bob.join(&inst, room).await;
    alice
        .until("bob in the room", |app| {
            crate::buffer::voice_room::participants(&app.core, &inst, room).len() == 2
        })
        .await;
    let _ = alice
        .app
        .update(Message::Voice(VoiceMsg::UserMenu(inst.clone(), bob_id)));
    assert_eq!(alice.app.volume_menu.as_ref().unwrap().name, "bob");
    let _ = alice.app.update(Message::Voice(VoiceMsg::UserVolume(1.5)));
    assert_eq!(alice.app.core.user_volume(&inst, bob_id), 1.5);
    let _ = alice.app.update(Message::Escape);
    assert!(alice.app.volume_menu.is_none());
    let people = crate::buffer::voice_room::participants(&alice.app.core, &inst, room);
    let bob_card = people.iter().find(|p| p.user == bob_id).unwrap();
    assert_eq!(bob_card.volume, Some(1.5));

    // Selecting a voice channel opens its voice room; the header buttons lead to the chat.
    let _ = alice.app.update(Message::Select(inst.clone(), room));
    let buffer = alice.app.panes.focused().unwrap().buffer.clone();
    assert_eq!(
        buffer,
        Buffer::VoiceRoom {
            inst: inst.clone(),
            channel: room
        }
    );
    let labels: Vec<&str> = crate::panes::voice_controls(&alice.app.core, &buffer)
        .into_iter()
        .map(|(text, _)| text)
        .collect();
    assert_eq!(labels, ["Share screen", "Leave voice", "Chat"]);
    let _ = alice.app.update(Message::OpenChat(inst.clone(), room));
    assert!(matches!(
        alice.app.panes.focused().unwrap().buffer,
        Buffer::Channel { channel, .. } if channel == room
    ));

    // Leaving.
    alice.core().leave_voice().await;
    assert!(voice_panel::panel(&alice.app.core, false).is_none());
    let labels: Vec<&str> = crate::panes::voice_controls(&alice.app.core, &buffer)
        .into_iter()
        .map(|(text, _)| text)
        .collect();
    assert_eq!(labels, ["Join voice", "Chat"]);
    bob.core().leave_voice().await;
    drop(server);
}

#[tokio::test]
async fn voice_settings_are_applied_and_saved() {
    let mut client = Client::new();
    let app = &mut client.app;
    app.settings_tab = settings::Tab::Voice;
    app.page = Some(Buffer::Settings);
    let send = |app: &mut App, m: settings::Message| {
        let _ = app.update(Message::Settings(m));
    };
    send(app, settings::Message::Mode(settings::Mode::PushToTalk));
    assert_eq!(app.core.config().voice.mode, "push_to_talk");
    assert_eq!(
        app.core.audio_settings().mode,
        hermes_client_core::audio::InputMode::PushToTalk
    );

    // An unknown key is not saved; a known one is.
    send(app, settings::Message::PttKey("Hyper".into()));
    send(app, settings::Message::SavePttKey);
    assert_eq!(app.core.config().voice.ptt_key, "");
    send(app, settings::Message::PttKey("Mouse4".into()));
    send(app, settings::Message::SavePttKey);
    assert_eq!(app.core.config().voice.ptt_key, "Mouse4");

    // Sliders apply on release.
    send(app, settings::Message::VadSensitivity(-40));
    assert_eq!(app.core.config().voice.vad_sensitivity, -50);
    send(app, settings::Message::SaveVoice);
    assert_eq!(app.core.config().voice.vad_sensitivity, -40);

    send(app, settings::Message::NoiseSuppression(false));
    assert!(!app.core.config().voice.noise_suppression);
    send(
        app,
        settings::Message::InputDevice(settings::Device(Some("USB Mic".into()))),
    );
    assert_eq!(app.core.config().voice.input_device, "USB Mic");
    send(app, settings::Message::JoinOn(settings::JoinOn::ALL[1]));
    assert_eq!(app.core.config().voice.join_on, "single_click");

    // The mic test stops when the tab is left.
    send(app, settings::Message::MicTest(true));
    assert!(app.mic_test);
    send(app, settings::Message::Tab(settings::Tab::Appearance));
    assert!(!app.mic_test);
}

#[tokio::test]
async fn incoming_calls_ring_and_stop() {
    let mut client = Client::new();
    let app = &mut client.app;
    app.incoming_call("h".into(), Id(5), Id(7));
    assert!(app.call.is_some());
    // Another channel's ring stop does not end it.
    let _ = app.update(Message::Core(CoreEvent::CallRingStopped {
        inst: "h".into(),
        channel: Id(6),
    }));
    assert!(app.call.is_some());
    let _ = app.update(Message::Core(CoreEvent::CallRingStopped {
        inst: "h".into(),
        channel: Id(5),
    }));
    assert!(app.call.is_none());

    app.incoming_call("h".into(), Id(5), Id(7));
    let _ = app.update(Message::Voice(VoiceMsg::DismissCall));
    assert!(app.call.is_none());

    // Ringing ends by itself after 30 s.
    app.incoming_call("h".into(), Id(5), Id(7));
    app.call.as_mut().unwrap().since -= voice::RING_FOR;
    let _ = app.update(Message::Voice(VoiceMsg::RingTick));
    assert!(app.call.is_none());
}

/// A real screen share end to end on this machine: alice shares the primary screen through the
/// real capture thread (the GPU path where there is one), a real server carries it, bob watches
/// and decodes it. Prints what arrived and how long decoding took. The screen must change for
/// frames to flow (play a video, or scroll something).
///
/// `cargo test -p hermes-gui --release share_end_to_end -- --ignored --nocapture`, shaped with
/// `HERMES_SHARE_RES` (`1080p`, `1440p`, `native`; default `native`), `HERMES_SHARE_FPS`
/// (default 60) and `HERMES_SHARE_SECS` (default 10).
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a real screen; a benchmark"]
async fn share_end_to_end() {
    let resolution = std::env::var("HERMES_SHARE_RES").unwrap_or_else(|_| "native".into());
    let env = |name: &str, default: u32| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let fps = env("HERMES_SHARE_FPS", 60);
    let secs = env("HERMES_SHARE_SECS", 10);

    let server = start("share-e2e").await;
    let mut alice = Client::new();
    let inst = alice.sign_up(&server, "alice", &server.setup_code).await;
    let room = voice_channel(&alice.core(), &inst);
    let space = *alice
        .core()
        .store()
        .instance(&inst)
        .unwrap()
        .spaces
        .keys()
        .next()
        .unwrap();
    let invite = alice
        .core()
        .rest(&inst)
        .unwrap()
        .create_invite(space, &CreateInviteRequest::default())
        .await
        .unwrap();
    let mut bob = Client::new();
    bob.sign_up(&server, "bob", &invite.code).await;
    let alice_id = alice
        .core()
        .store()
        .instance(&inst)
        .unwrap()
        .me_id()
        .unwrap();
    alice.join(&inst, room).await;
    bob.join(&inst, room).await;

    let (sink, mut frames) = tokio::sync::mpsc::channel(64);
    bob.core().set_screen_sink(sink);
    alice.core().start_screen_share().unwrap();
    let quality = crate::screen::capture::Quality::from_config(&resolution, fps);
    let preference = crate::screen::capture::Preference::from_config(
        &std::env::var("HERMES_SHARE_PREF").unwrap_or_default(),
    );
    let _capture =
        crate::screen::capture::Capture::start(alice.core(), "", quality, preference).unwrap();
    let bob_core = bob.core();
    bob.until("bob sees the share", move |_| {
        !bob_core.screen_sharers().is_empty()
    })
    .await;
    bob.core().watch_screens(&[alice_id]);

    let mut decoder = crate::screen::codec::Decoder::new().unwrap();
    println!("decoding with {}", decoder.kind());
    let (mut received, mut keyframes, mut decoded, mut bytes) = (0u32, 0u32, 0u32, 0usize);
    let mut decode_times = Vec::new();
    let mut gaps = Vec::new();
    let mut size = (0, 0);
    let mut last_arrival: Option<std::time::Instant> = None;
    let mut started_decoding = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(u64::from(secs));
    while let Ok(Some(frame)) = tokio::time::timeout_at(deadline, frames.recv()).await {
        received += 1;
        bytes += frame.data.len();
        keyframes += u32::from(frame.header.keyframe);
        if let Some(previous) = last_arrival {
            gaps.push(frame.received.duration_since(previous));
        }
        last_arrival = Some(frame.received);
        started_decoding |= frame.header.keyframe;
        if !started_decoding {
            continue;
        }
        let begin = std::time::Instant::now();
        if let Ok(Some(picture)) = decoder.decode(&frame.data) {
            decode_times.push(begin.elapsed());
            decoded += 1;
            size = (picture.width, picture.height);
        }
    }
    decode_times.sort();
    gaps.sort();
    let pct = |v: &[Duration], q: f64| v.get(((v.len() as f64 - 1.0) * q) as usize).copied();
    println!(
        "share end to end ({resolution}, {fps} fps target, {}x{} decoded) over {secs} s:\n  \
         received {received} frames ({:.1} fps, {keyframes} keyframes, {:.0} kbit/s), decoded \
         {decoded}\n  arrival gaps: median {:?}, p95 {:?}, max {:?}\n  decode: median {:?}, p95 \
         {:?}, max {:?}",
        size.0,
        size.1,
        f64::from(received) / f64::from(secs),
        bytes as f64 * 8.0 / 1000.0 / f64::from(secs),
        pct(&gaps, 0.5),
        pct(&gaps, 0.95),
        gaps.last(),
        pct(&decode_times, 0.5),
        pct(&decode_times, 0.95),
        decode_times.last(),
    );
    alice.core().stop_screen_share();
}
