//! End-to-end: voice in the TUI against a real `hermes-server` with its QUIC endpoint, on
//! virtual audio: `J` joins the selected voice channel, the sidebar lists who is in it, `m`/`D`
//! mute and deafen, the status line, `q` asks while in voice, `L` leaves, and an incoming DM call
//! is answered with `:voice`.

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hermes_client_core::address::{HostPort, InviteLink};
use hermes_client_core::audio::{FRAME, Io, VirtualIo};
use hermes_client_core::config::ClientConfig;
use hermes_client_core::proto::api::CreateInviteRequest;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::{Core, CoreEvents, CoreOptions, InstanceId};
use hermes_server::config::{Config, RegistrationMode, TlsMode};
use hermes_server::state::AppState;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tokio::sync::{Notify, mpsc};

use crate::app::{App, Focus, Popup, SidebarItem, TaskMsg};
use crate::commands::Command;
use crate::ui;

const WAIT: Duration = Duration::from_secs(15);

struct Server {
    host: HostPort,
    link: InviteLink,
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
    let dir = std::env::temp_dir().join(format!("hermes-tui-voice-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut config = Config::default();
    config.instance.name = "Friends".into();
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
        link: InviteLink::parse(&links.web).unwrap(),
        shutdown,
        dir,
    }
}

struct Silence;

impl VirtualIo for Silence {
    fn read(&mut self, frame: &mut [f32; FRAME]) {
        frame.fill(0.0);
    }

    fn write(&mut self, _frame: &[f32; FRAME]) {}
}

fn core() -> (Core, CoreEvents) {
    let mut options = CoreOptions::in_memory("hermes-tui");
    options.audio = Io::Virtual(Box::new(Silence));
    let mut config = ClientConfig::default();
    config.voice.sounds = false;
    Core::new(config, options)
}

async fn sign_up(core: &Core, server: &Server, name: &str, invite: &str) -> InstanceId {
    core.trust_fingerprint(&server.host, server.link.fingerprint.unwrap())
        .unwrap();
    let inst = core
        .register(&server.host, name, "correct horse", Some(invite))
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + WAIT;
    while !core
        .store()
        .instance(&inst)
        .is_some_and(|s| !s.spaces.is_empty())
    {
        assert!(tokio::time::Instant::now() < deadline, "{name}: no space");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    inst
}

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

async fn pump(
    app: &mut App,
    events: &mut CoreEvents,
    tasks: &mut mpsc::UnboundedReceiver<TaskMsg>,
    what: &str,
    done: impl Fn(&App) -> bool,
) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !done(app) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out: {what} (status: {:?})",
            app.status
        );
        tokio::select! {
            Some(e) = events.recv() => app.on_core_event(e),
            Some(m) = tasks.recv() => app.on_task(m),
            () = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }
}

fn screen(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn voice_users(app: &App) -> Vec<(String, bool, bool)> {
    app.sidebar
        .iter()
        .filter_map(|i| match i {
            SidebarItem::VoiceUser {
                name,
                muted,
                deafened,
                ..
            } => Some((name.clone(), *muted, *deafened)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn join_mute_deafen_leave_and_answer_a_call() {
    let server = start("voice").await;
    let (alice_core, mut events) = core();
    let inst = sign_up(&alice_core, &server, "alice", &server.link.code).await;
    let (tx, mut tasks) = mpsc::unbounded_channel();
    let mut app = App::new(alice_core.clone(), tx, 0);
    app.rebuild_sidebar();

    // Select the voice channel and press J.
    app.sidebar_sel = app
        .sidebar
        .iter()
        .position(|i| {
            matches!(
                i,
                SidebarItem::Channel {
                    kind: ChannelKind::Voice,
                    ..
                }
            )
        })
        .expect("a voice channel");
    assert_eq!(app.focus, Focus::Sidebar);
    app.on_key(key('J'));
    pump(&mut app, &mut events, &mut tasks, "in voice", |a| {
        a.core.voice_channel().is_some() && !voice_users(a).is_empty()
    })
    .await;
    assert_eq!(voice_users(&app), [("alice".to_owned(), false, false)]);
    pump(&mut app, &mut events, &mut tasks, "connected", |a| {
        crate::voice::status_text(&a.core).is_some_and(|t| t.ends_with("ms"))
    })
    .await;
    let text = screen(&mut app);
    assert!(text.contains("voice: "), "{text}");
    assert!(text.contains("     alice"), "{text}");

    // m mutes, D deafens (the status line and the sidebar say so).
    app.on_key(key('m'));
    assert!(app.core.self_mute());
    pump(
        &mut app,
        &mut events,
        &mut tasks,
        "muted in the sidebar",
        |a| voice_users(a) == [("alice".to_owned(), true, false)],
    )
    .await;
    assert!(
        crate::voice::status_text(&app.core)
            .unwrap()
            .ends_with(" m")
    );
    app.on_key(key('D'));
    assert!(app.core.self_deaf());
    pump(&mut app, &mut events, &mut tasks, "deafened", |a| {
        voice_users(a) == [("alice".to_owned(), true, true)]
    })
    .await;
    assert!(
        crate::voice::status_text(&app.core)
            .unwrap()
            .ends_with(" d")
    );
    // Unmuting while deafened undeafens too.
    app.on_key(key('m'));
    assert!(!app.core.self_mute() && !app.core.self_deaf());

    // q asks first while in voice.
    app.on_key(key('q'));
    assert!(matches!(app.popup, Some(Popup::Confirm { .. })));
    assert!(!app.quit);
    app.on_key(key('n'));
    assert!(app.popup.is_none());

    // L leaves.
    app.on_key(key('L'));
    pump(&mut app, &mut events, &mut tasks, "left", |a| {
        a.core.voice_channel().is_none() && voice_users(a).is_empty()
    })
    .await;
    assert!(crate::voice::status_text(&app.core).is_none());

    // Bob calls alice; `:voice` answers.
    let (bob, _bob_events) = core();
    let space = *alice_core
        .store()
        .instance(&inst)
        .unwrap()
        .spaces
        .keys()
        .next()
        .unwrap();
    let invite = alice_core
        .rest(&inst)
        .unwrap()
        .create_invite(space, &CreateInviteRequest::default())
        .await
        .unwrap();
    sign_up(&bob, &server, "bob", &invite.code).await;
    let alice_id = alice_core.store().instance(&inst).unwrap().me_id().unwrap();
    let dm = bob
        .rest(&inst)
        .unwrap()
        .create_dm(vec![alice_id])
        .await
        .unwrap()
        .id;
    bob.join_or_call(&inst, dm).await.unwrap();
    pump(&mut app, &mut events, &mut tasks, "ringing", |a| {
        a.ringing.as_ref().is_some_and(|(_, c, _)| *c == dm)
    })
    .await;
    assert!(app.status.as_ref().unwrap().0.contains("bob is calling"));
    app.run(Command::Voice(String::new()));
    pump(&mut app, &mut events, &mut tasks, "in the call", |a| {
        a.core.voice_channel() == Some((inst.clone(), dm))
    })
    .await;
    assert!(app.ringing.is_none());
    assert_eq!(app.current, Some((inst.clone(), dm)));
    pump(
        &mut app,
        &mut events,
        &mut tasks,
        "both in the DM call",
        |a| voice_users(a).len() == 2,
    )
    .await;
    alice_core.leave_voice().await;
    bob.leave_voice().await;
    drop(server);
}
