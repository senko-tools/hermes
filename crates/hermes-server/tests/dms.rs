//! DMs and group DMs (PLAN.md sections 8.2 and 10): `/dms`, recipients, renaming and leaving
//! through `/channels/{id}`, `dm_policy`, and their gateway events.

mod common;

use common::{TestServer, start, start_with};
use hermes_server::config::{DmPolicy, RegistrationMode};
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

fn ids(channel: &Value) -> Vec<&str> {
    channel["recipients"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["id"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn one_to_one_dms() {
    let server = start("dm-direct", RegistrationMode::Open).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (stranger, _) = server.register("stranger", None).await;
    let (mut ows, _) = server.identify(&owner).await;
    let (mut aws, _) = server.identify(&alice).await;

    let dm = server
        .post("/dms", Some(&owner), json!({ "recipient_ids": [alice_id] }))
        .await
        .ok();
    let dm_id = dm["id"].as_str().unwrap().to_owned();
    assert_eq!(dm["kind"], "dm");
    assert_eq!(dm["space_id"], Value::Null);
    assert_eq!(dm["owner_id"], Value::Null);
    let mut expected = vec![owner_id.as_str(), alice_id.as_str()];
    expected.sort_by_key(|id| id.parse::<u64>().unwrap());
    assert_eq!(ids(&dm), expected);
    for ws in [&mut ows, &mut aws] {
        let created = ws.next_event().await;
        assert_eq!(created["op"], "dm_create");
        assert_eq!(created["d"]["id"], dm_id);
    }

    // Asking again (from either side, listing yourself too) reuses the DM.
    let again = server
        .post(
            "/dms",
            Some(&alice),
            json!({ "recipient_ids": [owner_id, alice_id] }),
        )
        .await
        .ok();
    assert_eq!(again["id"], dm_id);

    // Messages reach the other recipient; strangers cannot see the DM.
    send_message(&server, &owner, &dm_id, "hi").await;
    let message = aws.next_event().await;
    assert_eq!(message["op"], "message_create");
    assert_eq!(message["d"]["channel_id"], dm_id);
    server
        .get(&format!("/channels/{dm_id}/messages"), Some(&stranger))
        .await
        .err(404, "NOT_FOUND");

    // Pins are allowed for any recipient.
    let mid = message["d"]["id"].as_str().unwrap();
    server
        .put(
            &format!("/channels/{dm_id}/messages/{mid}/pin"),
            Some(&alice),
            json!({}),
        )
        .await
        .ok();

    let list = server.get("/dms", Some(&alice)).await.ok();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["last_message_id"], mid);
    let (_ws, ready) = server.identify(&alice).await;
    assert_eq!(ready["dms"][0]["id"], dm_id);
    assert_eq!(ready["dms"][0]["recipients"].as_array().unwrap().len(), 2);

    // A 1:1 DM cannot be left, renamed or extended.
    server
        .delete(&format!("/channels/{dm_id}"), Some(&alice))
        .await
        .err(400, "BAD_REQUEST");
    server
        .patch(
            &format!("/channels/{dm_id}"),
            Some(&alice),
            json!({ "name": "us" }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
    let (_, carol_id) = server.member(&owner, &space_id, "carol").await;
    server
        .put(
            &format!("/dms/{dm_id}/recipients/{carol_id}"),
            Some(&alice),
            json!({}),
        )
        .await
        .err(400, "BAD_REQUEST");
}

#[tokio::test]
async fn validation_and_dm_policy() {
    let server = start("dm-policy", RegistrationMode::Open).await;
    let (owner, owner_id, _) = server.owner().await;
    let (stranger, _) = server.register("stranger", None).await;

    server
        .post("/dms", Some(&owner), json!({ "recipient_ids": [owner_id] }))
        .await
        .err(400, "VALIDATION_FAILED");
    server
        .post("/dms", Some(&owner), json!({ "recipient_ids": ["12345"] }))
        .await
        .err(400, "VALIDATION_FAILED");
    // `shared_space` (the default): no common space, no DM.
    server
        .post(
            "/dms",
            Some(&stranger),
            json!({ "recipient_ids": [owner_id] }),
        )
        .await
        .err(403, "MISSING_PERMISSION");

    let server = start_with("dm-policy-anyone", RegistrationMode::Open, |state| {
        state.config.privacy.dm_policy = DmPolicy::Anyone;
    })
    .await;
    let (_, owner_id, _) = server.owner().await;
    let (stranger, _) = server.register("stranger", None).await;
    server
        .post(
            "/dms",
            Some(&stranger),
            json!({ "recipient_ids": [owner_id] }),
        )
        .await
        .ok();
}

#[tokio::test]
async fn group_dms() {
    let server = start_with("dm-group", RegistrationMode::InviteOnly, |state| {
        state.config.limits.max_group_dm_size = 4;
    })
    .await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space_id, "bob").await;
    let (carol, carol_id) = server.member(&owner, &space_id, "carol").await;
    let (_, dave_id) = server.member(&owner, &space_id, "dave").await;
    let (mut aws, _) = server.identify(&alice).await;
    let (mut bws, _) = server.identify(&bob).await;
    let (mut cws, _) = server.identify(&carol).await;

    // At most four people, including the creator.
    server
        .post(
            "/dms",
            Some(&owner),
            json!({ "recipient_ids": [alice_id, bob_id, carol_id, dave_id] }),
        )
        .await
        .err(400, "BAD_REQUEST");
    let group = server
        .post(
            "/dms",
            Some(&owner),
            json!({ "recipient_ids": [alice_id, bob_id] }),
        )
        .await
        .ok();
    let gid = group["id"].as_str().unwrap().to_owned();
    assert_eq!(group["kind"], "group_dm");
    assert_eq!(group["owner_id"], owner_id);
    assert_eq!(ids(&group).len(), 3);
    assert_eq!(aws.next_event().await["op"], "dm_create");
    assert_eq!(bws.next_event().await["op"], "dm_create");
    // A second group with the same people is a new channel.
    let other = server
        .post(
            "/dms",
            Some(&owner),
            json!({ "recipient_ids": [alice_id, bob_id] }),
        )
        .await
        .ok();
    assert_ne!(other["id"], gid);
    assert_eq!(aws.next_event().await["op"], "dm_create");
    assert_eq!(bws.next_event().await["op"], "dm_create");

    // Any recipient may add people (up to the limit); the newcomer gets dm_create, the others
    // dm_update.
    let recipients = format!("/dms/{gid}/recipients");
    server
        .put(&format!("{recipients}/{carol_id}"), Some(&alice), json!({}))
        .await
        .ok();
    let created = cws.next_event().await;
    assert_eq!(created["op"], "dm_create");
    assert_eq!(ids(&created["d"]).len(), 4);
    let update = bws.next_event().await;
    assert_eq!(update["op"], "dm_update");
    assert_eq!(ids(&update["d"]).len(), 4);
    assert_eq!(aws.next_event().await["op"], "dm_update");
    server
        .put(&format!("{recipients}/{dave_id}"), Some(&alice), json!({}))
        .await
        .err(400, "BAD_REQUEST");
    // Adding someone already there is fine.
    server
        .put(&format!("{recipients}/{carol_id}"), Some(&bob), json!({}))
        .await
        .ok();

    // Any recipient may rename it; group DMs have nothing else to change.
    let renamed = server
        .patch(
            &format!("/channels/{gid}"),
            Some(&bob),
            json!({ "name": "  pumpkins " }),
        )
        .await
        .ok();
    assert_eq!(renamed["name"], "pumpkins");
    let update = aws.next_event().await;
    assert_eq!(
        (update["op"].as_str(), update["d"]["name"].as_str()),
        (Some("dm_update"), Some("pumpkins"))
    );
    server
        .patch(
            &format!("/channels/{gid}"),
            Some(&bob),
            json!({ "topic": "x" }),
        )
        .await
        .err(400, "VALIDATION_FAILED");

    // Only the owner removes others.
    server
        .delete(&format!("{recipients}/{bob_id}"), Some(&alice))
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .delete(&format!("{recipients}/{bob_id}"), Some(&owner))
        .await
        .ok();
    assert_eq!(bws.recv_op("dm_delete").await["d"], json!({ "id": gid }));
    let update = aws.recv_op("dm_update").await;
    assert_eq!(ids(&update["d"]).len(), 3);
    server
        .post(
            &format!("/channels/{gid}/messages"),
            Some(&bob),
            json!({ "content": "let me in", "nonce": "n" }),
        )
        .await
        .err(404, "NOT_FOUND");
    send_message(&server, &carol, &gid, "bye bob").await;
    assert_eq!(
        aws.recv_op("message_create").await["d"]["content"],
        "bye bob"
    );
    bws.assert_silent(300).await;
}

#[tokio::test]
async fn leaving_group_dms() {
    let server = start("dm-leave", RegistrationMode::InviteOnly).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space_id, "bob").await;
    let (mut aws, _) = server.identify(&alice).await;
    let group = server
        .post(
            "/dms",
            Some(&owner),
            json!({ "recipient_ids": [alice_id, bob_id] }),
        )
        .await
        .ok();
    let gid = group["id"].as_str().unwrap().to_owned();
    assert_eq!(aws.next_event().await["op"], "dm_create");

    // Removed recipients lose access.
    server
        .delete(&format!("/dms/{gid}/recipients/@me"), Some(&bob))
        .await
        .ok();
    server
        .post(
            &format!("/channels/{gid}/messages"),
            Some(&bob),
            json!({ "content": "hi", "nonce": "n" }),
        )
        .await
        .err(404, "NOT_FOUND");
    assert_eq!(aws.next_event().await["op"], "dm_update");

    // When the owner leaves, the remaining recipient with the lowest ID takes over.
    server
        .delete(&format!("/channels/{gid}"), Some(&owner))
        .await
        .ok();
    let update = aws.next_event().await;
    assert_eq!(update["op"], "dm_update");
    assert_eq!(update["d"]["owner_id"], alice_id);
    assert_eq!(ids(&update["d"]), [alice_id.as_str()]);

    // The last one out deletes it.
    server
        .delete(&format!("/channels/{gid}"), Some(&alice))
        .await
        .ok();
    assert_eq!(aws.next_event().await["op"], "dm_delete");
    let gid = gid.parse().unwrap();
    assert!(!server.state.cache().channels.contains_key(&gid));
}

#[tokio::test]
async fn closing_and_reopening_a_dm() {
    let server = start("dm-close", RegistrationMode::Open).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space_id, "bob").await;
    let dm = server
        .post("/dms", Some(&owner), json!({ "recipient_ids": [alice_id] }))
        .await
        .ok();
    let dm_id = dm["id"].as_str().unwrap().to_owned();
    let (mut aws, _) = server.identify(&alice).await;

    // Strangers and spaces cannot be closed; group DMs are left instead.
    server
        .delete(&format!("/dms/{dm_id}"), Some(&bob))
        .await
        .err(404, "NOT_FOUND");
    let group = server
        .post(
            "/dms",
            Some(&owner),
            json!({ "recipient_ids": [alice_id, bob_id] }),
        )
        .await
        .ok();
    assert_eq!(aws.recv_op("dm_create").await["d"]["id"], group["id"]);
    server
        .delete(
            &format!("/dms/{}", group["id"].as_str().unwrap()),
            Some(&alice),
        )
        .await
        .err(400, "BAD_REQUEST");

    // Closing hides it from Alice only, everywhere she lists DMs.
    server
        .delete(&format!("/dms/{dm_id}"), Some(&alice))
        .await
        .ok();
    let closed = aws.next_event().await;
    assert_eq!(closed["op"], "dm_delete");
    assert_eq!(closed["d"]["id"], dm_id);
    server
        .delete(&format!("/dms/{dm_id}"), Some(&alice))
        .await
        .ok();
    let listed = |list: &Value| -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert!(!listed(&server.get("/dms", Some(&alice)).await.ok()).contains(&dm_id));
    let (_ws, ready) = server.identify(&alice).await;
    assert!(!listed(&ready["dms"]).contains(&dm_id));
    assert!(listed(&server.get("/dms", Some(&owner)).await.ok()).contains(&dm_id));

    // A new message reopens it: dm_create arrives before the message.
    send_message(&server, &owner, &dm_id, "still there?").await;
    let reopened = aws.next_event().await;
    assert_eq!(reopened["op"], "dm_create");
    assert_eq!(reopened["d"]["id"], dm_id);
    assert_eq!(aws.next_event().await["op"], "message_create");
    assert!(listed(&server.get("/dms", Some(&alice)).await.ok()).contains(&dm_id));

    // Messaging the other person again reopens it for the caller.
    server
        .delete(&format!("/dms/{dm_id}"), Some(&alice))
        .await
        .ok();
    assert_eq!(aws.next_event().await["op"], "dm_delete");
    let again = server
        .post("/dms", Some(&alice), json!({ "recipient_ids": [owner_id] }))
        .await
        .ok();
    assert_eq!(again["id"], dm_id);
    assert_eq!(aws.next_event().await["op"], "dm_create");
    assert!(listed(&server.get("/dms", Some(&alice)).await.ok()).contains(&dm_id));
}

#[tokio::test]
async fn privacy_settings_and_message_requests() {
    let server = start_with("dm-privacy", RegistrationMode::Open, |state| {
        state.config.privacy.dm_policy = DmPolicy::Anyone;
    })
    .await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space_id, "bob").await;
    let (stranger, _) = server.register("stranger", None).await;
    let (mut aws, ready) = server.identify(&alice).await;
    assert_eq!(
        ready["privacy"],
        json!({ "dm_from": "everyone", "message_requests": true })
    );

    // Only people in her spaces reach Alice directly; everyone else becomes a request.
    let privacy = server
        .patch(
            "/users/@me/privacy",
            Some(&alice),
            json!({ "dm_from": "spaces" }),
        )
        .await
        .ok();
    assert_eq!(
        privacy,
        json!({ "dm_from": "spaces", "message_requests": true })
    );
    assert_eq!(aws.next_event().await["op"], "privacy_update");
    assert_eq!(
        server.get("/users/@me/privacy", Some(&alice)).await.ok(),
        privacy
    );
    server
        .patch(
            "/users/@me/privacy",
            Some(&alice),
            json!({ "dm_from": "sometimes" }),
        )
        .await
        .err(400, "BAD_REQUEST");

    let direct = server
        .post("/dms", Some(&bob), json!({ "recipient_ids": [alice_id] }))
        .await
        .ok();
    let created = aws.recv_op("dm_create").await;
    assert_eq!(created["d"]["id"], direct["id"]);
    assert_eq!(created["d"].get("request"), None, "{created}");

    let dm = server
        .post(
            "/dms",
            Some(&stranger),
            json!({ "recipient_ids": [alice_id] }),
        )
        .await
        .ok();
    let dm_id = dm["id"].as_str().unwrap().to_owned();
    // The sender sees a normal DM; Alice sees a request, also in her list and in ready.
    assert_eq!(dm.get("request"), None);
    let created = aws.recv_op("dm_create").await;
    assert_eq!(created["d"]["id"], dm_id);
    assert_eq!(created["d"]["request"], true);
    let list = server.get("/dms", Some(&alice)).await.ok();
    let listed = list
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == dm_id.as_str())
        .unwrap();
    assert_eq!(listed["request"], true);
    let (_ws, ready) = server.identify(&alice).await;
    let in_ready = ready["dms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == dm_id.as_str())
        .unwrap();
    assert_eq!(in_ready["request"], true);
    // A request shares no presence with the sender.
    let (_sws, sready) = server.identify(&stranger).await;
    assert!(
        !sready["presences"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["user_id"] == alice_id.as_str()),
        "{sready}"
    );

    // Messages still arrive, but a call does not ring.
    send_message(&server, &stranger, &dm_id, "hello?").await;
    assert_eq!(aws.next_event().await["op"], "message_create");
    server
        .post(
            &format!("/channels/{dm_id}/call/ring"),
            Some(&stranger),
            json!({}),
        )
        .await
        .ok();
    aws.assert_silent(300).await;

    // Ignoring closes the request for good: more messages do not bring it back.
    server
        .delete(&format!("/dms/{dm_id}"), Some(&alice))
        .await
        .ok();
    assert_eq!(aws.next_event().await["op"], "dm_delete");
    send_message(&server, &stranger, &dm_id, "please").await;
    aws.assert_silent(300).await;
    assert!(
        !server
            .get("/dms", Some(&alice))
            .await
            .ok()
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == dm_id.as_str())
    );

    // Accepting brings it back as a normal DM.
    let accepted = server
        .post(&format!("/dms/{dm_id}/accept"), Some(&alice), json!({}))
        .await
        .ok();
    assert_eq!(accepted.get("request"), None);
    let back = aws.next_event().await;
    assert_eq!(back["op"], "dm_create");
    assert_eq!(back["d"].get("request"), None);
    server
        .post(&format!("/dms/{dm_id}/accept"), Some(&bob), json!({}))
        .await
        .err(404, "NOT_FOUND");

    // Nobody, without requests: new DMs to Alice are refused, also as part of a group.
    server
        .patch(
            "/users/@me/privacy",
            Some(&alice),
            json!({ "dm_from": "nobody", "message_requests": false }),
        )
        .await
        .ok();
    let (carol, _) = server.member(&owner, &space_id, "carol").await;
    server
        .post("/dms", Some(&carol), json!({ "recipient_ids": [alice_id] }))
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .post(
            "/dms",
            Some(&carol),
            json!({ "recipient_ids": [alice_id, bob_id] }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
    // Existing DMs keep working.
    send_message(&server, &bob, direct["id"].as_str().unwrap(), "still ok").await;

    // Answering a request with a message accepts it.
    server
        .patch(
            "/users/@me/privacy",
            Some(&alice),
            json!({ "message_requests": true }),
        )
        .await
        .ok();
    let (dave, _) = server.register("dave", None).await;
    let dave_dm = server
        .post("/dms", Some(&dave), json!({ "recipient_ids": [alice_id] }))
        .await
        .ok();
    let dave_dm_id = dave_dm["id"].as_str().unwrap();
    send_message(&server, &alice, dave_dm_id, "hi dave").await;
    let list = server.get("/dms", Some(&alice)).await.ok();
    let listed = list
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == dave_dm_id)
        .unwrap();
    assert_eq!(listed.get("request"), None, "{listed}");
}
