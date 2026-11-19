//! `POST /users/devices` for several users at once (one query for all of them), and revoking the
//! devices of several signed-out sessions together.

use crate::common;
use common::device;

use common::{TestServer, start};
use hermes_server::config::RegistrationMode;
use serde_json::{Value, json};

async fn login(server: &TestServer, username: &str) -> String {
    server.login(username, "correct horse").await.ok()["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn query(server: &TestServer, token: &str, user_ids: &[&str]) -> Value {
    server
        .post(
            "/users/devices",
            Some(token),
            json!({ "user_ids": user_ids }),
        )
        .await
        .ok()
}

/// `(user_id, [(sign_key, revoked)])` per entry of a query answer.
fn summary(found: &Value) -> Vec<(String, Vec<(String, bool)>)> {
    found
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            let devices = entry["devices"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| {
                    (
                        d["sign_key"].as_str().unwrap().to_owned(),
                        d.get("revoked").is_some_and(|r| r == true),
                    )
                })
                .collect();
            (entry["user_id"].as_str().unwrap().to_owned(), devices)
        })
        .collect()
}

#[tokio::test]
async fn devices_of_several_users_come_back_in_the_order_asked() {
    let server = start("devices-query", RegistrationMode::Open).await;
    let (owner, ..) = server.owner().await;
    let _ = owner;
    let (alice, alice_id) = server.register("alice", None).await;
    let (bob, bob_id) = server.register("bob", None).await;
    let (_carol, carol_id) = server.register("carol", None).await;

    // Alice has two devices (oldest first), Bob one, Carol none.
    server
        .put("/users/@me/devices", Some(&alice), device(1))
        .await
        .ok();
    let alice2 = login(&server, "alice").await;
    server
        .put("/users/@me/devices", Some(&alice2), device(2))
        .await
        .ok();
    server
        .put("/users/@me/devices", Some(&bob), device(3))
        .await
        .ok();

    let unknown = "123456789";
    let found = query(
        &server,
        &bob,
        &[&carol_id, &alice_id, unknown, &bob_id, &alice_id],
    )
    .await;
    let alice_devices = vec![("sign1".to_owned(), false), ("sign2".to_owned(), false)];
    assert_eq!(
        summary(&found),
        [
            (carol_id.clone(), vec![]),
            (alice_id.clone(), alice_devices.clone()),
            (unknown.to_owned(), vec![]),
            (bob_id.clone(), vec![("sign3".to_owned(), false)]),
            (alice_id.clone(), alice_devices),
        ]
    );
    assert!(summary(&query(&server, &bob, &[]).await).is_empty());
}

#[tokio::test]
async fn a_password_change_revokes_the_devices_of_every_other_session() {
    let server = start("devices-revoke-many", RegistrationMode::Open).await;
    let (owner, ..) = server.owner().await;
    let _ = owner;
    let (alice, alice_id) = server.register("alice", None).await;
    let (bob, _) = server.register("bob", None).await;
    server
        .put("/users/@me/devices", Some(&alice), device(1))
        .await
        .ok();
    for n in [2, 3] {
        let token = login(&server, "alice").await;
        server
            .put("/users/@me/devices", Some(&token), device(n))
            .await
            .ok();
    }

    server
        .post(
            "/users/@me/password",
            Some(&alice),
            json!({ "current_password": "correct horse", "new_password": "battery staple" }),
        )
        .await
        .ok();

    assert_eq!(
        summary(&query(&server, &bob, &[&alice_id]).await),
        [(
            alice_id.clone(),
            vec![
                ("sign1".to_owned(), false),
                ("sign2".to_owned(), true),
                ("sign3".to_owned(), true),
            ]
        )]
    );
}
