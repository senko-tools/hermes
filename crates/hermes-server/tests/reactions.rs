//! Reactions: add, join, remove own, remove others', emoji validation, ADD_REACTIONS, the
//! 20-emoji limit.

mod common;

use common::start;
use hermes_proto::permissions::Permissions;
use hermes_server::config::RegistrationMode;
use serde_json::{Value, json};

/// `(emoji, count, me)` of a message as `token` sees it.
async fn reactions(
    server: &common::TestServer,
    token: &str,
    message: &str,
) -> Vec<(String, u64, bool)> {
    let m: Value = server.get(message, Some(token)).await.ok();
    m["reactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["emoji"].as_str().unwrap().to_owned(),
                r["count"].as_u64().unwrap(),
                r["me"].as_bool().unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn add_join_remove() {
    let server = start("reactions", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let (bob, _) = server.member(&owner, &space_id, "bob").await;
    let (carol, carol_id) = server.member(&owner, &space_id, "carol").await;
    let (stranger, _) = server.register("stranger", None).await;
    let general = server.channel_id(&space_id, "general");
    let sent = server
        .post(
            &format!("/channels/{general}/messages"),
            Some(&alice),
            json!({ "content": "boo", "nonce": "n" }),
        )
        .await
        .ok();
    let message = format!(
        "/channels/{general}/messages/{}",
        sent["id"].as_str().unwrap()
    );
    let react = |emoji: &str| format!("{message}/reactions/{emoji}");

    server.put(&react("🎃"), Some(&bob), json!(null)).await.ok();
    server.put(&react("🎃"), Some(&bob), json!(null)).await.ok();
    server
        .put(&react("👻"), Some(&alice), json!(null))
        .await
        .ok();
    server
        .put(&react("🎃"), Some(&alice), json!(null))
        .await
        .ok();
    assert_eq!(
        reactions(&server, &alice, &message).await,
        [("🎃".to_owned(), 2, true), ("👻".to_owned(), 1, true)],
        "in order of the first reaction"
    );
    assert_eq!(
        reactions(&server, &bob, &message).await,
        [("🎃".to_owned(), 2, true), ("👻".to_owned(), 1, false)]
    );

    for bad in ["abc", "x", "🎃🎃"] {
        server
            .put(&react(bad), Some(&bob), json!(null))
            .await
            .err(400, "VALIDATION_FAILED");
    }
    server
        .put(&react("🎃"), Some(&stranger), json!(null))
        .await
        .err(404, "NOT_FOUND");

    // Without ADD_REACTIONS you can only join existing reactions.
    let everyone = Permissions::DEFAULT_EVERYONE.difference(Permissions::ADD_REACTIONS);
    server
        .patch(
            &format!("/spaces/{space_id}/roles/{space_id}"),
            Some(&owner),
            json!({ "permissions": everyone.bits().to_string() }),
        )
        .await
        .ok();
    server
        .put(&react("🦇"), Some(&carol), json!(null))
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .put(&react("👻"), Some(&carol), json!(null))
        .await
        .ok();

    // Removing your own; removing again is not an error.
    server.delete(&react("🎃"), Some(&bob)).await.ok();
    server.delete(&react("🎃"), Some(&bob)).await.ok();
    assert_eq!(
        reactions(&server, &bob, &message).await,
        [("👻".to_owned(), 2, false), ("🎃".to_owned(), 1, false)],
        "alice's 🎃 is now the oldest one, added after 👻"
    );

    // Removing others' needs MANAGE_MESSAGES.
    let carols = format!("{}/{carol_id}", react("👻"));
    server
        .delete(&carols, Some(&bob))
        .await
        .err(403, "MISSING_PERMISSION");
    server.delete(&carols, Some(&owner)).await.ok();
    assert_eq!(
        reactions(&server, &owner, &message).await,
        [("👻".to_owned(), 1, false), ("🎃".to_owned(), 1, false)]
    );

    // Deleted messages cannot get reactions.
    server.delete(&message, Some(&alice)).await.ok();
    server
        .put(&react("🎃"), Some(&bob), json!(null))
        .await
        .err(404, "NOT_FOUND");
}

#[tokio::test]
async fn at_most_twenty_emoji() {
    let server = start("reactions-limit", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "general");
    let sent = server
        .post(
            &format!("/channels/{general}/messages"),
            Some(&owner),
            json!({ "content": "react!", "nonce": "n" }),
        )
        .await
        .ok();
    let message = format!(
        "/channels/{general}/messages/{}",
        sent["id"].as_str().unwrap()
    );
    let emoji = [
        "😀", "😃", "😄", "😁", "😆", "😅", "😂", "🤣", "😊", "😇", "🙂", "🙃", "😉", "😌", "😍",
        "🥰", "😘", "😗", "😙", "😚",
    ];
    for e in emoji {
        server
            .put(
                &format!("{message}/reactions/{e}"),
                Some(&owner),
                json!(null),
            )
            .await
            .ok();
    }
    server
        .put(
            &format!("{message}/reactions/🎃"),
            Some(&owner),
            json!(null),
        )
        .await
        .err(400, "BAD_REQUEST");
    // Joining an existing one still works.
    server
        .put(
            &format!("{message}/reactions/😀"),
            Some(&alice),
            json!(null),
        )
        .await
        .ok();
    let m = server.get(&message, Some(&alice)).await.ok();
    assert_eq!(m["reactions"].as_array().unwrap().len(), 20);
    assert_eq!(m["reactions"][0]["count"], 2);
}
