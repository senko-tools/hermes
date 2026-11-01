//! Voice through `Core` against a real `hermes-server` (PLAN.md section 19): joining, audio
//! between two clients on virtual devices, speaking events, per-user volume, mute, switching,
//! leaving, being disconnected, and reconnecting after a lost connection.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hermes_client_core::address::{Fingerprint, HostPort, InviteLink};
use hermes_client_core::audio::{FRAME, Io, VirtualIo, dsp};
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::api::{CreateInviteRequest, VoiceStatePatchRequest};
use hermes_client_core::proto::id::Id;
use hermes_client_core::{Core, CoreEvent, CoreEvents, CoreOptions, InstanceId, VoiceStatus};
use hermes_server::config::{Config, RegistrationMode, TlsMode};
use hermes_server::state::AppState;
use tokio::sync::Notify;

const WAIT: Duration = Duration::from_secs(15);

struct TestServer {
    host: HostPort,
    setup_code: String,
    fingerprint: Fingerprint,
    shutdown: Arc<Notify>,
    dir: PathBuf,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shutdown.notify_one();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn start(name: &str) -> TestServer {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = std::env::temp_dir().join(format!("hermes-cc-voice-{}-{name}", std::process::id()));
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
    TestServer {
        host: HostPort::parse(&format!("127.0.0.1:{port}")).unwrap(),
        setup_code: link.code,
        fingerprint: link.fingerprint.unwrap(),
        shutdown,
        dir,
    }
}

/// A microphone playing a tone (while `talking`), and speakers that record.
#[derive(Clone, Default)]
struct Device {
    talking: Arc<Mutex<bool>>,
    recorded: Arc<Mutex<Vec<f32>>>,
}

struct DeviceIo {
    device: Device,
    phase: usize,
}

impl VirtualIo for DeviceIo {
    fn read(&mut self, frame: &mut [f32; FRAME]) {
        let talking = *self.device.talking.lock().unwrap();
        for s in frame.iter_mut() {
            *s = if talking {
                0.3 * (self.phase as f32 * 2.0 * std::f32::consts::PI * 440.0 / 48_000.0).sin()
            } else {
                0.0
            };
            self.phase += 1;
        }
    }

    fn write(&mut self, frame: &[f32; FRAME]) {
        self.device
            .recorded
            .lock()
            .unwrap()
            .extend_from_slice(frame);
    }
}

impl Device {
    fn talk(&self, on: bool) {
        *self.talking.lock().unwrap() = on;
    }

    /// RMS of what played during the last `ms`.
    fn level(&self, ms: usize) -> f32 {
        let recorded = self.recorded.lock().unwrap();
        let n = (48 * ms).min(recorded.len());
        dsp::rms(&recorded[recorded.len() - n..])
    }
}

fn core(device: &Device) -> (Core, CoreEvents) {
    let mut options = CoreOptions::in_memory("hermes-test");
    // Plain sine tones are not voice to RNNoise; the RMS threshold opens the gate.
    let mut config = hermes_client_core::config::ClientConfig::default();
    config.voice.noise_suppression = false;
    // UI sounds would land in the recordings.
    config.voice.sounds = false;
    options.audio = Io::Virtual(Box::new(DeviceIo {
        device: device.clone(),
        phase: 0,
    }));
    Core::new(config, options)
}

async fn wait_for(events: &mut CoreEvents, what: &str, mut f: impl FnMut(&CoreEvent) -> bool) {
    let result = tokio::time::timeout(WAIT, async {
        while let Some(event) = events.recv().await {
            if f(&event) {
                return;
            }
        }
        panic!("event stream ended");
    })
    .await;
    assert!(result.is_ok(), "timed out waiting for {what}");
}

async fn sign_up(
    server: &TestServer,
    core: &Core,
    events: &mut CoreEvents,
    username: &str,
    invite: Option<&str>,
) -> InstanceId {
    core.trust_fingerprint(&server.host, server.fingerprint)
        .unwrap();
    let inst = core
        .register(&server.host, username, "correct horse", invite)
        .await
        .unwrap();
    wait_for(events, "connected", |e| {
        matches!(
            e,
            CoreEvent::InstanceStatus {
                status: ConnStatus::Connected,
                ..
            }
        )
    })
    .await;
    inst
}

/// The space and the IDs of its voice channels by name.
fn voice_channel(core: &Core, inst: &str, name: &str) -> (Id, Id) {
    let store = core.store();
    let space = store
        .instance(inst)
        .unwrap()
        .spaces
        .values()
        .next()
        .unwrap();
    let channel = space
        .channels
        .values()
        .find(|c| c.name.as_deref() == Some(name))
        .unwrap();
    (space.space.id, channel.id)
}

/// Waits until `f` holds, polling.
async fn eventually(what: &str, f: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !f() {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

struct Pair {
    server: TestServer,
    alice: Core,
    alice_events: CoreEvents,
    alice_dev: Device,
    bob: Core,
    bob_events: CoreEvents,
    bob_dev: Device,
    inst: InstanceId,
    space: Id,
    general: Id,
}

async fn pair(name: &str) -> Pair {
    let server = start(name).await;
    let alice_dev = Device::default();
    let bob_dev = Device::default();
    let (alice, mut alice_events) = core(&alice_dev);
    let (bob, mut bob_events) = core(&bob_dev);
    let setup = server.setup_code.clone();
    let inst = sign_up(&server, &alice, &mut alice_events, "alice", Some(&setup)).await;
    let (space, general) = voice_channel(&alice, &inst, "General");
    let invite = alice
        .rest(&inst)
        .unwrap()
        .create_invite(space, &CreateInviteRequest::default())
        .await
        .unwrap();
    sign_up(&server, &bob, &mut bob_events, "bob", Some(&invite.code)).await;
    Pair {
        server,
        alice,
        alice_events,
        alice_dev,
        bob,
        bob_events,
        bob_dev,
        inst,
        space,
        general,
    }
}

fn me(core: &Core, inst: &str) -> Id {
    core.store().instance(inst).unwrap().me_id().unwrap()
}

#[tokio::test]
async fn two_clients_talk() {
    let mut p = pair("talk").await;
    let (inst, general) = (p.inst.clone(), p.general);
    let alice_id = me(&p.alice, &inst);

    assert_eq!(p.alice.voice_status(), VoiceStatus::Disconnected);
    p.alice.join_voice(&inst, general).await.unwrap();
    assert_eq!(p.alice.voice_channel(), Some((inst.clone(), general)));
    assert!(matches!(
        p.alice.voice_status(),
        VoiceStatus::Connected { .. }
    ));
    p.bob.join_voice(&inst, general).await.unwrap();
    // The gateway shows both in the channel.
    eventually("voice states", || {
        p.bob.store().instance(&inst).is_some_and(|s| {
            s.voice_states
                .values()
                .filter(|v| v.channel_id == general)
                .count()
                == 2
        })
    })
    .await;

    // Alice talks: bob hears her and sees her speaking.
    p.alice_dev.talk(true);
    wait_for(
        &mut p.bob_events,
        "alice speaking",
        |e| matches!(e, CoreEvent::Speaking { user, speaking: true, .. } if *user == alice_id),
    )
    .await;
    wait_for(
        &mut p.alice_events,
        "own speaking",
        |e| matches!(e, CoreEvent::Speaking { user, speaking: true, .. } if *user == alice_id),
    )
    .await;
    assert!(p.bob.is_speaking(&inst, alice_id));
    assert!(p.alice.is_speaking(&inst, alice_id));
    eventually("bob hears alice", || p.bob_dev.level(300) > 0.1).await;
    // Alice does not hear herself.
    assert!(p.alice_dev.level(300) < 0.01);

    // Per-user volume 0 silences her for bob only.
    p.bob.set_user_volume(&inst, alice_id, 0.0).unwrap();
    assert_eq!(p.bob.user_volume(&inst, alice_id), 0.0);
    eventually("volume 0", || p.bob_dev.level(200) < 0.01).await;
    p.bob.set_user_volume(&inst, alice_id, 1.0).unwrap();
    eventually("volume back", || p.bob_dev.level(200) > 0.1).await;

    // Self mute: nothing is sent.
    p.alice.set_self_mute(true);
    assert!(p.alice.self_mute());
    wait_for(
        &mut p.bob_events,
        "alice stops speaking",
        |e| matches!(e, CoreEvent::Speaking { user, speaking: false, .. } if *user == alice_id),
    )
    .await;
    assert!(!p.bob.is_speaking(&inst, alice_id));
    eventually("silence", || p.bob_dev.level(200) < 0.01).await;
    eventually("muted voice state", || {
        p.bob
            .store()
            .instance(&inst)
            .and_then(|s| s.voice_states.get(&alice_id).copied())
            .is_some_and(|v| v.self_mute)
    })
    .await;
    p.alice.set_self_mute(false);

    // Leaving.
    p.alice.leave_voice().await;
    assert_eq!(p.alice.voice_status(), VoiceStatus::Disconnected);
    assert_eq!(p.alice.voice_channel(), None);
    eventually("alice left", || {
        p.bob
            .store()
            .instance(&inst)
            .is_some_and(|s| !s.voice_states.contains_key(&alice_id))
    })
    .await;
    drop(p.server);
}

#[tokio::test]
async fn dm_call_rings_until_the_caller_leaves() {
    let mut p = pair("ring").await;
    let inst = p.inst.clone();
    let (alice_id, bob_id) = (me(&p.alice, &inst), me(&p.bob, &inst));
    let dm = p
        .alice
        .rest(&inst)
        .unwrap()
        .create_dm(vec![bob_id])
        .await
        .unwrap()
        .id;
    // Right after creating the DM (it may not be in the store yet): still a call that rings.
    p.alice.join_or_call(&inst, dm).await.unwrap();
    wait_for(&mut p.bob_events, "incoming call", |e| {
        matches!(e, CoreEvent::IncomingCall { channel, caller, .. }
            if *channel == dm && *caller == alice_id)
    })
    .await;
    p.alice.leave_voice().await;
    wait_for(
        &mut p.bob_events,
        "ringing stops",
        |e| matches!(e, CoreEvent::CallRingStopped { channel, .. } if *channel == dm),
    )
    .await;
    drop(p.server);
}

#[tokio::test]
async fn switching_and_being_disconnected() {
    let mut p = pair("switch").await;
    let (inst, general, space) = (p.inst.clone(), p.general, p.space);
    let bob_id = me(&p.bob, &inst);
    let lounge = p
        .alice
        .rest(&inst)
        .unwrap()
        .create_channel(
            space,
            &hermes_client_core::proto::api::CreateChannelRequest {
                kind: hermes_client_core::proto::models::ChannelKind::Voice,
                name: "Lounge".into(),
                parent_id: None,
                topic: None,
                bitrate: None,
                user_limit: None,
            },
        )
        .await
        .unwrap()
        .id;

    p.bob.join_voice(&inst, general).await.unwrap();
    p.bob.join_voice(&inst, lounge).await.unwrap();
    assert_eq!(p.bob.voice_channel(), Some((inst.clone(), lounge)));
    eventually("bob in the lounge", || {
        p.alice
            .store()
            .instance(&inst)
            .and_then(|s| s.voice_states.get(&bob_id).copied())
            .is_some_and(|v| v.channel_id == lounge)
    })
    .await;

    // Moved by a moderator: the client follows.
    p.alice
        .rest(&inst)
        .unwrap()
        .update_voice_state(
            space,
            bob_id,
            &VoiceStatePatchRequest {
                channel_id: Some(Some(general)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    eventually("moved back", || {
        p.bob.voice_channel() == Some((inst.clone(), general))
    })
    .await;

    // Disconnected by a moderator: an error, and no reconnect.
    p.alice
        .rest(&inst)
        .unwrap()
        .update_voice_state(
            space,
            bob_id,
            &VoiceStatePatchRequest {
                channel_id: Some(None),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    wait_for(
        &mut p.bob_events,
        "kicked",
        |e| matches!(e, CoreEvent::Error(m) if m.contains("disconnected")),
    )
    .await;
    assert_eq!(p.bob.voice_status(), VoiceStatus::Disconnected);
    assert_eq!(p.bob.voice_channel(), None);

    // A full channel is refused.
    let small = p
        .alice
        .rest(&inst)
        .unwrap()
        .create_channel(
            space,
            &hermes_client_core::proto::api::CreateChannelRequest {
                kind: hermes_client_core::proto::models::ChannelKind::Voice,
                name: "Small".into(),
                parent_id: None,
                topic: None,
                bitrate: None,
                user_limit: Some(1),
            },
        )
        .await
        .unwrap()
        .id;
    p.alice.join_voice(&inst, small).await.unwrap();
    let err = p.bob.join_voice(&inst, small).await.unwrap_err();
    assert!(err.to_string().contains("full"), "{err}");
    assert_eq!(p.bob.voice_channel(), None);
    let _ = &mut p.alice_events;
}

#[tokio::test]
async fn reconnects_after_a_lost_connection() {
    let mut p = pair("reconnect").await;
    let (inst, general) = (p.inst.clone(), p.general);
    p.alice.join_voice(&inst, general).await.unwrap();
    p.bob.join_voice(&inst, general).await.unwrap();
    p.alice_dev.talk(true);
    eventually("bob hears alice", || p.bob_dev.level(300) > 0.1).await;

    p.alice.drop_voice_connection();
    wait_for(&mut p.alice_events, "reconnecting", |e| {
        matches!(
            e,
            CoreEvent::VoiceStatus(VoiceStatus::Reconnecting { attempt: 1 })
        )
    })
    .await;
    wait_for(&mut p.alice_events, "connected again", |e| {
        matches!(e, CoreEvent::VoiceStatus(VoiceStatus::Connected { .. }))
    })
    .await;
    assert_eq!(p.alice.voice_channel(), Some((inst.clone(), general)));
    // Audio flows again after the jitter buffer re-buffers.
    tokio::time::sleep(Duration::from_millis(500)).await;
    eventually("bob hears alice again", || p.bob_dev.level(300) > 0.1).await;
    let _ = &mut p.bob_events;
}
