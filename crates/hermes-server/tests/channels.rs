//! Space channels: create, update, delete, reorder; permission overrides.

mod common;

use common::start;
use hermes_proto::id::Id;
use hermes_proto::permissions::Permissions;
use hermes_server::config::RegistrationMode;
use serde_json::{Value, json};

fn bits(p: Permissions) -> String {
    p.bits().to_string()
}

#[tokio::test]
async fn create_update_delete() {
    let server = start("channels-crud", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let (stranger, _) = server.register("stranger", None).await;
    let channels = format!("/spaces/{space_id}/channels");
    let general = server.channel_id(&space_id, "general");

    let body = json!({ "kind": "text", "name": "memes" });
    server
        .post(&channels, Some(&stranger), body.clone())
        .await
        .err(404, "NOT_FOUND");
    server
        .post(&channels, Some(&alice), body)
        .await
        .err(403, "MISSING_PERMISSION");

    // A category with an override; channels created inside it copy the override.
    let category = server
        .post(
            &channels,
            Some(&owner),
            json!({ "kind": "category", "name": "Games" }),
        )
        .await
        .ok();
    assert_eq!(category["position"], 2, "after #general and General");
    let category_id = category["id"].as_str().unwrap().to_owned();
    server
        .put(
            &format!("/channels/{category_id}/overrides/{space_id}"),
            Some(&owner),
            json!({ "allow": "0", "deny": bits(Permissions::SEND_MESSAGES) }),
        )
        .await
        .ok();
    let text = server
        .post(
            &channels,
            Some(&owner),
            json!({ "kind": "text", "name": "chess", "parent_id": category_id, "topic": " moves " }),
        )
        .await
        .ok();
    assert_eq!(text["parent_id"], category_id);
    assert_eq!(text["topic"], "moves");
    assert_eq!(text["position"], 3);
    assert_eq!(text["bitrate"], json!(null));
    assert_eq!(
        text["overrides"],
        json!([{ "role_id": space_id, "allow": "0", "deny": bits(Permissions::SEND_MESSAGES) }])
    );
    let text_id = text["id"].as_str().unwrap().to_owned();

    let voice = server
        .post(
            &channels,
            Some(&owner),
            json!({ "kind": "voice", "name": "Lounge", "user_limit": 0 }),
        )
        .await
        .ok();
    assert_eq!(voice["bitrate"], 64000);
    assert_eq!(voice["user_limit"], json!(null), "0 = unlimited");
    let voice_id = voice["id"].as_str().unwrap().to_owned();

    for bad in [
        json!({ "kind": "dm", "name": "x" }),
        json!({ "kind": "text", "name": " " }),
        json!({ "kind": "text", "name": "x", "bitrate": 64000 }),
        json!({ "kind": "voice", "name": "x", "bitrate": 1000 }),
        json!({ "kind": "voice", "name": "x", "bitrate": 200000 }),
        json!({ "kind": "voice", "name": "x", "user_limit": 26 }),
        json!({ "kind": "category", "name": "x", "parent_id": category_id }),
        json!({ "kind": "text", "name": "x", "parent_id": general }),
        json!({ "kind": "text", "name": "x", "topic": "y".repeat(1025) }),
    ] {
        server
            .post(&channels, Some(&owner), bad.clone())
            .await
            .err(400, "VALIDATION_FAILED");
    }

    // Update.
    let path = format!("/channels/{text_id}");
    server
        .patch(&path, Some(&alice), json!({ "name": "x" }))
        .await
        .err(403, "MISSING_PERMISSION");
    let updated = server
        .patch(
            &path,
            Some(&owner),
            json!({ "name": "chess-club", "topic": null, "slowmode_secs": 30, "parent_id": null }),
        )
        .await
        .ok();
    assert_eq!(updated["name"], "chess-club");
    assert_eq!(updated["topic"], json!(null));
    assert_eq!(updated["slowmode_secs"], 30);
    assert_eq!(updated["parent_id"], json!(null));
    assert_eq!(
        updated["overrides"].as_array().unwrap().len(),
        1,
        "overrides stay when moving"
    );
    let updated = server
        .patch(&path, Some(&owner), json!({ "parent_id": category_id }))
        .await
        .ok();
    assert_eq!(updated["parent_id"], category_id);
    for bad in [
        json!({ "bitrate": 64000 }),
        json!({ "user_limit": 5 }),
        json!({ "slowmode_secs": 21601 }),
        json!({ "parent_id": voice_id }),
    ] {
        server
            .patch(&path, Some(&owner), bad)
            .await
            .err(400, "VALIDATION_FAILED");
    }
    let voice = server
        .patch(
            &format!("/channels/{voice_id}"),
            Some(&owner),
            json!({ "bitrate": 96000, "user_limit": 10 }),
        )
        .await
        .ok();
    assert_eq!(
        (voice["bitrate"].clone(), voice["user_limit"].clone()),
        (json!(96000), json!(10))
    );
    server
        .patch(
            &format!("/channels/{category_id}"),
            Some(&owner),
            json!({ "slowmode_secs": 5 }),
        )
        .await
        .err(400, "VALIDATION_FAILED");

    // Deleting a category moves its children to the top level.
    server
        .delete(&format!("/channels/{category_id}"), Some(&alice))
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .delete(&format!("/channels/{category_id}"), Some(&owner))
        .await
        .ok();
    let child = server.patch(&path, Some(&owner), json!({})).await.ok();
    assert_eq!(child["parent_id"], json!(null));
    let text_id: Id = text_id.parse().unwrap();
    assert_eq!(
        server.state.cache().channels[&text_id].channel.parent_id,
        None
    );
    server
        .delete(&format!("/channels/{category_id}"), Some(&owner))
        .await
        .err(404, "NOT_FOUND");

    let actions = server.audit_actions(&owner, &space_id).await;
    assert_eq!(
        actions,
        [
            "channel_delete",
            "channel_update",
            "channel_update",
            "channel_update",
            "channel_create",
            "channel_create",
            "override_update",
            "channel_create"
        ]
    );
}

#[tokio::test]
async fn overrides() {
    let server = start("channels-overrides", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let (bob, bob_id) = server.member(&owner, &space_id, "bob").await;
    let bob_id: Id = bob_id.parse().unwrap();
    let general = server.channel_id(&space_id, "general");
    let manager = server
        .role(
            &owner,
            &space_id,
            "manager",
            (Permissions::MANAGE_CHANNELS | Permissions::MANAGE_ROLES).bits(),
        )
        .await;
    server
        .assign(&owner, &space_id, &alice_id, &[&manager])
        .await;
    let everyone_override = format!("/channels/{general}/overrides/{space_id}");

    // MANAGE_CHANNELS + MANAGE_ROLES.
    server
        .put(
            &everyone_override,
            Some(&bob),
            json!({ "allow": "0", "deny": "0" }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
    // Only bits the caller has; allow and deny must not overlap.
    server
        .put(
            &everyone_override,
            Some(&alice),
            json!({ "allow": bits(Permissions::BAN_MEMBERS), "deny": "0" }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
    let send = bits(Permissions::SEND_MESSAGES);
    server
        .put(
            &everyone_override,
            Some(&alice),
            json!({ "allow": send, "deny": send }),
        )
        .await
        .err(400, "VALIDATION_FAILED");
    server
        .put(
            &format!("/channels/{general}/overrides/{}", Id(77)),
            Some(&alice),
            json!({ "allow": "0", "deny": "0" }),
        )
        .await
        .err(404, "NOT_FOUND");
    server
        .put(
            &everyone_override,
            Some(&alice),
            json!({ "allow": 1, "deny": "0" }),
        )
        .await
        .err(400, "BAD_REQUEST");

    // Hide #general from @everyone but keep it for the manager role.
    let view = bits(Permissions::VIEW_CHANNEL);
    server
        .put(
            &format!("/channels/{general}/overrides/{manager}"),
            Some(&alice),
            json!({ "allow": view, "deny": "0" }),
        )
        .await
        .ok();
    server
        .put(
            &everyone_override,
            Some(&alice),
            json!({ "allow": "0", "deny": view }),
        )
        .await
        .ok();
    let gid: Id = general.parse().unwrap();
    assert!(
        server
            .state
            .cache()
            .channel_permissions(gid, bob_id)
            .is_empty()
    );
    // Bob no longer sees the channel at all.
    server
        .patch(&format!("/channels/{general}"), Some(&bob), json!({}))
        .await
        .err(404, "NOT_FOUND");
    // Replacing an override keeps one entry per role.
    server
        .put(
            &everyone_override,
            Some(&alice),
            json!({ "allow": "0", "deny": send }),
        )
        .await
        .ok();
    let overrides: Vec<Value> = server
        .patch(&format!("/channels/{general}"), Some(&alice), json!({}))
        .await
        .ok()["overrides"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(overrides.len(), 2);
    assert!(
        server
            .state
            .cache()
            .channel_permissions(gid, bob_id)
            .contains(Permissions::VIEW_CHANNEL)
    );

    server.delete(&everyone_override, Some(&alice)).await.ok();
    server
        .delete(&everyone_override, Some(&alice))
        .await
        .err(404, "NOT_FOUND");
    assert_eq!(
        server.state.cache().channels[&gid].channel.overrides.len(),
        1
    );

    let actions = server.audit_actions(&owner, &space_id).await;
    assert_eq!(
        &actions[..4],
        [
            "override_delete",
            "override_update",
            "override_update",
            "override_update"
        ]
    );
}

#[tokio::test]
async fn reorder() {
    let server = start("channels-order", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let order = format!("/spaces/{space_id}/channels/order");
    let general = server.channel_id(&space_id, "general");
    let voice = server.channel_id(&space_id, "General");
    let category = server
        .post(
            &format!("/spaces/{space_id}/channels"),
            Some(&owner),
            json!({ "kind": "category", "name": "Talk" }),
        )
        .await
        .ok()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    server
        .put(&order, Some(&alice), json!([]))
        .await
        .err(403, "MISSING_PERMISSION");
    let channels = server
        .put(
            &order,
            Some(&owner),
            json!([
                { "id": category, "position": 0 },
                { "id": general, "position": 1, "parent_id": category },
                { "id": voice, "position": 2 }
            ]),
        )
        .await
        .ok();
    let summary: Vec<_> = channels
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap().to_owned(),
                c["position"].as_i64().unwrap(),
                c["parent_id"].as_str().map(str::to_owned),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("Talk".to_owned(), 0, None),
            ("general".to_owned(), 1, Some(category.clone())),
            ("General".to_owned(), 2, None)
        ]
    );
    let channels = server
        .put(
            &order,
            Some(&owner),
            json!([{ "id": general, "position": 1, "parent_id": null }]),
        )
        .await
        .ok();
    assert_eq!(channels[1]["parent_id"], json!(null));

    for bad in [
        json!([{ "id": Id(9), "position": 0 }]),
        json!([{ "id": general, "position": 0 }, { "id": general, "position": 1 }]),
        json!([{ "id": category, "position": 0, "parent_id": category }]),
        json!([{ "id": general, "position": 0, "parent_id": voice }]),
    ] {
        server
            .put(&order, Some(&owner), bad)
            .await
            .err(400, "VALIDATION_FAILED");
    }

    let actions = server.audit_actions(&owner, &space_id).await;
    assert_eq!(
        actions,
        ["channels_reorder", "channels_reorder", "channel_create"]
    );
    let log = server
        .get(&format!("/spaces/{space_id}/audit-log"), Some(&owner))
        .await
        .ok();
    assert_eq!(log[1]["details"]["channels"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn overrides_only_for_lower_roles() {
    let server = start("channels-override-hierarchy", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "general");
    // New roles go in right above @everyone: "senior" ends up above "manager".
    let senior = server.role(&owner, &space_id, "senior", 0).await;
    let manager = server
        .role(
            &owner,
            &space_id,
            "manager",
            (Permissions::MANAGE_CHANNELS | Permissions::MANAGE_ROLES | Permissions::VIEW_CHANNEL)
                .bits(),
        )
        .await;
    let junior = server.role(&owner, &space_id, "junior", 0).await;
    server
        .assign(&owner, &space_id, &alice_id, &[&manager])
        .await;
    let deny_view = json!({ "allow": "0", "deny": bits(Permissions::VIEW_CHANNEL) });

    // A higher role cannot be locked out by the manager; the owner can.
    let path = format!("/channels/{general}/overrides/{senior}");
    server
        .put(&path, Some(&alice), deny_view.clone())
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .put(&path, Some(&owner), deny_view.clone())
        .await
        .ok();
    server
        .delete(&path, Some(&alice))
        .await
        .err(403, "MISSING_PERMISSION");
    // Lower roles, her own and @everyone are fine.
    let deny_send = json!({ "allow": "0", "deny": bits(Permissions::SEND_MESSAGES) });
    for role in [&junior, &manager, &space_id] {
        let path = format!("/channels/{general}/overrides/{role}");
        server
            .put(&path, Some(&alice), deny_send.clone())
            .await
            .ok();
        server.delete(&path, Some(&alice)).await.ok();
    }
}
