//! End-to-end: the TUI adds an instance from an invite link, creates the owner account, opens
//! `#general`, sends a message and renders it, all against a real `hermes-server`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hermes_client_core::config::ClientConfig;
use hermes_client_core::{Core, CoreOptions};
use hermes_server::config::RegistrationMode;
use tokio::sync::mpsc;

use crate::app::{App, Mode, Popup, SidebarItem};
use crate::commands::Command;
use crate::test_support::{pump, screen};

const SCREEN_WIDTH: u16 = 110;

/// A server from `hermes_server::testing`.
struct Server {
    link: String,
    _server: hermes_server::testing::TestServer,
}

async fn start(name: &str) -> Server {
    let server = hermes_server::testing::spawn(&format!("tui-{name}"), false, |c| {
        c.instance.name = "Friends".into();
        c.registration.mode = RegistrationMode::InviteOnly;
    })
    .await;
    Server {
        link: server.link.clone(),
        _server: server,
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

#[tokio::test]
async fn invite_sign_up_open_channel_and_send() {
    let server = start("e2e").await;
    let (core, mut events) = Core::new(
        ClientConfig::default(),
        CoreOptions::in_memory("hermes-tui"),
    );
    let (tx, mut tasks) = mpsc::unbounded_channel();
    let mut app = App::new(core, tx, 0);
    assert!(screen(&mut app, SCREEN_WIDTH).contains(":instance add"));

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
    assert!(screen(&mut app, SCREEN_WIDTH).contains("Create an account"));
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
    let text = screen(&mut app, SCREEN_WIDTH);
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
    let text = screen(&mut app, SCREEN_WIDTH);
    assert!(text.contains("¯\\_(ツ"), "{text}");
}

#[tokio::test]
async fn menus_search_and_space_commands() {
    let server = start("menus").await;
    let (core, mut events) = Core::new(
        ClientConfig::default(),
        CoreOptions::in_memory("hermes-tui"),
    );
    let (tx, mut tasks) = mpsc::unbounded_channel();
    let mut app = App::new(core, tx, 0);
    app.run(Command::AddInstance(server.link.clone()));
    pump(&mut app, &mut events, &mut tasks, "the form", |a| {
        matches!(a.popup, Some(Popup::Form(_)))
    })
    .await;
    type_text(&mut app, "alice");
    app.on_key(key(KeyCode::Enter));
    type_text(&mut app, "correct horse");
    app.on_key(key(KeyCode::Enter));
    let general = |a: &App| {
        a.sidebar
            .iter()
            .position(|i| matches!(i, SidebarItem::Channel { name, .. } if name == "general"))
    };
    pump(&mut app, &mut events, &mut tasks, "#general", |a| {
        general(a).is_some()
    })
    .await;

    // `a` on a sidebar channel opens its menu; "Open" opens it.
    app.sidebar_sel = general(&app).unwrap();
    app.on_key(key(KeyCode::Char('a')));
    let Some(Popup::Menu { items, .. }) = &app.popup else {
        panic!("no menu: {:?}", app.popup)
    };
    assert_eq!(items[0].0, "Open");
    assert!(items.iter().any(|(l, _)| l == "Mute"));
    assert!(screen(&mut app, SCREEN_WIDTH).contains("Enter: do it"));
    app.on_key(key(KeyCode::Enter));
    assert!(app.popup.is_none());
    let (inst, channel) = app.current.clone().expect("opened");

    for text in ["the pizza party", "something else", "more pizza"] {
        app.on_key(key(KeyCode::Char('i')));
        type_text(&mut app, text);
        app.on_key(key(KeyCode::Enter));
        app.on_key(key(KeyCode::Esc));
    }
    pump(&mut app, &mut events, &mut tasks, "messages sent", |a| {
        a.core
            .store()
            .instance(&inst)
            .and_then(|s| s.window(channel))
            .is_some_and(|w| w.pending.is_empty() && w.messages.len() >= 3)
    })
    .await;

    // `/` starts a search; the results popup jumps to the message.
    app.on_key(key(KeyCode::Char('/')));
    assert_eq!(app.mode, Mode::Command);
    type_text(&mut app, "pizza");
    app.on_key(key(KeyCode::Enter));
    pump(&mut app, &mut events, &mut tasks, "search results", |a| {
        matches!(a.popup, Some(Popup::Results { .. }))
    })
    .await;
    let Some(Popup::Results { lines, .. }) = &app.popup else {
        unreachable!()
    };
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains("more pizza"), "newest first: {lines:?}");
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Enter));
    let selected = app.selected_message().map(|(_, _, content)| content);
    assert_eq!(selected.as_deref(), Some("the pizza party"));

    // The message menu offers pinning to the owner; then :pins lists it.
    app.on_key(key(KeyCode::Char('a')));
    let Some(Popup::Menu { items, .. }) = &app.popup else {
        panic!("no message menu")
    };
    let pin = items
        .iter()
        .position(|(l, _)| l == "Pin")
        .expect("Pin entry");
    for _ in 0..pin {
        app.on_key(key(KeyCode::Down));
    }
    app.on_key(key(KeyCode::Enter));
    pump(&mut app, &mut events, &mut tasks, "pinned", |a| {
        a.core
            .store()
            .instance(&inst)
            .and_then(|s| s.window(channel))
            .is_some_and(|w| w.messages.iter().any(|m| m.pinned))
    })
    .await;
    app.run(Command::Pins);
    pump(
        &mut app,
        &mut events,
        &mut tasks,
        "pins",
        |a| matches!(&a.popup, Some(Popup::Results { lines, .. }) if lines.len() == 1),
    )
    .await;
    app.on_key(key(KeyCode::Esc));

    // :create makes a channel in the space.
    app.run(crate::commands::parse("create text Game Night").unwrap());
    pump(&mut app, &mut events, &mut tasks, "#game-night", |a| {
        a.sidebar
            .iter()
            .any(|i| matches!(i, SidebarItem::Channel { name, .. } if name == "game-night"))
    })
    .await;
}
