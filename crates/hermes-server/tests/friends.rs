//! Tags (`name#1234`), changing the username, and friends.

mod common;

use common::{TestServer, start};
use hermes_server::config::RegistrationMode;
use serde_json::{Value, json};

const PASSWORD: &str = "correct horse";

fn handle(user: &Value) -> String {
    match user["discriminator"].as_str() {
        Some(tag) => format!("{}#{tag}", user["username"].as_str().unwrap()),
        None => user["username"].as_str().unwrap().to_owned(),
    }
}

async fn me(server: &TestServer, token: &str) -> Value {
    server.get("/users/@me", Some(token)).await.ok()
}

async fn add(server: &TestServer, token: &str, who: &str) -> common::Res {
    server
        .post("/users/@me/friends", Some(token), json!({ "handle": who }))
        .await
}

#[tokio::test]
async fn the_owner_has_no_tag_and_everyone_else_gets_one() {
    let server = start("tags-new", RegistrationMode::Open).await;
    let (owner, ..) = server.owner().await;
    assert_eq!(me(&server, &owner).await.get("discriminator"), None);

    let (a, a_id) = server.register("sam", None).await;
    let (b, b_id) = server.register("SAM", None).await;
    assert_ne!(a_id, b_id, "a name can be used by many accounts");
    let (a, b) = (me(&server, &a).await, me(&server, &b).await);
    assert_eq!(a["username"], "sam");
    assert_eq!(b["username"], "sam");
    let (ta, tb) = (
        a["discriminator"].as_str().unwrap(),
        b["discriminator"].as_str().unwrap(),
    );
    assert_ne!(ta, tb);
    for tag in [ta, tb] {
        assert_eq!(tag.len(), 4);
        assert!(tag.bytes().all(|b| b.is_ascii_digit()) && tag != "0000");
    }

    // With two accounts the tag is needed to sign in; it finds the right one.
    let res = server.login("sam", PASSWORD).await;
    res.err(401, "INVALID_CREDENTIALS");
    let res = server.login(&handle(&a), PASSWORD).await.ok();
    assert_eq!(res["user"]["id"], a["id"]);
    let res = server
        .login(&handle(&b).to_uppercase(), PASSWORD)
        .await
        .ok();
    assert_eq!(res["user"]["id"], b["id"]);

    // With one, the plain name still works, as it did before tags.
    let (_, id) = server.register("solo", None).await;
    let res = server.login("solo", PASSWORD).await.ok();
    assert_eq!(res["user"]["id"], id);
    server
        .login("sol", PASSWORD)
        .await
        .err(401, "INVALID_CREDENTIALS");
    server
        .login("solo#0000", PASSWORD)
        .await
        .err(401, "INVALID_CREDENTIALS");
}

#[tokio::test]
async fn changing_the_username_keeps_the_tag_when_it_can() {
    let server = start("tags-rename", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let tag = me(&server, &alice).await["discriminator"].clone();
    let (mut ws, _) = server.identify(&owner).await;

    // The password is needed.
    server
        .post(
            "/users/@me/username",
            Some(&alice),
            json!({ "username": "alicia", "current_password": "wrong" }),
        )
        .await
        .err(401, "INVALID_CREDENTIALS");
    // And names follow the same rules as when registering.
    server
        .post(
            "/users/@me/username",
            Some(&alice),
            json!({ "username": "no spaces", "current_password": PASSWORD }),
        )
        .await
        .err(400, "VALIDATION_FAILED");

    let user = server
        .post(
            "/users/@me/username",
            Some(&alice),
            json!({ "username": "Alicia", "current_password": PASSWORD }),
        )
        .await
        .ok();
    assert_eq!(user["username"], "alicia");
    assert_eq!(user["discriminator"], tag);
    let event = ws.recv_op("user_update").await;
    assert_eq!(event["d"]["id"], alice_id);
    assert_eq!(event["d"]["username"], "alicia");
    let res = server.login("alicia", PASSWORD).await.ok();
    assert_eq!(res["user"]["id"], alice_id);
    server
        .login("alice", PASSWORD)
        .await
        .err(401, "INVALID_CREDENTIALS");

    // Someone of the new name has the tag: another one is picked.
    let (bob, bob_id) = server.register("bob", None).await;
    let (carol, _) = server.register("carol", None).await;
    let carols = me(&server, &carol).await["discriminator"].clone();
    server
        .sql(&format!(
            "UPDATE users SET username = 'bob#{}' WHERE id = {bob_id}",
            carols.as_str().unwrap()
        ))
        .await;
    hermes_server::users::refresh(&server.state).await.unwrap();
    assert_eq!(me(&server, &bob).await["discriminator"], carols);
    let renamed = server
        .post(
            "/users/@me/username",
            Some(&carol),
            json!({ "username": "bob", "current_password": PASSWORD }),
        )
        .await
        .ok();
    assert_eq!(renamed["username"], "bob");
    assert_ne!(renamed["discriminator"], carols, "bob has that tag");

    // An account without a tag needs the name to be free.
    server
        .post(
            "/users/@me/username",
            Some(&owner),
            json!({ "username": "free_name", "current_password": PASSWORD }),
        )
        .await
        .ok();
    let (_, other_id) = server.register("taken", None).await;
    server
        .patch(
            &format!("/admin/users/{other_id}"),
            Some(&owner),
            json!({ "tagless": true }),
        )
        .await
        .ok();
    server
        .post(
            "/users/@me/username",
            Some(&owner),
            json!({ "username": "taken", "current_password": PASSWORD }),
        )
        .await
        .err(409, "CONFLICT");
}

#[tokio::test]
async fn admins_give_and_take_the_no_tag_privilege() {
    let server = start("tags-admin", RegistrationMode::Open).await;
    let (owner, ..) = server.owner().await;
    let (fides, fides_id) = server.register("fides", None).await;
    let (other, other_id) = server.register("fides", None).await;

    // Only instance admins may.
    server
        .patch(
            &format!("/admin/users/{other_id}"),
            Some(&fides),
            json!({ "tagless": true }),
        )
        .await
        .err(403, "MISSING_PERMISSION");

    server
        .patch(
            &format!("/admin/users/{fides_id}"),
            Some(&owner),
            json!({ "tagless": true }),
        )
        .await
        .ok();
    assert_eq!(me(&server, &fides).await.get("discriminator"), None);
    // The plain name now means this account, also with others of the name around.
    let res = server.login("fides", PASSWORD).await.ok();
    assert_eq!(res["user"]["id"], fides_id);

    // Only one account can have the name without a tag.
    server
        .patch(
            &format!("/admin/users/{other_id}"),
            Some(&owner),
            json!({ "tagless": true }),
        )
        .await
        .err(409, "CONFLICT");
    assert!(me(&server, &other).await["discriminator"].is_string());

    server
        .patch(
            &format!("/admin/users/{fides_id}"),
            Some(&owner),
            json!({ "tagless": false }),
        )
        .await
        .ok();
    assert!(me(&server, &fides).await["discriminator"].is_string());
}

#[tokio::test]
async fn friend_requests_from_asking_to_removing() {
    let server = start("friends-flow", RegistrationMode::Open).await;
    server.owner().await;
    let (alice, alice_id) = server.register("alice", None).await;
    let (bob, bob_id) = server.register("bob", None).await;
    let bob_handle = handle(&me(&server, &bob).await);
    let (mut alice_ws, _) = server.identify(&alice).await;
    let (mut bob_ws, ready) = server.identify(&bob).await;
    assert_eq!(ready["relationships"], json!([]));

    // Mistakes.
    add(&server, &alice, "nobody#0001")
        .await
        .err(404, "NOT_FOUND");
    add(&server, &alice, "bob#0000").await.err(404, "NOT_FOUND");
    let own = handle(&me(&server, &alice).await);
    add(&server, &alice, &own).await.err(400, "BAD_REQUEST");
    server
        .put(
            &format!("/users/@me/friends/{bob_id}"),
            Some(&alice),
            json!({}),
        )
        .await
        .err(404, "NOT_FOUND");

    // Asking.
    let sent = add(&server, &alice, &bob_handle).await.ok();
    assert_eq!(sent["kind"], "outgoing");
    assert_eq!(sent["user"]["id"], bob_id);
    let event = alice_ws.recv_op("relationship_update").await;
    assert_eq!(event["d"]["kind"], "outgoing");
    let event = bob_ws.recv_op("relationship_update").await;
    assert_eq!(event["d"]["kind"], "incoming");
    assert_eq!(event["d"]["user"]["id"], alice_id);
    // Asking twice changes nothing.
    assert_eq!(
        add(&server, &alice, &bob_handle).await.ok()["kind"],
        "outgoing"
    );

    // A request shows the profile, so it can be judged.
    server
        .get(&format!("/users/{alice_id}"), Some(&bob))
        .await
        .ok();
    // The asker cannot accept their own request.
    server
        .put(
            &format!("/users/@me/friends/{bob_id}"),
            Some(&alice),
            json!({}),
        )
        .await
        .err(404, "NOT_FOUND");

    // Accepting.
    let accepted = server
        .put(
            &format!("/users/@me/friends/{alice_id}"),
            Some(&bob),
            json!({}),
        )
        .await
        .ok();
    assert_eq!(accepted["kind"], "friend");
    assert_eq!(accepted["user"]["id"], alice_id);
    let event = alice_ws.recv_op("relationship_update").await;
    assert_eq!(event["d"]["kind"], "friend");
    // They share no space, but friends see each other's presence.
    let presence = bob_ws.recv_op("presence_update").await;
    assert_eq!(presence["d"]["user_id"], alice_id);

    let list = server.get("/users/@me/friends", Some(&alice)).await.ok();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["kind"], "friend");
    let (_, ready) = server.identify(&alice).await;
    assert_eq!(ready["relationships"][0]["user"]["id"], bob_id);

    // Profile changes reach friends.
    server
        .patch(
            "/users/@me",
            Some(&alice),
            json!({ "display_name": "Alice A." }),
        )
        .await
        .ok();
    let event = bob_ws.recv_op("user_update").await;
    assert_eq!(event["d"]["display_name"], "Alice A.");

    // Removing.
    let res = server
        .delete(&format!("/users/@me/friends/{alice_id}"), Some(&bob))
        .await;
    res.ok();
    let event = alice_ws.recv_op("relationship_remove").await;
    assert_eq!(event["d"]["user_id"], bob_id);
    let list = server.get("/users/@me/friends", Some(&alice)).await.ok();
    assert_eq!(list, json!([]));
    server
        .delete(&format!("/users/@me/friends/{alice_id}"), Some(&bob))
        .await
        .err(404, "NOT_FOUND");
}

#[tokio::test]
async fn asking_someone_who_already_asked_me_accepts() {
    let server = start("friends-mutual", RegistrationMode::Open).await;
    server.owner().await;
    let (alice, _) = server.register("alice", None).await;
    let (bob, _) = server.register("bob", None).await;
    let (alice_handle, bob_handle) = (
        handle(&me(&server, &alice).await),
        handle(&me(&server, &bob).await),
    );
    assert_eq!(
        add(&server, &alice, &bob_handle).await.ok()["kind"],
        "outgoing"
    );
    assert_eq!(
        add(&server, &bob, &alice_handle).await.ok()["kind"],
        "friend"
    );
}

#[tokio::test]
async fn friends_may_message_whatever_the_dm_settings_say() {
    let server = start("friends-dm", RegistrationMode::Open).await;
    server.owner().await;
    let (alice, _) = server.register("alice", None).await;
    let (bob, bob_id) = server.register("bob", None).await;
    let bob_handle = handle(&me(&server, &bob).await);
    server
        .patch(
            "/users/@me/privacy",
            Some(&bob),
            json!({ "dm_from": "nobody", "message_requests": false }),
        )
        .await
        .ok();
    server
        .post("/dms", Some(&alice), json!({ "recipient_ids": [bob_id] }))
        .await
        .err(403, "MISSING_PERMISSION");

    add(&server, &alice, &bob_handle).await.ok();
    let alice_id = me(&server, &alice).await["id"].clone();
    server
        .put(
            &format!("/users/@me/friends/{}", alice_id.as_str().unwrap()),
            Some(&bob),
            json!({}),
        )
        .await
        .ok();
    server
        .post("/dms", Some(&alice), json!({ "recipient_ids": [bob_id] }))
        .await
        .ok();
}

#[tokio::test]
async fn relationships_survive_a_restart() {
    let server = start("friends-restart", RegistrationMode::Open).await;
    server.owner().await;
    let (alice, _) = server.register("alice", None).await;
    let (bob, _) = server.register("bob", None).await;
    let bob_handle = handle(&me(&server, &bob).await);
    add(&server, &alice, &bob_handle).await.ok();
    let cache = hermes_server::state::load_from_db(&server.state.db)
        .await
        .unwrap();
    let alice_id = me(&server, &alice).await["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let list = cache.relationships_of(hermes_proto::id::Id(alice_id));
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].user.username, "bob");
    assert!(list[0].user.discriminator.is_some());
}
