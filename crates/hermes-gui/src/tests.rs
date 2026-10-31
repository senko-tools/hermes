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
