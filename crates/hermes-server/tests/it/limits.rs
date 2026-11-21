//! Limits the instance admin sets in `hermes.toml`: clients are told them, and the server holds
//! everyone to them.

use crate::common;

use common::start_with;
use hermes_server::config::RegistrationMode;
use hermes_server::time::now_ms;
use serde_json::{Value, json};

fn id(v: &Value) -> String {
    v["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn lowered_limits_are_told_and_held() {
    let server = start_with("limits", RegistrationMode::Open, |state| {
        let limits = &mut state.config.limits;
        limits.invite_default_days = 1;
        limits.invite_max_days = 2;
        limits.max_files_per_message = 1;
        limits.max_reactions_per_message = 2;
        limits.max_slowmode_minutes = 1;
        limits.max_pins_per_channel = 1;
    })
    .await;
    let (owner, _, space_id) = server.owner().await;
    let general = server.channel_id(&space_id, "general");
    let messages = format!("/channels/{general}/messages");

    // Clients learn them from GET /instance (and READY, built the same way).
    let info = server.get("/instance", None).await.ok();
    let limits = &info["limits"];
    assert_eq!(limits["invite_default_secs"], 24 * 3600);
    assert_eq!(limits["invite_max_secs"], 2 * 24 * 3600);
    assert_eq!(limits["max_files"], 1);
    assert_eq!(limits["max_reactions"], 2);
    assert_eq!(limits["max_slowmode_secs"], 60);
    assert_eq!(limits["max_pins"], 1);
    let (_, ready) = server.identify(&owner).await;
    assert_eq!(ready["instance"]["limits"], *limits);

    // Invites: no lifetime given lasts a day; more than two days is refused.
    let invites = format!("/spaces/{space_id}/invites");
    let before = now_ms();
    let invite = server.post(&invites, Some(&owner), json!({})).await.ok();
    let day = 24 * 3600 * 1000;
    let expires_at = invite["expires_at"].as_i64().unwrap();
    assert!((before + day..=now_ms() + day).contains(&expires_at));
    server
        .post(
            &invites,
            Some(&owner),
            json!({ "expires_in_secs": 2 * 24 * 3600 + 1 }),
        )
        .await
        .err(400, "VALIDATION_FAILED");

    // Files: one per message, plus its encrypted preview; three uploads are too many.
    let mut uploads = Vec::new();
    for name in ["a.txt", "b.txt", "c.txt"] {
        uploads.push(id(&server
            .upload(&owner, name, "text/plain", b"hi".to_vec())
            .await
            .ok()));
    }
    server
        .post(
            &messages,
            Some(&owner),
            json!({ "content": "files", "nonce": common::nonce(), "attachment_ids": uploads }),
        )
        .await
        .err(400, "VALIDATION_FAILED");

    // Reactions: two different emoji, then no more.
    let m1 = id(&server.send_message(&owner, &general, "one").await);
    let message = format!("{messages}/{m1}");
    for e in ["😀", "😃"] {
        server
            .put(
                &format!("{message}/reactions/{e}"),
                Some(&owner),
                json!(null),
            )
            .await
            .ok();
    }
    server
        .put(
            &format!("{message}/reactions/😄"),
            Some(&owner),
            json!(null),
        )
        .await
        .err(400, "BAD_REQUEST");

    // Slow mode: up to a minute.
    let channel = format!("/channels/{general}");
    server
        .patch(&channel, Some(&owner), json!({ "slowmode_secs": 61 }))
        .await
        .err(400, "VALIDATION_FAILED");
    server
        .patch(&channel, Some(&owner), json!({ "slowmode_secs": 60 }))
        .await
        .ok();
    server
        .patch(&channel, Some(&owner), json!({ "slowmode_secs": 0 }))
        .await
        .ok();

    // Pins: one per channel; unpinning makes room again.
    let m2 = id(&server.send_message(&owner, &general, "two").await);
    server
        .put(&format!("{messages}/{m1}/pin"), Some(&owner), json!(null))
        .await
        .ok();
    server
        .put(&format!("{messages}/{m2}/pin"), Some(&owner), json!(null))
        .await
        .err(400, "BAD_REQUEST");
    server
        .delete(&format!("{messages}/{m1}/pin"), Some(&owner))
        .await
        .ok();
    server
        .put(&format!("{messages}/{m2}/pin"), Some(&owner), json!(null))
        .await
        .ok();
    let pins = server
        .get(&format!("/channels/{general}/pins"), Some(&owner))
        .await
        .ok();
    assert_eq!(pins.as_array().unwrap().len(), 1);
}

#[test]
fn invite_default_above_its_maximum_is_refused() {
    let mut config = hermes_server::config::Config::default();
    config.limits.invite_default_days = 31;
    let (errors, _) = config.check();
    assert!(
        errors.iter().any(|e| e.contains("invite_default_days")),
        "{errors:?}"
    );
}
