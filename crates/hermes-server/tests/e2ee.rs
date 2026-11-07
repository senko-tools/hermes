//! End-to-end encryption, from the server's side: it holds public keys and opaque envelopes, keeps
//! plain DMs out, and shows that what it stores is not the message.

mod common;

use common::{TestServer, start_with};
use hermes_server::config::{DmPolicy, RegistrationMode};
use serde_json::{Value, json};

async fn server(name: &str) -> TestServer {
    start_with(name, RegistrationMode::Open, |state| {
        state.config.privacy.require_dm_encryption = true;
        state.config.privacy.dm_policy = DmPolicy::Anyone;
    })
    .await
}

fn device(n: u8) -> Value {
    json!({
        "device_id": format!("{n:032x}"),
        "sign_key": format!("sign{n}"),
        "dh_key": format!("dh{n}"),
        "signature": format!("sig{n}"),
    })
}

fn envelope(words: &str) -> Value {
    json!({
        "v": 1, "device": "00000000000000000000000000000001", "eph": "eph", "nonce": "nonce",
        "ciphertext": words, "keys": [{ "device": "00000000000000000000000000000002", "key": "k" }],
        "sig": "sig",
    })
}

/// Two friends with a DM between them.
async fn pair(server: &TestServer) -> (String, String, String, String) {
    let (owner, ..) = server.owner().await;
    let _ = owner;
    let (alice, _) = server.register("alice", None).await;
    let (bob, bob_id) = server.register("bob", None).await;
    let dm = server
        .post("/dms", Some(&alice), json!({ "recipient_ids": [bob_id] }))
        .await
        .ok();
    let dm = dm["id"].as_str().unwrap().to_owned();
    (alice, bob, bob_id, dm)
}

#[tokio::test]
async fn plain_direct_messages_are_refused_and_spaces_stay_plain() {
    let server = server("e2ee-refuses").await;
    let (alice, _, _, dm) = pair(&server).await;

    // A client from before encryption gets an error instead of leaking the message.
    let res = server
        .post(
            &format!("/channels/{dm}/messages"),
            Some(&alice),
            json!({ "content": "secret", "nonce": "n" }),
        )
        .await;
    res.err(400, "BAD_REQUEST");
    assert!(
        res.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("update Hermes")
    );

    // The words next to an envelope would defeat it.
    server
        .post(
            &format!("/channels/{dm}/messages"),
            Some(&alice),
            json!({ "content": "secret", "nonce": "n", "e2ee": envelope("x") }),
        )
        .await
        .err(400, "VALIDATION_FAILED");

    // A plain attachment without an envelope is a plain message too.
    let upload = server
        .upload(&alice, "a.txt", "text/plain", b"secret file".to_vec())
        .await
        .ok();
    server
        .post(
            &format!("/channels/{dm}/messages"),
            Some(&alice),
            json!({ "nonce": "n", "attachment_ids": [upload["id"]] }),
        )
        .await
        .err(400, "BAD_REQUEST");

    // Space channels are not encrypted, and do not take an envelope.
    let (owner, _, space) = (
        alice.clone(),
        (),
        server
            .post("/spaces", Some(&alice), json!({ "name": "A space" }))
            .await
            .ok(),
    );
    let space_id = space["space"]["id"].as_str().unwrap().to_owned();
    let general = server.channel_id(&space_id, "general");
    server
        .post(
            &format!("/channels/{general}/messages"),
            Some(&owner),
            json!({ "content": "plain", "nonce": "n" }),
        )
        .await
        .ok();
    server
        .post(
            &format!("/channels/{general}/messages"),
            Some(&owner),
            json!({ "nonce": "n", "e2ee": envelope("x") }),
        )
        .await
        .err(400, "VALIDATION_FAILED");
}

#[tokio::test]
async fn envelopes_are_stored_and_returned_as_they_came() {
    let server = server("e2ee-envelope").await;
    let (alice, bob, _, dm) = pair(&server).await;
    let (mut ws, _) = server.identify(&bob).await;

    let sent = server
        .post(
            &format!("/channels/{dm}/messages"),
            Some(&alice),
            json!({ "nonce": "n", "e2ee": envelope("opaque") }),
        )
        .await
        .ok();
    assert_eq!(sent["content"], "");
    assert_eq!(sent["e2ee"], envelope("opaque"));
    let event = ws.recv_op("message_create").await;
    assert_eq!(event["d"]["e2ee"], envelope("opaque"));
    assert_eq!(event["d"]["content"], "");
    let id = sent["id"].as_str().unwrap().to_owned();

    let history = server
        .get(&format!("/channels/{dm}/messages"), Some(&bob))
        .await
        .ok();
    assert_eq!(history[0]["e2ee"], envelope("opaque"));

    // Edits replace the envelope; plain edits are refused like plain messages.
    server
        .patch(
            &format!("/channels/{dm}/messages/{id}"),
            Some(&alice),
            json!({ "content": "plain edit" }),
        )
        .await
        .err(400, "BAD_REQUEST");
    let edited = server
        .patch(
            &format!("/channels/{dm}/messages/{id}"),
            Some(&alice),
            json!({ "e2ee": envelope("opaque2") }),
        )
        .await
        .ok();
    assert_eq!(edited["e2ee"], envelope("opaque2"));
    assert!(edited["edited_at"].is_number());

    // Deleting leaves nothing of it.
    server
        .delete(&format!("/channels/{dm}/messages/{id}"), Some(&alice))
        .await
        .ok();
    let history = server
        .get(&format!("/channels/{dm}/messages"), Some(&bob))
        .await
        .ok();
    assert_eq!(history[0]["deleted"], true);
    assert_eq!(history[0].get("e2ee"), None);
}

#[tokio::test]
async fn oversized_envelopes_are_refused() {
    let server = server("e2ee-large").await;
    let (alice, _, _, dm) = pair(&server).await;
    let huge = "x".repeat(hermes_proto::e2ee::MAX_ENVELOPE_BYTES + 1);
    server
        .post(
            &format!("/channels/{dm}/messages"),
            Some(&alice),
            json!({ "nonce": "n", "e2ee": envelope(&huge) }),
        )
        .await
        .err(413, "PAYLOAD_TOO_LARGE");
}

#[tokio::test]
async fn devices_are_published_read_and_revoked() {
    let server = server("e2ee-devices").await;
    let (alice, bob, _, _) = pair(&server).await;
    let alice_me = server.get("/users/@me", Some(&alice)).await.ok();
    let alice_id = alice_me["id"].as_str().unwrap().to_owned();
    let (mut bob_ws, _) = server.identify(&bob).await;

    // Malformed.
    let mut bad = device(1);
    bad["device_id"] = json!("not hex");
    server
        .put("/users/@me/devices", Some(&alice), bad)
        .await
        .err(400, "BAD_REQUEST");

    server
        .put("/users/@me/devices", Some(&alice), device(1))
        .await
        .ok();
    // Those who could need the key hear about it.
    let event = bob_ws.recv_op("devices_update").await;
    assert_eq!(event["d"]["user_id"], alice_id);
    assert_eq!(event["d"]["devices"][0]["sign_key"], "sign1");

    // Registering again, as on every connect, is silent.
    server
        .put("/users/@me/devices", Some(&alice), device(1))
        .await
        .ok();
    bob_ws.assert_silent(200).await;

    // A known ID cannot take other keys: that would be replacing someone's key.
    let mut other = device(1);
    other["sign_key"] = json!("attacker");
    server
        .put("/users/@me/devices", Some(&alice), other)
        .await
        .err(409, "CONFLICT");

    let found = server
        .post(
            "/users/devices",
            Some(&bob),
            json!({ "user_ids": [alice_id] }),
        )
        .await
        .ok();
    assert_eq!(found[0]["devices"][0]["device_id"], device(1)["device_id"]);
    assert_eq!(found[0]["devices"][0].get("revoked"), None);

    // Signing out revokes the session's device, but its key stays for checking old messages.
    server
        .post("/auth/logout", Some(&alice), json!({}))
        .await
        .ok();
    let event = bob_ws.recv_op("devices_update").await;
    assert_eq!(event["d"]["devices"][0]["revoked"], true);
    let found = server
        .post(
            "/users/devices",
            Some(&bob),
            json!({ "user_ids": [alice_id] }),
        )
        .await
        .ok();
    assert_eq!(found[0]["devices"][0]["revoked"], true);
    assert_eq!(found[0]["devices"][0]["sign_key"], "sign1");

    // Signing in on the same device brings it back.
    let token = server.login("alice", "correct horse").await.ok()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    server
        .put("/users/@me/devices", Some(&token), device(1))
        .await
        .ok();
    let found = server
        .post(
            "/users/devices",
            Some(&bob),
            json!({ "user_ids": [alice_id] }),
        )
        .await
        .ok();
    assert_eq!(found[0]["devices"][0].get("revoked"), None);
}
