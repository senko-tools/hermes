//! `Core` against a real `hermes-server` with a self-signed certificate (PLAN.md section 19,
//! "Client core"): probing and pinning, register/login, the gateway, optimistic sending,
//! notifications, the offline queue with resume, and logout on a revoked session.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hermes_client_core::address::{Fingerprint, HostPort, InviteLink};
use hermes_client_core::gateway::ConnStatus;
use hermes_client_core::proto::api::CreateInviteRequest;
use hermes_client_core::proto::e2ee::MessageLock;
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
                    e2ee: None,
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
async fn invite_links_never_replace_a_pin() {
    let server = start("invite-pin").await;
    let (core, _events) = core();
    // A new host: the invite's fingerprint is pinned.
    assert!(
        core.trust_invite_fingerprint(&server.host, server.fingerprint)
            .unwrap()
    );
    assert_eq!(
        core.probe_instance(&server.host).await.unwrap().cert,
        CertStatus::Pinned
    );
    // A crafted link with another fingerprint changes nothing.
    assert!(
        !core
            .trust_invite_fingerprint(&server.host, Fingerprint([7; 32]))
            .unwrap()
    );
    assert_eq!(
        core.probe_instance(&server.host).await.unwrap().cert,
        CertStatus::Pinned
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

/// Alice and Bob, in a space and a DM: `(alice, her events, her instance, bob, his events, his
/// instance, the DM)`. Keys are published once this returns.
async fn dm_pair(
    server: &TestServer,
) -> (
    Core,
    CoreEvents,
    InstanceId,
    Core,
    CoreEvents,
    InstanceId,
    Id,
) {
    let (alice, mut alice_events) = core();
    let a = sign_up(
        server,
        &alice,
        &mut alice_events,
        "alice",
        Some(&server.setup_code),
    )
    .await;
    let (space, _) = general(&alice, &a);
    let invite = alice
        .rest(&a)
        .unwrap()
        .create_invite(space, &CreateInviteRequest::default())
        .await
        .unwrap();
    let (bob, mut bob_events) = core();
    let b = sign_up(server, &bob, &mut bob_events, "bob", Some(&invite.code)).await;
    let bob_id = bob.store().instance(&b).unwrap().me_id().unwrap();
    let dm = alice
        .rest(&a)
        .unwrap()
        .create_dm(vec![bob_id])
        .await
        .unwrap()
        .id;
    eventually(&alice, "alice sees the DM", |s| {
        s.instance(&a).is_some_and(|s| s.dms.contains_key(&dm))
    })
    .await;
    eventually(&bob, "bob sees the DM", |s| {
        s.instance(&b).is_some_and(|s| s.dms.contains_key(&dm))
    })
    .await;
    let alice_id = alice.store().instance(&a).unwrap().me_id().unwrap();
    wait_for_device(&alice, &a, alice_id).await;
    wait_for_device(&alice, &a, bob_id).await;
    (alice, alice_events, a, bob, bob_events, b, dm)
}

/// Until the server hands out a device for `user`.
async fn wait_for_device(core: &Core, inst: &str, user: Id) {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let found = core
            .rest(inst)
            .unwrap()
            .query_devices(&[user])
            .await
            .unwrap();
        if found[0].devices.iter().any(|d| !d.revoked) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no device for {user}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_for_window(core: &Core, inst: &str, channel: Id, text: &str) {
    let what = format!("{text:?} to arrive");
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let _ = core
            .load_history(
                inst,
                channel,
                hermes_client_core::store::HistoryAnchor::Latest,
            )
            .await;
        let found = core
            .store()
            .instance(inst)
            .and_then(|s| s.window(channel))
            .is_some_and(|w| w.messages.iter().any(|m| m.content == text));
        if found {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn draft(text: &str) -> Draft {
    Draft {
        content: text.into(),
        reply_to: None,
        attachments: vec![],
    }
}

#[tokio::test]
async fn the_server_only_ever_sees_ciphertext_of_direct_messages() {
    let server = start("e2ee-dm").await;
    let (alice, _a_events, a, bob, _b_events, b, dm) = dm_pair(&server).await;
    alice
        .load_history(&a, dm, hermes_client_core::store::HistoryAnchor::Latest)
        .await
        .unwrap();

    const SECRET: &str = "the password is swordfish";
    alice.send_message(&a, dm, draft(SECRET)).unwrap();
    wait_for_window(&bob, &b, dm, SECRET).await;

    // Both read it, marked as what it is.
    eventually(&alice, "alice's own message comes back readable", |s| {
        s.instance(&a)
            .and_then(|s| s.window(dm))
            .is_some_and(|w| w.pending.is_empty() && w.messages.iter().any(|m| m.content == SECRET))
    })
    .await;
    {
        let store = bob.store();
        let window = store.instance(&b).unwrap().window(dm).unwrap();
        let message = window
            .messages
            .iter()
            .find(|m| m.content == SECRET)
            .unwrap();
        assert_eq!(message.lock, MessageLock::Sealed);
        assert_eq!(message.e2ee, None, "the sealed form goes away once opened");
    }

    // What the server hands out has no trace of it, in the API or in its database file.
    let raw = bob
        .rest(&b)
        .unwrap()
        .messages(dm, &Default::default())
        .await
        .unwrap();
    let sealed = raw
        .iter()
        .find(|m| m.e2ee.is_some())
        .expect("a sealed message");
    assert_eq!(sealed.content, "");
    let json = serde_json::to_string(sealed.e2ee.as_ref().unwrap()).unwrap();
    assert!(!json.contains("swordfish"));
    let mut in_files = false;
    for entry in std::fs::read_dir(server.dir.join("data"))
        .unwrap()
        .flatten()
    {
        if let Ok(bytes) = std::fs::read(entry.path()) {
            in_files |= bytes.windows(9).any(|w| w == b"swordfish");
        }
    }
    assert!(!in_files, "the message's words are in the server's files");

    // The server's search finds nothing of it.
    let results = bob
        .rest(&b)
        .unwrap()
        .search(&hermes_client_core::proto::api::SearchQuery {
            q: "swordfish".into(),
            ..Default::default()
        })
        .await
        .map(|r| r.messages.len())
        .unwrap_or(0);
    assert_eq!(results, 0);

    // Editing stays encrypted.
    let id = sealed.id;
    alice
        .edit_message(&a, dm, id, "the password is hunter2")
        .await
        .unwrap();
    wait_for_window(&bob, &b, dm, "the password is hunter2").await;
    let raw = bob
        .rest(&b)
        .unwrap()
        .messages(dm, &Default::default())
        .await
        .unwrap();
    let edited = raw.iter().find(|m| m.id == id).unwrap();
    assert!(edited.edited_at.is_some());
    assert_eq!(edited.content, "");
    assert!(
        !serde_json::to_string(&edited.e2ee)
            .unwrap()
            .contains("hunter2")
    );

    // And a reply carries its line of the original, which the server cannot make.
    bob.load_history(&b, dm, hermes_client_core::store::HistoryAnchor::Latest)
        .await
        .unwrap();
    bob.send_message(
        &b,
        dm,
        Draft {
            content: "noted".into(),
            reply_to: Some(id),
            attachments: vec![],
        },
    )
    .unwrap();
    wait_for_window(&alice, &a, dm, "noted").await;
    let store = alice.store();
    let window = store.instance(&a).unwrap().window(dm).unwrap();
    let reply = window
        .messages
        .iter()
        .find(|m| m.content == "noted")
        .unwrap();
    assert_eq!(
        reply.reply_to.as_ref().unwrap().excerpt,
        "the password is hunter2"
    );
}

#[tokio::test]
async fn files_in_direct_messages_are_encrypted_with_their_names() {
    let server = start("e2ee-file").await;
    let (alice, _a_events, a, bob, _b_events, b, dm) = dm_pair(&server).await;
    let path = server.dir.join("holiday-plans.txt");
    std::fs::write(&path, b"secret file contents").unwrap();

    let attachment = alice.upload_to(&a, dm, &path, None).await.unwrap();
    assert_eq!(attachment.filename, "holiday-plans.txt");
    assert_eq!(attachment.size, 20);
    alice
        .send_message(
            &a,
            dm,
            Draft {
                content: "see attached".into(),
                reply_to: None,
                attachments: vec![attachment.clone()],
            },
        )
        .unwrap();
    wait_for_window(&bob, &b, dm, "see attached").await;

    // Bob gets the real name and the file.
    let received = {
        let store = bob.store();
        let window = store.instance(&b).unwrap().window(dm).unwrap();
        let message = window
            .messages
            .iter()
            .find(|m| m.content == "see attached")
            .unwrap();
        message.attachments[0].clone()
    };
    assert_eq!(received.filename, "holiday-plans.txt");
    assert_eq!(received.content_type, "text/plain");
    assert_eq!(
        bob.download(&b, &received).await.unwrap(),
        b"secret file contents"
    );

    // The server has neither.
    let raw = bob
        .rest(&b)
        .unwrap()
        .messages(dm, &Default::default())
        .await
        .unwrap();
    let stored = &raw.iter().find(|m| m.e2ee.is_some()).unwrap().attachments[0];
    assert_ne!(stored.filename, "holiday-plans.txt");
    assert_eq!(stored.content_type, "application/octet-stream");
    let bytes = bob
        .rest(&b)
        .unwrap()
        .download(stored.id, &stored.filename)
        .await
        .unwrap();
    assert!(!bytes.windows(6).any(|w| w == b"secret"));
}

#[tokio::test]
async fn a_new_device_reads_only_what_comes_after_and_is_announced() {
    let server = start("e2ee-device").await;
    let (alice, mut a_events, a, bob, _b_events, b, dm) = dm_pair(&server).await;
    alice
        .load_history(&a, dm, hermes_client_core::store::HistoryAnchor::Latest)
        .await
        .unwrap();
    bob.load_history(&b, dm, hermes_client_core::store::HistoryAnchor::Latest)
        .await
        .unwrap();
    // Alice has seen Bob's keys by writing to him once.
    alice
        .send_message(&a, dm, draft("before the new device"))
        .unwrap();
    wait_for_window(&bob, &b, dm, "before the new device").await;
    assert!(alice.new_devices(&a, dm).is_empty());

    let (laptop, mut l_events) = core();
    laptop
        .trust_fingerprint(&server.host, server.fingerprint)
        .unwrap();
    let l = laptop
        .login(&server.host, "bob", "correct horse")
        .await
        .unwrap();
    wait_status(&mut l_events, &l, ConnStatus::Connected).await;

    // Alice is told, and the server cannot quietly add a listener.
    let deadline = tokio::time::Instant::now() + WAIT;
    while alice.new_devices(&a, dm).is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no notice of the new device"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let number = alice.conversation_keys(&a, dm).await.unwrap().safety_number;
    assert_eq!(
        number,
        bob.conversation_keys(&b, dm).await.unwrap().safety_number,
        "both sides compute the same number"
    );
    alice.acknowledge_devices(&a, dm);
    assert!(alice.new_devices(&a, dm).is_empty());
    let _ = a_events.try_recv();

    // The new device cannot read the past...
    laptop
        .load_history(&l, dm, hermes_client_core::store::HistoryAnchor::Latest)
        .await
        .unwrap();
    {
        let store = laptop.store();
        let window = store.instance(&l).unwrap().window(dm).unwrap();
        assert!(
            window
                .messages
                .iter()
                .any(|m| m.lock == MessageLock::NotForThisDevice)
        );
        assert!(
            !window
                .messages
                .iter()
                .any(|m| m.content.contains("before the new device"))
        );
    }
    // ... but reads what comes after, and so does the old one.
    alice
        .send_message(&a, dm, draft("after the new device"))
        .unwrap();
    wait_for_window(&laptop, &l, dm, "after the new device").await;
    wait_for_window(&bob, &b, dm, "after the new device").await;
}

#[tokio::test]
async fn nothing_is_sent_in_clear_to_someone_without_keys() {
    let server = start("e2ee-no-keys").await;
    let (alice, mut a_events, a, bob, _b_events, b, dm) = dm_pair(&server).await;
    alice
        .load_history(&a, dm, hermes_client_core::store::HistoryAnchor::Latest)
        .await
        .unwrap();
    // Bob signs out: his device is revoked, and no other remains.
    let bob_id = bob.store().instance(&b).unwrap().me_id().unwrap();
    bob.rest(&b).unwrap().logout().await.unwrap();
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let found = alice
            .rest(&a)
            .unwrap()
            .query_devices(&[bob_id])
            .await
            .unwrap();
        if found[0].devices.iter().all(|d| d.revoked) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    alice.send_message(&a, dm, draft("to nobody")).unwrap();
    // It fails where the sender can see it, with the reason.
    eventually(&alice, "the message to fail", |s| {
        s.instance(&a).and_then(|s| s.window(dm)).is_some_and(|w| {
            w.pending.iter().any(
                |p| matches!(&p.state, PendingState::Failed(why) if why.contains("encryption")),
            )
        })
    })
    .await;
    let raw = alice
        .rest(&a)
        .unwrap()
        .messages(dm, &Default::default())
        .await
        .unwrap();
    assert!(raw.iter().all(|m| !m.content.contains("to nobody")));
    let _ = a_events.try_recv();
}
