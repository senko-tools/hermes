//! Dispatch events and their audiences (PLAN.md sections 9.4 and 11.5): every change made through
//! REST reaches exactly the sessions that can see it.

mod common;

use common::{TestServer, Ws, start};
use hermes_proto::permissions::Permissions;
use hermes_server::config::RegistrationMode;
use serde_json::{Value, json};

fn bits(p: Permissions) -> String {
    p.bits().to_string()
}

async fn send_message(server: &TestServer, token: &str, channel_id: &str, content: &str) -> Value {
    server
        .post(
            &format!("/channels/{channel_id}/messages"),
            Some(token),
            json!({ "content": content, "nonce": format!("nonce-{content}") }),
        )
        .await
        .ok()
}

/// Sends a message in `channel_id` and checks that it is the next event `ws` receives, proving
/// that nothing else was delivered before it.
async fn expect_marker(server: &TestServer, token: &str, channel_id: &str, ws: &mut Ws) {
    send_message(server, token, channel_id, "marker").await;
    let event = ws.next_event().await;
    assert_eq!(
        (event["op"].as_str(), event["d"]["content"].as_str()),
        (Some("message_create"), Some("marker")),
        "{event}"
    );
}

#[tokio::test]
async fn message_events_respect_view_channel() {
    let server = start("ev-messages", RegistrationMode::InviteOnly).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "general");
    let secret = server
        .post(
            &format!("/spaces/{space_id}/channels"),
            Some(&owner),
            json!({ "kind": "text", "name": "secret" }),
        )
        .await
        .ok()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    server
        .put(
            &format!("/channels/{secret}/overrides/{space_id}"),
            Some(&owner),
            json!({ "allow": "0", "deny": bits(Permissions::VIEW_CHANNEL) }),
        )
        .await
        .ok();
    let (mut ows, _) = server.identify(&owner).await;
    let (mut aws, _) = server.identify(&alice).await;

    // Alice cannot see #secret, so she gets nothing from it.
    send_message(&server, &owner, &secret, "psst").await;
    let own = ows.next_event().await;
    assert_eq!(own["op"], "message_create");
    assert_eq!(own["d"]["nonce"], "nonce-psst");
    server
        .post(
            &format!("/channels/{secret}/typing"),
            Some(&owner),
            json!({}),
        )
        .await
        .ok();
    expect_marker(&server, &owner, &general, &mut aws).await;

    // In #general the author gets the nonce, everyone else does not.
    let own = ows.next_event().await;
    assert_eq!(own["d"]["nonce"], "nonce-marker");
    send_message(&server, &alice, &general, "hi").await;
    let theirs = ows.next_event().await;
    assert_eq!(theirs["d"]["content"], "hi");
    assert!(theirs["d"].get("nonce").is_none(), "{theirs}");
    assert_eq!(aws.next_event().await["d"]["nonce"], "nonce-hi");

    // Typing goes to the others only.
    server
        .post(
            &format!("/channels/{general}/typing"),
            Some(&alice),
            json!({}),
        )
        .await
        .ok();
    let typing = ows.next_event().await;
    assert_eq!(typing["op"], "typing_start");
    assert_eq!(typing["d"]["user_id"], alice_id);
    assert_eq!(typing["d"]["channel_id"], general);
    expect_marker(&server, &owner, &general, &mut aws).await;
}

#[tokio::test]
async fn message_changes_reactions_pins_and_acks() {
    let server = start("ev-message-changes", RegistrationMode::InviteOnly).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "general");
    let (mut ows, _) = server.identify(&owner).await;
    let (mut aws, _) = server.identify(&alice).await;
    let message = send_message(&server, &owner, &general, "hello").await;
    let mid = message["id"].as_str().unwrap().to_owned();
    assert_eq!(aws.next_event().await["op"], "message_create");
    assert_eq!(ows.next_event().await["op"], "message_create");
    let path = format!("/channels/{general}/messages/{mid}");

    server
        .put(&format!("{path}/reactions/🎃"), Some(&owner), json!({}))
        .await
        .ok();
    let reaction = aws.next_event().await;
    assert_eq!(reaction["op"], "reaction_add");
    assert_eq!(
        reaction["d"],
        json!({ "message_id": mid, "channel_id": general, "emoji": "🎃", "user_id": owner_id })
    );
    // Reacting again changes nothing and sends nothing.
    server
        .put(&format!("{path}/reactions/🎃"), Some(&owner), json!({}))
        .await
        .ok();
    server
        .delete(&format!("{path}/reactions/🎃"), Some(&owner))
        .await
        .ok();
    assert_eq!(aws.next_event().await["op"], "reaction_remove");

    server
        .patch(&path, Some(&owner), json!({ "content": "edited" }))
        .await
        .ok();
    let update = aws.next_event().await;
    assert_eq!(update["op"], "message_update");
    assert_eq!(update["d"]["content"], "edited");

    server
        .put(&format!("{path}/pin"), Some(&owner), json!({}))
        .await
        .ok();
    let pinned = aws.next_event().await;
    assert_eq!(
        (pinned["op"].as_str(), pinned["d"]["pinned"].as_bool()),
        (Some("message_update"), Some(true))
    );

    server.delete(&path, Some(&owner)).await.ok();
    let deleted = aws.next_event().await;
    assert_eq!(deleted["op"], "message_delete");
    assert_eq!(deleted["d"], json!({ "id": mid, "channel_id": general }));

    // Read states go to the user's own sessions only.
    server
        .post(
            &format!("/channels/{general}/ack"),
            Some(&alice),
            json!({ "message_id": mid }),
        )
        .await
        .ok();
    let read = aws.next_event().await;
    assert_eq!(read["op"], "read_state_update");
    assert_eq!(
        read["d"],
        json!({ "channel_id": general, "last_read_id": mid, "mention_count": 0 })
    );
    // The owner saw the reaction and edit events, but not Alice's read state.
    for op in [
        "reaction_add",
        "reaction_remove",
        "message_update",
        "message_update",
        "message_delete",
    ] {
        assert_eq!(ows.next_event().await["op"], op);
    }
    expect_marker(&server, &alice, &general, &mut ows).await;
}

#[tokio::test]
async fn permission_changes_show_and_hide_channels() {
    let server = start("ev-visibility", RegistrationMode::InviteOnly).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "general");
    let voice = server.channel_id(&space_id, "General");
    let (mut ows, _) = server.identify(&owner).await;
    let (mut aws, _) = server.identify(&alice).await;

    // Hiding #general from @everyone: Alice loses it, the owner sees the new override.
    server
        .put(
            &format!("/channels/{general}/overrides/{space_id}"),
            Some(&owner),
            json!({ "allow": "0", "deny": bits(Permissions::VIEW_CHANNEL) }),
        )
        .await
        .ok();
    let gone = aws.next_event().await;
    assert_eq!(gone["op"], "channel_delete");
    assert_eq!(gone["d"], json!({ "id": general, "space_id": space_id }));
    let update = ows.next_event().await;
    assert_eq!(update["op"], "channel_update");
    assert_eq!(update["d"]["overrides"].as_array().unwrap().len(), 1);

    // A role that may view it brings it back.
    let role = server
        .role(&owner, &space_id, "viewers", Permissions::empty().bits())
        .await;
    assert_eq!(aws.next_event().await["op"], "role_create");
    server
        .put(
            &format!("/channels/{general}/overrides/{role}"),
            Some(&owner),
            json!({ "allow": bits(Permissions::VIEW_CHANNEL), "deny": "0" }),
        )
        .await
        .ok();
    server.assign(&owner, &space_id, &alice_id, &[&role]).await;
    let member = aws.next_event().await;
    assert_eq!(member["op"], "member_update");
    assert_eq!(member["d"]["role_ids"], json!([role]));
    let back = aws.next_event().await;
    assert_eq!(back["op"], "channel_create");
    assert_eq!(back["d"]["id"], general);

    // Deleting the role hides it again.
    server
        .delete(&format!("/spaces/{space_id}/roles/{role}"), Some(&owner))
        .await
        .ok();
    assert_eq!(aws.next_event().await["op"], "role_delete");
    assert_eq!(aws.next_event().await["op"], "channel_delete");

    // Removing the @everyone override shows it once more.
    server
        .delete(
            &format!("/channels/{general}/overrides/{space_id}"),
            Some(&owner),
        )
        .await
        .ok();
    assert_eq!(aws.next_event().await["op"], "channel_create");
    expect_marker(&server, &owner, &voice, &mut aws).await;
}

#[tokio::test]
async fn space_channel_and_role_events() {
    let server = start("ev-space", RegistrationMode::InviteOnly).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "general");
    let (mut ows, _) = server.identify(&owner).await;
    let (mut aws, _) = server.identify(&alice).await;

    server
        .patch(
            &format!("/spaces/{space_id}"),
            Some(&owner),
            json!({ "name": "Renamed" }),
        )
        .await
        .ok();
    let update = aws.next_event().await;
    assert_eq!(update["op"], "space_update");
    assert_eq!(update["d"]["name"], "Renamed");

    let channel = server
        .post(
            &format!("/spaces/{space_id}/channels"),
            Some(&owner),
            json!({ "kind": "text", "name": "memes" }),
        )
        .await
        .ok();
    let memes = channel["id"].as_str().unwrap().to_owned();
    assert_eq!(aws.next_event().await["op"], "channel_create");
    server
        .patch(
            &format!("/channels/{memes}"),
            Some(&owner),
            json!({ "topic": "cats" }),
        )
        .await
        .ok();
    let update = aws.next_event().await;
    assert_eq!(
        (update["op"].as_str(), update["d"]["topic"].as_str()),
        (Some("channel_update"), Some("cats"))
    );
    server
        .put(
            &format!("/spaces/{space_id}/channels/order"),
            Some(&owner),
            json!([{ "id": memes, "position": 0 }]),
        )
        .await
        .ok();
    assert_eq!(aws.next_event().await["op"], "channel_update");
    server
        .delete(&format!("/channels/{memes}"), Some(&owner))
        .await
        .ok();
    let gone = aws.next_event().await;
    assert_eq!(gone["op"], "channel_delete");
    assert_eq!(gone["d"]["id"], memes);

    let role = server.role(&owner, &space_id, "mods", 0).await;
    assert_eq!(aws.next_event().await["d"]["id"], role);
    server
        .patch(
            &format!("/spaces/{space_id}/roles/{role}"),
            Some(&owner),
            json!({ "name": "moderators" }),
        )
        .await
        .ok();
    let update = aws.next_event().await;
    assert_eq!(
        (update["op"].as_str(), update["d"]["name"].as_str()),
        (Some("role_update"), Some("moderators"))
    );

    // Nickname changes reach everyone.
    server
        .patch(
            &format!("/spaces/{space_id}/members/@me"),
            Some(&alice),
            json!({ "nickname": "ali" }),
        )
        .await
        .ok();
    let member = ows.recv_op("member_update").await;
    assert_eq!(member["d"]["user"]["id"], alice_id);
    assert_eq!(member["d"]["nickname"], "ali");
    assert_eq!(aws.next_event().await["op"], "member_update");
    expect_marker(&server, &owner, &general, &mut aws).await;
}

#[tokio::test]
async fn joining_leaving_and_deleting_spaces() {
    let server = start("ev-membership", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (bob, bob_id) = server.register("bob", None).await;
    let general = server.channel_id(&space_id, "general");
    let (mut ows, _) = server.identify(&owner).await;
    let (mut bws, ready) = server.identify(&bob).await;
    assert_eq!(ready["spaces"], json!([]));

    // Joining: Bob gets the space, members get Bob.
    let invite = server
        .post(
            &format!("/spaces/{space_id}/invites"),
            Some(&owner),
            json!({}),
        )
        .await
        .ok();
    let code = invite["code"].as_str().unwrap();
    server
        .post(&format!("/invites/{code}"), Some(&bob), json!({}))
        .await
        .ok();
    let created = bws.next_event().await;
    assert_eq!(created["op"], "space_create");
    assert_eq!(created["d"]["space"]["id"], space_id);
    assert_eq!(created["d"]["channels"].as_array().unwrap().len(), 2);
    let added = ows.next_event().await;
    assert_eq!(added["op"], "member_add");
    assert_eq!(added["d"]["user"]["id"], bob_id);

    // A ban that deletes messages: one bulk delete per channel, and Bob loses the space.
    let m1 = send_message(&server, &bob, &general, "spam1").await;
    let m2 = send_message(&server, &bob, &general, "spam2").await;
    assert_eq!(ows.next_event().await["d"]["content"], "spam1");
    assert_eq!(ows.next_event().await["d"]["content"], "spam2");
    server
        .put(
            &format!("/spaces/{space_id}/bans/{bob_id}"),
            Some(&owner),
            json!({ "delete_messages_hours": 1 }),
        )
        .await
        .ok();
    assert_eq!(
        bws.recv_op("space_delete").await["d"],
        json!({ "id": space_id })
    );
    let removed = ows.next_event().await;
    assert_eq!(removed["op"], "member_remove");
    assert_eq!(
        removed["d"],
        json!({ "space_id": space_id, "user_id": bob_id })
    );
    let bulk = ows.next_event().await;
    assert_eq!(bulk["op"], "message_delete_bulk");
    assert_eq!(bulk["d"]["channel_id"], general);
    assert_eq!(bulk["d"]["ids"], json!([m1["id"], m2["id"]]));
    // Bob no longer receives the space's events.
    send_message(&server, &owner, &general, "after").await;
    bws.assert_silent(300).await;

    // Creating and deleting a space.
    let space = server
        .post("/spaces", Some(&bob), json!({ "name": "Bob's" }))
        .await
        .ok();
    let created = bws.next_event().await;
    assert_eq!(created["op"], "space_create");
    assert_eq!(created["d"]["space"]["id"], space["space"]["id"]);
    let bob_space = space["space"]["id"].as_str().unwrap();
    server
        .delete(&format!("/spaces/{bob_space}"), Some(&bob))
        .await
        .ok();
    let deleted = bws.next_event().await;
    assert_eq!(deleted["op"], "space_delete");
    assert_eq!(deleted["d"], json!({ "id": bob_space }));
    assert!(deleted["s"].as_u64() > created["s"].as_u64());
}

#[tokio::test]
async fn kicks_and_leaving() {
    let server = start("ev-kick", RegistrationMode::InviteOnly).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space_id, "bob").await;
    let (mut ows, _) = server.identify(&owner).await;
    let (mut aws, _) = server.identify(&alice).await;
    let (mut bws, _) = server.identify(&bob).await;

    server
        .delete(
            &format!("/spaces/{space_id}/members/{alice_id}"),
            Some(&owner),
        )
        .await
        .ok();
    assert_eq!(aws.next_event().await["op"], "space_delete");
    assert_eq!(bws.next_event().await["op"], "member_remove");

    server
        .delete(&format!("/spaces/{space_id}/members/@me"), Some(&bob))
        .await
        .ok();
    assert_eq!(bws.next_event().await["op"], "space_delete");
    let removed = ows.recv_op("member_remove").await;
    assert_eq!(removed["d"]["user_id"], alice_id);
    let removed = ows.recv_op("member_remove").await;
    assert_eq!(removed["d"]["user_id"], bob_id);
}
