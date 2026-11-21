//! Rate limits, request validation limits, `/healthz` and graceful shutdown.

use crate::common;

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
    // Made-up tokens do not each get a bucket: the address has a limit too.
    let mut limited = false;
    for i in 0..200 {
        let token = format!("made-up-{i}");
        if server.get("/users/@me", Some(&token)).await.code() == "RATE_LIMITED" {
            limited = true;
            break;
        }
    }
    assert!(limited, "made-up tokens got past every limit");
    // /healthz is not under the REST limit (it has its own per address).
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
async fn profile_search_and_export_have_their_own_limits() {
    // 4 per minute: profiles get half (2), search a quarter (1), export at least 1.
    let server = start_with("lifecycle-action-limits", RegistrationMode::Open, |state| {
        state.config.limits.actions_per_minute = 4;
    })
    .await;
    let (owner, owner_id, _) = server.owner().await;
    for _ in 0..2 {
        server
            .get(&format!("/users/{owner_id}"), Some(&owner))
            .await
            .ok();
    }
    let res = server
        .get(&format!("/users/{owner_id}"), Some(&owner))
        .await;
    res.err(429, "RATE_LIMITED");
    server.get("/search?q=hello", Some(&owner)).await.ok();
    server
        .get("/search?q=hello", Some(&owner))
        .await
        .err(429, "RATE_LIMITED");
    server.get("/users/@me/export", Some(&owner)).await.ok();
    server
        .get("/users/@me/export", Some(&owner))
        .await
        .err(429, "RATE_LIMITED");
}

#[tokio::test]
async fn web_pages_are_limited_per_ip() {
    let server = start("lifecycle-page-limit", RegistrationMode::Open).await;
    let mut limited = false;
    for _ in 0..50 {
        let res = server
            .client()
            .get(format!("{}/invite/AAAAAAAAAA", server.root))
            .send()
            .await
            .unwrap();
        if res.status().as_u16() == 429 {
            limited = true;
            break;
        }
        assert_eq!(res.status().as_u16(), 404);
    }
    assert!(limited, "the invite page was never limited");
}

#[tokio::test]
async fn gateway_sign_ins_are_limited_per_person() {
    let server = start_with(
        "lifecycle-identify-limit",
        RegistrationMode::Open,
        |state| {
            state.config.limits.gateway_identifies_per_minute = 2;
        },
    )
    .await;
    let (owner, _, _) = server.owner().await;
    for _ in 0..2 {
        let (ws, _) = server.identify(&owner).await;
        drop(ws);
    }
    let mut ws = server.connect().await;
    ws.send_identify(&owner).await;
    assert_eq!(ws.closed().await, 4008);
}

#[tokio::test]
async fn revoked_sessions_stop_working_at_once() {
    let server = start("lifecycle-session-cache", RegistrationMode::Open).await;
    let (owner, _, _) = server.owner().await;
    // Cached by this request.
    server.get("/users/@me", Some(&owner)).await.ok();
    server.post("/auth/logout", Some(&owner), json!({})).await;
    server
        .get("/users/@me", Some(&owner))
        .await
        .err(401, "UNAUTHORIZED");

    // Deleted outside the server (the user commands): gone after the next refresh.
    let token = server.login("owner", "correct horse").await.ok()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    server.get("/users/@me", Some(&token)).await.ok();
    server.sql("DELETE FROM sessions").await;
    hermes_server::users::refresh(&server.state).await.unwrap();
    server
        .get("/users/@me", Some(&token))
        .await
        .err(401, "UNAUTHORIZED");
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
                    json!({ "content": "hi", "nonce": common::nonce() }),
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

#[tokio::test]
async fn maintenance_ends_what_has_gone_unused() {
    let server = start_with("lifecycle-maintenance", RegistrationMode::Open, |_| {}).await;
    let (owner, _, _) = server.owner().await;
    let (idle, _) = server.register("idle", None).await;
    let now = hermes_server::time::now_ms();
    let long_ago = now - 200 * 24 * 60 * 60 * 1000;
    server
        .sql(&format!(
            "UPDATE sessions SET last_seen_at = {long_ago} WHERE user_id =
               (SELECT id FROM users WHERE username LIKE 'idle%')"
        ))
        .await;
    hermes_server::maintenance::run(&server.state, now)
        .await
        .unwrap();
    server
        .get("/users/@me", Some(&idle))
        .await
        .err(401, "UNAUTHORIZED");
    server.get("/users/@me", Some(&owner)).await.ok();
}
