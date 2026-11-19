//! Voice through `Core` against a real `hermes-server` (PLAN.md section 19): joining, audio
//! between two clients on virtual devices, speaking events, per-user volume, mute, switching,
//! leaving, being disconnected, and reconnecting after a lost connection.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use hermes_client_core::address::{Fingerprint, HostPort, InviteLink};
use hermes_client_core::audio::{FRAME, Io, VirtualIo, dsp};
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::api::{CreateInviteRequest, VoiceStatePatchRequest};
use hermes_client_core::proto::id::Id;
use hermes_client_core::voice::ScreenFrame;
use hermes_client_core::{Core, CoreEvent, CoreEvents, CoreOptions, InstanceId, VoiceStatus};
use hermes_server::config::Config;

const WAIT: Duration = Duration::from_secs(15);

/// A server with voice from `hermes_server::testing`, with what these tests use of its
/// invite link.
struct TestServer {
    host: HostPort,
    setup_code: String,
    fingerprint: Fingerprint,
    _server: hermes_server::testing::TestServer,
}

async fn start(name: &str) -> TestServer {
    let server = hermes_server::testing::spawn(&format!("cc-voice-{name}"), true, |c| {
        c.instance.name = "Voice".into();
    })
    .await;
    let link = InviteLink::parse(&server.link).unwrap();
    TestServer {
        host: HostPort::parse(&server.host()).unwrap(),
        setup_code: link.code,
        fingerprint: link.fingerprint.unwrap(),
        _server: server,
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
async fn alone_in_a_room_sends_no_audio() {
    let mut p = pair("alone").await;
    let (inst, general) = (p.inst.clone(), p.general);
    let sent = |core: &Core| core.voice_link().map_or(0, |l| l.sent_bytes);

    p.alice.join_voice(&inst, general).await.unwrap();
    p.alice_dev.talk(true);
    assert!(!p.alice.voice_has_listeners());
    let before = sent(&p.alice);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    // Pings and control only: talking would be about 16 kB in two seconds.
    let alone = sent(&p.alice) - before;
    assert!(alone < 4_000, "sent {alone} bytes alone");

    p.bob.join_voice(&inst, general).await.unwrap();
    eventually("alice hears of bob", || p.alice.voice_has_listeners()).await;
    eventually("bob hears alice", || p.bob_dev.level(300) > 0.1).await;
    let before = sent(&p.alice);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let together = sent(&p.alice) - before;
    assert!(
        together > 8_000,
        "sent only {together} bytes with bob there"
    );

    // Bob leaves: silent again.
    p.bob.leave_voice().await;
    eventually("alone again", || !p.alice.voice_has_listeners()).await;
    let _ = &mut p.alice_events;
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

#[tokio::test]
async fn screen_shares_flow_to_watchers() {
    let mut p = pair("share").await;
    let (inst, general) = (p.inst.clone(), p.general);
    let alice_id = me(&p.alice, &inst);
    assert!(p.alice.start_screen_share().is_err(), "not in voice yet");
    p.alice.join_voice(&inst, general).await.unwrap();
    p.bob.join_voice(&inst, general).await.unwrap();
    let (sink, mut frames) = tokio::sync::mpsc::channel(8);
    p.bob.set_screen_sink(sink);
    // Encrypted: nothing can be read until the sender keys have gone around.
    eventually("the keys to go around", || {
        [&p.alice, &p.bob].iter().all(|core| {
            core.voice_encryption()
                .is_some_and(|e| e.ready && e.waiting.is_empty() && e.no_keys.is_empty())
        })
    })
    .await;
    assert_eq!(
        p.alice.voice_encryption().unwrap().code,
        p.bob.voice_encryption().unwrap().code,
        "everyone in the room sees the same code"
    );

    p.alice.start_screen_share().unwrap();
    assert!(p.alice.sharing_screen());
    eventually("nobody watches yet", || p.alice.share_watchers() == Some(0)).await;
    eventually("bob sees the share", || {
        p.bob.screen_sharers() == [(inst.clone(), alice_id)]
    })
    .await;
    p.bob.watch_screens(&[alice_id]);
    eventually("bob watches", || p.alice.share_watchers() == Some(1)).await;
    wait_for(&mut p.alice_events, "a keyframe request", |e| {
        matches!(e, CoreEvent::KeyframeRequested)
    })
    .await;
    let frame = |keyframe, timestamp| ScreenFrame {
        keyframe,
        width: 640,
        height: 360,
        timestamp,
        data: vec![0, 0, 0, 1, 0x65, 1, 2, 3],
    };
    assert!(p.alice.send_screen_frame(frame(true, 0)));
    let got = tokio::time::timeout(WAIT, frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.user, alice_id);
    assert_eq!((got.header.width, got.header.height), (640, 360));
    assert!(got.header.keyframe);
    assert_eq!(got.data, [0, 0, 0, 1, 0x65, 1, 2, 3]);

    // Unwatched, nothing more arrives.
    p.bob.watch_screens(&[]);
    eventually("nobody watches", || p.alice.share_watchers() == Some(0)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(p.alice.send_screen_frame(frame(false, 100)));
    assert!(
        tokio::time::timeout(Duration::from_millis(500), frames.recv())
            .await
            .is_err()
    );

    p.alice.stop_screen_share();
    assert_eq!(p.alice.share_watchers(), None);
    assert!(!p.alice.send_screen_frame(frame(true, 200)), "no share");
    eventually("the share ended", || p.bob.screen_sharers().is_empty()).await;
}

/// Pushes a screen share's worth of data through a real local server, with no capture or codec
/// involved, and prints what arrived and how late: if this keeps up but real shares stutter, the
/// problem is in the clients' capture/encode/decode/draw, not the server or the transport.
///
/// `cargo test -p hermes-client-core --release --test voice share_transport_benchmark -- --ignored --nocapture`
/// Shape it with `HERMES_BENCH_FPS` (default 30), `HERMES_BENCH_KBPS` (default 8000) and
/// `HERMES_BENCH_SECS` (default 10). A keyframe (8x a delta frame) is sent every 2 seconds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a benchmark: run it on purpose"]
async fn share_transport_benchmark() {
    let env = |name: &str, default: u32| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let fps = env("HERMES_BENCH_FPS", 30).max(1);
    let kbps = env("HERMES_BENCH_KBPS", 8000);
    let secs = env("HERMES_BENCH_SECS", 10).max(1);
    // A second holds one keyframe per two seconds at 8x a delta frame: fps - 0.5 + 4 units.
    let units = f64::from(fps) + 3.5;
    let delta = (f64::from(kbps) * 1000.0 / 8.0 / units) as usize;
    let max = hermes_client_core::proto::voice::MAX_VIDEO_FRAME
        - hermes_client_core::proto::voice::VIDEO_HEADER_LEN;
    let key = (delta * 8).min(max);

    let mut p = pair("share-bench").await;
    let (inst, general) = (p.inst.clone(), p.general);
    let alice_id = me(&p.alice, &inst);
    p.alice.join_voice(&inst, general).await.unwrap();
    p.bob.join_voice(&inst, general).await.unwrap();
    let (sink, mut frames) = tokio::sync::mpsc::channel(64);
    p.bob.set_screen_sink(sink);
    p.alice.start_screen_share().unwrap();
    eventually("bob sees the share", || !p.bob.screen_sharers().is_empty()).await;
    p.bob.watch_screens(&[alice_id]);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let started = std::time::Instant::now();
    let total = fps * secs;
    let sender = {
        let alice = p.alice.clone();
        tokio::spawn(async move {
            let mut refused = 0u32;
            let mut ticker = tokio::time::interval(Duration::from_secs(1) / fps);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            for n in 0..total {
                ticker.tick().await;
                let keyframe = n % (fps * 2) == 0;
                let size = if keyframe { key } else { delta };
                let mut data = vec![0x5a; size];
                data[..5].copy_from_slice(&[0, 0, 0, 1, if keyframe { 0x65 } else { 0x41 }]);
                let timestamp = u32::try_from(started.elapsed().as_millis()).unwrap();
                if !alice.send_screen_frame(ScreenFrame {
                    keyframe,
                    width: 3840,
                    height: 2160,
                    timestamp,
                    data,
                }) {
                    refused += 1;
                }
            }
            refused
        })
    };

    let mut delays = Vec::new();
    let (mut received, mut bytes) = (0u32, 0usize);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(u64::from(secs) + 3);
    while let Ok(Some(frame)) = tokio::time::timeout_at(deadline, frames.recv()).await {
        let now = u32::try_from(started.elapsed().as_millis()).unwrap();
        delays.push(now.saturating_sub(frame.header.timestamp));
        received += 1;
        bytes += frame.data.len();
        if received == total {
            break;
        }
    }
    let refused = sender.await.unwrap();
    delays.sort_unstable();
    let pct = |q: f64| {
        delays
            .get(((delays.len() as f64 - 1.0) * q) as usize)
            .copied()
    };
    println!(
        "share transport: {fps} fps target, {kbps} kbit/s target (delta {delta} B, keyframe {key} \
         B) for {secs} s\n  sent {} (refused by the sender's queue: {refused}), received {received} \
         ({:.1} fps, {:.0} kbit/s)\n  delay ms: median {:?}, p95 {:?}, p99 {:?}, max {:?}",
        total - refused,
        f64::from(received) / f64::from(secs),
        bytes as f64 * 8.0 / 1000.0 / f64::from(secs),
        pct(0.5),
        pct(0.95),
        pct(0.99),
        delays.last(),
    );
    let _ = (&mut p.alice_events, &mut p.bob_events);
}

/// An address that does not answer (an IPv6 that does not reach the server, say) is passed over
/// for the next one, as the HTTPS connection does.
#[tokio::test]
async fn voice_tries_the_next_address_when_one_does_not_answer() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = std::env::temp_dir().join(format!("hermes-cc-voice-{}-fallback", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut config = Config::default();
    config.storage.data_dir = dir.join("data");
    std::fs::create_dir_all(&config.storage.data_dir).unwrap();
    let (endpoint, cert_sha256) =
        hermes_server::voice::endpoint::bind(&config, "127.0.0.1:0".parse().unwrap()).unwrap();
    let real = endpoint.local_addr().unwrap();
    let server = endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = server.accept().await {
            tokio::spawn(async move {
                if let Ok(conn) = incoming.await {
                    // Held until the client closes it.
                    let _ = conn.closed().await;
                }
            });
        }
    });
    // A UDP port nobody listens on any more.
    let dead = std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let quic = hermes_client_core::proto::api::QuicEndpoint {
        host: "127.0.0.1".into(),
        port: real.port(),
        cert_sha256,
    };
    let (answered, conn) =
        hermes_client_core::voice::connect_any(&quic, &[dead, real], Duration::from_secs(1))
            .await
            .expect("the second address answers");
    assert_eq!(answered, real);
    conn.close(0u32.into(), b"done");

    let err = hermes_client_core::voice::connect_any(&quic, &[dead], Duration::from_secs(1))
        .await
        .expect_err("nothing answers");
    assert!(err.to_string().contains("timed out"), "{err}");
    endpoint.close(0u32.into(), b"done");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Diagnostics against a working server: every check of it is Working, voice included.
#[tokio::test]
async fn diagnostics_find_a_working_server_working() {
    use hermes_client_core::diagnostics::Level;

    let server = start("diagnostics").await;
    let (core, mut events) = core(&Device::default());
    let inst = sign_up(
        &server,
        &core,
        &mut events,
        "owner",
        Some(&server.setup_code),
    )
    .await;
    // The device's keys are registered just after signing in.
    let mut checks = Vec::new();
    for _ in 0..50 {
        checks = core.diagnose_instance(&inst).await;
        if checks.iter().all(|c| c.level == Level::Working) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Address lookup",
            "HTTPS (text, files)",
            "Certificate",
            "Live connection",
            "Version",
            "Voice (UDP)",
            "Encryption keys",
        ]
    );
    for check in &checks {
        assert_eq!(check.level, Level::Working, "{check:?}");
    }
    assert!(
        checks[1].detail.starts_with("IPv4 127.0.0.1: "),
        "{:?}",
        checks[1]
    );
    assert_eq!(core.diagnose_storage().level, Level::Warning, "in memory");
}
