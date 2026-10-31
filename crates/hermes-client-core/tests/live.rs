//! `Core` against a real `hermes-server` with a self-signed certificate (PLAN.md section 19,
//! "Client core"): probing and pinning, register/login, the gateway, optimistic sending,
//! notifications, the offline queue with resume, and logout on a revoked session.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hermes_client_core::address::{Fingerprint, HostPort, InviteLink};
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::api::CreateInviteRequest;
use hermes_client_core::proto::id::Id;
use hermes_client_core::store::{Changed, PendingState};
use hermes_client_core::{
    CertStatus, Core, CoreError, CoreEvent, CoreEvents, CoreOptions, Draft, InstanceId,
};
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
    let dir = std::env::temp_dir().join(format!("hermes-cc-live-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut config = Config::default();
    config.instance.name = "Live".into();
    config.storage.data_dir = dir.join("data");
    config.tls.mode = TlsMode::SelfSigned;
    config.registration.mode = RegistrationMode::Open;
    config.limits.requests_per_second = 100_000;
    config.limits.login_attempts_per_minute = 100_000;
    config.limits.messages_per_5s = 100_000;
    let tls = hermes_server::tls::load(&config).unwrap();
    let fp = *tls.fingerprint().unwrap();
    let mut state = AppState::load(config).await.unwrap();
    state.fingerprint = Some(fp);
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
        None,
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

fn core() -> (Core, CoreEvents) {
    Core::new(Default::default(), CoreOptions::in_memory("hermes-test"))
}

/// Waits for an event matching `f`, skipping others.
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

async fn wait_status(events: &mut CoreEvents, inst: &str, want: ConnStatus) {
    wait_for(events, &format!("{want:?}"), |e| {
        matches!(e, CoreEvent::InstanceStatus { inst: i, status } if i == inst && *status == want)
    })
    .await;
}

/// Polls `f` on the store until it is true.
async fn eventually(core: &Core, what: &str, f: impl Fn(&hermes_client_core::Store) -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !f(&core.store()) {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Registers and connects; returns the instance ID once `ready` arrived.
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
    wait_status(events, &inst, ConnStatus::Connected).await;
    inst
}

fn general(core: &Core, inst: &str) -> (Id, Id) {
    let store = core.store();
    let s = store.instance(inst).unwrap();
    let space = s.spaces.values().next().unwrap();
    let channel = space
        .channels
        .values()
        .find(|c| c.name.as_deref() == Some("general"))
        .unwrap();
    (space.space.id, channel.id)
}

#[tokio::test]
async fn probe_pin_register_and_chat() {
    let server = start("chat").await;
    let (alice, mut alice_events) = core();

    let probe = alice.probe_instance(&server.host).await.unwrap();
    assert_eq!(probe.info.name, "Live");
    assert_eq!(probe.fingerprint, server.fingerprint);
    assert_eq!(probe.cert, CertStatus::Unknown);
    let err = alice
        .register(
            &server.host,
            "alice",
            "correct horse",
            Some(&server.setup_code),
        )
        .await
        .unwrap_err();
    assert_eq!(err, CoreError::UntrustedCertificate(server.fingerprint));

    let inst = sign_up(
        &server,
        &alice,
        &mut alice_events,
        "alice",
        Some(&server.setup_code),
    )
    .await;
    assert_eq!(inst, server.host.display());
    assert_eq!(
        alice.probe_instance(&server.host).await.unwrap().cert,
        CertStatus::Pinned
    );
    let (space, channel) = general(&alice, &inst);
    let alice_id = alice.store().instance(&inst).unwrap().me_id().unwrap();

    // Bob joins with an invite from Alice.
    let invite = alice
        .rest(&inst)
        .unwrap()
        .create_invite(space, &CreateInviteRequest::default())
        .await
        .unwrap();
    let (bob, mut bob_events) = core();
    // The preview needs no account, only a trusted certificate.
    assert!(matches!(
        bob.invite_preview(&server.host, &invite.code).await,
        Err(CoreError::UntrustedCertificate(_))
    ));
    bob.trust_fingerprint(&server.host, server.fingerprint)
        .unwrap();
    let preview = bob
        .invite_preview(&server.host, &invite.code)
        .await
        .unwrap();
    assert_eq!(preview.space.unwrap().id, space);
    sign_up(&server, &bob, &mut bob_events, "bob", Some(&invite.code)).await;
    eventually(&alice, "alice sees bob join", |s| {
        s.instance(&inst).unwrap().spaces[&space].members.len() == 2
    })
    .await;

    // Alice sends: pending at once, then the real message.
    alice.set_visible_channels([(inst.clone(), channel)]);
    alice.open_channel(&inst, channel).await.unwrap();
    let local = alice
        .send_message(
            &inst,
            channel,
            Draft {
                content: "hello bob".into(),
                ..Draft::default()
            },
        )
        .unwrap();
    {
        let store = alice.store();
        let w = store.instance(&inst).unwrap().window(channel).unwrap();
        assert!(
            w.pending(local).is_some(),
            "shown before the server answered"
        );
    }
    eventually(&alice, "message confirmed", |s| {
        let w = s.instance(&inst).unwrap().window(channel).unwrap();
        w.pending.is_empty() && w.messages.iter().any(|m| m.content == "hello bob")
    })
    .await;

    // Bob sees it when opening the channel.
    eventually(&bob, "bob's channel updated", |s| {
        s.instance(&inst).unwrap().unread(channel)
    })
    .await;
    bob.open_channel(&inst, channel).await.unwrap();
    {
        let store = bob.store();
        let w = store.instance(&inst).unwrap().window(channel).unwrap();
        assert_eq!(w.messages.last().unwrap().content, "hello bob");
    }

    // A mention notifies Alice when the channel is not focused.
    alice.set_app_focused(false);
    bob.send_message(
        &inst,
        channel,
        Draft {
            content: format!("hi <@{alice_id}>"),
            ..Draft::default()
        },
    )
    .unwrap();
    wait_for(&mut alice_events, "a notification", |e| {
        matches!(e, CoreEvent::Notification { notification, .. }
            if notification.mention && notification.author == "bob")
    })
    .await;
    eventually(&alice, "mention counted", |s| {
        s.instance(&inst).unwrap().mention_count(channel) == 1
    })
    .await;

    // Reactions and edits arrive live.
    let message_id = {
        let store = alice.store();
        let w = store.instance(&inst).unwrap().window(channel).unwrap();
        w.messages.last().unwrap().id
    };
    bob.react(&inst, channel, message_id, "👍").await.unwrap();
    eventually(&alice, "reaction", |s| {
        let w = s.instance(&inst).unwrap().window(channel).unwrap();
        w.get(message_id).is_some_and(|m| {
            m.reactions
                .iter()
                .any(|r| r.emoji == "👍" && r.count == 1 && !r.me)
        })
    })
    .await;
    bob.edit_message(&inst, channel, message_id, "edited")
        .await
        .unwrap();
    eventually(&alice, "edit", |s| {
        let w = s.instance(&inst).unwrap().window(channel).unwrap();
        w.get(message_id).is_some_and(|m| m.content == "edited")
    })
    .await;

    // Marking read clears the count on the server too (it comes back as read_state_update).
    alice.mark_read(&inst, channel, message_id);
    assert_eq!(
        alice
            .store()
            .instance(&inst)
            .unwrap()
            .mention_count(channel),
        0
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let rest = alice.rest(&inst).unwrap();
    let me = rest.me().await.unwrap();
    assert_eq!(me.user.username, "alice");
}

#[tokio::test]
async fn offline_messages_queue_and_missed_events_replay_on_resume() {
    let server = start("resume").await;
    let (alice, mut alice_events) = core();
    let inst = sign_up(
        &server,
        &alice,
        &mut alice_events,
        "alice",
        Some(&server.setup_code),
    )
    .await;
    let (space, channel) = general(&alice, &inst);
    let invite = alice
        .rest(&inst)
        .unwrap()
        .create_invite(space, &CreateInviteRequest::default())
        .await
        .unwrap();
    let (bob, mut bob_events) = core();
    // The preview needs no account, only a trusted certificate.
    assert!(matches!(
        bob.invite_preview(&server.host, &invite.code).await,
        Err(CoreError::UntrustedCertificate(_))
    ));
    bob.trust_fingerprint(&server.host, server.fingerprint)
        .unwrap();
    let preview = bob
        .invite_preview(&server.host, &invite.code)
        .await
        .unwrap();
    assert_eq!(preview.space.unwrap().id, space);
    sign_up(&server, &bob, &mut bob_events, "bob", Some(&invite.code)).await;
    bob.set_visible_channels([(inst.clone(), channel)]);
    bob.open_channel(&inst, channel).await.unwrap();

    bob.set_online(&inst, false).unwrap();
    wait_status(&mut bob_events, &inst, ConnStatus::Offline).await;

    for text in ["one", "two"] {
        alice
            .rest(&inst)
            .unwrap()
            .send_message(
                channel,
                &hermes_client_core::proto::api::SendMessageRequest {
                    content: text.into(),
                    nonce: hermes_client_core::uuid_v4(),
                    reply_to_id: None,
                    attachment_ids: vec![],
                },
            )
            .await
            .unwrap();
    }
    let local = bob
        .send_message(
            &inst,
            channel,
            Draft {
                content: "three".into(),
                ..Draft::default()
            },
        )
        .unwrap();
    {
        let store = bob.store();
        let w = store.instance(&inst).unwrap().window(channel).unwrap();
        assert_eq!(w.pending(local).unwrap().state, PendingState::Queued);
        assert!(w.messages.iter().all(|m| m.content != "one"));
    }

    bob.set_online(&inst, true).unwrap();
    wait_status(&mut bob_events, &inst, ConnStatus::Connected).await;
    eventually(&bob, "replayed and sent in order", |s| {
        let w = s.instance(&inst).unwrap().window(channel).unwrap();
        let texts: Vec<&str> = w.messages.iter().map(|m| m.content.as_str()).collect();
        w.pending.is_empty() && texts.ends_with(&["one", "two", "three"])
    })
    .await;
}

#[tokio::test]
async fn changed_certificates_are_refused() {
    let server = start("pin").await;
    let (core, _events) = core();
    core.trust_fingerprint(&server.host, Fingerprint([1; 32]))
        .unwrap();
    let probe = core.probe_instance(&server.host).await.unwrap();
    assert_eq!(
        probe.cert,
        CertStatus::Changed {
            pinned: Fingerprint([1; 32])
        }
    );
    let err = core
        .login(&server.host, "nobody", "whatever1")
        .await
        .unwrap_err();
    assert_eq!(
        err,
        CoreError::CertificateChanged {
            pinned: Fingerprint([1; 32]),
            now: server.fingerprint
        }
    );
}

#[tokio::test]
async fn revoked_sessions_log_out_and_login_works() {
    let server = start("revoke").await;
    let (alice, mut events) = core();
    let inst = sign_up(
        &server,
        &alice,
        &mut events,
        "alice",
        Some(&server.setup_code),
    )
    .await;

    // Another device signs in and revokes this session.
    let other = hermes_client_core::rest::Rest::new(
        &server.host,
        hermes_client_core::tls::Trust::Pinned(server.fingerprint),
    )
    .unwrap();
    let auth = other
        .login(&hermes_client_core::proto::api::LoginRequest {
            username: "alice".into(),
            password: "correct horse".into(),
            device_name: "Phone".into(),
        })
        .await
        .unwrap();
    let other = other.with_token(&auth.token);
    let current = alice.rest(&inst).unwrap().sessions().await.unwrap();
    let mine = current.iter().find(|s| s.current).unwrap().id;
    other.delete_session(mine).await.unwrap();
    wait_status(&mut events, &inst, ConnStatus::LoggedOut).await;

    // Signing in again reconnects.
    let again = alice
        .login(&server.host, "alice", "correct horse")
        .await
        .unwrap();
    assert_eq!(again, inst);
    wait_for(&mut events, "ready", |e| {
        matches!(
            e,
            CoreEvent::StoreChanged {
                what: Changed::All,
                ..
            }
        )
    })
    .await;
    wait_status(&mut events, &inst, ConnStatus::Connected).await;
}
