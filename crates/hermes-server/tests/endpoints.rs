//! Cross-cutting endpoint coverage (PLAN.md sections 13 and 19): `GET /instance`, every
//! authenticated endpoint refusing requests without a valid token, and each space permission bit
//! gating its endpoint.

mod common;

use common::{Res, TestServer, start};
use hermes_proto::PROTOCOL_VERSION;
use hermes_proto::permissions::Permissions;
use hermes_server::config::RegistrationMode;
use reqwest::Method;
use serde_json::{Value, json};

#[tokio::test]
async fn instance_info_needs_no_auth() {
    let server = start("endpoints-instance", RegistrationMode::Open).await;
    let info = server.get("/instance", None).await.ok();
    assert_eq!(info["name"], "Test Instance");
    assert_eq!(info["description"], "");
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(info["protocol_version"], PROTOCOL_VERSION);
    assert_eq!(info["registration_mode"], "open");
    assert_eq!(info["limits"]["max_upload_mb"], 25);
    assert_eq!(info["limits"]["max_message_length"], 4000);
    assert_eq!(info["voice"]["enabled"], true);
    // Deserializes into the wire type clients use.
    serde_json::from_value::<hermes_proto::api::InstanceResponse>(info).unwrap();
}

/// Every route under `/api/v1` that needs a token. IDs are well-formed but need not exist:
/// authentication is checked before anything is looked up.
const AUTHENTICATED: &[(&str, &str)] = &[
    ("POST", "/auth/logout"),
    ("GET", "/auth/sessions"),
    ("DELETE", "/auth/sessions/1000"),
    ("GET", "/users/@me"),
    ("POST", "/users/@me/password"),
    ("POST", "/spaces"),
    ("PATCH", "/spaces/1000"),
    ("DELETE", "/spaces/1000"),
    ("POST", "/spaces/1000/transfer"),
    ("GET", "/spaces/1000/members"),
    ("PATCH", "/spaces/1000/members/1000"),
    ("DELETE", "/spaces/1000/members/@me"),
    ("GET", "/spaces/1000/bans"),
    ("PUT", "/spaces/1000/bans/1000"),
    ("DELETE", "/spaces/1000/bans/1000"),
    ("GET", "/spaces/1000/audit-log"),
    ("POST", "/spaces/1000/roles"),
    ("PUT", "/spaces/1000/roles/order"),
    ("PATCH", "/spaces/1000/roles/1000"),
    ("DELETE", "/spaces/1000/roles/1000"),
    ("POST", "/spaces/1000/channels"),
    ("PUT", "/spaces/1000/channels/order"),
    ("POST", "/spaces/1000/invites"),
    ("GET", "/spaces/1000/invites"),
    ("PATCH", "/channels/1000"),
    ("DELETE", "/channels/1000"),
    ("PUT", "/channels/1000/overrides/1000"),
    ("DELETE", "/channels/1000/overrides/1000"),
    ("GET", "/channels/1000/messages"),
    ("POST", "/channels/1000/messages"),
    ("GET", "/channels/1000/messages/1000"),
    ("PATCH", "/channels/1000/messages/1000"),
    ("DELETE", "/channels/1000/messages/1000"),
    ("PUT", "/channels/1000/messages/1000/pin"),
    ("DELETE", "/channels/1000/messages/1000/pin"),
    ("GET", "/channels/1000/pins"),
    ("PUT", "/channels/1000/messages/1000/reactions/%F0%9F%98%80"),
    (
        "DELETE",
        "/channels/1000/messages/1000/reactions/%F0%9F%98%80",
    ),
    (
        "DELETE",
        "/channels/1000/messages/1000/reactions/%F0%9F%98%80/1000",
    ),
    ("POST", "/channels/1000/typing"),
    ("POST", "/channels/1000/ack"),
    ("POST", "/attachments"),
    ("GET", "/attachments/1000/thumb"),
    ("GET", "/attachments/1000/a.png"),
    ("GET", "/dms"),
    ("POST", "/dms"),
    ("DELETE", "/dms/1000"),
    ("PUT", "/dms/1000/recipients/1000"),
    ("DELETE", "/dms/1000/recipients/1000"),
    ("POST", "/invites/abcdefgh"),
    ("DELETE", "/invites/abcdefgh"),
    ("POST", "/voice/join"),
    ("POST", "/voice/leave"),
    ("PATCH", "/voice/@me"),
    ("PATCH", "/spaces/1000/voice-states/1000"),
    ("POST", "/channels/1000/call/ring"),
];

#[tokio::test]
async fn authenticated_endpoints_refuse_missing_and_invalid_tokens() {
    let server = start("endpoints-auth", RegistrationMode::Open).await;
    let (token, _, _) = server.owner().await;
    server
        .post("/auth/logout", Some(&token), json!({}))
        .await
        .ok();

    for &(method, path) in AUTHENTICATED {
        let method: Method = method.parse().unwrap();
        for token in [None, Some("hms_not-a-real-token"), Some(token.as_str())] {
            let res = server
                .request(method.clone(), path, token, Some(json!({})))
                .await;
            assert_eq!(
                (res.status.as_u16(), res.code()),
                (401, "UNAUTHORIZED"),
                "{method} {path} with {token:?}: {}",
                res.body
            );
        }
    }
}

/// One permission check: the request needs `bit` on top of `base`.
struct Case {
    bit: Permissions,
    base: Permissions,
    method: Method,
    path: String,
    body: Value,
    /// Status and code without the bit.
    denied: (u16, &'static str),
}

#[tokio::test]
async fn each_space_permission_bit_gates_its_endpoint() {
    let server = start("endpoints-bits", RegistrationMode::Open).await;
    let (owner, _, space) = server.owner().await;
    let (member, _) = server.member(&owner, &space, "member").await;
    let mut victims = Vec::new();
    for name in ["victim1", "victim2", "victim3"] {
        victims.push(server.member(&owner, &space, name).await.1);
    }
    let general = server.channel_id(&space, "general");
    let voice_channel = server.channel_id(&space, "General");
    let everyone = space.clone(); // @everyone's role ID is the space ID
    server
        .patch(
            &format!("/spaces/{space}/roles/{everyone}"),
            Some(&owner),
            json!({ "permissions": "0" }),
        )
        .await
        .ok();
    let role = server.role(&owner, &space, "tester", 0).await;
    let member_id = server.get("/users/@me", Some(&member)).await.ok()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    server.assign(&owner, &space, &member_id, &[&role]).await;

    let owner_message = server
        .post(
            &format!("/channels/{general}/messages"),
            Some(&owner),
            json!({ "content": "hello", "nonce": "owner-1" }),
        )
        .await
        .ok()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let upload = server
        .upload(&member, "a.txt", "text/plain", b"hi".to_vec())
        .await
        .ok()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let view = Permissions::VIEW_CHANNEL;
    let send = view | Permissions::SEND_MESSAGES;
    let none = Permissions::empty();
    let forbidden = (403, "MISSING_PERMISSION");
    let message = |nonce: &str| json!({ "content": "hi", "nonce": nonce });
    let cases = [
        Case {
            bit: Permissions::MANAGE_SPACE,
            base: none,
            method: Method::PATCH,
            path: format!("/spaces/{space}"),
            body: json!({ "description": "new" }),
            denied: forbidden,
        },
        Case {
            bit: Permissions::MANAGE_ROLES,
            base: none,
            method: Method::POST,
            path: format!("/spaces/{space}/roles"),
            body: json!({ "name": "new role" }),
            denied: forbidden,
        },
        Case {
            bit: Permissions::MANAGE_CHANNELS,
            base: none,
            method: Method::POST,
            path: format!("/spaces/{space}/channels"),
            body: json!({ "kind": "text", "name": "new-channel" }),
            denied: forbidden,
        },
        Case {
            bit: Permissions::KICK_MEMBERS,
            base: none,
            method: Method::DELETE,
            path: format!("/spaces/{space}/members/{}", victims[0]),
            body: Value::Null,
            denied: forbidden,
        },
        Case {
            bit: Permissions::BAN_MEMBERS,
            base: none,
            method: Method::PUT,
            path: format!("/spaces/{space}/bans/{}", victims[1]),
            body: json!({}),
            denied: forbidden,
        },
        Case {
            bit: Permissions::CREATE_INVITES,
            base: none,
            method: Method::POST,
            path: format!("/spaces/{space}/invites"),
            body: json!({}),
            denied: forbidden,
        },
        Case {
            bit: Permissions::MANAGE_INVITES,
            base: none,
            method: Method::GET,
            path: format!("/spaces/{space}/invites"),
            body: Value::Null,
            denied: forbidden,
        },
        Case {
            bit: Permissions::CHANGE_NICKNAME,
            base: none,
            method: Method::PATCH,
            path: format!("/spaces/{space}/members/@me"),
            body: json!({ "nickname": "Mem" }),
            denied: forbidden,
        },
        Case {
            bit: Permissions::MANAGE_NICKNAMES,
            base: none,
            method: Method::PATCH,
            path: format!("/spaces/{space}/members/{}", victims[2]),
            body: json!({ "nickname": "Vic" }),
            denied: forbidden,
        },
        Case {
            bit: Permissions::VIEW_AUDIT_LOG,
            base: none,
            method: Method::GET,
            path: format!("/spaces/{space}/audit-log"),
            body: Value::Null,
            denied: forbidden,
        },
        Case {
            bit: Permissions::VIEW_CHANNEL,
            base: none,
            method: Method::POST,
            path: format!("/channels/{general}/ack"),
            body: json!({ "message_id": owner_message }),
            denied: (404, "NOT_FOUND"),
        },
        Case {
            bit: Permissions::SEND_MESSAGES,
            base: view,
            method: Method::POST,
            path: format!("/channels/{general}/messages"),
            body: message("send"),
            denied: forbidden,
        },
        Case {
            bit: Permissions::READ_HISTORY,
            base: view,
            method: Method::GET,
            path: format!("/channels/{general}/messages"),
            body: Value::Null,
            denied: forbidden,
        },
        Case {
            bit: Permissions::ATTACH_FILES,
            base: send,
            method: Method::POST,
            path: format!("/channels/{general}/messages"),
            body: json!({ "content": "file", "nonce": "attach", "attachment_ids": [upload] }),
            denied: forbidden,
        },
        Case {
            bit: Permissions::ADD_REACTIONS,
            base: view,
            method: Method::PUT,
            path: format!("/channels/{general}/messages/{owner_message}/reactions/%F0%9F%98%80"),
            body: Value::Null,
            denied: forbidden,
        },
        Case {
            bit: Permissions::MANAGE_MESSAGES,
            base: view,
            method: Method::PUT,
            path: format!("/channels/{general}/messages/{owner_message}/pin"),
            body: Value::Null,
            denied: forbidden,
        },
        // MUTE_MEMBERS, DEAFEN_MEMBERS and MOVE_MEMBERS need someone in voice; tests/voice.rs.
        Case {
            bit: Permissions::CONNECT,
            base: view,
            method: Method::POST,
            path: "/voice/join".into(),
            body: json!({ "channel_id": voice_channel }),
            denied: forbidden,
        },
    ];

    let set_role = |perms: Permissions| {
        let path = format!("/spaces/{space}/roles/{role}");
        let owner = owner.clone();
        let server: &TestServer = &server;
        async move {
            server
                .patch(
                    &path,
                    Some(&owner),
                    json!({ "permissions": perms.bits().to_string() }),
                )
                .await
                .ok();
        }
    };
    let check_denied = |res: &Res, case: &Case, what: &str| {
        assert_eq!(
            (res.status.as_u16(), res.code()),
            case.denied,
            "{:?} {what}: {} {}",
            case.bit,
            case.path,
            res.body
        );
    };

    for case in &cases {
        let body = (!case.body.is_null()).then(|| case.body.clone());
        set_role(case.base).await;
        let res = server
            .request(case.method.clone(), &case.path, Some(&member), body.clone())
            .await;
        check_denied(&res, case, "without the bit");

        set_role(case.base | case.bit).await;
        let res = server
            .request(case.method.clone(), &case.path, Some(&member), body)
            .await;
        assert!(
            res.status.is_success(),
            "{:?} with the bit: {} {} {}",
            case.bit,
            case.path,
            res.status,
            res.body
        );
    }

    // ADMINISTRATOR alone grants everything, e.g. the audit log.
    set_role(Permissions::ADMINISTRATOR).await;
    server
        .get(&format!("/spaces/{space}/audit-log"), Some(&member))
        .await
        .ok();
}
