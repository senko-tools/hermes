//! Consistency of the HTTP API under concurrent requests (a delete racing an edit, a ban racing
//! a join) and of message payloads built for a page of history.

use crate::common;

use common::start;
use hermes_proto::id::Id;
use hermes_server::config::RegistrationMode;
use serde_json::{Value, json};

/// Every part of a message (attachments, reactions, mentions, reply) belongs to that message
/// only, and replies to the same message all carry it.
#[tokio::test]
async fn history_keeps_each_message_parts_apart() {
    let server = start("consistency-wire", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space_id, "bob").await;
    let general = server.channel_id(&space_id, "general");
    let path = format!("/channels/{general}/messages");
    server
        .sql(&format!(
            "INSERT INTO attachments (id, uploader_id, filename, content_type, size, sha256, created_at)
             VALUES (60, {alice_id}, 'a.txt', 'text/plain', 1, x'00', 0),
                    (61, {alice_id}, 'b.txt', 'text/plain', 2, x'00', 0),
                    (62, {bob_id}, 'c.txt', 'text/plain', 3, x'00', 0)"
        ))
        .await;
    let send = |token: &str, body: Value| {
        let (path, token) = (path.clone(), token.to_owned());
        let server = &server;
        async move { server.post(&path, Some(&token), body).await.ok() }
    };

    let first = send(
        &alice,
        json!({ "content": "files", "nonce": common::nonce(), "attachment_ids": ["61", "60"] }),
    )
    .await;
    let first_id = first["id"].as_str().unwrap().to_owned();
    let second = send(
        &bob,
        json!({ "content": format!("<@{alice_id}>"), "nonce": common::nonce(), "reply_to_id": first_id,
                "attachment_ids": ["62"] }),
    )
    .await;
    let third = send(
        &alice,
        json!({ "content": "me too", "nonce": common::nonce(), "reply_to_id": first_id }),
    )
    .await;
    for (message, emoji, token) in [
        (&first, "🎃", &alice),
        (&first, "👻", &bob),
        (&second, "👻", &alice),
    ] {
        let mid = message["id"].as_str().unwrap();
        server
            .put(
                &format!("{path}/{mid}/reactions/{emoji}"),
                Some(token),
                json!(null),
            )
            .await
            .ok();
    }

    let history = server.get(&path, Some(&alice)).await.ok();
    let history = history.as_array().unwrap();
    assert_eq!(history.len(), 3);
    let files = |m: &Value| -> Vec<String> {
        m["attachments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["filename"].as_str().unwrap().to_owned())
            .collect()
    };
    let emoji = |m: &Value| -> Vec<(String, bool)> {
        m["reactions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["emoji"].as_str().unwrap().to_owned(),
                    r["me"].as_bool().unwrap(),
                )
            })
            .collect()
    };
    assert_eq!(files(&history[0]), ["a.txt", "b.txt"], "upload order");
    assert_eq!(files(&history[1]), ["c.txt"]);
    assert!(files(&history[2]).is_empty());
    assert_eq!(
        emoji(&history[0]),
        [("🎃".to_owned(), true), ("👻".to_owned(), false)]
    );
    assert_eq!(emoji(&history[1]), [("👻".to_owned(), true)]);
    assert!(emoji(&history[2]).is_empty());
    assert_eq!(history[0]["mentions"]["users"], json!([]));
    assert_eq!(history[1]["mentions"]["users"], json!([alice_id]));
    assert_eq!(history[2]["mentions"]["users"], json!([]));
    assert_eq!(history[0]["reply_to"], Value::Null);
    for reply in [&history[1], &history[2]] {
        assert_eq!(reply["reply_to"]["id"], json!(first_id));
        assert_eq!(reply["reply_to"]["author_id"], json!(alice_id));
        assert_eq!(reply["reply_to"]["excerpt"], "files");
    }
    assert_eq!(history[2]["id"], third["id"]);
}

/// A reaction on a deleted message is refused like one on an unknown message.
#[tokio::test]
async fn no_reactions_on_deleted_messages() {
    let server = start("consistency-react", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let general = server.channel_id(&space_id, "general");
    let path = format!("/channels/{general}/messages");
    let m = server
        .post(
            &path,
            Some(&owner),
            json!({ "content": "hi", "nonce": common::nonce() }),
        )
        .await
        .ok();
    let mid = m["id"].as_str().unwrap();
    server
        .delete(&format!("{path}/{mid}"), Some(&owner))
        .await
        .ok();
    server
        .put(
            &format!("{path}/{mid}/reactions/🎃"),
            Some(&owner),
            json!(null),
        )
        .await
        .err(404, "NOT_FOUND");
}

/// A moderator deleting messages while their author edits them: every message ends up deleted
/// and without content, whichever request wins.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_racing_an_edit_stays_deleted() {
    let server = start("consistency-edit", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "general");
    let path = format!("/channels/{general}/messages");
    let mut ids = Vec::new();
    for i in 0..20 {
        let m = server
            .post(
                &path,
                Some(&alice),
                json!({ "content": format!("secret {i}"), "nonce": common::nonce() }),
            )
            .await
            .ok();
        ids.push(m["id"].as_str().unwrap().to_owned());
    }
    for id in &ids {
        let message = format!("{path}/{id}");
        let (edit, delete) = tokio::join!(
            server.patch(&message, Some(&alice), json!({ "content": "edited" })),
            server.delete(&message, Some(&owner)),
        );
        assert!(
            edit.status.is_success() || edit.code() == "NOT_FOUND",
            "{edit:?}"
        );
        delete.ok();
    }
    let history = server
        .get(&format!("{path}?limit=100"), Some(&owner))
        .await
        .ok();
    for m in history.as_array().unwrap() {
        assert_eq!(m["deleted"], true, "{m}");
        assert_eq!(m["content"], "", "{m}");
    }
}

/// Users banned while they join through an invite: whichever comes first, none of them ends up
/// a member, and the cache agrees with the database.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ban_racing_a_join_keeps_the_user_out() {
    let server = start("consistency-ban", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let invite = server
        .post(
            &format!("/spaces/{space_id}/invites"),
            Some(&owner),
            json!({}),
        )
        .await
        .ok();
    let code = invite["code"].as_str().unwrap().to_owned();
    let mut users = Vec::new();
    for i in 0..6 {
        users.push(server.register(&format!("user{i}"), None).await);
    }
    for (token, user_id) in &users {
        let (join_path, ban_path) = (
            format!("/invites/{code}"),
            format!("/spaces/{space_id}/bans/{user_id}"),
        );
        let (join, ban) = tokio::join!(
            server.post(&join_path, Some(token), json!(null)),
            server.put(&ban_path, Some(&owner), json!({})),
        );
        assert!(
            join.status.is_success() || join.code() == "BANNED",
            "{join:?}"
        );
        ban.ok();
    }
    let space: Id = space_id.parse().unwrap();
    for (_, user_id) in &users {
        let user: Id = user_id.parse().unwrap();
        let in_db: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM space_members WHERE space_id = ? AND user_id = ?",
        )
        .bind(space.to_i64())
        .bind(user.to_i64())
        .fetch_optional(&server.state.db.read)
        .await
        .unwrap();
        assert_eq!(in_db, None, "banned user {user_id} is a member");
        assert!(!server.state.cache().is_member(space, user));
    }
}

/// A rename to a handle the database has but the cache does not know yet (a `hermes-server user`
/// command before the 10 second refresh, or a registration racing it) is a conflict, not an
/// internal error, and changes nothing.
#[tokio::test]
async fn rename_to_a_handle_only_the_database_has_is_a_conflict() {
    let server = start("consistency-rename", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (_, bob_id) = server.member(&owner, &space_id, "bob").await;
    // The server's own 10 second refresh can pick bob's new handle up between the database
    // write and the rename. The cache then knows it, alice is given a free tag, and the rename
    // rightly succeeds: that is not the case under test, so it goes again with another name.
    for attempt in 0..5 {
        let me = server.get("/users/@me", Some(&alice)).await.ok();
        let (name, tag) = (
            me["username"].as_str().unwrap().to_owned(),
            me["discriminator"].as_str().unwrap().to_owned(),
        );
        let wanted = format!("newname{attempt}");
        server
            .sql(&format!(
                "UPDATE users SET username = '{wanted}#{tag}' WHERE id = {bob_id}"
            ))
            .await;
        let res = server
            .post(
                "/users/@me/username",
                Some(&alice),
                json!({ "username": wanted, "current_password": "correct horse" }),
            )
            .await;
        if res.status.is_success() {
            assert_ne!(
                res.body["discriminator"],
                json!(tag),
                "the cache had bob's handle"
            );
            continue;
        }
        res.err(409, "CONFLICT");
        let me = server.get("/users/@me", Some(&alice)).await.ok();
        assert_eq!(me["id"], json!(alice_id));
        assert_eq!(me["username"], json!(name));
        assert_eq!(me["discriminator"], json!(tag));
        return;
    }
    panic!("the refresh raced every attempt");
}
