//! Messages: send, history, edit, delete, pins, attachments, mentions, read states, slowmode,
//! typing.

mod common;

use common::{TestServer, start};
use hermes_proto::id::Id;
use hermes_proto::models::{Channel, ChannelKind, PresenceStatus};
use hermes_proto::permissions::Permissions;
use hermes_server::config::RegistrationMode;
use serde_json::{Value, json};

fn bits(p: Permissions) -> String {
    p.bits().to_string()
}

async fn send(server: &TestServer, token: &str, channel: &str, content: &str) -> Value {
    server
        .post(
            &format!("/channels/{channel}/messages"),
            Some(token),
            json!({ "content": content, "nonce": "n" }),
        )
        .await
        .ok()
}

fn id(message: &Value) -> String {
    message["id"].as_str().unwrap().to_owned()
}

/// `(last_read_id, mention_count)` of a user's read state, if any.
async fn read_state(server: &TestServer, user_id: &str, channel: &str) -> Option<(i64, i64)> {
    sqlx::query_as(
        "SELECT last_read_id, mention_count FROM read_states
         WHERE user_id = ? AND channel_id = ?",
    )
    .bind(user_id.parse::<i64>().unwrap())
    .bind(channel.parse::<i64>().unwrap())
    .fetch_optional(&server.state.db.read)
    .await
    .unwrap()
}

#[tokio::test]
async fn send_and_history() {
    let server = start("messages-send", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (stranger, _) = server.register("stranger", None).await;
    let general = server.channel_id(&space_id, "general");
    let path = format!("/channels/{general}/messages");

    server
        .post(
            &path,
            Some(&stranger),
            json!({ "content": "hi", "nonce": "n" }),
        )
        .await
        .err(404, "NOT_FOUND");
    let first = server
        .post(
            &path,
            Some(&alice),
            json!({ "content": " hello\n", "nonce": "abc-1" }),
        )
        .await
        .ok();
    assert_eq!(first["content"], " hello\n", "content is stored as sent");
    assert_eq!(first["author"]["id"], alice_id);
    assert_eq!(first["channel_id"], general);
    assert_eq!(first["nonce"], "abc-1");
    assert_eq!(first["kind"], "default");
    assert_eq!(first["deleted"], false);
    assert_eq!(first["edited_at"], json!(null));
    assert_eq!(first["reactions"], json!([]));
    let gid: Id = general.parse().unwrap();
    assert_eq!(
        server.state.cache().channels[&gid].channel.last_message_id,
        Some(id(&first).parse().unwrap())
    );

    for bad in [
        json!({ "content": "   ", "nonce": "n" }),
        json!({ "content": "hi", "nonce": "" }),
        json!({ "content": "hi", "nonce": "x".repeat(65) }),
        json!({ "content": "bell\u{7}", "nonce": "n" }),
        json!({ "content": "x".repeat(4001), "nonce": "n" }),
        json!({ "content": "hi", "nonce": "n", "attachment_ids": (1..=11).map(|i| i.to_string()).collect::<Vec<_>>() }),
        json!({ "content": "hi", "nonce": "n", "reply_to_id": "12345" }),
    ] {
        server
            .post(&path, Some(&alice), bad.clone())
            .await
            .err(400, "VALIDATION_FAILED");
    }
    server
        .post(&path, Some(&alice), json!({ "content": "no nonce" }))
        .await
        .err(400, "BAD_REQUEST");

    // Replies carry an excerpt of the first 100 characters.
    let long = "a".repeat(150);
    let target = send(&server, &alice, &general, &long).await;
    let reply = server
        .post(
            &path,
            Some(&owner),
            json!({ "content": "re", "nonce": "n", "reply_to_id": id(&target) }),
        )
        .await
        .ok();
    assert_eq!(reply["reply_to"]["id"], id(&target));
    assert_eq!(reply["reply_to"]["author_id"], alice_id);
    assert_eq!(reply["reply_to"]["excerpt"], "a".repeat(100));

    let mut ids = vec![id(&first), id(&target), id(&reply)];
    for i in 0..4 {
        ids.push(id(&send(&server, &owner, &general, &format!("m{i}")).await));
    }
    let history = |query: String| {
        let server = &server;
        let path = &path;
        let alice = &alice;
        async move {
            let list = server
                .get(&format!("{path}{query}"), Some(alice))
                .await
                .ok();
            list.as_array()
                .unwrap()
                .iter()
                .map(|m| m["id"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(history(String::new()).await, ids, "oldest first");
    assert_eq!(history("?limit=2".into()).await, ids[5..]);
    assert_eq!(
        history(format!("?before={}&limit=2", ids[3])).await,
        ids[1..3]
    );
    assert_eq!(
        history(format!("?after={}&limit=2", ids[3])).await,
        ids[4..6]
    );
    assert_eq!(
        history(format!("?around={}&limit=3", ids[3])).await,
        ids[2..5]
    );
    assert_eq!(
        history(format!("?after={}", ids[6])).await,
        Vec::<String>::new()
    );
    server
        .get(
            &format!("{path}?before={}&after={}", ids[1], ids[0]),
            Some(&alice),
        )
        .await
        .err(400, "BAD_REQUEST");

    let one = server
        .get(&format!("{path}/{}", ids[1]), Some(&alice))
        .await
        .ok();
    assert_eq!(one["content"], long);
    server
        .get(&format!("{path}/{}", Id(1)), Some(&alice))
        .await
        .err(404, "NOT_FOUND");

    // Categories have no messages.
    let category = server
        .post(
            &format!("/spaces/{space_id}/channels"),
            Some(&owner),
            json!({ "kind": "category", "name": "Cat" }),
        )
        .await
        .ok();
    server
        .post(
            &format!("/channels/{}/messages", id(&category)),
            Some(&owner),
            json!({ "content": "hi", "nonce": "n" }),
        )
        .await
        .err(400, "BAD_REQUEST");

    // READ_HISTORY and SEND_MESSAGES are checked separately.
    let everyone_override = format!("/channels/{general}/overrides/{space_id}");
    server
        .put(
            &everyone_override,
            Some(&owner),
            json!({ "allow": "0", "deny": bits(Permissions::READ_HISTORY) }),
        )
        .await
        .ok();
    server
        .get(&path, Some(&alice))
        .await
        .err(403, "MISSING_PERMISSION");
    send(&server, &alice, &general, "still allowed").await;
    server
        .put(
            &everyone_override,
            Some(&owner),
            json!({ "allow": "0", "deny": bits(Permissions::SEND_MESSAGES) }),
        )
        .await
        .ok();
    server
        .post(
            &path,
            Some(&alice),
            json!({ "content": "hi", "nonce": "n" }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
}

#[tokio::test]
async fn edit_delete_pin() {
    let server = start("messages-edit", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space_id, "bob").await;
    let general = server.channel_id(&space_id, "general");
    let path = format!("/channels/{general}/messages");

    let m1 = id(&send(&server, &alice, &general, "first").await);
    let m2 = id(&send(&server, &bob, &general, "second").await);

    // Edit: author only.
    server
        .patch(
            &format!("{path}/{m1}"),
            Some(&owner),
            json!({ "content": "x" }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
    let edited = server
        .patch(
            &format!("{path}/{m1}"),
            Some(&alice),
            json!({ "content": "first (edited)" }),
        )
        .await
        .ok();
    assert_eq!(edited["content"], "first (edited)");
    assert!(edited["edited_at"].as_i64().unwrap() > 0);
    server
        .patch(
            &format!("{path}/{m1}"),
            Some(&alice),
            json!({ "content": " " }),
        )
        .await
        .err(400, "VALIDATION_FAILED");

    // Pins: MANAGE_MESSAGES in spaces.
    server
        .put(&format!("{path}/{m1}/pin"), Some(&alice), json!(null))
        .await
        .err(403, "MISSING_PERMISSION");
    for m in [&m1, &m2, &m2] {
        server
            .put(&format!("{path}/{m}/pin"), Some(&owner), json!(null))
            .await
            .ok();
    }
    let pins = server
        .get(&format!("/channels/{general}/pins"), Some(&bob))
        .await
        .ok();
    let pinned: Vec<_> = pins.as_array().unwrap().iter().map(id).collect();
    assert_eq!(pinned, [m2.clone(), m1.clone()], "newest first");
    assert_eq!(pins[0]["pinned"], true);
    server
        .delete(&format!("{path}/{m2}/pin"), Some(&owner))
        .await
        .ok();
    let pins = server
        .get(&format!("/channels/{general}/pins"), Some(&bob))
        .await
        .ok();
    assert_eq!(pins.as_array().unwrap().len(), 1);

    // Delete: the author, or MANAGE_MESSAGES (audited).
    server
        .delete(&format!("{path}/{m1}"), Some(&bob))
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .delete(&format!("{path}/{m1}"), Some(&alice))
        .await
        .ok();
    server
        .delete(&format!("{path}/{m2}"), Some(&owner))
        .await
        .ok();
    let tombstone = server.get(&format!("{path}/{m1}"), Some(&bob)).await.ok();
    assert_eq!(tombstone["deleted"], true);
    assert_eq!(tombstone["content"], "");
    assert_eq!(tombstone["pinned"], false, "deleting unpins");
    let pins = server
        .get(&format!("/channels/{general}/pins"), Some(&bob))
        .await
        .ok();
    assert_eq!(pins, json!([]));
    server
        .delete(&format!("{path}/{m1}"), Some(&alice))
        .await
        .err(404, "NOT_FOUND");
    server
        .patch(
            &format!("{path}/{m1}"),
            Some(&alice),
            json!({ "content": "back" }),
        )
        .await
        .err(404, "NOT_FOUND");
    server
        .put(&format!("{path}/{m1}/pin"), Some(&owner), json!(null))
        .await
        .err(404, "NOT_FOUND");
    // Replies to deleted messages keep the reference with an empty excerpt.
    let reply = server
        .post(
            &path,
            Some(&bob),
            json!({ "content": "what?", "nonce": "n", "reply_to_id": m1 }),
        )
        .await
        .ok();
    assert_eq!(reply["reply_to"]["excerpt"], "");

    let log = server
        .get(&format!("/spaces/{space_id}/audit-log"), Some(&owner))
        .await
        .ok();
    assert_eq!(log[0]["action"], "message_delete");
    assert_eq!(log[0]["target_id"], m2);
    assert_eq!(
        log[0]["details"],
        json!({ "channel_id": general, "author_id": bob_id })
    );
    assert_eq!(
        log.as_array().unwrap().len(),
        1,
        "own deletes are not audited"
    );

    // In a DM, any recipient may pin, nobody may delete others' messages.
    let dm = Id(777);
    server
        .sql(&format!(
            "INSERT INTO channels (id, kind, created_at) VALUES ({dm}, 'dm', 0)"
        ))
        .await;
    server
        .sql(&format!(
            "INSERT INTO dm_recipients (channel_id, user_id) VALUES ({dm}, {alice_id}), ({dm}, {bob_id})"
        ))
        .await;
    server.state.cache_mut().insert_channel(
        Channel {
            id: dm,
            space_id: None,
            kind: ChannelKind::Dm,
            parent_id: None,
            name: None,
            topic: None,
            position: 0,
            bitrate: None,
            user_limit: None,
            slowmode_secs: 0,
            overrides: Vec::new(),
            recipients: Vec::new(),
            owner_id: None,
            last_message_id: None,
            request: false,
        },
        vec![alice_id.parse().unwrap(), bob_id.parse().unwrap()],
    );
    let dm_message = id(&send(&server, &alice, &dm.to_string(), "psst").await);
    server
        .put(
            &format!("/channels/{dm}/messages/{dm_message}/pin"),
            Some(&bob),
            json!(null),
        )
        .await
        .ok();
    server
        .delete(&format!("/channels/{dm}/messages/{dm_message}"), Some(&bob))
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .get(&format!("/channels/{dm}/messages"), Some(&owner))
        .await
        .err(404, "NOT_FOUND");
}

#[tokio::test]
async fn attachments() {
    let server = start("messages-attachments", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (_, bob_id) = server.member(&owner, &space_id, "bob").await;
    let general = server.channel_id(&space_id, "general");
    let path = format!("/channels/{general}/messages");
    server
        .sql(&format!(
            "INSERT INTO attachments (id, uploader_id, filename, content_type, size, width, height, sha256, created_at)
             VALUES (50, {alice_id}, 'cat.png', 'image/png', 1234, 64, 48, x'00', 0),
                    (51, {bob_id}, 'dog.png', 'image/png', 99, NULL, NULL, x'00', 0),
                    (52, {alice_id}, 'notes.txt', 'text/plain', 5, NULL, NULL, x'00', 0)"
        ))
        .await;

    let message = server
        .post(
            &path,
            Some(&alice),
            json!({ "content": "", "nonce": "n", "attachment_ids": ["50", "50"] }),
        )
        .await
        .ok();
    assert_eq!(
        message["attachments"],
        json!([{
            "id": "50", "filename": "cat.png", "content_type": "image/png", "size": 1234,
            "width": 64, "height": 48, "has_thumb": false
        }])
    );
    let message_id = id(&message);
    // Already attached, someone else's, or unknown.
    for attachment in ["50", "51", "99"] {
        server
            .post(
                &path,
                Some(&alice),
                json!({ "content": "x", "nonce": "n", "attachment_ids": [attachment] }),
            )
            .await
            .err(400, "VALIDATION_FAILED");
    }
    // A failed send leaves nothing behind.
    let history = server.get(&path, Some(&alice)).await.ok();
    assert_eq!(history.as_array().unwrap().len(), 1);

    // An empty edit is fine while there are attachments.
    server
        .patch(
            &format!("{path}/{message_id}"),
            Some(&alice),
            json!({ "content": "" }),
        )
        .await
        .ok();

    server
        .put(
            &format!("/channels/{general}/overrides/{space_id}"),
            Some(&owner),
            json!({ "allow": "0", "deny": bits(Permissions::ATTACH_FILES) }),
        )
        .await
        .ok();
    server
        .post(
            &path,
            Some(&alice),
            json!({ "content": "x", "nonce": "n", "attachment_ids": ["52"] }),
        )
        .await
        .err(403, "MISSING_PERMISSION");

    // Deleting unlinks the attachments.
    server
        .delete(&format!("{path}/{message_id}"), Some(&alice))
        .await
        .ok();
    let (linked,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM attachments WHERE message_id IS NOT NULL")
            .fetch_one(&server.state.db.read)
            .await
            .unwrap();
    assert_eq!(linked, 0);
}

#[tokio::test]
async fn mentions_and_read_states() {
    let server = start("messages-mentions", RegistrationMode::Open).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space_id, "bob").await;
    let (_, carol_id) = server.member(&owner, &space_id, "carol").await;
    let general = server.channel_id(&space_id, "general");
    let team = server.role(&owner, &space_id, "team", 0).await;
    let fans = server.role(&owner, &space_id, "fans", 0).await;
    server
        .patch(
            &format!("/spaces/{space_id}/roles/{fans}"),
            Some(&owner),
            json!({ "mentionable": true }),
        )
        .await
        .ok();
    server.assign(&owner, &space_id, &bob_id, &[&team]).await;
    server.assign(&owner, &space_id, &carol_id, &[&fans]).await;

    // Without MENTION_EVERYONE: no @everyone, no non-mentionable roles; unknown users dropped.
    let m = send(
        &server,
        &alice,
        &general,
        &format!(
            "<@{bob_id}> @everyone <@&{team}> <@&{fans}> <@{}> `<@{carol_id}>`",
            Id(999)
        ),
    )
    .await;
    assert_eq!(
        m["mentions"],
        json!({ "users": [bob_id], "roles": [fans], "everyone": false, "here": false })
    );
    assert_eq!(read_state(&server, &bob_id, &general).await, Some((0, 1)));
    assert_eq!(read_state(&server, &carol_id, &general).await, Some((0, 1)));
    assert_eq!(read_state(&server, &owner_id, &general).await, None);
    assert_eq!(
        read_state(&server, &alice_id, &general).await,
        None,
        "not the author"
    );
    // Stored mentions come back in history.
    let history = server
        .get(&format!("/channels/{general}/messages"), Some(&bob))
        .await
        .ok();
    assert_eq!(history[0]["mentions"], m["mentions"]);

    // The owner may mention everyone; the author is not counted.
    let everyone = send(&server, &owner, &general, "@everyone hi").await;
    assert_eq!(everyone["mentions"]["everyone"], true);
    for (user, count) in [(&alice_id, 1), (&bob_id, 2), (&carol_id, 2)] {
        assert_eq!(read_state(&server, user, &general).await.unwrap().1, count);
    }
    assert_eq!(read_state(&server, &owner_id, &general).await, None);

    // @here only counts online users.
    server
        .state
        .cache_mut()
        .presences
        .insert(carol_id.parse().unwrap(), PresenceStatus::Online);
    server
        .state
        .cache_mut()
        .presences
        .insert(bob_id.parse().unwrap(), PresenceStatus::Invisible);
    send(&server, &owner, &general, "@here").await;
    assert_eq!(read_state(&server, &carol_id, &general).await.unwrap().1, 3);
    assert_eq!(read_state(&server, &bob_id, &general).await.unwrap().1, 2);

    // Users who cannot see the channel are not counted.
    let secret = server
        .post(
            &format!("/spaces/{space_id}/channels"),
            Some(&owner),
            json!({ "kind": "text", "name": "secret" }),
        )
        .await
        .ok();
    let secret = id(&secret);
    server
        .put(
            &format!("/channels/{secret}/overrides/{team}"),
            Some(&owner),
            json!({ "allow": "0", "deny": bits(Permissions::VIEW_CHANNEL) }),
        )
        .await
        .ok();
    send(
        &server,
        &alice,
        &secret,
        &format!("<@{bob_id}> <@{carol_id}>"),
    )
    .await;
    assert_eq!(read_state(&server, &bob_id, &secret).await, None);
    assert_eq!(read_state(&server, &carol_id, &secret).await, Some((0, 1)));

    // Ack resets the count; it may move backwards; the message must be in the channel.
    let ack = format!("/channels/{general}/ack");
    server
        .post(&ack, Some(&bob), json!({ "message_id": id(&everyone) }))
        .await
        .ok();
    let everyone_id: i64 = id(&everyone).parse().unwrap();
    assert_eq!(
        read_state(&server, &bob_id, &general).await,
        Some((everyone_id, 0))
    );
    server
        .post(&ack, Some(&bob), json!({ "message_id": id(&m) }))
        .await
        .ok();
    assert_eq!(
        read_state(&server, &bob_id, &general).await.unwrap().0,
        id(&m).parse::<i64>().unwrap()
    );
    server
        .post(&ack, Some(&bob), json!({ "message_id": secret }))
        .await
        .err(404, "NOT_FOUND");

    // A mention newer than the read position counts; editing does not count again.
    let late = send(&server, &alice, &general, &format!("<@{bob_id}>")).await;
    assert_eq!(read_state(&server, &bob_id, &general).await.unwrap().1, 1);
    let edited = server
        .patch(
            &format!("/channels/{general}/messages/{}", id(&late)),
            Some(&alice),
            json!({ "content": format!("<@{carol_id}>") }),
        )
        .await
        .ok();
    assert_eq!(edited["mentions"]["users"], json!([carol_id]));
    assert_eq!(read_state(&server, &carol_id, &general).await.unwrap().1, 3);
}

#[tokio::test]
async fn slowmode_and_typing() {
    let server = start("messages-slowmode", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let (stranger, _) = server.register("stranger", None).await;
    let general = server.channel_id(&space_id, "general");
    let voice = server.channel_id(&space_id, "General");
    server
        .patch(
            &format!("/channels/{general}"),
            Some(&owner),
            json!({ "slowmode_secs": 60 }),
        )
        .await
        .ok();

    send(&server, &alice, &general, "one").await;
    let res = server
        .post(
            &format!("/channels/{general}/messages"),
            Some(&alice),
            json!({ "content": "two", "nonce": "n" }),
        )
        .await;
    res.err(429, "RATE_LIMITED");
    let wait = res.body["error"]["retry_after_ms"].as_u64().unwrap();
    assert!((1..=60_000).contains(&wait), "{wait}");
    // Other channels (voice channels have a text chat too) and BYPASS_SLOWMODE are unaffected.
    send(&server, &alice, &voice, "elsewhere").await;
    send(&server, &owner, &general, "owner 1").await;
    send(&server, &owner, &general, "owner 2").await;

    let typing = format!("/channels/{general}/typing");
    server.post(&typing, Some(&alice), json!(null)).await.ok();
    server
        .post(&typing, Some(&stranger), json!(null))
        .await
        .err(404, "NOT_FOUND");
    server
        .put(
            &format!("/channels/{general}/overrides/{space_id}"),
            Some(&owner),
            json!({ "allow": "0", "deny": bits(Permissions::SEND_MESSAGES) }),
        )
        .await
        .ok();
    server
        .post(&typing, Some(&alice), json!(null))
        .await
        .err(403, "MISSING_PERMISSION");
}
