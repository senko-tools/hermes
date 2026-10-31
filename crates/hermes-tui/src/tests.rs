//! End-to-end: the TUI adds an instance from an invite link, creates the owner account, opens
//! `#general`, sends a message and renders it, all against a real `hermes-server`.

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hermes_client_core::config::ClientConfig;
use hermes_client_core::{Core, CoreEvents, CoreOptions};
use hermes_server::config::{Config, RegistrationMode, TlsMode};
use hermes_server::state::AppState;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tokio::sync::{Notify, mpsc};

use crate::app::{App, Mode, Popup, SidebarItem, TaskMsg};
use crate::commands::Command;
use crate::ui;

struct Server {
    link: String,
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
    let dir = std::env::temp_dir().join(format!("hermes-tui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = Config::default();
    config.instance.name = "Friends".into();
    config.instance.public_host = "127.0.0.1".into();
    config.network.port = tcp.local_addr().unwrap().port();
    config.storage.data_dir = dir.join("data");
    config.tls.mode = TlsMode::SelfSigned;
    config.registration.mode = RegistrationMode::InviteOnly;
    let tls = hermes_server::tls::load(&config).unwrap();
    let fp = *tls.fingerprint().unwrap();
    let mut state = AppState::load(config).await.unwrap();
    state.fingerprint = Some(fp);
    let links = hermes_server::setup::first_start(&state, Some(&fp))
        .await
        .unwrap()
        .unwrap();
    let shutdown = Arc::new(Notify::new());
    let signal = {
        let shutdown = shutdown.clone();
        async move { shutdown.notified().await }
    };
    tokio::spawn(hermes_server::server::run(
        Arc::new(state),
        tcp,
        tls,
        None,
        signal,
    ));
    Server {
        link: links.web,
        shutdown,
        dir,
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

/// Feeds core events and task results to the app until `done` holds.
async fn pump(
    app: &mut App,
    events: &mut CoreEvents,
    tasks: &mut mpsc::UnboundedReceiver<TaskMsg>,
    what: &str,
    done: impl Fn(&App) -> bool,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
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
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
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

#[tokio::test]
async fn invite_sign_up_open_channel_and_send() {
    let server = start("e2e").await;
    let (core, mut events) = Core::new(
        ClientConfig::default(),
        CoreOptions::in_memory("hermes-tui"),
    );
    let (tx, mut tasks) = mpsc::unbounded_channel();
    let mut app = App::new(core, tx, 0);
    assert!(screen(&mut app).contains(":instance add"));

    // The invite carries the fingerprint, so no trust prompt: straight to the account form.
    app.run(Command::AddInstance(server.link.clone()));
    pump(&mut app, &mut events, &mut tasks, "the form", |a| {
        matches!(a.popup, Some(Popup::Form(_)))
    })
    .await;
    let Some(Popup::Form(form)) = &app.popup else {
        unreachable!()
    };
    assert!(form.register, "an invite link starts with account creation");
    assert!(screen(&mut app).contains("Create an account"));
    type_text(&mut app, "alice");
    app.on_key(key(KeyCode::Enter));
    type_text(&mut app, "correct horse");
    app.on_key(key(KeyCode::Enter));
    pump(
        &mut app,
        &mut events,
        &mut tasks,
        "#general in the sidebar",
        |a| {
            a.sidebar
                .iter()
                .any(|i| matches!(i, SidebarItem::Channel { name, .. } if name == "general"))
        },
    )
    .await;
    assert!(app.popup.is_none());

    // Open #general from the sidebar and send a message.
    app.sidebar_sel = app
        .sidebar
        .iter()
        .position(|i| matches!(i, SidebarItem::Channel { name, .. } if name == "general"))
        .unwrap();
    app.on_key(key(KeyCode::Enter));
    assert!(app.current.is_some());
    app.on_key(key(KeyCode::Char('i')));
    assert_eq!(app.mode, Mode::Insert);
    type_text(&mut app, "hello from the **TUI**");
    app.on_key(key(KeyCode::Enter));
    let (inst, channel) = app.current.clone().unwrap();
    pump(
        &mut app,
        &mut events,
        &mut tasks,
        "message confirmed",
        |a| {
            a.core
                .store()
                .instance(&inst)
                .and_then(|s| s.window(channel))
                .is_some_and(|w| {
                    w.pending.is_empty() && w.messages.iter().any(|m| m.content.contains("TUI"))
                })
        },
    )
    .await;
    let text = screen(&mut app);
    assert!(text.contains("hello from the TUI"), "{text}");
    assert!(text.contains("#general — Friends"), "{text}");
    assert!(text.contains("alice"), "{text}");

    // Slash commands and :set.
    app.on_key(key(KeyCode::Esc));
    app.run(Command::Set("buffer.group_minutes".into(), "3".into()));
    assert_eq!(app.core.config().buffer.group_minutes, 3);
    app.on_key(key(KeyCode::Char('i')));
    type_text(&mut app, "/shrug");
    app.on_key(key(KeyCode::Enter));
    pump(&mut app, &mut events, &mut tasks, "shrug sent", |a| {
        a.core
            .store()
            .instance(&inst)
            .and_then(|s| s.window(channel))
            .is_some_and(|w| w.messages.iter().any(|m| m.content.contains("(ツ)")))
    })
    .await;
    let text = screen(&mut app);
    assert!(text.contains("¯\\_(ツ"), "{text}");
}
