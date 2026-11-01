//! Voice end to end (PLAN.md section 19, "Voice"): two voicebots in one room on a real server,
//! one playing tone bursts and the other recording, measuring loss and latency; then the same
//! with the `hermes-voicebot` binary and WAV files.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hermes_client_core::address::{Fingerprint, HostPort, InviteLink};
use hermes_client_core::proto::api::CreateInviteRequest;
use hermes_client_core::{Core, CoreOptions};
use hermes_server::config::{Config, RegistrationMode, TlsMode};
use hermes_server::state::AppState;
use hermes_voicebot::{Bot, Login, signal, wav};
use tokio::sync::Notify;

struct TestServer {
    host: HostPort,
    setup_code: String,
    fingerprint: Fingerprint,
    state: Arc<AppState>,
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
    let dir = std::env::temp_dir().join(format!("hermes-voicebot-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut config = Config::default();
    config.storage.data_dir = dir.join("data");
    config.tls.mode = TlsMode::SelfSigned;
    config.registration.mode = RegistrationMode::InviteOnly;
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
    let state = Arc::new(state);
    tokio::spawn(hermes_server::server::run(
        state.clone(),
        tcp,
        tls,
        Some(quic),
        signal,
    ));
    TestServer {
        host: HostPort::parse(&format!("127.0.0.1:{port}")).unwrap(),
        setup_code: link.code,
        fingerprint: link.fingerprint.unwrap(),
        state,
        shutdown,
        dir,
    }
}

fn login(server: &TestServer, username: &str, invite: &str) -> Login {
    Login {
        server: server.host.clone(),
        fingerprint: Some(server.fingerprint),
        username: username.into(),
        password: "correct horse".into(),
        register: Some(Some(invite.into())),
    }
}

/// A user who owns the space and hands out invites.
async fn owner(server: &TestServer) -> (Core, String) {
    let (core, _events) = Core::new(Default::default(), CoreOptions::in_memory("test"));
    core.trust_fingerprint(&server.host, server.fingerprint)
        .unwrap();
    let inst = core
        .register(
            &server.host,
            "owner",
            "correct horse",
            Some(&server.setup_code),
        )
        .await
        .unwrap();
    (core, inst)
}

async fn invite(core: &Core, inst: &str, server: &TestServer) -> String {
    let space = *server.state.cache().spaces.keys().next().unwrap();
    core.rest(inst)
        .unwrap()
        .create_invite(space, &CreateInviteRequest::default())
        .await
        .unwrap()
        .code
}

#[tokio::test]
async fn two_voicebots_measure_loss_and_latency() {
    let server = start("bots").await;
    let (owner, inst) = owner(&server).await;
    let code = invite(&owner, &inst, &server).await;
    let listener = Bot::connect(&login(&server, "listener", &code), false)
        .await
        .unwrap();
    let speaker = Bot::connect(&login(&server, "speaker", &code), false)
        .await
        .unwrap();
    listener.join("General").await.unwrap();

    let sent = signal::bursts(6, 160, 340);
    speaker.play(sent.clone(), false);
    speaker.join("general").await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !speaker.finished_playing() {
        assert!(Instant::now() < deadline, "playing takes too long");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(600)).await;

    let (heard, heard_at) = listener.recording();
    let sent_at = speaker.source_start().unwrap();
    let heard_at = heard_at.unwrap();
    let ms = |t: Instant| t.duration_since(heard_at.min(sent_at)).as_secs_f64() * 1000.0;
    let report = signal::compare(&sent, ms(sent_at), &heard, ms(heard_at));
    println!("{report:?}");
    assert_eq!(report.sent_bursts, 6);
    // Shared CI runners (two clients and a server on two slow vCPUs) can stall for a moment;
    // the strict numbers are for real machines.
    let ci = std::env::var_os("CI").is_some();
    let (min_heard, max_median) = if ci { (5, 750.0) } else { (6, 250.0) };
    assert!(report.heard_bursts >= min_heard, "{report:?}");
    assert!(report.loss_pct < 10.0, "{report:?}");
    let median = report.median_latency_ms().unwrap();
    // Locally: jitter buffer (40–60 ms) + a frame + scheduling.
    assert!((0.0..max_median).contains(&median), "{report:?}");
    assert!(matches!(
        listener.status(),
        hermes_client_core::VoiceStatus::Connected { .. }
    ));
    speaker.leave().await;
    listener.leave().await;
}

#[tokio::test]
async fn the_voicebot_binary_plays_and_records_wav_files() {
    let server = start("cli").await;
    let (owner, inst) = owner(&server).await;
    let dir = server.dir.clone();
    let tone = dir.join("tone.wav");
    let out = dir.join("out.wav");
    // A 44.1 kHz stereo file, to go through the resampler too.
    let bursts = hermes_voicebot::signal::bursts(4, 200, 300);
    let resampled: Vec<f32> = (0..bursts.len() * 441 / 480)
        .flat_map(|i| {
            let s = bursts[i * 480 / 441];
            [s, s]
        })
        .collect();
    wav::write(
        &tone,
        &wav::Wav {
            rate: 44_100,
            channels: 2,
            samples: resampled,
        },
    )
    .unwrap();

    let bot = |user: &str, code: &str| {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_hermes-voicebot"));
        cmd.args([
            "--server",
            &server.host.to_string(),
            "--fingerprint",
            &server.fingerprint.to_string(),
            "--user",
            user,
            "--password",
            "correct horse",
            "--register",
            "--invite",
            code,
            "--join",
            "General",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
        cmd
    };
    let code = invite(&owner, &inst, &server).await;
    let mut listener = bot("listener", &code)
        .args(["--record", out.to_str().unwrap(), "--duration", "6"])
        .spawn()
        .unwrap();
    // Start the speaker once the listener is in the room.
    let deadline = Instant::now() + Duration::from_secs(20);
    while server.state.cache().voice_states.is_empty() {
        assert!(Instant::now() < deadline, "the listener did not join");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut speaker = bot("speaker", &code)
        .args(["--play", tone.to_str().unwrap()])
        .spawn()
        .unwrap();
    let (speaker, listener) =
        tokio::task::spawn_blocking(move || (speaker.wait(), listener.wait()))
            .await
            .unwrap();
    assert!(speaker.unwrap().success());
    assert!(listener.unwrap().success());

    let recorded = wav::read(&out).unwrap();
    assert_eq!((recorded.rate, recorded.channels), (48_000, 1));
    let (onsets, _) = signal::onsets(&recorded.samples);
    assert_eq!(onsets.len(), 4, "{onsets:?}");
}
