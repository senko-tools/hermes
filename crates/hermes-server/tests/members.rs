//! Members: listing, nicknames, role assignment with the hierarchy rule, kick, leave, bans.

mod common;

use common::start;
use hermes_proto::id::Id;
use hermes_proto::permissions::Permissions;
use hermes_server::config::RegistrationMode;
use hermes_server::time::{HOUR_MS, now_ms};
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn list_and_nicknames() {
    let server = start("members-nick", RegistrationMode::Open).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space_id, "bob").await;
    let (stranger, _) = server.register("stranger", None).await;
    let members = format!("/spaces/{space_id}/members");

    let page = server
        .get(&format!("{members}?limit=2"), Some(&bob))
        .await
        .ok();
    let ids: Vec<_> = page
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["user"]["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, [owner_id.clone(), alice_id.clone()]);
    let rest = server
        .get(&format!("{members}?after={alice_id}"), Some(&bob))
        .await
        .ok();
    assert_eq!(rest[0]["user"]["id"], bob_id);
    assert_eq!(rest.as_array().unwrap().len(), 1);
    server
        .get(&members, Some(&stranger))
        .await
        .err(404, "NOT_FOUND");

    // Own nickname: CHANGE_NICKNAME (in @everyone by default); `@me` works.
    let me = server
        .patch(
            &format!("{members}/@me"),
            Some(&alice),
            json!({ "nickname": " Ali " }),
        )
        .await
        .ok();
    assert_eq!(me["nickname"], "Ali");
    let me = server
        .patch(
            &format!("{members}/{alice_id}"),
            Some(&alice),
            json!({ "nickname": null }),
        )
        .await
        .ok();
    assert_eq!(me["nickname"], json!(null));
    server
        .patch(
            &format!("{members}/@me"),
            Some(&alice),
            json!({ "nickname": "x".repeat(33) }),
        )
        .await
        .err(400, "VALIDATION_FAILED");

    // Others' nicknames: MANAGE_NICKNAMES.
    server
        .patch(
            &format!("{members}/{bob_id}"),
            Some(&alice),
            json!({ "nickname": "Bobby" }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
    let bob_member = server
        .patch(
            &format!("{members}/{bob_id}"),
            Some(&owner),
            json!({ "nickname": "Bobby" }),
        )
        .await
        .ok();
    assert_eq!(bob_member["nickname"], "Bobby");
    server
        .patch(
            &format!("{members}/{}", Id(12345)),
            Some(&owner),
            json!({ "nickname": "Ghost" }),
        )
        .await
        .err(404, "NOT_FOUND");

    // Without CHANGE_NICKNAME in @everyone, members cannot rename themselves.
    let everyone = Permissions::DEFAULT_EVERYONE.difference(Permissions::CHANGE_NICKNAME);
    server
        .patch(
            &format!("/spaces/{space_id}/roles/{space_id}"),
            Some(&owner),
            json!({ "permissions": everyone.bits().to_string() }),
        )
        .await
        .ok();
    server
        .patch(
            &format!("{members}/@me"),
            Some(&alice),
            json!({ "nickname": "Ali" }),
        )
        .await
        .err(403, "MISSING_PERMISSION");

    // Only the moderation change is audited, not alice's own nickname.
    let actions = server.audit_actions(&owner, &space_id).await;
    assert_eq!(actions, ["role_update", "member_update"]);
}

#[tokio::test]
async fn roles_kick_leave_and_hierarchy() {
    let server = start("members-roles", RegistrationMode::Open).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space_id, "bob").await;
    let (_, carol_id) = server.member(&owner, &space_id, "carol").await;
    let members = format!("/spaces/{space_id}/members");

    let mod_perms = Permissions::MANAGE_ROLES | Permissions::KICK_MEMBERS;
    let moderator = server
        .role(&owner, &space_id, "mod", mod_perms.bits())
        .await;
    // New roles go in at position 1, so `helper` ends up below `mod`.
    let helper = server.role(&owner, &space_id, "helper", 0).await;
    server
        .assign(&owner, &space_id, &alice_id, &[&moderator])
        .await;
    server
        .assign(&owner, &space_id, &carol_id, &[&moderator])
        .await;

    // Without MANAGE_ROLES.
    server
        .patch(
            &format!("{members}/{carol_id}"),
            Some(&bob),
            json!({ "role_ids": [helper] }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
    // Roles below your highest role can be given to members below you.
    let member = server
        .patch(
            &format!("{members}/{bob_id}"),
            Some(&alice),
            json!({ "role_ids": [helper, helper] }),
        )
        .await
        .ok();
    assert_eq!(member["role_ids"], json!([helper]));
    // Not your own highest role, and not to members at your level.
    server
        .patch(
            &format!("{members}/{bob_id}"),
            Some(&alice),
            json!({ "role_ids": [moderator] }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .patch(
            &format!("{members}/{carol_id}"),
            Some(&alice),
            json!({ "role_ids": [moderator, helper] }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
    // Your own roles: only ones below you.
    let me = server
        .patch(
            &format!("{members}/@me"),
            Some(&alice),
            json!({ "role_ids": [moderator, helper] }),
        )
        .await
        .ok();
    assert_eq!(me["role_ids"].as_array().unwrap().len(), 2);
    for bad in [json!([space_id]), json!([Id(4242)])] {
        server
            .patch(
                &format!("{members}/{bob_id}"),
                Some(&owner),
                json!({ "role_ids": bad }),
            )
            .await
            .err(400, "VALIDATION_FAILED");
    }

    // Kick: KICK_MEMBERS and the hierarchy rule.
    server
        .delete(&format!("{members}/{alice_id}"), Some(&bob))
        .await
        .err(403, "MISSING_PERMISSION");
    for target in [&carol_id, &owner_id] {
        server
            .delete(&format!("{members}/{target}"), Some(&alice))
            .await
            .err(403, "MISSING_PERMISSION");
    }
    server
        .delete(&format!("{members}/{bob_id}"), Some(&alice))
        .await
        .ok();
    server.get(&members, Some(&bob)).await.err(404, "NOT_FOUND");
    server
        .delete(&format!("{members}/{bob_id}"), Some(&alice))
        .await
        .err(404, "NOT_FOUND");

    // Leaving: anyone but the owner.
    server
        .delete(&format!("{members}/@me"), Some(&owner))
        .await
        .err(400, "BAD_REQUEST");
    server
        .delete(&format!("{members}/@me"), Some(&alice))
        .await
        .ok();
    let sid: Id = space_id.parse().unwrap();
    assert!(
        !server
            .state
            .cache()
            .is_member(sid, alice_id.parse().unwrap())
    );

    let actions = server.audit_actions(&owner, &space_id).await;
    assert_eq!(
        actions,
        [
            "member_kick",
            "member_update",
            "member_update",
            "member_update",
            "member_update",
            "role_create",
            "role_create"
        ],
        "leaving is not audited"
    );
}

#[tokio::test]
async fn bans() {
    let server = start("members-bans", RegistrationMode::Open).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space_id, "bob").await;
    let bans = format!("/spaces/{space_id}/bans");
    let general = server.channel_id(&space_id, "general");

    // Bob wrote one message now and one two days ago.
    let recent = Id::min_at_unix_ms(now_ms() as u64);
    let old = Id::min_at_unix_ms((now_ms() - 48 * HOUR_MS) as u64);
    server
        .sql(&format!(
            "INSERT INTO messages (id, channel_id, author_id, content, pinned)
             VALUES ({recent}, {general}, {bob_id}, 'recent', 1),
                    ({old}, {general}, {bob_id}, 'old', 0)"
        ))
        .await;
    server
        .sql(&format!(
            "INSERT INTO reactions (message_id, emoji, user_id, created_at)
             VALUES ({recent}, '🎃', {alice_id}, 0)"
        ))
        .await;

    server
        .put(&format!("{bans}/{bob_id}"), Some(&alice), json!({}))
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .get(&bans, Some(&alice))
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .put(&format!("{bans}/{owner_id}"), Some(&owner), json!({}))
        .await
        .err(400, "BAD_REQUEST");
    server
        .put(&format!("{bans}/{}", Id(4242)), Some(&owner), json!({}))
        .await
        .err(404, "NOT_FOUND");
    server
        .put(
            &format!("{bans}/{bob_id}"),
            Some(&owner),
            json!({ "delete_messages_hours": 169 }),
        )
        .await
        .err(400, "VALIDATION_FAILED");

    server
        .put(
            &format!("{bans}/{bob_id}"),
            Some(&owner),
            json!({ "reason": " spam ", "delete_messages_hours": 24 }),
        )
        .await
        .ok();
    let sid: Id = space_id.parse().unwrap();
    assert!(!server.state.cache().is_member(sid, bob_id.parse().unwrap()));

    // Only the recent message was deleted, with its reactions.
    let rows: Vec<(i64, String, bool, bool)> =
        sqlx::query_as("SELECT id, content, deleted, pinned FROM messages ORDER BY id")
            .fetch_all(&server.state.db.read)
            .await
            .unwrap();
    assert_eq!(
        rows,
        [
            (old.to_i64(), "old".into(), false, false),
            (recent.to_i64(), String::new(), true, false)
        ]
    );
    let (reactions,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM reactions")
        .fetch_one(&server.state.db.read)
        .await
        .unwrap();
    assert_eq!(reactions, 0);

    let list = server.get(&bans, Some(&owner)).await.ok();
    assert_eq!(list[0]["user"]["id"], bob_id);
    assert_eq!(list[0]["reason"], "spam");

    // Banned users cannot join again.
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
        .request(Method::POST, &format!("/invites/{code}"), Some(&bob), None)
        .await
        .err(403, "BANNED");

    // Users who are not members can be banned too.
    let (_, stranger_id) = server.register("stranger", None).await;
    server
        .put(&format!("{bans}/{stranger_id}"), Some(&owner), json!({}))
        .await
        .ok();

    server
        .delete(&format!("{bans}/{bob_id}"), Some(&owner))
        .await
        .ok();
    server
        .delete(&format!("{bans}/{bob_id}"), Some(&owner))
        .await
        .err(404, "NOT_FOUND");
    server
        .request(Method::POST, &format!("/invites/{code}"), Some(&bob), None)
        .await
        .ok();

    let log = server
        .get(&format!("/spaces/{space_id}/audit-log"), Some(&owner))
        .await
        .ok();
    let actions: Vec<_> = log
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["member_unban", "member_ban", "member_ban"]);
    assert_eq!(
        log[2]["details"],
        json!({ "reason": "spam", "delete_messages_hours": 24, "deleted_messages": 1 })
    );
}
