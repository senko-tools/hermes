//! Spaces: create (with the per-user limit), update, transfer, delete; the audit log.

mod common;

use common::start;
use hermes_proto::id::Id;
use hermes_proto::permissions::Permissions;
use hermes_server::config::RegistrationMode;
use serde_json::json;

#[tokio::test]
async fn create_space_and_limit() {
    let server = start("spaces-create", RegistrationMode::Open).await;
    let (owner, ..) = server.owner().await;
    let (alice, alice_id) = server.register("alice", None).await;

    let snapshot = server
        .post(
            "/spaces",
            Some(&alice),
            json!({ "name": "  Pumpkin Patch " }),
        )
        .await
        .ok();
    assert_eq!(snapshot["space"]["name"], "Pumpkin Patch");
    assert_eq!(snapshot["space"]["owner_id"], alice_id);
    let space_id = snapshot["space"]["id"].as_str().unwrap();
    let roles = snapshot["roles"].as_array().unwrap();
    assert_eq!(roles.len(), 1);
    assert_eq!(roles[0]["id"], space_id, "@everyone has the space ID");
    let channels: Vec<_> = snapshot["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["name"].as_str().unwrap(), c["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(channels, [("general", "text"), ("General", "voice")]);
    assert_eq!(snapshot["members"][0]["user"]["id"], alice_id);

    server
        .post("/spaces", Some(&alice), json!({ "name": "x" }))
        .await
        .err(400, "VALIDATION_FAILED");
    server
        .post("/spaces", None, json!({ "name": "Nope" }))
        .await
        .err(401, "UNAUTHORIZED");

    // `limits.max_spaces_per_user` = 10 owned spaces; admins have no limit.
    for i in 1..10 {
        server
            .post(
                "/spaces",
                Some(&alice),
                json!({ "name": format!("Space {i}") }),
            )
            .await
            .ok();
    }
    server
        .post("/spaces", Some(&alice), json!({ "name": "Eleven" }))
        .await
        .err(400, "BAD_REQUEST");
    for i in 0..10 {
        server
            .post(
                "/spaces",
                Some(&owner),
                json!({ "name": format!("Admin {i}") }),
            )
            .await
            .ok();
    }
}

#[tokio::test]
async fn update_transfer_delete() {
    let server = start("spaces-update", RegistrationMode::Open).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (stranger, stranger_id) = server.register("stranger", None).await;
    let path = format!("/spaces/{space_id}");

    // Non-members cannot tell whether the space exists; members need MANAGE_SPACE.
    server
        .patch(&path, Some(&stranger), json!({ "name": "Mine" }))
        .await
        .err(404, "NOT_FOUND");
    server
        .patch(&path, Some(&alice), json!({ "name": "Mine" }))
        .await
        .err(403, "MISSING_PERMISSION");

    let space = server
        .patch(
            &path,
            Some(&owner),
            json!({ "name": "Spooky", "description": "  boo \n" }),
        )
        .await
        .ok();
    assert_eq!(space["name"], "Spooky");
    assert_eq!(space["description"], "boo");
    let space = server
        .patch(&path, Some(&owner), json!({ "description": null }))
        .await
        .ok();
    assert_eq!(space["description"], json!(null));
    assert_eq!(space["name"], "Spooky", "absent fields are unchanged");
    // No change, no audit entry.
    server.patch(&path, Some(&owner), json!({})).await.ok();
    server
        .patch(&path, Some(&owner), json!({ "name": "a\u{0}b" }))
        .await
        .err(400, "VALIDATION_FAILED");

    // Icons must be images the caller uploaded.
    server
        .sql(&format!(
            "INSERT INTO attachments (id, uploader_id, filename, content_type, size, sha256, created_at)
             VALUES (77, {owner_id}, 'cat.png', 'image/png', 3, x'00', 0),
                    (78, {alice_id}, 'dog.png', 'image/png', 3, x'00', 0),
                    (79, {owner_id}, 'notes.txt', 'text/plain', 3, x'00', 0)"
        ))
        .await;
    let space = server
        .patch(&path, Some(&owner), json!({ "icon_id": "77" }))
        .await
        .ok();
    assert_eq!(space["icon"], json!({ "id": "77", "filename": "cat.png" }));
    for id in ["78", "79", "999"] {
        server
            .patch(&path, Some(&owner), json!({ "icon_id": id }))
            .await
            .err(400, "VALIDATION_FAILED");
    }
    let space = server
        .patch(&path, Some(&owner), json!({ "icon_id": null }))
        .await
        .ok();
    assert_eq!(space["icon"], json!(null));

    // Transfer: owner only, to a member.
    let transfer = format!("{path}/transfer");
    server
        .post(&transfer, Some(&alice), json!({ "user_id": alice_id }))
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .post(&transfer, Some(&owner), json!({ "user_id": stranger_id }))
        .await
        .err(400, "VALIDATION_FAILED");
    server
        .post(&transfer, Some(&owner), json!({ "user_id": alice_id }))
        .await
        .ok();
    let sid: Id = space_id.parse().unwrap();
    assert_eq!(
        server.state.cache().spaces[&sid].space.owner_id.to_string(),
        alice_id
    );

    // The new owner has every permission, including the audit log.
    let actions = server.audit_actions(&alice, &space_id).await;
    assert_eq!(
        actions,
        [
            "space_transfer",
            "space_update",
            "space_update",
            "space_update",
            "space_update"
        ]
    );

    // Delete: owner only.
    server
        .delete(&path, Some(&owner))
        .await
        .err(403, "MISSING_PERMISSION");
    server.delete(&path, Some(&alice)).await.ok();
    assert!(!server.state.cache().spaces.contains_key(&sid));
    assert_eq!(server.state.cache().space_channels(sid).count(), 0);
    server
        .get(&format!("{path}/members"), Some(&alice))
        .await
        .err(404, "NOT_FOUND");
}

#[tokio::test]
async fn audit_log_permission_and_pagination() {
    let server = start("spaces-audit", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let log = format!("/spaces/{space_id}/audit-log");

    server
        .get(&log, Some(&alice))
        .await
        .err(403, "MISSING_PERMISSION");
    for i in 0..5 {
        server
            .patch(
                &format!("/spaces/{space_id}"),
                Some(&owner),
                json!({ "name": format!("Name {i}") }),
            )
            .await
            .ok();
    }
    let auditor = server
        .role(
            &owner,
            &space_id,
            "auditor",
            Permissions::VIEW_AUDIT_LOG.bits(),
        )
        .await;
    server
        .assign(&owner, &space_id, &alice_id, &[&auditor])
        .await;

    let all = server.get(&log, Some(&alice)).await.ok();
    let all = all.as_array().unwrap();
    assert_eq!(all.len(), 7);
    assert_eq!(all[0]["action"], "member_update");
    assert_eq!(all[0]["target_id"], alice_id);
    assert_eq!(all[1]["action"], "role_create");
    assert_eq!(all[2]["details"], json!({ "name": "Name 4" }));

    let page = server
        .get(
            &format!("{log}?limit=2&before={}", all[2]["id"].as_str().unwrap()),
            Some(&alice),
        )
        .await
        .ok();
    let names: Vec<_> = page
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["details"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Name 3", "Name 2"]);
    server
        .get(&format!("{log}?before=abc"), Some(&alice))
        .await
        .err(400, "BAD_REQUEST");
}
