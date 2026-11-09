//! Profiles, user lookup, notification settings, the mentions inbox, search and admin users.

mod common;

use common::{TestServer, start};
use hermes_proto::permissions::Permissions;
use hermes_server::config::RegistrationMode;
use serde_json::{Value, json};

/// When `user` was last online, as `viewer` sees it, once a write has had the time to land.
async fn last_online(server: &TestServer, viewer: &str, user: &str) -> Value {
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    server
        .get(&format!("/users/{user}"), Some(viewer))
        .await
        .ok()["last_online_at"]
        .clone()
}

#[tokio::test]
async fn last_online_counts_being_visible_and_server_activity_only() {
    use hermes_server::gateway::last_online::MIN_VISIBLE_MS;

    let server = start("users-last-online", RegistrationMode::InviteOnly).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "general");
    let id = alice_id.parse().unwrap();
    assert_eq!(last_online(&server, &owner, &alice_id).await, Value::Null);

    // Connecting and going invisible at once leaves nothing.
    let (mut ws, _) = server.identify(&alice).await;
    ws.send(json!({ "op": "presence_update", "d": { "status": "invisible" } }))
        .await;
    assert_eq!(last_online(&server, &owner, &alice_id).await, Value::Null);

    // While invisible, a DM does not count.
    let dm = server
        .post("/dms", Some(&alice), json!({ "recipient_ids": [owner_id] }))
        .await
        .ok();
    send(&server, &alice, dm["id"].as_str().unwrap(), "psst").await;
    assert_eq!(last_online(&server, &owner, &alice_id).await, Value::Null);

    // Writing in a server does.
    send(&server, &alice, &general, "hello").await;
    let written = last_online(&server, &owner, &alice_id).await;
    assert!(written.is_i64(), "{written}");

    // Idle after being visible long enough: the moment it ended counts, and idling adds nothing.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    ws.send(json!({ "op": "presence_update", "d": { "status": "online" } }))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    server.state.last_online.backdate(id, MIN_VISIBLE_MS);
    ws.send(json!({ "op": "presence_update", "d": { "status": "idle" } }))
        .await;
    let idle_at = last_online(&server, &owner, &alice_id).await;
    assert!(
        idle_at.as_i64() > written.as_i64(),
        "{idle_at} after {written}"
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(last_online(&server, &owner, &alice_id).await, idle_at);
}

async fn send(server: &TestServer, token: &str, channel: &str, content: &str) -> Value {
    server
        .post(
            &format!("/channels/{channel}/messages"),
            Some(token),
            json!({ "content": content, "nonce": common::nonce() }),
        )
        .await
        .ok()
}

#[tokio::test]
async fn profile_changes_reach_everyone_who_sees_me() {
    let server = start("users-profile", RegistrationMode::Open).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (stranger, _) = server.register("stranger", None).await;
    let (mut ws, _) = server.identify(&owner).await;

    let me = server
        .patch(
            "/users/@me",
            Some(&alice),
            json!({ "display_name": "Alice A.", "status_text": "  coding  " }),
        )
        .await
        .ok();
    assert_eq!(me["display_name"], "Alice A.");
    assert_eq!(me["status_text"], "coding", "trimmed");
    assert_eq!(me["instance_admin"], false);
    let event = ws.recv_op("user_update").await;
    assert_eq!(event["d"]["id"], alice_id);
    assert_eq!(event["d"]["display_name"], "Alice A.");

    // Clearing with null, and validation.
    let me = server
        .patch("/users/@me", Some(&alice), json!({ "status_text": null }))
        .await
        .ok();
    assert_eq!(me["status_text"], json!(null));
    assert_eq!(me["display_name"], "Alice A.", "untouched");
    server
        .patch(
            "/users/@me",
            Some(&alice),
            json!({ "display_name": "x".repeat(40) }),
        )
        .await
        .err(400, "VALIDATION_FAILED");
    server
        .patch("/users/@me", Some(&alice), json!({ "avatar_id": "12345" }))
        .await
        .err(400, "VALIDATION_FAILED");
    server
        .patch(
            "/users/@me",
            Some(&alice),
            json!({ "status_text": "two\nlines" }),
        )
        .await
        .err(400, "VALIDATION_FAILED");

    // Lookup: people I share a space with, not strangers.
    let profile = server
        .get(&format!("/users/{alice_id}"), Some(&owner))
        .await
        .ok();
    assert_eq!(profile["username"], "alice");
    assert_eq!(profile["mutual_space_ids"], json!([space_id]));
    server
        .get(&format!("/users/{owner_id}"), Some(&stranger))
        .await
        .err(404, "NOT_FOUND");
}

#[tokio::test]
async fn notification_settings_are_saved_and_synced() {
    let server = start("users-notify", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (stranger, _) = server.register("stranger", None).await;
    let general = server.channel_id(&space_id, "general");
    let (mut ws, _) = server.identify(&owner).await;

    let path = format!("/users/@me/notification-settings/{general}");
    server
        .put(
            &path,
            Some(&owner),
            json!({ "level": "all", "muted_until": -1 }),
        )
        .await
        .ok();
    let event = ws.recv_op("notification_settings_update").await;
    assert_eq!(event["d"]["target_id"], general);
    assert_eq!(event["d"]["level"], "all");
    assert_eq!(event["d"]["muted_until"], -1);
    // Saved: a new session's ready has it.
    let (_, ready) = server.identify(&owner).await;
    assert_eq!(ready["notification_settings"][0]["level"], "all");

    server
        .put(
            &format!("/users/@me/notification-settings/{space_id}"),
            Some(&owner),
            json!({ "level": "none" }),
        )
        .await
        .ok();
    ws.recv_op("notification_settings_update").await;
    server.delete(&path, Some(&owner)).await.ok();
    let event = ws.recv_op("notification_settings_delete").await;
    assert_eq!(event["d"]["target_id"], general);
    let (_, ready) = server.identify(&owner).await;
    let settings = ready["notification_settings"].as_array().unwrap();
    assert_eq!(settings.len(), 1);
    assert_eq!(settings[0]["target_id"], space_id);

    // Only spaces and channels I can see.
    server
        .put(&path, Some(&stranger), json!({ "level": "all" }))
        .await
        .err(404, "NOT_FOUND");
    server
        .put(&path, Some(&owner), json!({ "level": "loud" }))
        .await
        .err(400, "BAD_REQUEST");
}

#[tokio::test]
async fn search_finds_only_what_i_may_read() {
    let server = start("users-search", RegistrationMode::Open).await;
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
            json!({ "allow": "0", "deny": Permissions::VIEW_CHANNEL.bits().to_string() }),
        )
        .await
        .ok();

    send(&server, &alice, &general, "the pizza party is on friday").await;
    send(&server, &owner, &general, "Pizzas ordered!").await;
    send(&server, &owner, &secret, "secret pizza recipe").await;
    let deleted = send(&server, &alice, &general, "pizza typo").await;
    server
        .delete(
            &format!(
                "/channels/{general}/messages/{}",
                deleted["id"].as_str().unwrap()
            ),
            Some(&alice),
        )
        .await
        .ok();

    let found = server.get("/search?q=pizz", Some(&alice)).await.ok();
    assert_eq!(
        found["total"], 2,
        "prefix match, not the private channel, not deleted"
    );
    let messages = found["messages"].as_array().unwrap();
    assert_eq!(messages[0]["content"], "Pizzas ordered!", "newest first");
    let found = server.get("/search?q=pizza", Some(&owner)).await.ok();
    assert_eq!(found["total"], 3, "the owner sees the private channel");

    let found = server
        .get(
            &format!("/search?q=pizza&author_id={alice_id}"),
            Some(&owner),
        )
        .await
        .ok();
    assert_eq!(found["total"], 1);
    let found = server
        .get(
            &format!("/search?q=pizza&channel_id={secret}"),
            Some(&alice),
        )
        .await
        .ok();
    assert_eq!(found["total"], 0);
    let found = server
        .get("/search?q=%22party%20friday", Some(&alice))
        .await
        .ok();
    assert_eq!(found["total"], 1, "quotes are not FTS syntax errors");
    server
        .get("/search?q=%20", Some(&alice))
        .await
        .err(400, "VALIDATION_FAILED");
    server
        .get(&format!("/search?q={}", "a".repeat(300)), Some(&alice))
        .await
        .err(400, "VALIDATION_FAILED");
}

#[tokio::test]
async fn mentions_inbox_lists_messages_that_ping_me() {
    let server = start("users-mentions", RegistrationMode::Open).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "general");
    let role = server.role(&owner, &space_id, "helpers", 0).await;
    server
        .patch(
            &format!("/spaces/{space_id}/roles/{role}"),
            Some(&owner),
            json!({ "mentionable": true }),
        )
        .await
        .ok();
    server.assign(&owner, &space_id, &alice_id, &[&role]).await;

    send(&server, &owner, &general, &format!("hi <@{alice_id}>")).await;
    send(&server, &owner, &general, "nobody").await;
    send(&server, &owner, &general, &format!("<@&{role}> help")).await;
    send(&server, &alice, &general, &format!("self <@{alice_id}>")).await;
    send(&server, &alice, &general, &format!("<@{owner_id}>")).await;

    let inbox = server.get("/users/@me/mentions", Some(&alice)).await.ok();
    let contents: Vec<&str> = inbox
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["content"].as_str().unwrap())
        .collect();
    assert_eq!(contents.len(), 2, "{contents:?}");
    assert!(contents[0].ends_with("help"), "newest first");
    let page = server
        .get("/users/@me/mentions?limit=1", Some(&alice))
        .await
        .ok();
    assert_eq!(page.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn admins_manage_users() {
    let server = start("users-admin", RegistrationMode::Open).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;

    server
        .get("/admin/users", Some(&alice))
        .await
        .err(403, "MISSING_PERMISSION");
    let users = server.get("/admin/users", Some(&owner)).await.ok();
    let users = users.as_array().unwrap();
    assert_eq!(users.len(), 2);
    assert_eq!(users[0]["id"], owner_id);
    assert_eq!(users[0]["instance_admin"], true);

    server
        .patch(
            &format!("/admin/users/{owner_id}"),
            Some(&owner),
            json!({ "disabled": true }),
        )
        .await
        .err(400, "BAD_REQUEST");
    server
        .patch(
            &format!("/admin/users/{alice_id}"),
            Some(&owner),
            json!({ "disabled": true }),
        )
        .await
        .ok();
    server
        .get("/users/@me", Some(&alice))
        .await
        .err(401, "UNAUTHORIZED");
    server
        .login("alice", "correct horse")
        .await
        .err(403, "ACCOUNT_DISABLED");
    server
        .patch(
            &format!("/admin/users/{alice_id}"),
            Some(&owner),
            json!({ "disabled": false, "instance_admin": true }),
        )
        .await
        .ok();
    let alice = server.login("alice", "correct horse").await.ok();
    assert_eq!(alice["user"]["instance_admin"], true);
}

#[tokio::test]
async fn the_server_applies_changes_made_by_the_user_commands() {
    let server = start("users-refresh", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (mut ws, _) = server.identify(&alice).await;

    // What `hermes-server user disable alice` writes.
    server
        .sql(&format!(
            "UPDATE users SET disabled = 1 WHERE id = {alice_id}"
        ))
        .await;
    server
        .sql(&format!("DELETE FROM sessions WHERE user_id = {alice_id}"))
        .await;
    let (users, sessions) = hermes_server::users::refresh(&server.state).await.unwrap();
    assert_eq!((users, sessions), (1, 1));
    assert_eq!(ws.closed().await, 4011);
    server
        .get("/users/@me", Some(&alice))
        .await
        .err(401, "UNAUTHORIZED");
    let id = alice_id.parse().unwrap();
    assert!(server.state.cache().users[&id].disabled);
    assert_eq!(
        hermes_server::users::refresh(&server.state).await.unwrap(),
        (0, 0)
    );
}

#[tokio::test]
async fn two_factor_login_needs_a_code() {
    let server = start("users-2fa", RegistrationMode::Open).await;
    server.owner().await;
    let (token, _) = server.register("alice", None).await;
    let begun = server
        .post(
            "/users/@me/2fa/begin",
            Some(&token),
            json!({ "password": common::PASSWORD }),
        )
        .await
        .ok();
    let secret = begun["secret"].as_str().unwrap().to_owned();
    assert_eq!(
        server.get("/users/@me/2fa", Some(&token)).await.ok()["enabled"],
        false,
        "not until a code is confirmed"
    );
    server
        .post(
            "/users/@me/2fa/confirm",
            Some(&token),
            json!({ "code": "000000" }),
        )
        .await
        .err(401, "INVALID_CREDENTIALS");
    server
        .sql(&format!(
            "UPDATE users SET totp_secret = '{secret}', totp_pending = NULL"
        ))
        .await;
    server
        .login("alice", common::PASSWORD)
        .await
        .err(401, "TWO_FACTOR_REQUIRED");
}

#[tokio::test]
async fn deleting_an_account_leaves_no_name_behind() {
    let server = start("users-delete", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "general");
    let path = format!("/channels/{general}/messages");
    let sent = server
        .post(
            &path,
            Some(&alice),
            json!({ "content": "bye", "nonce": common::nonce() }),
        )
        .await
        .ok();
    let export = server.get("/users/@me/export", Some(&alice)).await.ok();
    assert_eq!(export["messages"][0]["content"], "bye");
    server
        .post(
            "/users/@me/delete",
            Some(&alice),
            json!({ "password": "wrong", "delete_messages": true }),
        )
        .await
        .err(401, "INVALID_CREDENTIALS");
    server
        .post(
            "/users/@me/delete",
            Some(&alice),
            json!({ "password": common::PASSWORD, "delete_messages": true }),
        )
        .await
        .ok();
    server
        .get("/users/@me", Some(&alice))
        .await
        .err(401, "UNAUTHORIZED");
    let history = server.get(&path, Some(&owner)).await.ok();
    let gone = history
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == sent["id"])
        .unwrap();
    assert_eq!(gone["content"], "");
    assert!(
        gone["author"]["username"]
            .as_str()
            .unwrap()
            .contains("deleted")
    );
    // A server is not left without an owner.
    server
        .post(
            "/users/@me/delete",
            Some(&owner),
            json!({ "password": common::PASSWORD }),
        )
        .await
        .err(400, "BAD_REQUEST");
}

#[tokio::test]
async fn someone_blocked_cannot_start_a_dm() {
    let server = start("users-block", RegistrationMode::Open).await;
    server.owner().await;
    let (alice, alice_id) = server.register("alice", None).await;
    let (bob, bob_id) = server.register("bob", None).await;
    server
        .put(
            &format!("/users/@me/blocks/{bob_id}"),
            Some(&alice),
            json!(null),
        )
        .await
        .ok();
    assert_eq!(
        server.get("/users/@me/blocks", Some(&alice)).await.ok(),
        json!([bob_id])
    );
    server
        .post("/dms", Some(&bob), json!({ "recipient_ids": [alice_id] }))
        .await
        .err(403, "MISSING_PERMISSION");
}
