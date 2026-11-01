//! End-to-end: the add-instance dialog against a real `hermes-server`: invite link with a
//! fingerprint → create the owner account; plain address → trust on first use → sign in; a space
//! invite shows its preview; a changed certificate shows the warning.

use std::sync::Arc;
use std::time::Duration;

use hermes_client_core::address::{Fingerprint, HostPort};
use hermes_client_core::proto::api::{CreateInviteRequest, RegistrationMode as Registration};
use hermes_client_core::{CertStatus, Core, CoreOptions};
use hermes_server::config::{Config, RegistrationMode, TlsMode};
use hermes_server::state::AppState;
use tokio::sync::Notify;

use crate::app::{App, Level};
use crate::message::Message;
use crate::modal::add_instance::{self, Step, Tab};

struct Server {
    link: String,
    host: HostPort,
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
    let dir = std::env::temp_dir().join(format!("hermes-gui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = tcp.local_addr().unwrap().port();
    let mut config = Config::default();
    config.instance.name = "Friends".into();
    config.instance.description = "Our place".into();
    config.instance.public_host = "127.0.0.1".into();
    config.network.port = port;
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
        host: HostPort::parse(&format!("127.0.0.1:{port}")).unwrap(),
        shutdown,
        dir,
    }
}

fn app() -> App {
    let (core, _events) = Core::new(Default::default(), CoreOptions::in_memory("hermes-gui"));
    App::new(core, None, None)
}

fn dialog(app: &App) -> &add_instance::AddInstance {
    app.add_instance.as_ref().expect("dialog open")
}

fn send(app: &mut App, message: add_instance::Message) {
    let _ = app.update(Message::AddInstance(message));
}

/// Opens the dialog with `input` and runs the probe the dialog would start.
async fn probe(app: &mut App, input: &str) {
    let _ = app.update(Message::OpenAddInstance);
    send(app, add_instance::Message::Address(input.to_owned()));
    send(app, add_instance::Message::SubmitAddress);
    assert!(dialog(app).busy);
    let result = add_instance::probe(app.core.clone(), input.to_owned()).await;
    send(app, add_instance::Message::Probed(result));
    assert!(!dialog(app).busy);
}

/// Fills the account step and runs the sign-in or registration the dialog would start.
async fn submit(app: &mut App, username: &str, password: &str) {
    send(app, add_instance::Message::Username(username.to_owned()));
    send(app, add_instance::Message::Password(password.to_owned()));
    send(app, add_instance::Message::Submit);
    let Step::Account(account) = &dialog(app).step else {
        panic!("not on the account step: {:?}", dialog(app));
    };
    assert!(dialog(app).busy, "{:?}", dialog(app).error);
    let result = add_instance::submit(app.core.clone(), (**account).clone()).await;
    send(app, add_instance::Message::Done(result));
}

fn account(app: &App) -> &add_instance::Account {
    match &dialog(app).step {
        Step::Account(account) => account,
        other => panic!("not on the account step: {other:?}"),
    }
}

async fn first_space(core: &Core, inst: &str) -> hermes_client_core::proto::id::Id {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(id) = core
            .store()
            .instance(inst)
            .and_then(|s| s.spaces.keys().next().copied())
        {
            return id;
        }
        assert!(tokio::time::Instant::now() < deadline, "no space arrived");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn invite_link_creates_the_account_then_others_sign_in_and_join() {
    let server = start("add-instance").await;

    // The owner clicks the setup invite link: its #fp= is trusted without asking.
    let mut owner = app();
    probe(&mut owner, &server.link).await;
    let step = account(&owner);
    assert_eq!(step.name, "Friends");
    assert_eq!(step.description, "Our place");
    assert_eq!(
        step.tab,
        Tab::Register,
        "invite links start on Create account"
    );
    submit(&mut owner, "Alice", "correct horse").await;
    assert!(owner.add_instance.is_none(), "dialog closes");
    let inst = server.host.display();
    assert_eq!(owner.core.instance_ids(), std::slice::from_ref(&inst));
    assert_eq!(owner.status.as_ref().unwrap().level, Level::Info);
    assert_eq!(
        owner.core.config().instance(&inst).unwrap().username,
        "alice"
    );

    // A friend types the plain address: no fingerprint, so the dialog asks.
    let mut friend = app();
    probe(&mut friend, &server.host.display()).await;
    match &dialog(&friend).step {
        Step::Fingerprint { probe, .. } => assert_eq!(probe.cert, CertStatus::Unknown),
        other => panic!("expected the fingerprint step: {other:?}"),
    }
    send(&mut friend, add_instance::Message::Trust);
    let step = account(&friend);
    assert_eq!(step.tab, Tab::SignIn);
    assert!(!step.can_register(), "invite_only without an invite");
    send(&mut friend, add_instance::Message::Tab(Tab::Register));
    assert_eq!(
        account(&friend).tab,
        Tab::SignIn,
        "Create account stays disabled"
    );

    // Wrong password: the error shows in the dialog, which stays open.
    submit(&mut friend, "alice", "wrong").await;
    assert!(dialog(&friend).error.is_some());
    assert!(!dialog(&friend).busy);
    submit(&mut friend, "alice", "correct horse").await;
    assert!(friend.add_instance.is_none(), "{:?}", friend.add_instance);
    assert_eq!(friend.core.instance_ids(), std::slice::from_ref(&inst));

    // A space invite shows the space before the account step.
    let space = first_space(&owner.core, &inst).await;
    let invite = owner
        .core
        .rest(&inst)
        .unwrap()
        .create_invite(space, &CreateInviteRequest::default())
        .await
        .unwrap();
    let link = owner.core.invite_link(&inst, &invite.code).unwrap();
    assert!(link.contains("#fp="), "{link}");
    let mut bob = app();
    probe(&mut bob, &link).await;
    let step = account(&bob);
    let preview = step
        .preview
        .as_ref()
        .and_then(|p| p.space.as_ref())
        .unwrap();
    assert_eq!(preview.id, space);
    assert_eq!(preview.member_count, 1);
    assert_eq!(step.tab, Tab::Register);
    submit(&mut bob, "bob", "battery staple").await;
    assert!(bob.add_instance.is_none());
    let bob_space = first_space(&bob.core, &inst).await;
    assert_eq!(bob_space, space, "bob joined the invite's space");
}

#[tokio::test]
async fn changed_certificate_shows_the_warning() {
    let server = start("changed-cert").await;
    let mut app = app();
    let other = Fingerprint([7; 32]);
    app.core.trust_fingerprint(&server.host, other).unwrap();
    probe(&mut app, &server.host.display()).await;
    match &dialog(&app).step {
        Step::Fingerprint { probe, .. } => {
            assert_eq!(probe.cert, CertStatus::Changed { pinned: other });
        }
        other => panic!("expected the warning: {other:?}"),
    }
    // Back returns to the address, with it filled in.
    send(&mut app, add_instance::Message::Back);
    assert_eq!(
        dialog(&app).step,
        Step::Address {
            input: server.host.display()
        }
    );
    send(&mut app, add_instance::Message::Cancel);
    assert!(app.add_instance.is_none());
}

#[tokio::test]
async fn bad_address_is_an_error_in_the_dialog() {
    let mut app = app();
    probe(&mut app, "not a host:99999").await;
    assert!(matches!(dialog(&app).step, Step::Address { .. }));
    assert!(dialog(&app).error.is_some());
}

#[test]
fn account_rules() {
    let base = add_instance::Account {
        host: HostPort::parse("example.com").unwrap(),
        name: "x".into(),
        description: String::new(),
        registration: Registration::InviteOnly,
        invite: None,
        preview: None,
        tab: Tab::Register,
        username: "ab".into(),
        password: "pw".into(),
    };
    assert!(!base.can_register());
    let with_invite = add_instance::Account {
        invite: Some("code".into()),
        ..base.clone()
    };
    assert!(with_invite.can_register());
    let open = add_instance::Account {
        registration: Registration::Open,
        ..base.clone()
    };
    assert!(open.can_register());

    assert!(base.validate().is_ok());
    for bad in ["a", "with space", "ünï", &"x".repeat(33)] {
        let account = add_instance::Account {
            username: bad.to_owned(),
            ..base.clone()
        };
        assert!(account.validate().is_err(), "{bad}");
    }
    let sign_in = add_instance::Account {
        username: "Any Name".into(),
        tab: Tab::SignIn,
        ..base.clone()
    };
    assert!(
        sign_in.validate().is_ok(),
        "sign in leaves names to the server"
    );
    let empty = add_instance::Account {
        password: String::new(),
        ..base
    };
    assert!(empty.validate().is_err());
}

#[tokio::test]
async fn menus_confirm_destructive_actions_and_escape_closes() {
    use crate::sidebar::menu::{Action, Target};
    use hermes_client_core::proto::id::Id;

    let mut app = app();
    let space = Target::Space("h".into(), Id(10));
    let _ = app.update(Message::OpenMenu(space.clone()));
    assert!(app.menu.is_some());
    let _ = app.update(Message::Escape);
    assert!(app.menu.is_none());

    // Destructive entries ask first; the menu closes either way.
    let _ = app.update(Message::OpenMenu(space.clone()));
    let _ = app.update(Message::Menu(space.clone(), Action::LeaveSpace));
    assert!(app.menu.is_none());
    assert_eq!(app.confirm, Some((space.clone(), Action::LeaveSpace)));
    let _ = app.update(Message::Escape);
    assert!(app.confirm.is_none());

    // Copying reports what was copied.
    let _ = app.update(Message::Menu(
        Target::Channel("h".into(), Id(11)),
        Action::CopyChannelId,
    ));
    assert_eq!(app.status.as_ref().unwrap().text, "Channel ID copied");
    assert!(app.confirm.is_none());
}

/// Waits until `f` holds for the core's store.
async fn eventually(core: &Core, what: &str, f: impl Fn(&hermes_client_core::Store) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !f(&core.store()) {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn channel_buffer_rows_follow_history_reads_and_deletes() {
    use crate::buffer::Buffer;
    use crate::buffer::channel::rows::{self, Row};
    use hermes_client_core::Draft;

    let server = start("channel-buffer").await;
    let mut alice = app();
    probe(&mut alice, &server.link).await;
    submit(&mut alice, "alice", "correct horse").await;
    let inst = server.host.display();
    let space = first_space(&alice.core, &inst).await;
    let general = alice.core.store().instance(&inst).unwrap().spaces[&space]
        .channels
        .values()
        .find(|c| c.name.as_deref() == Some("general"))
        .unwrap()
        .id;
    let invite = alice
        .core
        .rest(&inst)
        .unwrap()
        .create_invite(space, &CreateInviteRequest::default())
        .await
        .unwrap();
    let mut bob = app();
    probe(
        &mut bob,
        &alice.core.invite_link(&inst, &invite.code).unwrap(),
    )
    .await;
    submit(&mut bob, "bob", "battery staple").await;
    first_space(&bob.core, &inst).await;

    let send = |text: &str| {
        bob.core
            .send_message(
                &inst,
                general,
                Draft {
                    content: text.into(),
                    ..Default::default()
                },
            )
            .unwrap();
    };
    send("first");
    eventually(&alice.core, "alice sees the first message", |s| {
        s.instance(&inst)
            .and_then(|s| s.channel(general))
            .is_some_and(|c| c.last_message_id.is_some())
    })
    .await;
    let first = alice
        .core
        .store()
        .instance(&inst)
        .unwrap()
        .channel(general)
        .unwrap()
        .last_message_id
        .unwrap();
    // Alice reads up to the first message, then Bob writes two more.
    alice.core.mark_read(&inst, general, first);
    eventually(&alice.core, "the read marker", |s| {
        s.instance(&inst)
            .and_then(|s| s.read_states.get(&general))
            .is_some_and(|r| r.last_read_id == first)
    })
    .await;
    send("second");
    send("third");
    eventually(&alice.core, "three messages", |s| {
        s.instance(&inst)
            .and_then(|s| s.channel(general))
            .and_then(|c| c.last_message_id)
            .is_some_and(|l| l > first)
    })
    .await;

    // Alice opens #general: the divider sits before "second".
    let _ = alice.update(Message::Select(inst.clone(), general));
    assert_eq!(
        alice.panes.focused().unwrap().buffer,
        Buffer::Channel {
            inst: inst.clone(),
            channel: general
        }
    );
    assert_eq!(
        alice.panes.focused().unwrap().channel.read_marker,
        Some(first)
    );
    alice.core.open_channel(&inst, general).await.unwrap();
    let build = |app: &App| {
        let store = app.core.store();
        let s = store.instance(&inst).unwrap();
        let window = s.window(general).unwrap();
        let rows = rows::build(
            s,
            window,
            &rows::Options {
                group_minutes: 7,
                utc_offset: 0,
                read_marker: app.panes.focused().unwrap().channel.read_marker,
            },
        );
        rows.iter()
            .map(|r| match r {
                Row::NewMessages => "new".to_owned(),
                Row::Message(m) => window.get(m.id).unwrap().content.clone(),
                _ => String::new(),
            })
            .filter(|t| !t.is_empty() && !t.contains("joined"))
            .collect::<Vec<_>>()
    };
    let shown = build(&alice);
    let tail: Vec<&str> = shown
        .iter()
        .rev()
        .take(4)
        .rev()
        .map(String::as_str)
        .collect();
    assert_eq!(tail, ["first", "new", "second", "third"], "{shown:?}");

    // Bob deletes "third": it disappears from Alice's list.
    let third = {
        let store = alice.core.store();
        let w = store.instance(&inst).unwrap().window(general).unwrap();
        w.messages.iter().find(|m| m.content == "third").unwrap().id
    };
    bob.core
        .delete_message(&inst, general, third)
        .await
        .unwrap();
    eventually(&alice.core, "the delete", |s| {
        s.instance(&inst)
            .and_then(|s| s.window(general))
            .and_then(|w| w.get(third))
            .is_some_and(|m| m.deleted)
    })
    .await;
    assert!(!build(&alice).contains(&"third".to_owned()));

    // A reaction toggles: the chip is "mine" (highlighted) after adding, gone after removing.
    let second = {
        let store = alice.core.store();
        let w = store.instance(&inst).unwrap().window(general).unwrap();
        w.messages
            .iter()
            .find(|m| m.content == "second")
            .unwrap()
            .id
    };
    let reactions = |app: &App| {
        app.core
            .store()
            .instance(&inst)
            .and_then(|s| s.window(general))
            .and_then(|w| w.get(second))
            .map(|m| m.reactions.clone())
            .unwrap_or_default()
    };
    alice
        .core
        .react(&inst, general, second, "👍")
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !reactions(&alice)
        .iter()
        .any(|r| r.emoji == "👍" && r.me && r.count == 1)
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{:?}",
            reactions(&alice)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    alice
        .core
        .unreact(&inst, general, second, "👍")
        .await
        .unwrap();
    while !reactions(&alice).is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{:?}",
            reactions(&alice)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn input_box_completes_sends_attaches_replies_and_edits() {
    use crate::buffer::channel::{self, input};
    use hermes_client_core::proto::id::Id;
    use iced::widget::text_editor::{Action as EditorAction, Motion};

    let server = start("input-box").await;
    let mut alice = app();
    probe(&mut alice, &server.link).await;
    submit(&mut alice, "alice", "correct horse").await;
    let inst = server.host.display();
    let space = first_space(&alice.core, &inst).await;
    let general = alice.core.store().instance(&inst).unwrap().spaces[&space]
        .channels
        .values()
        .find(|c| c.name.as_deref() == Some("general"))
        .unwrap()
        .id;
    let invite = alice
        .core
        .rest(&inst)
        .unwrap()
        .create_invite(space, &CreateInviteRequest::default())
        .await
        .unwrap();
    let mut bob = app();
    probe(
        &mut bob,
        &alice.core.invite_link(&inst, &invite.code).unwrap(),
    )
    .await;
    submit(&mut bob, "bob", "battery staple").await;
    first_space(&bob.core, &inst).await;
    let bob_id = bob.core.store().instance(&inst).unwrap().me_id().unwrap();
    eventually(&alice.core, "alice knows bob", |s| {
        s.instance(&inst)
            .is_some_and(|s| s.users.contains_key(&bob_id))
    })
    .await;

    let _ = alice.update(Message::Select(inst.clone(), general));
    alice.core.open_channel(&inst, general).await.unwrap();
    let pane = alice.panes.focus;
    let input_msg = |app: &mut App, m: input::Message| {
        let _ = app.update(Message::Channel(pane, channel::Message::Input(m)));
    };
    fn composer<'a>(app: &'a App, inst: &str, general: Id) -> &'a input::Composer {
        app.composers.get(&(inst.to_owned(), general)).unwrap()
    }

    // Typing "@bo" offers Bob; accepting inserts his ID form.
    alice
        .composers
        .get_mut(&(inst.clone(), general))
        .unwrap()
        .set_text("hi @bo");
    input_msg(
        &mut alice,
        input::Message::Edit(EditorAction::Move(Motion::DocumentEnd)),
    );
    assert!(
        composer(&alice, &inst, general)
            .suggestions
            .iter()
            .any(|s| s.insert == format!("<@{bob_id}> ")),
        "{:?}",
        composer(&alice, &inst, general).suggestions
    );
    input_msg(&mut alice, input::Message::Accept(usize::MAX));
    assert!(
        composer(&alice, &inst, general)
            .content
            .text()
            .starts_with(&format!("hi <@{bob_id}> ")),
        "the insert ends with a space to keep typing"
    );

    // A file is attached: it must finish uploading before sending.
    let file = std::env::temp_dir().join(format!("hermes-gui-{}-note.txt", std::process::id()));
    std::fs::write(&file, b"remember the snacks").unwrap();
    let _ = alice.update(Message::FilesPicked(
        inst.clone(),
        general,
        vec![file.clone()],
    ));
    let local = composer(&alice, &inst, general).uploads[0].local;
    input_msg(&mut alice, input::Message::Submit);
    assert!(
        !composer(&alice, &inst, general).is_empty(),
        "not sent while uploading"
    );
    let uploaded = alice
        .core
        .upload(&inst, &file)
        .await
        .map_err(|e| e.to_string());
    let _ = alice.update(Message::Uploaded(inst.clone(), general, local, uploaded));
    input_msg(&mut alice, input::Message::Submit);
    assert!(composer(&alice, &inst, general).is_empty());
    assert!(composer(&alice, &inst, general).uploads.is_empty());

    let first = loop {
        let found = bob
            .core
            .rest(&inst)
            .unwrap()
            .messages(general, &Default::default())
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.author.username == "alice" && !m.attachments.is_empty());
        if let Some(m) = found {
            break m;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(first.content, format!("hi <@{bob_id}>"));
    assert_eq!(first.mentions.users, [bob_id]);
    assert_eq!(
        first.attachments[0].filename,
        file.file_name().unwrap().to_string_lossy()
    );

    // Reply mode: the next message replies to the first; Escape-like Cancel leaves the mode.
    eventually(&alice.core, "alice's own message", |s| {
        s.instance(&inst)
            .and_then(|s| s.window(general))
            .is_some_and(|w| w.get(first.id).is_some())
    })
    .await;
    let reply = Message::Menu(
        crate::sidebar::menu::Target::Message(inst.clone(), general, first.id),
        crate::sidebar::menu::Action::Reply,
    );
    let _ = alice.update(reply.clone());
    assert_eq!(composer(&alice, &inst, general).reply_to, Some(first.id));
    input_msg(&mut alice, input::Message::Cancel);
    assert_eq!(composer(&alice, &inst, general).reply_to, None);
    let _ = alice.update(reply);
    alice
        .composers
        .get_mut(&(inst.clone(), general))
        .unwrap()
        .set_text("/shrug sure");
    input_msg(&mut alice, input::Message::Submit);
    let reply = loop {
        let found = bob
            .core
            .rest(&inst)
            .unwrap()
            .messages(general, &Default::default())
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.reply_to.is_some());
        if let Some(m) = found {
            break m;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(reply.reply_to.unwrap().id, first.id);
    assert!(reply.content.starts_with("sure "), "{}", reply.content);

    // Up in an empty box edits my last message.
    eventually(&alice.core, "the reply", |s| {
        s.instance(&inst)
            .and_then(|s| s.window(general))
            .is_some_and(|w| w.get(reply.id).is_some())
    })
    .await;
    input_msg(&mut alice, input::Message::EditLast);
    assert_eq!(composer(&alice, &inst, general).editing, Some(reply.id));
    assert_eq!(composer(&alice, &inst, general).text(), reply.content);
    input_msg(&mut alice, input::Message::Cancel);
    assert_eq!(composer(&alice, &inst, general).editing, None);
    assert!(composer(&alice, &inst, general).is_empty());

    // Drafts stay with their channel.
    alice
        .composers
        .get_mut(&(inst.clone(), general))
        .unwrap()
        .set_text("unfinished");
    let _ = alice.update(Message::OpenSettings);
    let _ = alice.update(Message::Select(inst.clone(), general));
    assert_eq!(composer(&alice, &inst, general).text(), "unfinished");
    let _ = std::fs::remove_file(&file);
}

#[tokio::test]
async fn typing_anywhere_goes_into_the_message_box() {
    use iced::keyboard::{self, Key, Modifiers};

    const ANY: keyboard::key::Physical =
        keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
    let server = start("type-to-focus").await;
    let mut owner = app();
    probe(&mut owner, &server.link).await;
    submit(&mut owner, "alice", "correct horse").await;
    let inst = server.host.display();
    let space = first_space(&owner.core, &inst).await;
    let general = owner
        .core
        .store()
        .instance(&inst)
        .and_then(|s| {
            s.spaces[&space]
                .channels
                .values()
                .find(|c| c.name.as_deref() == Some("general"))
                .map(|c| c.id)
        })
        .expect("#general");
    let _ = owner.update(Message::Select(inst.clone(), general));
    let typed = |app: &mut App, c: &str, captured| {
        let _ = app.update(Message::Key(
            Key::Character(c.into()),
            ANY,
            Modifiers::empty(),
            captured,
        ));
    };
    typed(&mut owner, "h", false);
    typed(&mut owner, "i", false);
    typed(&mut owner, "x", true);
    assert_eq!(
        owner.composers[&(inst.clone(), general)].text(),
        "hi",
        "keys the box already took are not typed twice"
    );
    // Ctrl+letters are shortcuts, not text.
    let _ = owner.update(Message::Key(
        Key::Character("k".into()),
        ANY,
        Modifiers::CTRL,
        false,
    ));
    assert_eq!(owner.composers[&(inst, general)].text(), "hi");
}
