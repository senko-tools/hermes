//! Roles: create, update, delete, reorder; granting only own bits; the hierarchy rule.

mod common;

use common::start;
use hermes_proto::id::Id;
use hermes_proto::permissions::Permissions;
use hermes_server::config::RegistrationMode;
use serde_json::{Value, json};

fn bits(p: Permissions) -> String {
    p.bits().to_string()
}

/// `(name, position)` of every role, lowest first.
fn order(roles: &Value) -> Vec<(String, i64)> {
    roles
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["name"].as_str().unwrap().to_owned(),
                r["position"].as_i64().unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn create_update_delete() {
    let server = start("roles-crud", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let roles = format!("/spaces/{space_id}/roles");

    server
        .post(&roles, Some(&alice), json!({ "name": "nope" }))
        .await
        .err(403, "MISSING_PERMISSION");
    let role = server
        .post(
            &roles,
            Some(&owner),
            json!({
                "name": " mod ", "color": 0xff8800, "hoist": true,
                "permissions": bits(Permissions::MANAGE_ROLES)
            }),
        )
        .await
        .ok();
    assert_eq!(role["name"], "mod");
    assert_eq!(role["color"], 0xff8800);
    assert_eq!(role["hoist"], true);
    assert_eq!(role["mentionable"], false);
    assert_eq!(role["position"], 1);
    assert_eq!(role["space_id"], space_id);
    let moderator = role["id"].as_str().unwrap().to_owned();
    for bad in [
        json!({ "name": "" }),
        json!({ "name": "x", "color": 0x1000000 }),
        json!({ "name": "x".repeat(33) }),
    ] {
        server
            .post(&roles, Some(&owner), bad)
            .await
            .err(400, "VALIDATION_FAILED");
    }
    let helper = server.role(&owner, &space_id, "helper", 0).await;
    let sid: Id = space_id.parse().unwrap();
    let cached: Vec<_> = server.state.cache().spaces[&sid]
        .roles
        .iter()
        .map(|r| (r.name.clone(), r.position))
        .collect();
    assert_eq!(
        cached,
        [
            ("@everyone".to_owned(), 0),
            ("helper".to_owned(), 1),
            ("mod".to_owned(), 2)
        ]
    );
    server
        .assign(&owner, &space_id, &alice_id, &[&moderator])
        .await;

    // Only bits you have can be granted; removing and re-adding your own bits is fine.
    server
        .post(
            &roles,
            Some(&alice),
            json!({ "name": "banner", "permissions": bits(Permissions::BAN_MEMBERS) }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .patch(
            &format!("{roles}/{helper}"),
            Some(&alice),
            json!({ "permissions": bits(Permissions::BAN_MEMBERS) }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
    let updated = server
        .patch(
            &format!("{roles}/{helper}"),
            Some(&alice),
            json!({
                "name": "helpers", "color": null, "mentionable": true,
                "permissions": bits(Permissions::SEND_MESSAGES | Permissions::MANAGE_ROLES)
            }),
        )
        .await
        .ok();
    assert_eq!(updated["name"], "helpers");
    assert_eq!(updated["mentionable"], true);
    assert_eq!(updated["position"], 1);
    // Not your own (or a higher) role.
    server
        .patch(
            &format!("{roles}/{moderator}"),
            Some(&alice),
            json!({ "name": "boss" }),
        )
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .patch(&format!("{roles}/{}", Id(999)), Some(&owner), json!({}))
        .await
        .err(404, "NOT_FOUND");

    // @everyone: only the permissions can change, and it cannot be deleted.
    let everyone = format!("{roles}/{space_id}");
    server
        .patch(&everyone, Some(&owner), json!({ "name": "all" }))
        .await
        .err(400, "BAD_REQUEST");
    let role = server
        .patch(
            &everyone,
            Some(&owner),
            json!({ "permissions": bits(Permissions::VIEW_CHANNEL) }),
        )
        .await
        .ok();
    assert_eq!(role["permissions"], bits(Permissions::VIEW_CHANNEL));
    server
        .delete(&everyone, Some(&owner))
        .await
        .err(400, "BAD_REQUEST");

    // Deleting removes the role from members and overrides.
    let general = server.channel_id(&space_id, "general");
    server
        .put(
            &format!("/channels/{general}/overrides/{helper}"),
            Some(&owner),
            json!({ "allow": "0", "deny": bits(Permissions::SEND_MESSAGES) }),
        )
        .await
        .ok();
    server
        .assign(&owner, &space_id, &alice_id, &[&moderator, &helper])
        .await;
    server
        .delete(&format!("{roles}/{moderator}"), Some(&alice))
        .await
        .err(403, "MISSING_PERMISSION");
    server
        .delete(&format!("{roles}/{helper}"), Some(&alice))
        .await
        .ok();
    {
        let cache = server.state.cache();
        let member = cache.member(sid, alice_id.parse().unwrap()).unwrap();
        assert_eq!(member.role_ids, [moderator.parse::<Id>().unwrap()]);
        let general: Id = general.parse().unwrap();
        assert!(cache.channels[&general].channel.overrides.is_empty());
        assert_eq!(cache.spaces[&sid].roles.len(), 2);
    }
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM channel_overrides")
        .fetch_one(&server.state.db.read)
        .await
        .unwrap();
    assert_eq!(count, 0);

    let actions = server.audit_actions(&owner, &space_id).await;
    assert_eq!(
        actions,
        [
            "role_delete",
            "member_update",
            "override_update",
            "role_update",
            "role_update",
            "member_update",
            "role_create",
            "role_create"
        ]
    );
}

#[tokio::test]
async fn reorder() {
    let server = start("roles-order", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let order_path = format!("/spaces/{space_id}/roles/order");

    // Created in this order, each at position 1: c(3) b(2) a(1) after all three.
    let c = server
        .role(&owner, &space_id, "c", Permissions::MANAGE_ROLES.bits())
        .await;
    let b = server.role(&owner, &space_id, "b", 0).await;
    let a = server.role(&owner, &space_id, "a", 0).await;

    let roles = server
        .put(
            &order_path,
            Some(&owner),
            json!([{ "id": a, "position": 10 }]),
        )
        .await
        .ok();
    assert_eq!(
        order(&roles),
        [
            ("@everyone".to_owned(), 0),
            ("b".to_owned(), 1),
            ("c".to_owned(), 2),
            ("a".to_owned(), 3)
        ]
    );

    // alice has `c` (position 2): she may only move `b`, and only below 2.
    server.assign(&owner, &space_id, &alice_id, &[&c]).await;
    for (id, position) in [(&a, 1), (&c, 1), (&b, 2)] {
        server
            .put(
                &order_path,
                Some(&alice),
                json!([{ "id": id, "position": position }]),
            )
            .await
            .err(403, "MISSING_PERMISSION");
    }
    let roles = server
        .put(
            &order_path,
            Some(&alice),
            json!([{ "id": b, "position": 1 }]),
        )
        .await
        .ok();
    assert_eq!(order(&roles)[1], ("b".to_owned(), 1));

    for bad in [
        json!([{ "id": space_id, "position": 1 }]),
        json!([{ "id": Id(5), "position": 1 }]),
        json!([{ "id": b, "position": 0 }]),
        json!([{ "id": b, "position": 1 }, { "id": b, "position": 1 }]),
    ] {
        server
            .put(&order_path, Some(&owner), bad)
            .await
            .err(400, "VALIDATION_FAILED");
    }
    server
        .put(&order_path, Some(&owner), json!({}))
        .await
        .err(400, "BAD_REQUEST");

    // Only real moves are audited.
    let actions = server.audit_actions(&owner, &space_id).await;
    assert_eq!(actions[0], "member_update");
    assert_eq!(actions[1], "roles_reorder");
}
