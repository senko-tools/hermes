//! The mentions inbox (`GET /users/@me/mentions`): the query driven by `message_mentions` returns
//! exactly what the earlier per-message scan returned, and the endpoint covers every kind of mention.

mod common;

use common::{TempDir, TestServer, start};
use hermes_proto::id::Id;
use hermes_proto::permissions::Permissions;
use hermes_server::config::RegistrationMode;
use hermes_server::db::{self, Db};
use serde_json::{Value, json};

/// The inbox query before migration 0019: every message of the readable channels, newest
/// first, checked for a matching mention one by one.
const SCAN_QUERY: &str = "
    SELECT m.id FROM messages m
    WHERE m.deleted = 0 AND m.author_id != ?1 AND m.id < ?4
      AND m.channel_id IN (SELECT value FROM json_each(?3))
      AND EXISTS (
        SELECT 1 FROM message_mentions mm WHERE mm.message_id = m.id AND (
          (mm.kind = 'user' AND mm.target_id = ?1)
          OR (mm.kind = 'role' AND mm.target_id IN (SELECT value FROM json_each(?2)))
          OR mm.kind IN ('everyone', 'here')))
    ORDER BY m.id DESC LIMIT ?5";

/// A small deterministic pseudo-random generator (no `rand` dependency).
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }
}

fn json_ids(ids: &[i64]) -> String {
    serde_json::to_string(ids).unwrap()
}

async fn exec(db: &Db, sql: String) {
    // Test data built from integers only.
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .execute(&db.write)
        .await
        .unwrap();
}

#[tokio::test]
async fn mention_driven_query_matches_the_message_scan() {
    let dir = TempDir::new("mentions-equivalence");
    let db = Db::open(dir.path()).await.unwrap();
    for user in 1..=4 {
        exec(
            &db,
            format!(
                "INSERT INTO users (id, username, password_hash, created_at)
                 VALUES ({user}, 'u{user}', 'x', 0)"
            ),
        )
        .await;
    }
    exec(
        &db,
        "INSERT INTO spaces (id, name, owner_id, created_at) VALUES (10, 's', 1, 0)".into(),
    )
    .await;
    for role in [10, 11, 12] {
        exec(
            &db,
            format!(
                "INSERT INTO roles (id, space_id, name, position, permissions)
                 VALUES ({role}, 10, 'r{role}', {role}, 0)"
            ),
        )
        .await;
    }
    for channel in 20..24 {
        exec(
            &db,
            format!(
                "INSERT INTO channels (id, space_id, kind, name, created_at)
                 VALUES ({channel}, 10, 'text', 'c{channel}', 0)"
            ),
        )
        .await;
    }

    let mut rng = Lcg(42);
    for id in 100..700_i64 {
        let channel = 20 + rng.below(4);
        let author = 1 + rng.below(4);
        let deleted = u64::from(rng.below(10) == 0);
        exec(
            &db,
            format!(
                "INSERT INTO messages (id, channel_id, author_id, content, deleted)
                 VALUES ({id}, {channel}, {author}, 'm', {deleted})"
            ),
        )
        .await;
        // Mostly no mentions; sometimes several kinds on one message (so one message can match
        // more than once and must still be listed once).
        for _ in 0..rng.below(4).saturating_sub(1) {
            let (kind, target) = match rng.below(6) {
                0 | 1 => ("user", (1 + rng.below(4)).to_string()),
                2 | 3 => ("role", (10 + rng.below(3)).to_string()),
                4 => ("everyone", "NULL".into()),
                _ => ("here", "NULL".into()),
            };
            exec(
                &db,
                format!(
                    "INSERT OR IGNORE INTO message_mentions (message_id, kind, target_id)
                     VALUES ({id}, '{kind}', {target})"
                ),
            )
            .await;
        }
    }

    let role_sets: [&[i64]; 3] = [&[], &[11], &[11, 12]];
    let channel_sets: [&[i64]; 3] = [&[20, 21, 22, 23], &[21, 23], &[]];
    let mut checked = 0;
    for user in 1..=4_i64 {
        for roles in role_sets {
            for channels in channel_sets {
                for before in [None, Some(650_i64), Some(400), Some(101)] {
                    for limit in [1_u32, 7, 50, 100] {
                        let scan: Vec<i64> = sqlx::query_scalar(SCAN_QUERY)
                            .bind(user)
                            .bind(json_ids(roles))
                            .bind(json_ids(channels))
                            .bind(before.unwrap_or(i64::MAX))
                            .bind(i64::from(limit))
                            .fetch_all(&db.read)
                            .await
                            .unwrap();
                        let ids = |list: &[i64]| list.iter().map(|&i| Id::from_i64(i)).collect();
                        let roles: Vec<Id> = ids(roles);
                        let channels: Vec<Id> = ids(channels);
                        let found: Vec<i64> = db::messages::mentioning(
                            &db.read,
                            Id::from_i64(user),
                            &roles,
                            &channels,
                            before.map(Id::from_i64),
                            limit,
                        )
                        .await
                        .unwrap()
                        .iter()
                        .map(|m| m.id.to_i64())
                        .collect();
                        assert_eq!(
                            found, scan,
                            "user {user} roles {roles:?} channels {channels:?} before {before:?} limit {limit}"
                        );
                        checked += usize::from(!scan.is_empty());
                    }
                }
            }
        }
    }
    assert!(checked > 100, "the data should produce non-empty pages");
    db.close().await;
}

async fn send(server: &TestServer, token: &str, channel: &str, content: &str) -> Value {
    server
        .post(
            &format!("/channels/{channel}/messages"),
            Some(token),
            json!({ "content": content, "nonce": common::nonce() }),
        )
        .await
        .ok()
}

fn contents(page: &Value) -> Vec<&str> {
    page.as_array()
        .unwrap()
        .iter()
        .map(|m| m["content"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn inbox_lists_every_kind_of_mention_once_and_pages() {
    let server = start("mentions-kinds", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let general = server.channel_id(&space_id, "general");
    let role = server.role(&owner, &space_id, "helpers", 0).await;
    server.assign(&owner, &space_id, &alice_id, &[&role]).await;
    let secret = server
        .post(
            &format!("/spaces/{space_id}/channels"),
            Some(&owner),
            json!({ "kind": "text", "name": "secret" }),
        )
        .await
        .ok()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    server
        .put(
            &format!("/channels/{secret}/overrides/{space_id}"),
            Some(&owner),
            json!({ "allow": "0", "deny": Permissions::VIEW_CHANNEL.bits().to_string() }),
        )
        .await
        .ok();

    send(&server, &owner, &general, "@everyone standup").await;
    send(&server, &owner, &general, "@here lunch").await;
    send(
        &server,
        &owner,
        &general,
        &format!("both <@{alice_id}> <@&{role}>"),
    )
    .await;
    send(&server, &owner, &secret, &format!("hidden <@{alice_id}>")).await;
    let gone = send(&server, &owner, &general, &format!("gone <@{alice_id}>")).await;
    send(&server, &owner, &general, "no mention").await;
    server
        .delete(
            &format!(
                "/channels/{general}/messages/{}",
                gone["id"].as_str().unwrap()
            ),
            Some(&owner),
        )
        .await
        .ok();

    let inbox = server.get("/users/@me/mentions", Some(&alice)).await.ok();
    assert_eq!(
        contents(&inbox),
        [
            format!("both <@{alice_id}> <@&{role}>"),
            "@here lunch".into(),
            "@everyone standup".into(),
        ],
        "newest first, listed once, deleted and unreadable messages left out"
    );

    let first = server
        .get("/users/@me/mentions?limit=1", Some(&alice))
        .await
        .ok();
    assert_eq!(contents(&first).len(), 1);
    let before = first[0]["id"].as_str().unwrap();
    let rest = server
        .get(
            &format!("/users/@me/mentions?before={before}"),
            Some(&alice),
        )
        .await
        .ok();
    assert_eq!(contents(&rest), ["@here lunch", "@everyone standup"]);
}
