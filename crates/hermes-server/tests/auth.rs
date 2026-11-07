//! Register, login, logout, sessions, `/users/@me`, and the first-start setup invite.

mod common;

use common::{Res, start};
use hermes_proto::id::Id;
use hermes_proto::models::ChannelKind;
use hermes_server::config::RegistrationMode;
use hermes_server::setup;
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn setup_invite_creates_the_admin_and_default_space() {
    let server = start("setup", RegistrationMode::Open).await;

    // Restarting before anyone registered reuses the same setup invite.
    let again = setup::first_start(&server.state, None)
        .await
        .unwrap()
        .unwrap();
    assert!(
        again
            .web
            .ends_with(&format!("/invite/{}", server.setup_code))
    );

    // The first account needs the setup invite, even in open mode.
    server
        .register_raw("eager", None)
        .await
        .err(403, "REGISTRATION_CLOSED");

    let body = server
        .register_raw("Owner", Some(&server.setup_code))
        .await
        .ok();
    assert!(body["token"].as_str().unwrap().starts_with("hms_"));
    assert_eq!(body["user"]["username"], "owner");
    assert_eq!(body["user"]["instance_admin"], true);
    let owner_id: Id = body["user"]["id"].as_str().unwrap().parse().unwrap();

    {
        let cache = server.state.cache();
        assert_eq!(cache.spaces.len(), 1);
        let space = cache.spaces.values().next().unwrap();
        assert_eq!(space.space.name, "Test Instance");
        assert_eq!(space.space.owner_id, owner_id);
        assert!(space.members.contains_key(&owner_id));
        assert_eq!(space.roles.len(), 1);
        assert!(space.roles[0].is_everyone());
        let mut channels: Vec<_> = cache
            .channels
            .values()
            .map(|c| {
                (
                    c.channel.kind,
                    c.channel.name.clone().unwrap(),
                    c.channel.bitrate,
                )
            })
            .collect();
        channels.sort_by_key(|c| c.1.clone());
        assert_eq!(
            channels,
            [
                (ChannelKind::Voice, "General".into(), Some(64_000)),
                (ChannelKind::Text, "general".into(), None),
            ]
        );
    }
    // The cache matches what a fresh load from the database sees.
    let reloaded = hermes_server::state::load_from_db(&server.state.db)
        .await
        .unwrap();
    assert_eq!(*server.state.cache(), reloaded);

    // The setup invite was single use, and no new one is printed once users exist.
    server
        .register_raw("second", Some(&server.setup_code))
        .await
        .err(404, "INVITE_INVALID");
    assert!(
        setup::first_start(&server.state, None)
            .await
            .unwrap()
            .is_none()
    );

    // Later accounts are not admins.
    let (token, _) = server.register("second", None).await;
    let me = server.get("/users/@me", Some(&token)).await.ok();
    assert_eq!(me["username"], "second");
    assert_eq!(me["instance_admin"], false);
}

#[tokio::test]
async fn registration_modes() {
    // invite_only: an invite is required; a space invite joins its space.
    let server = start("mode-invite-only", RegistrationMode::InviteOnly).await;
    let (owner, _, space_id) = server.owner().await;
    server
        .register_raw("alice", None)
        .await
        .err(403, "REGISTRATION_CLOSED");
    server
        .register_raw("alice", Some("AAAAAAAAAA"))
        .await
        .err(404, "INVITE_INVALID");
    server
        .register_raw("alice", Some("not a code"))
        .await
        .err(404, "INVITE_INVALID");
    let invite = server
        .post(
            &format!("/spaces/{space_id}/invites"),
            Some(&owner),
            json!({}),
        )
        .await
        .ok();
    let code = invite["code"].as_str().unwrap();
    let (_, alice_id) = server.register("alice", Some(code)).await;
    let space: Id = space_id.parse().unwrap();
    let alice: Id = alice_id.parse().unwrap();
    assert!(server.state.cache().is_member(space, alice));
    let invites = server
        .get(&format!("/spaces/{space_id}/invites"), Some(&owner))
        .await
        .ok();
    assert_eq!(invites[0]["uses"], 1);

    // closed: nobody can register after the first account, even with an invite.
    let server = start("mode-closed", RegistrationMode::Closed).await;
    let (owner, _, space_id) = server.owner().await;
    let invite = server
        .post(
            &format!("/spaces/{space_id}/invites"),
            Some(&owner),
            json!({}),
        )
        .await
        .ok();
    server
        .register_raw("bob", invite["code"].as_str())
        .await
        .err(403, "REGISTRATION_CLOSED");

    // open: no invite needed; an invite is still honored.
    let server = start("mode-open", RegistrationMode::Open).await;
    server.owner().await;
    server.register("carol", None).await;
}

#[tokio::test]
async fn register_validation_and_conflicts() {
    let server = start("register-validation", RegistrationMode::Open).await;
    server.owner().await;

    let res = server
        .post(
            "/auth/register",
            None,
            json!({ "username": "a b", "password": "short", "device_name": " " }),
        )
        .await;
    res.err(400, "VALIDATION_FAILED");
    let fields = &res.body["error"]["fields"];
    for field in ["username", "password", "device_name"] {
        assert!(fields[field].is_string(), "{field}: {}", res.body);
    }

    server.register("dave", None).await;
    // Names are not unique any more: the tag tells them apart.
    server.register("DAVE", None).await;

    // Malformed and incomplete bodies.
    server
        .post("/auth/register", None, json!({ "username": "x" }))
        .await
        .err(400, "BAD_REQUEST");
    server.get("/nope", None).await.err(404, "NOT_FOUND");
}

#[tokio::test]
async fn login_logout_and_sessions() {
    let server = start("sessions", RegistrationMode::Open).await;
    let (first, owner_id, _) = server.owner().await;

    server
        .login("owner", "wrong password")
        .await
        .err(401, "INVALID_CREDENTIALS");
    server
        .login("nobody", "correct horse")
        .await
        .err(401, "INVALID_CREDENTIALS");
    let login = server.login(" OWNER ", "correct horse").await.ok();
    assert_eq!(login["user"]["id"], owner_id);
    let second = login["token"].as_str().unwrap().to_owned();
    assert_ne!(first, second);

    let sessions = server.get("/auth/sessions", Some(&second)).await.ok();
    let sessions = sessions.as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0]["device_name"], "Test PC");
    assert_eq!(sessions[0]["current"], false);
    assert_eq!(sessions[1]["device_name"], "Laptop");
    assert_eq!(sessions[1]["current"], true);
    let first_session = sessions[0]["id"].as_str().unwrap().to_owned();

    // Other users cannot see or revoke our sessions.
    let (other, _) = server.register("eve", None).await;
    let theirs = server.get("/auth/sessions", Some(&other)).await.ok();
    assert_eq!(theirs.as_array().unwrap().len(), 1);
    server
        .delete(&format!("/auth/sessions/{first_session}"), Some(&other))
        .await
        .err(404, "NOT_FOUND");
    server
        .delete("/auth/sessions/123", Some(&second))
        .await
        .err(404, "NOT_FOUND");
    server
        .delete("/auth/sessions/abc", Some(&second))
        .await
        .err(400, "BAD_REQUEST");

    // Revoke the first session from the second one.
    let res = server
        .delete(&format!("/auth/sessions/{first_session}"), Some(&second))
        .await;
    assert_eq!(res.status, 204);
    server
        .get("/users/@me", Some(&first))
        .await
        .err(401, "UNAUTHORIZED");

    // Logout revokes the current session.
    let res = server
        .request(Method::POST, "/auth/logout", Some(&second), None)
        .await;
    assert_eq!(res.status, 204);
    server
        .get("/auth/sessions", Some(&second))
        .await
        .err(401, "UNAUTHORIZED");

    // Missing or malformed tokens.
    server
        .get("/auth/sessions", None)
        .await
        .err(401, "UNAUTHORIZED");
    server
        .get("/auth/sessions", Some("hms_made_up"))
        .await
        .err(401, "UNAUTHORIZED");
}

#[tokio::test]
async fn disabled_accounts_are_locked_out() {
    let server = start("disabled", RegistrationMode::Open).await;
    server.owner().await;
    let (token, user_id) = server.register("mallory", None).await;
    let id: Id = user_id.parse().unwrap();

    // What `hermes-server user disable` will do: set the flag and revoke sessions.
    server
        .sql(&format!(
            "UPDATE users SET disabled = 1 WHERE id = {}",
            id.to_i64()
        ))
        .await;
    server
        .state
        .cache_mut()
        .users
        .get_mut(&id)
        .unwrap()
        .disabled = true;

    server
        .get("/users/@me", Some(&token))
        .await
        .err(403, "ACCOUNT_DISABLED");
    // The right password is needed to learn that the account is disabled.
    server
        .login("mallory", "wrong password")
        .await
        .err(401, "INVALID_CREDENTIALS");
    server
        .login("mallory", "correct horse")
        .await
        .err(403, "ACCOUNT_DISABLED");
}

#[tokio::test]
async fn change_password_revokes_other_sessions() {
    let server = start("password", RegistrationMode::Open).await;
    let (first, _, _) = server.owner().await;
    let second = server.login("owner", "correct horse").await.ok()["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let change =
        |current: &str, new: &str| json!({ "current_password": current, "new_password": new });
    server
        .post(
            "/users/@me/password",
            Some(&first),
            change("wrong password", "new password 1"),
        )
        .await
        .err(401, "INVALID_CREDENTIALS");
    server
        .post(
            "/users/@me/password",
            Some(&first),
            change("correct horse", "short"),
        )
        .await
        .err(400, "VALIDATION_FAILED");

    let res: Res = server
        .post(
            "/users/@me/password",
            Some(&first),
            change("correct horse", "new password 1"),
        )
        .await;
    assert_eq!(res.status, 204, "{}", res.body);

    server.get("/users/@me", Some(&first)).await.ok();
    server
        .get("/users/@me", Some(&second))
        .await
        .err(401, "UNAUTHORIZED");
    server
        .login("owner", "correct horse")
        .await
        .err(401, "INVALID_CREDENTIALS");
    server.login("owner", "new password 1").await.ok();
}
