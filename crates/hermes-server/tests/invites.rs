//! Space invites: create, list, preview, join, delete; limits, expiry, bans and permissions.

mod common;

use common::start;
use hermes_proto::id::Id;
use hermes_proto::permissions::Permissions;
use hermes_server::config::RegistrationMode;
use hermes_server::time::now_ms;
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn create_preview_join() {
    let server = start("invites-join", RegistrationMode::Open).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.register("alice", None).await;

    let before = now_ms();
    let invite = server
        .post(
            &format!("/spaces/{space_id}/invites"),
            Some(&owner),
            json!({}),
        )
        .await
        .ok();
    let code = invite["code"].as_str().unwrap().to_owned();
    assert_eq!(code.len(), 10);
    assert_eq!(invite["creator_id"], owner_id);
    assert_eq!(invite["max_uses"], json!(null));
    assert_eq!(invite["uses"], 0);
    let week = 7 * 24 * 3600 * 1000;
    let expires_at = invite["expires_at"].as_i64().unwrap();
    assert!((before + week..=now_ms() + week).contains(&expires_at));
    assert_eq!(invite["space"]["name"], "Test Instance");

    // Preview needs no auth.
    let preview = server.get(&format!("/invites/{code}"), None).await.ok();
    assert_eq!(preview["space"]["id"], space_id);
    assert_eq!(preview["space"]["member_count"], 1);
    server
        .get("/invites/AAAAAAAAAA", None)
        .await
        .err(404, "INVITE_INVALID");

    // Joining returns the space as in `ready`.
    let joined = server
        .request(
            Method::POST,
            &format!("/invites/{code}"),
            Some(&alice),
            None,
        )
        .await
        .ok();
    assert_eq!(joined["space"]["id"], space_id);
    assert_eq!(joined["roles"].as_array().unwrap().len(), 1);
    let channels: Vec<_> = joined["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(channels, ["general", "General"]);
    let members: Vec<_> = joined["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["user"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(members, [owner_id.as_str(), alice_id.as_str()]);

    // Joining again is harmless and does not count as a use.
    server
        .request(
            Method::POST,
            &format!("/invites/{code}"),
            Some(&alice),
            None,
        )
        .await
        .ok();
    let list = server
        .get(&format!("/spaces/{space_id}/invites"), Some(&owner))
        .await
        .ok();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["uses"], 1);
    assert_eq!(list[0]["space"]["member_count"], 2);

    // Members can create invites (@everyone has CREATE_INVITES) but not list them.
    server
        .post(
            &format!("/spaces/{space_id}/invites"),
            Some(&alice),
            json!({}),
        )
        .await
        .ok();
    server
        .get(&format!("/spaces/{space_id}/invites"), Some(&alice))
        .await
        .err(403, "MISSING_PERMISSION");
}

#[tokio::test]
async fn limits_and_expiry() {
    let server = start("invites-limits", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.register("alice", None).await;
    let (bob, _) = server.register("bob", None).await;
    let path = format!("/spaces/{space_id}/invites");

    let once = server
        .post(&path, Some(&owner), json!({ "max_uses": 1 }))
        .await
        .ok();
    assert_eq!(once["max_uses"], 1);
    let code = once["code"].as_str().unwrap();
    server
        .request(
            Method::POST,
            &format!("/invites/{code}"),
            Some(&alice),
            None,
        )
        .await
        .ok();
    server
        .request(Method::POST, &format!("/invites/{code}"), Some(&bob), None)
        .await
        .err(404, "INVITE_INVALID");
    server
        .get(&format!("/invites/{code}"), None)
        .await
        .err(404, "INVITE_INVALID");

    // `0` means unlimited / never.
    let forever = server
        .post(
            &path,
            Some(&owner),
            json!({ "max_uses": 0, "expires_in_secs": 0 }),
        )
        .await
        .ok();
    assert_eq!(forever["max_uses"], json!(null));
    assert_eq!(forever["expires_at"], json!(null));

    let res = server
        .post(
            &path,
            Some(&owner),
            json!({ "expires_in_secs": 30 * 24 * 3600 + 1 }),
        )
        .await;
    res.err(400, "VALIDATION_FAILED");
    assert!(res.body["error"]["fields"]["expires_in_secs"].is_string());

    // An expired invite (written directly, the API cannot create one in the past).
    let space: Id = space_id.parse().unwrap();
    server
        .sql(&format!(
            "INSERT INTO invites (code, space_id, uses, expires_at, created_at)
             VALUES ('Expired123', {}, 0, {}, 0)",
            space.to_i64(),
            now_ms() - 1
        ))
        .await;
    server
        .get("/invites/Expired123", None)
        .await
        .err(404, "INVITE_INVALID");
    server
        .request(Method::POST, "/invites/Expired123", Some(&bob), None)
        .await
        .err(404, "INVITE_INVALID");
}

#[tokio::test]
async fn bans_and_permissions() {
    let server = start("invites-perms", RegistrationMode::Open).await;
    let (owner, owner_id, space_id) = server.owner().await;
    let (alice, alice_id) = server.register("alice", None).await;
    let (bob, _) = server.register("bob", None).await;
    let path = format!("/spaces/{space_id}/invites");
    let code = server.post(&path, Some(&owner), json!({})).await.ok()["code"]
        .as_str()
        .unwrap()
        .to_owned();

    // Non-members cannot tell whether the space exists.
    server
        .post(&path, Some(&bob), json!({}))
        .await
        .err(404, "NOT_FOUND");
    server.get(&path, Some(&bob)).await.err(404, "NOT_FOUND");
    server
        .post("/spaces/1/invites", Some(&owner), json!({}))
        .await
        .err(404, "NOT_FOUND");

    // Banned users cannot join.
    let (space, alice_uid, owner_uid): (Id, Id, Id) = (
        space_id.parse().unwrap(),
        alice_id.parse().unwrap(),
        owner_id.parse().unwrap(),
    );
    server
        .sql(&format!(
            "INSERT INTO bans (space_id, user_id, banned_by, created_at) VALUES ({}, {}, {}, 0)",
            space.to_i64(),
            alice_uid.to_i64(),
            owner_uid.to_i64()
        ))
        .await;
    server
        .request(
            Method::POST,
            &format!("/invites/{code}"),
            Some(&alice),
            None,
        )
        .await
        .err(403, "BANNED");

    // Without CREATE_INVITES on @everyone, members cannot create invites.
    server
        .request(Method::POST, &format!("/invites/{code}"), Some(&bob), None)
        .await
        .ok();
    {
        let mut cache = server.state.cache_mut();
        let everyone = &mut cache.spaces.get_mut(&space).unwrap().roles[0];
        everyone.permissions.remove(Permissions::CREATE_INVITES);
    }
    server
        .post(&path, Some(&bob), json!({}))
        .await
        .err(403, "MISSING_PERMISSION");
    server.post(&path, Some(&owner), json!({})).await.ok();
}

#[tokio::test]
async fn delete_invites() {
    let server = start("invites-delete", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.register("alice", None).await;
    let (bob, _) = server.register("bob", None).await;
    let path = format!("/spaces/{space_id}/invites");
    let owners = server.post(&path, Some(&owner), json!({})).await.ok()["code"]
        .as_str()
        .unwrap()
        .to_owned();
    for token in [&alice, &bob] {
        server
            .request(
                Method::POST,
                &format!("/invites/{owners}"),
                Some(token),
                None,
            )
            .await
            .ok();
    }
    let alices = server.post(&path, Some(&alice), json!({})).await.ok()["code"]
        .as_str()
        .unwrap()
        .to_owned();

    // Bob is neither the creator nor has MANAGE_INVITES.
    server
        .delete(&format!("/invites/{alices}"), Some(&bob))
        .await
        .err(403, "MISSING_PERMISSION");
    // The creator can.
    assert_eq!(
        server
            .delete(&format!("/invites/{alices}"), Some(&alice))
            .await
            .status,
        204
    );
    // The owner can delete anyone's.
    let bobs = server.post(&path, Some(&bob), json!({})).await.ok()["code"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        server
            .delete(&format!("/invites/{bobs}"), Some(&owner))
            .await
            .status,
        204
    );
    server
        .delete(&format!("/invites/{bobs}"), Some(&owner))
        .await
        .err(404, "NOT_FOUND");
    server
        .get(&format!("/invites/{bobs}"), None)
        .await
        .err(404, "INVITE_INVALID");
}

#[tokio::test]
async fn instance_invites() {
    let server = start("invites-instance", RegistrationMode::InviteOnly).await;
    let (owner, _, _) = server.owner().await;
    server
        .sql("INSERT INTO invites (code, uses, created_at) VALUES ('Instance01', 0, 0)")
        .await;

    // Registers without joining any space.
    let preview = server.get("/invites/Instance01", None).await.ok();
    assert_eq!(preview["space"], json!(null));
    assert_eq!(preview["creator_id"], json!(null));
    let (token, user_id) = server.register("alice", Some("Instance01")).await;
    let alice: Id = user_id.parse().unwrap();
    assert!(
        server
            .state
            .cache()
            .spaces
            .values()
            .all(|s| !s.members.contains_key(&alice))
    );
    // Logged-in users cannot "join" it.
    server
        .request(Method::POST, "/invites/Instance01", Some(&token), None)
        .await
        .err(404, "INVITE_INVALID");
    // Only instance admins may delete server-created instance invites.
    server
        .delete("/invites/Instance01", Some(&token))
        .await
        .err(403, "MISSING_PERMISSION");
    assert_eq!(
        server
            .delete("/invites/Instance01", Some(&owner))
            .await
            .status,
        204
    );
}
