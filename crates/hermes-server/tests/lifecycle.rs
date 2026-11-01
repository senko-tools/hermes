//! Rate limits, request validation limits, `/healthz` and graceful shutdown.

mod common;

use common::{start, start_with};
use hermes_server::config::RegistrationMode;
use serde_json::json;

#[tokio::test]
async fn healthz_needs_no_auth() {
    let server = start("lifecycle-healthz", RegistrationMode::Open).await;
    let res = server
        .client()
        .get(format!("{}/healthz", server.root))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    assert_eq!(res.text().await.unwrap(), "ok");
}

#[tokio::test]
async fn rest_requests_are_limited_per_session_and_ip() {
    let server = start_with("lifecycle-rest-limit", RegistrationMode::Open, |state| {
        state.config.limits.requests_per_second = 3;
    })
    .await;
    let (alice, _, _) = server.owner().await;
    for _ in 0..3 {
        server.get("/users/@me", Some(&alice)).await.ok();
    }
    let res = server.get("/users/@me", Some(&alice)).await;
    res.err(429, "RATE_LIMITED");
    assert!(res.body["error"]["retry_after_ms"].as_u64().unwrap() >= 1000);
    let raw = server.get_raw("/users/@me", Some(&alice)).await;
    assert_eq!(raw.status().as_u16(), 429);
    assert!(raw.headers().contains_key("retry-after"));

    // Requests without a token are limited per IP, independently of alice's token. Wait until
    // the bucket used by registering has refilled.
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    for _ in 0..3 {
        server
            .get("/invites/AAAAAAAAAA", None)
            .await
            .err(404, "INVITE_INVALID");
    }
    server
        .get("/invites/AAAAAAAAAA", None)
        .await
        .err(429, "RATE_LIMITED");
    // /healthz is not limited.
    for _ in 0..5 {
        let res = server
            .client()
            .get(format!("{}/healthz", server.root))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status().as_u16(), 200);
    }
}

#[tokio::test]
async fn login_attempts_are_limited_per_ip() {
    let server = start_with("lifecycle-login-limit", RegistrationMode::Open, |state| {
        state.config.limits.login_attempts_per_minute = 2;
    })
    .await;
    server.owner().await;
    server
        .login("owner", "wrong password")
        .await
        .err(401, "INVALID_CREDENTIALS");
    server.login("owner", "correct horse").await.ok();
    let res = server.login("owner", "correct horse").await;
    res.err(429, "RATE_LIMITED");
    // 60 s / 2 attempts: the next one is about 30 s away.
    let wait = res.body["error"]["retry_after_ms"].as_u64().unwrap();
    assert!((1000..=30_000).contains(&wait), "{wait}");
}

#[tokio::test]
async fn messages_are_limited_per_user_per_channel() {
    let server = start_with("lifecycle-message-limit", RegistrationMode::Open, |state| {
        state.config.limits.messages_per_5s = 2;
    })
    .await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "general");
    let send = |token: String, channel: String| {
        let server = &server;
        async move {
            server
                .post(
                    &format!("/channels/{channel}/messages"),
                    Some(&token),
                    json!({ "content": "hi", "nonce": "n" }),
                )
                .await
        }
    };
    send(owner.clone(), general.clone()).await.ok();
    send(owner.clone(), general.clone()).await.ok();
    let res = send(owner.clone(), general.clone()).await;
    res.err(429, "RATE_LIMITED");
    // 5 s / 2 messages: at most 2.5 s to wait.
    let wait = res.body["error"]["retry_after_ms"].as_u64().unwrap();
    assert!((1..=2500).contains(&wait), "{wait}");

    // Another user in the same channel, and the same user elsewhere, are not affected.
    send(alice, general).await.ok();
    let other = server
        .post(
            &format!("/spaces/{space_id}/channels"),
            Some(&owner),
            json!({ "kind": "text", "name": "other" }),
        )
        .await
        .ok();
    send(owner, other["id"].as_str().unwrap().to_owned())
        .await
        .ok();
}

#[tokio::test]
async fn gateway_ops_are_limited() {
    let server = start("lifecycle-gateway-limit", RegistrationMode::Open).await;
    let (token, _, _) = server.owner().await;
    let (mut ws, _) = server.identify(&token).await;
    // identify was op 1 of 120.
    for _ in 0..120 {
        ws.send(json!({ "op": "presence_update", "d": { "status": "online" } }))
            .await;
    }
    assert_eq!(ws.closed().await, 4008);
}

#[tokio::test]
async fn oversized_json_bodies_are_rejected() {
    let server = start("lifecycle-json-limit", RegistrationMode::Open).await;
    let (token, _, _) = server.owner().await;
    let res = server
        .post(
            "/spaces",
            Some(&token),
            json!({ "name": "x".repeat(hermes_server::api::JSON_BODY_LIMIT) }),
        )
        .await;
    res.err(413, "PAYLOAD_TOO_LARGE");
    // Large but within the limit: an ordinary validation error.
    server
        .post(
            "/spaces",
            Some(&token),
            json!({ "name": "x".repeat(10_000) }),
        )
        .await
        .err(400, "VALIDATION_FAILED");
}

#[tokio::test]
async fn shutdown_closes_gateways_with_4100_and_stops_serving() {
    let server = start("lifecycle-shutdown", RegistrationMode::Open).await;
    let (token, _, _) = server.owner().await;
    let (mut identified, _) = server.identify(&token).await;
    let mut fresh = server.connect().await;

    let started = std::time::Instant::now();
    server.shut_down().await.unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(identified.closed().await, 4100);
    assert_eq!(fresh.closed().await, 4100);
    assert!(server.state.db.write.is_closed());
    assert!(
        server
            .client()
            .get(format!("{}/healthz", server.root))
            .send()
            .await
            .is_err(),
        "still accepting connections"
    );
}
