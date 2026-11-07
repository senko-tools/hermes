//! Voice (PLAN.md sections 10, 12 and 19): `/voice/join` tokens, QUIC auth, the control stream,
//! forwarding rules, voice states, moderation, user limits, DM calls and ringing.

mod common;

use std::time::Duration;

use common::voice::VoiceClient;
use common::{start, start_with};
use hermes_proto::permissions::Permissions;
use hermes_server::config::RegistrationMode;
use serde_json::json;

fn bits(p: Permissions) -> String {
    p.bits().to_string()
}

#[tokio::test]
async fn join_checks_permissions_and_returns_the_endpoint() {
    let server = start("voice-join", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let (stranger, _) = server.register("stranger", None).await;
    let general = server.channel_id(&space_id, "General");
    let text = server.channel_id(&space_id, "general");
    let (_ws, ready) = server.identify(&owner).await;

    let join = server.voice_token(&owner, &general).await;
    assert!(join["token"].as_str().unwrap().starts_with("hms_"));
    assert_eq!(join["expires_in_secs"], 30);
    assert_eq!(join["quic"]["host"], "127.0.0.1");
    assert_eq!(
        join["quic"]["port"].as_u64().unwrap(),
        server.state.voice.endpoint().unwrap().port as u64
    );
    assert_eq!(join["quic"]["cert_sha256"], ready["voice"]["cert_sha256"]);
    assert_eq!(ready["voice"]["enabled"], true);
    assert_eq!(ready["voice"]["quic_port"], join["quic"]["port"]);
    let fp = join["quic"]["cert_sha256"].as_str().unwrap();
    assert_eq!(fp.len(), 32 * 3 - 1);

    server
        .post("/voice/join", Some(&owner), json!({ "channel_id": text }))
        .await
        .err(400, "BAD_REQUEST");
    server
        .post(
            "/voice/join",
            Some(&stranger),
            json!({ "channel_id": general }),
        )
        .await
        .err(404, "NOT_FOUND");

    // Without CONNECT.
    server
        .put(
            &format!("/channels/{general}/overrides/{space_id}"),
            Some(&owner),
            json!({ "allow": "0", "deny": bits(Permissions::CONNECT) }),
        )
        .await
        .ok();
    server
        .post(
            "/voice/join",
            Some(&alice),
            json!({ "channel_id": general }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
}

#[tokio::test]
async fn voice_can_be_disabled() {
    let server = start_with("voice-disabled", RegistrationMode::Open, |state| {
        state.config.voice.enabled = false;
    })
    .await;
    let (owner, _, space_id) = server.owner().await;
    let general = server.channel_id(&space_id, "General");
    server
        .post(
            "/voice/join",
            Some(&owner),
            json!({ "channel_id": general }),
        )
        .await
        .err(403, "VOICE_DISABLED");
    let (_ws, ready) = server.identify(&owner).await;
    assert_eq!(
        ready["voice"],
        json!({ "enabled": false, "quic_port": null, "cert_sha256": null })
    );
}

#[tokio::test]
async fn quic_auth_with_valid_reused_and_expired_tokens() {
    let server = start_with("voice-auth", RegistrationMode::Open, |state| {
        state.voice.set_token_lifetime(Duration::from_millis(300));
    })
    .await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "General");
    let (mut aws, _) = server.identify(&alice).await;

    let join = server.voice_token(&owner, &general).await;
    let mut client = VoiceClient::connect(&join).await;
    client
        .send(json!({ "op": "auth", "d": { "token": join["token"], "self_mute": true, "self_deaf": false } }))
        .await;
    let ready = client.recv_op("ready").await;
    assert_eq!(ready["d"]["participant_id"], 1);
    assert_eq!(ready["d"]["room_id"], general);
    assert_eq!(ready["d"]["bitrate"], 64000);
    assert_eq!(ready["d"]["participants"], json!([]));
    let update = aws.recv_op("voice_state_update").await["d"].clone();
    assert_eq!(update["user_id"], owner_id);
    assert_eq!(update["channel_id"], general);
    assert_eq!(update["self_mute"], true);
    // Voice states are part of `ready` too, for users who connect later.
    let (_ws, ready) = server.identify(&alice).await;
    assert_eq!(ready["spaces"][0]["voice_states"][0]["user_id"], owner_id);

    // Ping.
    client.send(json!({ "op": "ping", "d": { "t": 42 } })).await;
    assert_eq!(client.recv_op("pong").await["d"]["t"], 42);

    // A used token does not work again; the connection is closed after the error.
    let mut reused = VoiceClient::connect(&join).await;
    reused
        .send(json!({ "op": "auth", "d": { "token": join["token"], "self_mute": false, "self_deaf": false } }))
        .await;
    let error = reused.recv().await;
    assert_eq!(error["op"], "error");
    assert_eq!(error["d"]["code"], "UNAUTHORIZED");
    assert!(reused.try_recv().await.is_none());

    // Expired.
    let join = server.voice_token(&alice, &general).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let mut late = VoiceClient::connect(&join).await;
    late.send(json!({ "op": "auth", "d": { "token": join["token"], "self_mute": false, "self_deaf": false } }))
        .await;
    assert_eq!(late.recv().await["d"]["code"], "UNAUTHORIZED");

    // Anything before auth closes the connection.
    let join = server.voice_token(&alice, &general).await;
    let mut eager = VoiceClient::connect(&join).await;
    eager.send(json!({ "op": "ping", "d": { "t": 1 } })).await;
    assert_eq!(eager.recv().await["d"]["code"], "UNAUTHORIZED");
    assert!(eager.try_recv().await.is_none());
}

#[tokio::test]
async fn forwarding_rules() {
    let server = start("voice-forward", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, _) = server.member(&owner, &space_id, "bob").await;
    let general = server.channel_id(&space_id, "General");

    let (mut a, a_ready) = server.voice_join(&alice, &general).await;
    let a_pid = a_ready["participant_id"].as_u64().unwrap() as u16;
    let (mut b, b_ready) = server.voice_join(&bob, &general).await;
    assert_eq!(
        b_ready["participants"],
        json!([{ "participant_id": a_pid, "user_id": alice_id }])
    );
    let joined = a.recv_op("participant_joined").await;
    assert_eq!(joined["d"]["participant_id"], b_ready["participant_id"]);

    // The server overwrites the sender ID; nobody hears themselves.
    assert_eq!(a.deliver(&b, 0).await, (5, Some(a_pid)));
    assert_eq!(a.recv_audio(100).await, None);

    // Malformed packets are dropped.
    a.conn.send_datagram(vec![0x21; 20].into()).unwrap();
    a.conn.send_datagram(vec![0x11; 5].into()).unwrap();
    assert_eq!(b.recv_audio(300).await, None);

    // Self-muted senders are dropped.
    a.send(json!({ "op": "set_state", "d": { "self_mute": true, "self_deaf": false } }))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(a.deliver(&b, 10).await.0, 0);
    a.send(json!({ "op": "set_state", "d": { "self_mute": false, "self_deaf": false } }))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(a.deliver(&b, 20).await.0, 5);

    // Deafened listeners get nothing.
    b.send(json!({ "op": "set_state", "d": { "self_mute": false, "self_deaf": true } }))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(a.deliver(&b, 30).await.0, 0);
    b.send(json!({ "op": "set_state", "d": { "self_mute": false, "self_deaf": false } }))
        .await;

    // Server mute.
    server
        .patch(
            &format!("/spaces/{space_id}/voice-states/{alice_id}"),
            Some(&owner),
            json!({ "server_mute": true }),
        )
        .await
        .ok();
    let state = a.recv_op("server_state").await;
    assert_eq!(
        state["d"],
        json!({ "server_mute": true, "server_deaf": false })
    );
    assert_eq!(a.deliver(&b, 40).await.0, 0);
    server
        .patch(
            &format!("/spaces/{space_id}/voice-states/{alice_id}"),
            Some(&owner),
            json!({ "server_mute": false }),
        )
        .await
        .ok();
    a.recv_op("server_state").await;
    assert_eq!(a.deliver(&b, 50).await.0, 5);

    // Without SPEAK (re-checked at most once a second).
    server
        .put(
            &format!("/channels/{general}/overrides/{space_id}"),
            Some(&owner),
            json!({ "allow": "0", "deny": bits(Permissions::SPEAK) }),
        )
        .await
        .ok();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(a.deliver(&b, 60).await.0, 0);
}

#[tokio::test]
async fn switching_leaving_and_joining_elsewhere() {
    let server = start("voice-switch", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "General");
    let lounge = server
        .post(
            &format!("/spaces/{space_id}/channels"),
            Some(&owner),
            json!({ "kind": "voice", "name": "Lounge", "bitrate": 32000 }),
        )
        .await
        .ok()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (mut ows, _) = server.identify(&owner).await;
    let (mut o, _) = server.voice_join(&owner, &general).await;
    let (mut a, _) = server.voice_join(&alice, &general).await;
    o.recv_op("participant_joined").await;
    ows.recv_op("voice_state_update").await;
    ows.recv_op("voice_state_update").await;

    // Switch on the same connection.
    let join = server.voice_token(&alice, &lounge).await;
    a.send(json!({ "op": "switch", "d": { "token": join["token"] } }))
        .await;
    let ready = a.recv_op("ready").await;
    assert_eq!(ready["d"]["room_id"], lounge);
    assert_eq!(ready["d"]["bitrate"], 32000);
    o.recv_op("participant_left").await;
    let left = ows.recv_op("voice_state_update").await;
    assert_eq!(
        left["d"],
        json!({ "user_id": alice_id, "channel_id": null })
    );
    let joined = ows.recv_op("voice_state_update").await;
    assert_eq!(joined["d"]["channel_id"], lounge);
    assert_eq!(
        server.state.voice.channel_of(alice_id.parse().unwrap()),
        Some(lounge.parse().unwrap())
    );

    // Someone else's token cannot be used for a switch.
    let join = server.voice_token(&owner, &lounge).await;
    a.send(json!({ "op": "switch", "d": { "token": join["token"] } }))
        .await;
    assert_eq!(a.recv_op("error").await["d"]["code"], "UNAUTHORIZED");

    // Joining from another connection kicks the first one.
    let (mut a2, _) = server.voice_join(&alice, &general).await;
    let kicked = a.recv_op("kicked").await;
    assert_eq!(kicked["d"]["reason"], "joined_elsewhere");
    o.recv_op("participant_joined").await;

    // `leave` keeps the connection.
    a2.send(json!({ "op": "leave", "d": {} })).await;
    o.recv_op("participant_left").await;
    assert_eq!(
        server.state.voice.channel_of(alice_id.parse().unwrap()),
        None
    );
    a2.send(json!({ "op": "ping", "d": { "t": 7 } })).await;
    assert_eq!(a2.recv_op("pong").await["d"]["t"], 7);

    // Closing the connection leaves voice too.
    drop(o);
    let left = loop {
        let frame = ows.recv_op("voice_state_update").await;
        if frame["d"]["channel_id"].is_null() && frame["d"]["user_id"] != alice_id {
            break frame;
        }
    };
    assert!(left["d"]["channel_id"].is_null());

    // `POST /voice/leave` and `PATCH /voice/@me`.
    let (mut a3, _) = server.voice_join(&alice, &general).await;
    server
        .patch("/voice/@me", Some(&alice), json!({ "self_deaf": true }))
        .await
        .ok();
    let state = loop {
        let frame = ows.recv_op("voice_state_update").await;
        if frame["d"]["self_deaf"] == true {
            break frame;
        }
    };
    assert_eq!(state["d"]["user_id"], alice_id);
    server
        .post("/voice/leave", Some(&alice), json!({}))
        .await
        .ok();
    assert_eq!(a3.recv_op("kicked").await["d"]["reason"], "disconnected");
}

#[tokio::test]
async fn moderators_mute_move_and_disconnect() {
    let server = start("voice-moderation", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space_id, "bob").await;
    let general = server.channel_id(&space_id, "General");
    let lounge = server
        .post(
            &format!("/spaces/{space_id}/channels"),
            Some(&owner),
            json!({ "kind": "voice", "name": "Lounge" }),
        )
        .await
        .ok()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (mut a, _) = server.voice_join(&alice, &general).await;
    let path = format!("/spaces/{space_id}/voice-states/{alice_id}");

    // Members without the bits cannot.
    for body in [
        json!({ "server_mute": true }),
        json!({ "server_deaf": true }),
        json!({ "channel_id": lounge }),
        json!({ "channel_id": null }),
    ] {
        server
            .patch(&path, Some(&bob), body)
            .await
            .err(403, "MISSING_PERMISSION");
    }
    // Users not in voice have no voice state.
    server
        .patch(
            &format!("/spaces/{space_id}/voice-states/{bob_id}"),
            Some(&owner),
            json!({ "server_mute": true }),
        )
        .await
        .err(404, "NOT_FOUND");

    server
        .patch(&path, Some(&owner), json!({ "server_deaf": true }))
        .await
        .ok();
    let state = a.recv_op("server_state").await;
    assert_eq!(state["d"]["server_deaf"], true);

    // Move: the client gets a token and switches with it.
    server
        .patch(&path, Some(&owner), json!({ "channel_id": lounge }))
        .await
        .ok();
    let moved = a.recv_op("moved").await;
    a.send(json!({ "op": "switch", "d": { "token": moved["d"]["token"] } }))
        .await;
    assert_eq!(a.recv_op("ready").await["d"]["room_id"], lounge);

    // Disconnect.
    server
        .patch(&path, Some(&owner), json!({ "channel_id": null }))
        .await
        .ok();
    assert_eq!(a.recv_op("kicked").await["d"]["reason"], "disconnected");
    assert!(server.state.cache().voice_states.is_empty());

    let actions = server.audit_actions(&owner, &space_id).await;
    assert_eq!(
        &actions[..3],
        ["member_disconnect", "member_move", "member_voice_update"]
    );
}

#[tokio::test]
async fn moderators_cannot_act_on_higher_members() {
    let server = start("voice-hierarchy", RegistrationMode::Open).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space_id, "bob").await;
    let general = server.channel_id(&space_id, "General");
    let moderator = server
        .role(
            &owner,
            &space_id,
            "moderator",
            (Permissions::MUTE_MEMBERS | Permissions::DEAFEN_MEMBERS | Permissions::MOVE_MEMBERS)
                .bits(),
        )
        .await;
    server
        .assign(&owner, &space_id, &bob_id, &[&moderator])
        .await;
    let (_o, _) = server.voice_join(&owner, &general).await;
    let (_a, _) = server.voice_join(&alice, &general).await;
    let alice_path = format!("/spaces/{space_id}/voice-states/{alice_id}");

    // Below him: fine.
    server
        .patch(&alice_path, Some(&bob), json!({ "server_mute": true }))
        .await
        .ok();
    // The owner is out of reach.
    for body in [
        json!({ "server_mute": true }),
        json!({ "server_deaf": true }),
        json!({ "channel_id": null }),
    ] {
        server
            .patch(
                &format!("/spaces/{space_id}/voice-states/{owner_id}"),
                Some(&bob),
                body,
            )
            .await
            .err(403, "MISSING_PERMISSION");
    }
    // So is someone with the same highest role.
    server
        .assign(&owner, &space_id, &alice_id, &[&moderator])
        .await;
    server
        .patch(&alice_path, Some(&bob), json!({ "server_mute": false }))
        .await
        .err(403, "MISSING_PERMISSION");
    // Her own state is hers to change (she has the bit).
    server
        .patch(&alice_path, Some(&alice), json!({ "server_mute": false }))
        .await
        .ok();
}

#[tokio::test]
async fn user_limits() {
    let server = start("voice-limits", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let (bob, _) = server.member(&owner, &space_id, "bob").await;
    let small = server
        .post(
            &format!("/spaces/{space_id}/channels"),
            Some(&owner),
            json!({ "kind": "voice", "name": "Small", "user_limit": 1 }),
        )
        .await
        .ok()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (_a, _) = server.voice_join(&alice, &small).await;
    server
        .post("/voice/join", Some(&bob), json!({ "channel_id": small }))
        .await
        .err(409, "VOICE_FULL");
    // Rejoining your own room is not blocked by your own seat.
    server.voice_token(&alice, &small).await;
    // MOVE_MEMBERS bypasses the limit.
    let (_o, ready) = server.voice_join(&owner, &small).await;
    assert_eq!(ready["participants"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn channel_changes_reach_the_room() {
    let server = start("voice-channel-changes", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let (bob, _) = server.member(&owner, &space_id, "bob").await;
    let general = server.channel_id(&space_id, "General");
    let (mut a, _) = server.voice_join(&alice, &general).await;
    let (mut b, _) = server.voice_join(&bob, &general).await;

    server
        .patch(
            &format!("/channels/{general}"),
            Some(&owner),
            json!({ "bitrate": 96000 }),
        )
        .await
        .ok();
    assert_eq!(a.recv_op("bitrate_changed").await["d"]["bitrate"], 96000);

    // Kicked from the space: removed from the room.
    server
        .delete(
            &format!("/spaces/{space_id}/members/{}", server_user(&server, "bob")),
            Some(&owner),
        )
        .await
        .ok();
    assert_eq!(b.recv_op("kicked").await["d"]["reason"], "removed");
    a.recv_op("participant_left").await;

    // Channel deleted.
    server
        .delete(&format!("/channels/{general}"), Some(&owner))
        .await
        .ok();
    assert_eq!(a.recv_op("kicked").await["d"]["reason"], "channel_deleted");
    assert!(server.state.cache().voice_states.is_empty());
}

fn server_user(server: &common::TestServer, username: &str) -> String {
    server
        .state
        .cache()
        .users
        .values()
        .find(|u| u.user.username == username)
        .unwrap()
        .user
        .id
        .to_string()
}

#[tokio::test]
async fn dm_calls_ring_and_post_messages() {
    let server = start("voice-dm", RegistrationMode::Open).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, _) = server.member(&owner, &space_id, "bob").await;
    let dm = server
        .post("/dms", Some(&owner), json!({ "recipient_ids": [alice_id] }))
        .await
        .ok();
    let dm_id = dm["id"].as_str().unwrap().to_owned();
    let (mut ows, _) = server.identify(&owner).await;
    let (mut aws, _) = server.identify(&alice).await;
    let (mut bws, _) = server.identify(&bob).await;

    // Only DMs ring, and only recipients may.
    let general = server.channel_id(&space_id, "General");
    server
        .post(
            &format!("/channels/{general}/call/ring"),
            Some(&owner),
            json!({}),
        )
        .await
        .err(400, "BAD_REQUEST");
    server
        .post(
            &format!("/channels/{dm_id}/call/ring"),
            Some(&bob),
            json!({}),
        )
        .await
        .err(404, "NOT_FOUND");

    let (mut o, _) = server.voice_join(&owner, &dm_id).await;
    let started = ows.recv_op("message_create").await["d"].clone();
    assert_eq!(started["kind"], "system");
    assert_eq!(started["content"], "owner started a call");
    assert_eq!(started["author"]["id"], owner_id);

    server
        .post(
            &format!("/channels/{dm_id}/call/ring"),
            Some(&owner),
            json!({}),
        )
        .await
        .ok();
    let ring = aws.recv_op("call_ring").await;
    assert_eq!(
        ring["d"],
        json!({ "channel_id": dm_id, "caller_id": owner_id })
    );
    ows.assert_silent(200).await;
    bws.assert_silent(200).await;

    // Answering stops the ringing on all of alice's sessions.
    let (_a, ready) = server.voice_join(&alice, &dm_id).await;
    assert_eq!(ready["participants"][0]["user_id"], owner_id);
    let stop = aws.recv_op("call_ring_stop").await;
    assert_eq!(stop["d"]["channel_id"], dm_id);
    o.recv_op("participant_joined").await;

    // DM voice states are in `ready`.
    let (_ws, ready) = server.identify(&alice).await;
    assert_eq!(ready["dm_voice_states"].as_array().unwrap().len(), 2);

    // The call ends when the room empties.
    drop(o);
    drop(_a);
    let ended = ows.recv_op("message_create").await["d"].clone();
    assert_eq!(ended["kind"], "system");
    assert_eq!(ended["content"], "Call ended · 1 min");

    // A caller who hangs up stops the ringing.
    let (o, _) = server.voice_join(&owner, &dm_id).await;
    server
        .post(
            &format!("/channels/{dm_id}/call/ring"),
            Some(&owner),
            json!({ "user_ids": [alice_id] }),
        )
        .await
        .ok();
    aws.recv_op("call_ring").await;
    drop(o);
    aws.recv_op("call_ring_stop").await;
}

#[tokio::test]
async fn screen_shares_reach_only_watchers_in_the_room() {
    let server = start("voice-share", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let (bob, _) = server.member(&owner, &space_id, "bob").await;
    let (carol, _) = server.member(&owner, &space_id, "carol").await;
    let general = server.channel_id(&space_id, "General");
    let (mut a, a_ready) = server.voice_join(&alice, &general).await;
    let (mut b, _) = server.voice_join(&bob, &general).await;
    let (c, _) = server.voice_join(&carol, &general).await;
    let alice_pid = a_ready["participant_id"].as_u64().unwrap();

    // Frames before `share_start` go nowhere.
    b.send(json!({ "op": "watch", "d": { "participant_id": alice_pid, "watching": true } }))
        .await;
    a.send_video(true, 0, b"early").await;
    assert!(b.recv_video(300).await.is_none());

    a.send(json!({ "op": "share_start", "d": {} })).await;
    let started = b.recv_op("share_started").await;
    assert_eq!(started["d"]["participant_id"], alice_pid);
    // Bob watches (again, now that it runs): Alice is asked for a keyframe.
    b.send(json!({ "op": "watch", "d": { "participant_id": alice_pid, "watching": true } }))
        .await;
    a.recv_op("keyframe_requested").await;
    a.send_video(true, 10, b"frame one").await;
    let (header, payload) = b.recv_video(2000).await.expect("bob watches");
    assert_eq!(payload, b"frame one");
    assert_eq!(
        header.sender_id as u64, alice_pid,
        "the server sets the sender"
    );
    assert!(header.keyframe);
    // Carol does not watch: nothing for her.
    assert!(c.recv_video(300).await.is_none());

    // Someone joining later learns about the share in `ready`.
    let (dave, _) = server.member(&owner, &space_id, "dave").await;
    let (_d, d_ready) = server.voice_join(&dave, &general).await;
    assert_eq!(d_ready["sharing"], json!([alice_pid]));

    // Frames too large are dropped.
    a.send_video(false, 20, &vec![0u8; voice_max_frame()]).await;
    assert!(b.recv_video(500).await.is_none());

    // Without STREAM, sharing is refused.
    server
        .patch(
            &format!("/spaces/{space_id}/roles/{space_id}"),
            Some(&owner),
            json!({ "permissions": bits(Permissions::DEFAULT_EVERYONE - Permissions::STREAM) }),
        )
        .await
        .ok();
    b.send(json!({ "op": "share_start", "d": {} })).await;
    let refused = b.recv_op("error").await;
    assert_eq!(refused["d"]["code"], "MISSING_PERMISSION");
    // A running share stops forwarding once the permission is gone (rechecked every second).
    tokio::time::sleep(Duration::from_millis(1100)).await;
    a.send_video(true, 30, b"after").await;
    assert!(b.recv_video(500).await.is_none());

    // Stopping tells the room.
    a.send(json!({ "op": "share_stop", "d": {} })).await;
    let stopped = b.recv_op("share_stopped").await;
    assert_eq!(stopped["d"]["participant_id"], alice_pid);
}

#[tokio::test]
async fn share_audio_reaches_only_watchers() {
    use hermes_proto::voice::AudioKind;

    let server = start("voice-share-audio", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let (bob, _) = server.member(&owner, &space_id, "bob").await;
    let (carol, _) = server.member(&owner, &space_id, "carol").await;
    let general = server.channel_id(&space_id, "General");
    let (mut a, a_ready) = server.voice_join(&alice, &general).await;
    let (mut b, b_ready) = server.voice_join(&bob, &general).await;
    let (c, _) = server.voice_join(&carol, &general).await;
    let alice_pid = a_ready["participant_id"].as_u64().unwrap() as u16;
    assert_eq!(
        b_ready["share_audio"], true,
        "the server forwards share audio"
    );
    let watch = json!({ "op": "watch", "d": { "participant_id": alice_pid, "watching": true } });

    // Not sharing: dropped, even for someone who asked to watch.
    b.send(watch.clone()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    a.send_share_audio(1);
    assert!(b.recv_kinds(300).await.is_empty());

    a.send(json!({ "op": "share_start", "d": {} })).await;
    b.recv_op("share_started").await;
    b.send(watch).await;
    a.recv_op("keyframe_requested").await;
    for seq in 2..5 {
        a.send_share_audio(seq);
    }
    let got = b.recv_kinds(300).await;
    assert_eq!(got, vec![(AudioKind::Share, alice_pid); 3], "Bob watches");
    assert!(c.recv_kinds(300).await.is_empty(), "Carol does not watch");

    // A muted microphone does not mute the share's sound.
    a.send(json!({ "op": "set_state", "d": { "self_mute": true, "self_deaf": false } }))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    a.send_share_audio(5);
    assert_eq!(b.recv_kinds(300).await.len(), 1);

    // A deafened watcher hears nothing.
    b.send(json!({ "op": "set_state", "d": { "self_mute": false, "self_deaf": true } }))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    a.send_share_audio(6);
    assert!(b.recv_kinds(300).await.is_empty());
    b.send(json!({ "op": "set_state", "d": { "self_mute": false, "self_deaf": false } }))
        .await;

    // Over the rate, the rest are dropped.
    tokio::time::sleep(Duration::from_millis(300)).await;
    for seq in 100..140 {
        a.send_share_audio(seq);
    }
    let got = b.recv_kinds(300).await.len();
    assert!(
        (10..=14).contains(&got),
        "{got} of 40 sent at once got through"
    );

    // After the share stops, nothing.
    a.send(json!({ "op": "share_stop", "d": {} })).await;
    b.recv_op("share_stopped").await;
    a.send_share_audio(200);
    assert!(b.recv_kinds(300).await.is_empty());
}

#[tokio::test]
async fn sharers_learn_how_many_watch() {
    let server = start("voice-share-watchers", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let (bob, _) = server.member(&owner, &space_id, "bob").await;
    let (carol, _) = server.member(&owner, &space_id, "carol").await;
    let general = server.channel_id(&space_id, "General");
    let (mut a, a_ready) = server.voice_join(&alice, &general).await;
    let (mut b, _) = server.voice_join(&bob, &general).await;
    let (mut c, _) = server.voice_join(&carol, &general).await;
    let alice_pid = a_ready["participant_id"].as_u64().unwrap();
    let watch = |watching: bool| json!({ "op": "watch", "d": { "participant_id": alice_pid, "watching": watching } });
    let count = |msg: serde_json::Value| msg["d"]["count"].as_u64().unwrap();

    // Starting tells the sharer that nobody watches yet.
    a.send(json!({ "op": "share_start", "d": {} })).await;
    assert_eq!(count(a.recv_op("share_watchers").await), 0);
    b.recv_op("share_started").await;
    c.recv_op("share_started").await;

    b.send(watch(true)).await;
    assert_eq!(count(a.recv_op("share_watchers").await), 1);
    c.send(watch(true)).await;
    assert_eq!(count(a.recv_op("share_watchers").await), 2);
    // Watching twice changes nothing; stopping does.
    c.send(watch(true)).await;
    c.send(watch(false)).await;
    assert_eq!(count(a.recv_op("share_watchers").await), 1);

    // A new watcher gets nothing before the sharer's next keyframe (it could not decode it).
    a.send_video(false, 10, b"delta").await;
    assert!(b.recv_video(300).await.is_none());
    a.send_video(true, 20, b"key").await;
    let (header, payload) = b.recv_video(2000).await.expect("the keyframe");
    assert!(header.keyframe);
    assert_eq!(payload, b"key");
    a.send_video(false, 30, b"delta after").await;
    let (_, payload) = b.recv_video(2000).await.expect("deltas after it");
    assert_eq!(payload, b"delta after");

    // A watcher leaving the room counts too.
    b.send(json!({ "op": "leave", "d": {} })).await;
    assert_eq!(count(a.recv_op("share_watchers").await), 0);
}

fn voice_max_frame() -> usize {
    hermes_proto::voice::MAX_VIDEO_FRAME
}
