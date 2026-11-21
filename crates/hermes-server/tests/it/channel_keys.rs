//! Encrypted server channels, from the server's side: numbered keys, who may hand them to whom,
//! and that nothing readable gets in once they are required.

use crate::common;

use common::{TestServer, start_with};
use hermes_proto::permissions::Permissions;
use hermes_server::config::RegistrationMode;
use serde_json::{Value, json};

async fn server(name: &str) -> TestServer {
    start_with(name, RegistrationMode::Open, |state| {
        state.config.privacy.require_channel_encryption = true;
    })
    .await
}

fn envelope(epoch: Option<u32>) -> Value {
    let mut value = json!({
        "v": 1, "device": "d", "eph": "", "nonce": common::nonce(), "ciphertext": "opaque",
        "keys": [{ "device": "", "key": "k" }], "sig": "s",
    });
    if let Some(epoch) = epoch {
        value["epoch"] = json!(epoch);
    }
    value
}

fn key(epoch: u32, user: &str, device: &str, sender: &str) -> Value {
    json!({
        "epoch": epoch, "user_id": user, "device": device, "sender_id": sender,
        "sender_device": "their-device", "eph": "e", "key": "k", "sig": "s",
    })
}

async fn publish_device(server: &TestServer, token: &str, device: &str) {
    server
        .put(
            "/users/@me/devices",
            Some(token),
            json!({
                "device_id": device, "sign_key": "sign", "dh_key": "dh", "signature": "sig",
            }),
        )
        .await
        .ok();
}

async fn start_epoch(server: &TestServer, token: &str, channel: &str, epoch: u32, keys: Value) {
    server
        .post(
            &format!("/channels/{channel}/keys/epochs"),
            Some(token),
            json!({ "epoch": epoch, "keys": keys }),
        )
        .await
        .ok();
}

async fn keys_of(server: &TestServer, token: &str, channel: &str, device: &str) -> Value {
    server
        .get(
            &format!("/channels/{channel}/keys?device={device}"),
            Some(token),
        )
        .await
        .ok()
}

#[tokio::test]
async fn a_channel_takes_only_messages_under_its_current_key() {
    let server = server("keys-messages").await;
    let (owner, owner_id, space) = server.owner().await;
    let general = server.channel_id(&space, "general");
    let path = format!("/channels/{general}/messages");

    // A client from before this gets an error instead of writing in clear.
    let res = server
        .post(
            &path,
            Some(&owner),
            json!({ "content": "plain", "nonce": common::nonce() }),
        )
        .await;
    res.err(400, "BAD_REQUEST");
    assert!(
        res.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("update Hermes")
    );

    // No key yet, and then a key that is not the newest.
    let send = |epoch| json!({ "nonce": common::nonce(), "e2ee": envelope(epoch) });
    server
        .post(&path, Some(&owner), send(Some(1)))
        .await
        .err(409, "CONFLICT");
    start_epoch(
        &server,
        &owner,
        &general,
        1,
        json!([key(1, &owner_id, "d", &owner_id)]),
    )
    .await;
    server.post(&path, Some(&owner), send(Some(1))).await.ok();
    start_epoch(
        &server,
        &owner,
        &general,
        2,
        json!([key(2, &owner_id, "d", &owner_id)]),
    )
    .await;
    server
        .post(&path, Some(&owner), send(Some(1)))
        .await
        .err(409, "CONFLICT");
    server
        .post(&path, Some(&owner), send(None))
        .await
        .err(409, "CONFLICT");
    let sent = server.post(&path, Some(&owner), send(Some(2))).await.ok();
    assert_eq!(sent["content"], "");
    assert_eq!(sent["e2ee"]["epoch"], 2);
}

#[tokio::test]
async fn epochs_are_numbered_one_after_another() {
    let server = server("keys-epochs").await;
    let (owner, owner_id, space) = server.owner().await;
    let general = server.channel_id(&space, "general");
    let path = format!("/channels/{general}/keys/epochs");
    let body = |epoch| json!({ "epoch": epoch, "keys": [key(epoch, &owner_id, "d", &owner_id)] });

    server
        .post(&path, Some(&owner), body(2))
        .await
        .err(409, "CONFLICT");
    server.post(&path, Some(&owner), body(1)).await.ok();
    server
        .post(&path, Some(&owner), body(1))
        .await
        .err(409, "CONFLICT");
    let keys = keys_of(&server, &owner, &general, "d").await;
    assert_eq!(keys["current_epoch"], 1);
    assert_eq!(keys["keys"].as_array().unwrap().len(), 1);
    assert!(
        keys_of(&server, &owner, &general, "other").await["keys"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn keys_only_go_to_those_who_can_read_and_from_the_sender() {
    let server = server("keys-who").await;
    let (owner, owner_id, space) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space, "alice").await;
    let (_, stranger_id) = server.register("stranger", None).await;
    let general = server.channel_id(&space, "general");
    start_epoch(
        &server,
        &owner,
        &general,
        1,
        json!([key(1, &owner_id, "d", &owner_id)]),
    )
    .await;
    let path = format!("/channels/{general}/keys");

    server
        .put(
            &path,
            Some(&owner),
            json!({ "keys": [key(1, &stranger_id, "s", &owner_id)] }),
        )
        .await
        .err(400, "VALIDATION_FAILED");
    // Signing in someone else's name.
    server
        .put(
            &path,
            Some(&alice),
            json!({ "keys": [key(1, &alice_id, "a", &owner_id)] }),
        )
        .await
        .err(400, "VALIDATION_FAILED");
    // An epoch that does not exist.
    server
        .put(
            &path,
            Some(&owner),
            json!({ "keys": [key(9, &alice_id, "a", &owner_id)] }),
        )
        .await
        .err(400, "VALIDATION_FAILED");

    server
        .put(
            &path,
            Some(&owner),
            json!({ "keys": [key(1, &alice_id, "a", &owner_id)] }),
        )
        .await
        .ok();
    assert_eq!(
        keys_of(&server, &alice, &general, "a").await["keys"][0]["epoch"],
        1
    );

    // Someone outside the server gets nothing at all.
    let (outsider, _) = server.register("outsider", None).await;
    server
        .get(&format!("{path}?device=x"), Some(&outsider))
        .await
        .err(404, "NOT_FOUND");
}

#[tokio::test]
async fn a_request_reaches_the_members_and_ends_with_the_handover() {
    let server = server("keys-request").await;
    let (owner, owner_id, space) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space, "alice").await;
    let general = server.channel_id(&space, "general");
    publish_device(&server, &alice, "0000000000000000000000000000abcd").await;
    start_epoch(
        &server,
        &owner,
        &general,
        1,
        json!([key(1, &owner_id, "d", &owner_id)]),
    )
    .await;
    let (mut owner_ws, _) = server.identify(&owner).await;
    let (mut alice_ws, _) = server.identify(&alice).await;

    let request = format!("/channels/{general}/keys/requests");
    server
        .post(&request, Some(&alice), json!({ "device": "someone-elses" }))
        .await
        .err(400, "VALIDATION_FAILED");
    server
        .post(
            &request,
            Some(&alice),
            json!({ "device": "0000000000000000000000000000abcd" }),
        )
        .await
        .ok();
    let event = owner_ws.recv_op("channel_key_request").await;
    assert_eq!(event["d"]["user_id"], alice_id);
    assert_eq!(event["d"]["device"], "0000000000000000000000000000abcd");

    // Someone coming online later finds it waiting.
    let waiting = server
        .get("/users/@me/key-requests", Some(&owner))
        .await
        .ok();
    assert_eq!(waiting.as_array().unwrap().len(), 1);

    server
        .put(
            &format!("/channels/{general}/keys"),
            Some(&owner),
            json!({ "keys": [key(1, &alice_id, "0000000000000000000000000000abcd", &owner_id)] }),
        )
        .await
        .ok();
    let delivered = alice_ws.recv_op("channel_keys_delivered").await;
    assert_eq!(delivered["d"]["channel_id"], general);
    let waiting = server
        .get("/users/@me/key-requests", Some(&owner))
        .await
        .ok();
    assert!(waiting.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn approved_channels_take_keys_only_from_those_who_may_approve() {
    let server = server("keys-approved").await;
    let (owner, owner_id, space) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space, "bob").await;
    let general = server.channel_id(&space, "general");
    start_epoch(
        &server,
        &owner,
        &general,
        1,
        json!([key(1, &owner_id, "d", &owner_id)]),
    )
    .await;
    server
        .put(
            &format!("/spaces/{space}/key-policy"),
            Some(&owner),
            json!({ "handover": "approved", "new_members_read_history": true }),
        )
        .await
        .ok();
    let path = format!("/channels/{general}/keys");
    server
        .put(
            &path,
            Some(&alice),
            json!({ "keys": [key(1, &bob_id, "b", &alice_id)] }),
        )
        .await
        .err(403, "MISSING_PERMISSION");

    let role = server
        .role(&owner, &space, "manager", Permissions::MANAGE_SPACE.bits())
        .await;
    server.assign(&owner, &space, &alice_id, &[&role]).await;
    server
        .put(
            &path,
            Some(&alice),
            json!({ "keys": [key(1, &bob_id, "b", &alice_id)] }),
        )
        .await
        .ok();
    assert_eq!(
        keys_of(&server, &bob, &general, "b").await["keys"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn the_key_has_to_be_replaced_when_a_holder_leaves() {
    let server = server("keys-rotate").await;
    let (owner, owner_id, space) = server.owner().await;
    let (_, alice_id) = server.member(&owner, &space, "alice").await;
    let general = server.channel_id(&space, "general");
    start_epoch(
        &server,
        &owner,
        &general,
        1,
        json!([
            key(1, &owner_id, "d", &owner_id),
            key(1, &alice_id, "a", &owner_id)
        ]),
    )
    .await;
    assert_eq!(
        keys_of(&server, &owner, &general, "d").await["rotate_needed"],
        false
    );
    let members = server
        .get(&format!("/channels/{general}/keys/members"), Some(&owner))
        .await
        .ok();
    assert_eq!(members["holders"].as_array().unwrap().len(), 2);

    server
        .delete(&format!("/spaces/{space}/members/{alice_id}"), Some(&owner))
        .await
        .ok();
    assert_eq!(
        keys_of(&server, &owner, &general, "d").await["rotate_needed"],
        true
    );
    start_epoch(
        &server,
        &owner,
        &general,
        2,
        json!([key(2, &owner_id, "d", &owner_id)]),
    )
    .await;
    assert_eq!(
        keys_of(&server, &owner, &general, "d").await["rotate_needed"],
        false
    );
}

#[tokio::test]
async fn only_the_owner_sets_a_server_and_admins_a_channel() {
    let server = server("keys-policy").await;
    let (owner, _, space) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space, "alice").await;
    let general = server.channel_id(&space, "general");
    let rule = json!({ "handover": "approved", "new_members_read_history": false });
    let space_path = format!("/spaces/{space}/key-policy");
    let channel_path = format!("/channels/{general}/key-policy");
    let hide = json!({ "new_members_read_history": false });

    let role = server
        .role(&owner, &space, "admin", Permissions::ADMINISTRATOR.bits())
        .await;
    server
        .put(&channel_path, Some(&alice), hide.clone())
        .await
        .err(403, "MISSING_PERMISSION");
    server.assign(&owner, &space, &alice_id, &[&role]).await;
    // An administrator still cannot change the whole server.
    server
        .put(&space_path, Some(&alice), rule.clone())
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .put(
            &space_path,
            Some(&owner),
            json!({
                "handover": "approved", "new_members_read_history": true,
            }),
        )
        .await
        .ok();
    server.put(&channel_path, Some(&alice), hide).await.ok();

    // The channel changes one thing and follows the server for the rest.
    let keys = keys_of(&server, &owner, &general, "d").await;
    assert_eq!(keys["policy"], rule);
    let own = server.get(&channel_path, Some(&owner)).await.ok();
    assert_eq!(own["handover"], Value::Null);
    assert_eq!(own["new_members_read_history"], false);
}

#[tokio::test]
async fn an_encrypted_message_says_who_it_mentions() {
    let server = server("keys-mentions").await;
    let (owner, owner_id, space) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space, "alice").await;
    let general = server.channel_id(&space, "general");
    start_epoch(
        &server,
        &owner,
        &general,
        1,
        json!([key(1, &owner_id, "d", &owner_id)]),
    )
    .await;
    let path = format!("/channels/{general}/messages");
    let sent = server
        .post(
            &path,
            Some(&owner),
            json!({
                "nonce": common::nonce(),
                "e2ee": envelope(Some(1)),
                "mentions": { "users": [alice_id], "roles": [], "everyone": false, "here": false },
            }),
        )
        .await
        .ok();
    assert_eq!(sent["mentions"]["users"], json!([alice_id]));

    // Without the permission, a claim of @everyone is dropped as it is for plain text.
    let (bob_token, _) = server.member(&owner, &space, "bob").await;
    let _ = alice;
    start_epoch(
        &server,
        &owner,
        &general,
        2,
        json!([key(2, &owner_id, "d", &owner_id)]),
    )
    .await;
    // The @everyone role has the ID of its server.
    let id = space.clone();
    let denied = (Permissions::SEND_MESSAGES | Permissions::VIEW_CHANNEL).bits();
    server
        .patch(
            &format!("/spaces/{space}/roles/{id}"),
            Some(&owner),
            json!({ "permissions": denied.to_string() }),
        )
        .await
        .ok();
    let sent = server
        .post(
            &path,
            Some(&bob_token),
            json!({
                "nonce": common::nonce(),
                "e2ee": envelope(Some(2)),
                "mentions": { "users": [], "roles": [], "everyone": true, "here": false },
            }),
        )
        .await
        .ok();
    assert_eq!(sent["mentions"]["everyone"], false);
}

#[tokio::test]
async fn someone_who_can_only_read_cannot_replace_the_key() {
    let server = server("keys-read-only").await;
    let (owner, owner_id, space) = server.owner().await;
    let (bob, bob_id) = server.member(&owner, &space, "bob").await;
    let general = server.channel_id(&space, "general");
    server
        .put(
            &format!("/channels/{general}/overrides/{space}"),
            Some(&owner),
            json!({ "allow": "0", "deny": Permissions::SEND_MESSAGES.bits().to_string() }),
        )
        .await
        .ok();
    server
        .post(
            &format!("/channels/{general}/keys/epochs"),
            Some(&bob),
            json!({ "epoch": 1, "keys": [key(1, &bob_id, "d", &bob_id)] }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
    // Whoever may manage the server still can, whatever the channel says.
    start_epoch(
        &server,
        &owner,
        &general,
        1,
        json!([key(1, &owner_id, "d", &owner_id)]),
    )
    .await;
}

#[tokio::test]
async fn nobody_can_swap_a_key_someone_else_handed_over() {
    let server = server("keys-swap").await;
    let (owner, owner_id, space) = server.owner().await;
    let (bob, bob_id) = server.member(&owner, &space, "bob").await;
    let (mallory, mallory_id) = server.member(&owner, &space, "mallory").await;
    let general = server.channel_id(&space, "general");
    publish_device(&server, &bob, "0000000000000000000000000000b0b0").await;
    start_epoch(
        &server,
        &owner,
        &general,
        1,
        json!([key(
            1,
            &bob_id,
            "0000000000000000000000000000b0b0",
            &owner_id
        )]),
    )
    .await;
    let path = format!("/channels/{general}/keys");
    let mut forged = key(1, &bob_id, "0000000000000000000000000000b0b0", &mallory_id);
    forged["key"] = json!("forged");
    server
        .put(&path, Some(&mallory), json!({ "keys": [forged] }))
        .await;
    let held = keys_of(&server, &bob, &general, "0000000000000000000000000000b0b0").await;
    assert!(!held.to_string().contains("forged"), "{held}");

    // The one who handed it over may hand it over again.
    let mut again = key(1, &bob_id, "0000000000000000000000000000b0b0", &owner_id);
    again["key"] = json!("again");
    server
        .put(&path, Some(&owner), json!({ "keys": [again] }))
        .await
        .ok();
    let held = keys_of(&server, &bob, &general, "0000000000000000000000000000b0b0").await;
    assert!(held.to_string().contains("again"), "{held}");
}

#[tokio::test]
async fn deleting_a_banned_members_messages_removes_their_ciphertext() {
    let server = server("keys-ban-purge").await;
    let (owner, owner_id, space) = server.owner().await;
    let (bob, bob_id) = server.member(&owner, &space, "bob").await;
    let general = server.channel_id(&space, "general");
    start_epoch(
        &server,
        &owner,
        &general,
        1,
        json!([key(1, &owner_id, "d", &owner_id)]),
    )
    .await;
    let path = format!("/channels/{general}/messages");
    server
        .post(
            &path,
            Some(&bob),
            json!({ "nonce": common::nonce(), "e2ee": envelope(Some(1)) }),
        )
        .await
        .ok();
    server
        .put(
            &format!("/spaces/{space}/bans/{bob_id}"),
            Some(&owner),
            json!({ "delete_messages_hours": 24 }),
        )
        .await
        .ok();
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE e2ee IS NOT NULL")
        .fetch_one(&server.state.db.read)
        .await
        .unwrap();
    assert_eq!(left, 0);
}
