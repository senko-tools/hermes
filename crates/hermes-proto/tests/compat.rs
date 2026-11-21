//! Clients and servers of different versions keep talking (`hermes_proto::compat`).
//!
//! `tests/compat/<version>/` holds what a server of that version sent, frozen. Every build must
//! still read every one of them (an old server and a new client). The `newer_*` tests add what a
//! newer server might send (unknown fields, enum values, list items) and check that this build
//! reads around it (a new server and an old client). Each release adds its own folder.

use hermes_proto::api::{InstanceResponse, RegistrationMode};
use hermes_proto::gateway::{InstanceLimits, ServerEvent, ServerMessage};
use hermes_proto::models::{
    AuditAction, AuditEntry, ChannelKind, Message, MessageKind, NotificationLevel, Status,
};
use hermes_proto::{ErrorBody, ErrorCode};
use serde_json::{Value, json};

fn sample(version: &str, name: &str) -> Value {
    let path = format!(
        "{}/tests/compat/{version}/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap()
}

fn ready_of(msg: ServerMessage) -> hermes_proto::gateway::Ready {
    match msg.event {
        ServerEvent::Ready(ready) => *ready,
        other => panic!("not ready: {other:?}"),
    }
}

#[test]
fn reads_what_1_2_0_sent() {
    let ready: ServerMessage = serde_json::from_value(sample("1.2.0", "ready.json")).unwrap();
    let ready = ready_of(ready);
    assert!(
        ready.instance.features.is_empty(),
        "1.2.0 had no features list"
    );
    assert_eq!(ready.spaces[0].channels[0].kind, ChannelKind::Text);

    let created: ServerMessage =
        serde_json::from_value(sample("1.2.0", "message_create.json")).unwrap();
    assert!(matches!(created.event, ServerEvent::MessageCreate(_)));

    let list: Vec<Message> = serde_json::from_value(sample("1.2.0", "messages.json")).unwrap();
    assert_eq!(list.len(), 1);

    let instance: InstanceResponse = serde_json::from_value(json!({
        "name": "Hermes", "description": "", "version": "1.2.0", "protocol_version": 2,
        "registration_mode": "invite_only",
        "limits": { "max_upload_mb": 25, "max_message_length": 4000, "channels_encrypted": false },
        "voice": { "enabled": true }
    }))
    .unwrap();
    assert!(instance.features.is_empty());
    // Limits that came later read as what every server had then.
    assert_eq!(instance.limits, InstanceLimits::default());
}

#[test]
fn newer_server_fields_and_values_are_read_around() {
    let mut value = sample("1.2.0", "ready.json");
    let d = &mut value["d"];
    d["something_new"] = json!({ "a": 1 });
    d["privacy"]["dm_from"] = json!("pigeons_only");
    d["instance"]["features"] = json!(["bio", "polls_v9"]);
    d["presences"][0]["status"] = json!("busy_on_mars");
    let space = &mut d["spaces"][0];
    space["space"]["banner_color"] = json!("#ff0000");
    let mut thread = space["channels"][0].clone();
    thread["kind"] = json!("thread");
    thread["id"] = json!("12");
    space["channels"].as_array_mut().unwrap().push(thread);
    // An item nothing can read: dropped, the rest kept.
    space["roles"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": 5 }));
    d["notification_settings"] =
        json!([{ "target_id": "11", "level": "only_on_tuesdays", "muted_until": null }]);

    let ready = ready_of(serde_json::from_value(value).unwrap());
    assert_eq!(ready.instance.features, vec!["bio", "polls_v9"]);
    assert_eq!(ready.presences[0].status, Status::Offline);
    assert_eq!(ready.privacy, Default::default());
    let kinds: Vec<_> = ready.spaces[0].channels.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, vec![ChannelKind::Text, ChannelKind::Unknown]);
    assert!(!ChannelKind::Unknown.has_messages());
    assert_eq!(ready.spaces[0].roles.len(), 1);
    // A level from a newer server: that one setting is dropped, not the whole `ready`.
    assert!(ready.notification_settings.is_empty());
    let _ = NotificationLevel::All;
}

#[test]
fn newer_message_kinds_show_their_content() {
    let mut value = sample("1.2.0", "message_create.json");
    value["d"]["kind"] = json!("poll");
    value["d"]["poll"] = json!({ "question": "?" });
    let msg: ServerMessage = serde_json::from_value(value).unwrap();
    let ServerEvent::MessageCreate(m) = msg.event else {
        panic!("not a message");
    };
    assert_eq!(m.kind, MessageKind::Default);
    assert_eq!(m.content, "hello **world**");
}

#[test]
fn newer_ops_and_codes_are_skipped_or_softened() {
    // Unknown ops fail to decode as a whole; the gateway client skips such frames.
    assert!(
        serde_json::from_value::<ServerMessage>(json!({ "op": "poll_vote", "s": 9, "d": {} }))
            .is_err()
    );
    let body: ErrorBody = serde_json::from_value(json!({
        "error": { "code": "POLL_CLOSED", "message": "This poll is closed", "retry_after_ms": null }
    }))
    .unwrap();
    assert_eq!(body.error.code, ErrorCode::BadRequest);
    assert_eq!(body.error.message, "This poll is closed");

    // Strict on its own (a server refuses what it doesn't know); soft where a client reads it.
    assert!(serde_json::from_value::<RegistrationMode>(json!("by_lottery")).is_err());
    let instance: InstanceResponse = serde_json::from_value(json!({
        "name": "Hermes", "description": "", "version": "9.0.0", "protocol_version": 3,
        "registration_mode": "by_lottery",
        "limits": { "max_upload_mb": 25, "max_message_length": 4000 },
        "voice": { "enabled": true }, "features": ["x"]
    }))
    .unwrap();
    assert_eq!(instance.registration_mode, RegistrationMode::InviteOnly);

    let entry: AuditEntry = serde_json::from_value(json!({
        "id": "1", "actor_id": "2", "action": "poll_close",
        "target_id": null, "details": {}, "created_at": 0
    }))
    .unwrap();
    assert_eq!(entry.action, AuditAction::Unknown);
    assert!(!AuditAction::ALL.contains(&AuditAction::Unknown));
}
