//! Gateway lifecycle (PLAN.md section 11): hello, identify/ready, heartbeat, resume,
//! invalid_session, close codes, revocation and presences.

mod common;

use std::time::Duration;

use common::{TestServer, start, start_with};
use hermes_proto::permissions::Permissions;
use hermes_server::config::RegistrationMode;
use serde_json::{Value, json};

async fn send_message(server: &TestServer, token: &str, channel_id: &str, content: &str) -> Value {
    server
        .post(
            &format!("/channels/{channel_id}/messages"),
            Some(token),
            json!({ "content": content, "nonce": content }),
        )
        .await
        .ok()
}

#[tokio::test]
async fn identify_ready_and_heartbeat() {
    let server = start("gw-ready", RegistrationMode::InviteOnly).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let mut ws = server.connect().await;
    ws.send_identify(&owner).await;
    let ready = ws.recv_op("ready").await;
    assert_eq!(ready["s"], Value::Null, "ready is a control op");
    let ready = &ready["d"];
    assert_eq!(ready["session_id"].as_str().unwrap().len(), 32);
    assert_eq!(ready["user"]["id"], owner_id);
    assert_eq!(ready["user"]["instance_admin"], true);
    let spaces = ready["spaces"].as_array().unwrap();
    assert_eq!(spaces.len(), 1);
    assert_eq!(spaces[0]["space"]["id"], space_id);
    assert_eq!(spaces[0]["channels"].as_array().unwrap().len(), 2);
    assert_eq!(spaces[0]["members"][0]["user"]["id"], owner_id);
    assert_eq!(ready["dms"], json!([]));
    assert_eq!(
        ready["presences"],
        json!([{ "user_id": owner_id, "status": "online" }])
    );
    assert_eq!(ready["voice"]["enabled"], true);
    // The test server's voice endpoint listens on a random port.
    let quic_port = server.state.voice.endpoint().unwrap().port;
    assert_eq!(ready["voice"]["quic_port"], quic_port);
    assert!(ready["voice"]["cert_sha256"].is_string());
    assert_eq!(ready["instance"]["name"], "Test Instance");
    assert_eq!(ready["instance"]["limits"]["max_message_length"], 4000);

    ws.send(json!({ "op": "heartbeat", "d": { "last_seq": 0 } }))
        .await;
    let ack = ws.next_event().await;
    assert_eq!(ack, json!({ "op": "heartbeat_ack", "s": null, "d": {} }));
}

#[tokio::test]
async fn ready_hides_channels_without_view_channel() {
    let server = start("gw-ready-hidden", RegistrationMode::InviteOnly).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "general");
    server
        .put(
            &format!("/channels/{general}/overrides/{space_id}"),
            Some(&owner),
            json!({ "allow": "0", "deny": Permissions::VIEW_CHANNEL.bits().to_string() }),
        )
        .await
        .ok();
    let (_ws, ready) = server.identify(&alice).await;
    let channels = ready["spaces"][0]["channels"].as_array().unwrap();
    assert_eq!(channels.len(), 1);
    assert_ne!(channels[0]["id"], general);
}

#[tokio::test]
async fn close_codes() {
    let server = start("gw-close", RegistrationMode::InviteOnly).await;
    let (owner, _, _) = server.owner().await;

    // An op before identify.
    let mut ws = server.connect().await;
    ws.send(json!({ "op": "heartbeat", "d": { "last_seq": 0 } }))
        .await;
    assert_eq!(ws.closed().await, 4003);

    // A wrong token.
    let mut ws = server.connect().await;
    ws.send_identify("hms_nope").await;
    assert_eq!(ws.closed().await, 4004);

    // An unsupported protocol version.
    let mut ws = server.connect().await;
    ws.send(json!({ "op": "identify", "d": {
        "token": owner, "protocol_version": 99,
        "client": { "name": "t", "version": "0", "os": "t" },
    }}))
    .await;
    assert_eq!(ws.closed().await, 4010);

    // Identifying twice.
    let (mut ws, _) = server.identify(&owner).await;
    ws.send_identify(&owner).await;
    assert_eq!(ws.closed().await, 4005);

    // Unknown op and malformed frames.
    let mut ws = server.connect().await;
    ws.send(json!({ "op": "nope", "d": {} })).await;
    assert_eq!(ws.closed().await, 4001);
    let mut ws = server.connect().await;
    ws.send_raw("not json").await;
    assert_eq!(ws.closed().await, 4002);

    // A disabled account cannot connect.
    server.sql("UPDATE users SET disabled = 1").await;
    for user in server.state.cache_mut().users.values_mut() {
        user.disabled = true;
    }
    let mut ws = server.connect().await;
    ws.send_identify(&owner).await;
    assert_eq!(ws.closed().await, 4004);
}

#[tokio::test]
async fn dispatch_sequence_and_resume() {
    let server = start("gw-resume", RegistrationMode::InviteOnly).await;
    let (owner, _, space_id) = server.owner().await;
    let general = server.channel_id(&space_id, "general");
    let (mut ws, ready) = server.identify(&owner).await;
    let session_id = ready["session_id"].as_str().unwrap().to_owned();

    send_message(&server, &owner, &general, "one").await;
    let first = ws.recv_op("message_create").await;
    assert_eq!(first["d"]["content"], "one");
    assert_eq!(first["d"]["nonce"], "one", "the author gets the nonce");
    let last_seq = first["s"].as_u64().unwrap();
    assert!(last_seq >= 1);
    drop(ws);

    // Missed while disconnected.
    send_message(&server, &owner, &general, "two").await;
    send_message(&server, &owner, &general, "three").await;

    let mut ws = server.connect().await;
    ws.send_resume(&owner, &session_id, last_seq).await;
    // Everything after `last_seq`, in order, then `resumed`. Besides the messages, the replay
    // may hold the owner's own presence going offline.
    let mut replayed = Vec::new();
    let resumed = loop {
        let frame = ws.recv().await;
        if frame["op"] == "resumed" {
            break frame;
        }
        replayed.push(frame);
    };
    assert_eq!(resumed, json!({ "op": "resumed", "s": null, "d": {} }));
    let seqs: Vec<u64> = replayed.iter().map(|f| f["s"].as_u64().unwrap()).collect();
    let expected: Vec<u64> = (last_seq + 1..=last_seq + seqs.len() as u64).collect();
    assert_eq!(seqs, expected);
    let contents: Vec<&str> = replayed
        .iter()
        .filter(|f| f["op"] == "message_create")
        .map(|f| f["d"]["content"].as_str().unwrap())
        .collect();
    assert_eq!(contents, ["two", "three"]);

    // New events continue the sequence.
    send_message(&server, &owner, &general, "four").await;
    let four = ws.recv_op("message_create").await;
    assert!(four["s"].as_u64().unwrap() > *seqs.last().unwrap());
}

#[tokio::test]
async fn invalid_session_then_identify() {
    let server = start_with("gw-invalid", RegistrationMode::InviteOnly, |state| {
        state.gateway.timings.resume_buffer = 1;
    })
    .await;
    let (owner, _, space_id) = server.owner().await;
    let general = server.channel_id(&space_id, "general");

    // Unknown session: identify again on the same connection.
    let mut ws = server.connect().await;
    ws.send_resume(&owner, "unknown", 0).await;
    assert_eq!(
        ws.recv().await,
        json!({ "op": "invalid_session", "s": null, "d": { "resumable": false } })
    );
    ws.send_identify(&owner).await;
    let ready = ws.recv_op("ready").await;
    let session_id = ready["d"]["session_id"].as_str().unwrap().to_owned();
    drop(ws);

    // The buffer keeps one event; two were missed.
    send_message(&server, &owner, &general, "a").await;
    send_message(&server, &owner, &general, "b").await;
    let mut ws = server.connect().await;
    ws.send_resume(&owner, &session_id, 0).await;
    assert_eq!(ws.recv().await["op"], "invalid_session");

    // `last_seq` ahead of the session.
    let (ws2, ready) = server.identify(&owner).await;
    let session_id = ready["session_id"].as_str().unwrap().to_owned();
    drop(ws2);
    let mut ws = server.connect().await;
    ws.send_resume(&owner, &session_id, 1000).await;
    assert_eq!(ws.closed().await, 4007);

    // Another token cannot resume the session.
    let (other, _) = server
        .register("alice", Some(&new_invite(&server, &owner, &space_id).await))
        .await;
    let mut ws = server.connect().await;
    ws.send_resume(&other, &session_id, 0).await;
    assert_eq!(ws.recv().await["op"], "invalid_session");
}

async fn new_invite(server: &TestServer, token: &str, space_id: &str) -> String {
    server
        .post(
            &format!("/spaces/{space_id}/invites"),
            Some(token),
            json!({}),
        )
        .await
        .ok()["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn missing_heartbeats_close_with_4009() {
    let server = start_with("gw-heartbeat", RegistrationMode::InviteOnly, |state| {
        state.gateway.timings.heartbeat_interval = Duration::from_millis(150);
    })
    .await;
    let (owner, _, _) = server.owner().await;

    // Heartbeating keeps the connection open past two intervals.
    let (mut ws, _) = server.identify(&owner).await;
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        ws.send(json!({ "op": "heartbeat", "d": { "last_seq": 0 } }))
            .await;
        assert_eq!(ws.recv_op("heartbeat_ack").await["op"], "heartbeat_ack");
    }
    // Silence closes it.
    assert_eq!(ws.closed().await, 4009);

    // So does never identifying.
    let mut ws = server.connect().await;
    assert_eq!(ws.closed().await, 4009);
}

#[tokio::test]
async fn revoked_sessions_close_with_4011() {
    let server = start("gw-revoke", RegistrationMode::InviteOnly).await;
    let (owner, _, _) = server.owner().await;
    let laptop = server.login("owner", "correct horse").await.ok()["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let (mut pc, _) = server.identify(&owner).await;
    let (mut ws, ready) = server.identify(&laptop).await;
    let session_id = ready["session_id"].as_str().unwrap().to_owned();
    server
        .post("/auth/logout", Some(&laptop), json!({}))
        .await
        .ok();
    assert_eq!(ws.closed().await, 4011);
    // The token is gone, so resuming fails authentication.
    let mut ws = server.connect().await;
    ws.send_resume(&laptop, &session_id, 0).await;
    assert_eq!(ws.closed().await, 4004);
    // The other session is untouched.
    pc.send(json!({ "op": "heartbeat", "d": { "last_seq": 0 } }))
        .await;
    assert_eq!(pc.recv_op("heartbeat_ack").await["op"], "heartbeat_ack");

    // Changing the password revokes the other sessions.
    let phone = server.login("owner", "correct horse").await.ok()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let (mut ws, _) = server.identify(&phone).await;
    server
        .post(
            "/users/@me/password",
            Some(&owner),
            json!({ "current_password": "correct horse", "new_password": "battery staple" }),
        )
        .await
        .ok();
    assert_eq!(ws.closed().await, 4011);
}

/// The next `presence_update` about `user_id`.
async fn presence_of(ws: &mut common::Ws, user_id: &str) -> Value {
    loop {
        let frame = ws.recv_op("presence_update").await;
        if frame["d"]["user_id"] == user_id {
            return frame["d"].clone();
        }
    }
}

#[tokio::test]
async fn presences() {
    let server = start("gw-presence", RegistrationMode::InviteOnly).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (mut ows, _) = server.identify(&owner).await;

    let (mut aws, ready) = server.identify(&alice).await;
    assert_eq!(ready["presences"].as_array().unwrap().len(), 2);
    let online = presence_of(&mut ows, &alice_id).await;
    assert_eq!(online, json!({ "user_id": alice_id, "status": "online" }));

    aws.send(json!({ "op": "presence_update", "d": { "status": "invisible" } }))
        .await;
    assert_eq!(presence_of(&mut ows, &alice_id).await["status"], "offline");

    aws.send(json!({ "op": "presence_update", "d": { "status": "dnd" } }))
        .await;
    assert_eq!(presence_of(&mut ows, &alice_id).await["status"], "dnd");

    // A second session does not change anything; closing the last one goes offline.
    let (aws2, _) = server.identify(&alice).await;
    drop(aws);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        server
            .state
            .cache()
            .presence(alice_id.parse().unwrap())
            .status,
        hermes_proto::models::Status::Dnd
    );
    drop(aws2);
    let offline = presence_of(&mut ows, &alice_id).await;
    assert_eq!(offline, json!({ "user_id": alice_id, "status": "offline" }));
}
