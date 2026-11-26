//! SQLite access (PLAN.md section 7.3). All SQL lives in this module.
//!
//! Queries use `sqlx::query!`, checked at compile time against `.sqlx/` (offline mode), so building
//! needs no database. After adding or changing a query, regenerate `.sqlx/` from the
//! `crates/hermes-server` directory (needs `cargo install sqlx-cli --version 0.9.0
//! --no-default-features --features sqlite,rustls`):
//!
//! ```sh
//! export DATABASE_URL=sqlite:///tmp/hermes-dev.db   # any scratch path
//! sqlx database create && sqlx migrate run
//! cargo sqlx prepare -- --all-targets
//! ```

pub mod attachments;
pub mod audit;
pub mod auth;
pub mod channel_keys;
pub mod channels;
pub mod devices;
pub mod dms;
pub mod friends;
pub mod gateway;
pub mod invites;
pub mod members;
pub mod messages;
pub mod roles;
pub mod spaces;
pub mod state;
pub mod users;

use std::path::{Path, PathBuf};
use std::time::Duration;

use hermes_proto::id::Id;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

/// Name of the database file inside `storage.data_dir`.
pub const DB_FILE: &str = "hermes.db";

/// The write-ahead log is cut back to this many bytes after a checkpoint.
const JOURNAL_SIZE_LIMIT: u64 = 4 * 1024 * 1024;

/// Connections in the read pool. Writes go through a single connection (PLAN.md section 21).
const READ_CONNECTIONS: u32 = 4;

/// Migrations from `crates/hermes-server/migrations`, embedded into the binary.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// The database: a pool for reads and a single-connection pool for writes, so writers queue in
/// the pool instead of failing with `SQLITE_BUSY`.
#[derive(Debug, Clone)]
pub struct Db {
    pub read: SqlitePool,
    pub write: SqlitePool,
}

impl Db {
    /// Opens (creating if needed) `<data_dir>/hermes.db` and applies pending migrations.
    pub async fn open(data_dir: &Path) -> Result<Self, sqlx::Error> {
        std::fs::create_dir_all(data_dir)?;
        let options = connect_options(&data_dir.join(DB_FILE));

        let write = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await?;
        MIGRATOR.run(&write).await?;

        let read = SqlitePoolOptions::new()
            .max_connections(READ_CONNECTIONS)
            .connect_with(options)
            .await?;
        Ok(Self { read, write })
    }

    /// Closes both pools, checkpointing the WAL. First lets SQLite refresh its query statistics.
    pub async fn close(&self) {
        if let Err(err) = sqlx::query("PRAGMA optimize").execute(&self.write).await {
            tracing::warn!("could not optimize the database: {err}");
        }
        self.read.close().await;
        self.write.close().await;
    }
}

/// Bytes the database takes up on disk, its write-ahead log included.
pub fn database_size(data_dir: &Path) -> u64 {
    [DB_FILE.to_owned(), format!("{DB_FILE}-wal")]
        .iter()
        .filter_map(|name| std::fs::metadata(data_dir.join(name)).ok())
        .map(|file| file.len())
        .sum()
}

/// Writes a consistent copy of `<data_dir>/hermes.db` to `output` with `VACUUM INTO`, which is
/// safe while the server runs (WAL). Fails if `output` exists or the database does not.
pub async fn backup(data_dir: &Path, output: &Path) -> Result<(), sqlx::Error> {
    let source = data_dir.join(DB_FILE);
    if !source.is_file() {
        return Err(sqlx::Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} does not exist", source.display()),
        )));
    }
    if output.exists() {
        return Err(sqlx::Error::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", output.display()),
        )));
    }
    let options = connect_options(&source).create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    let result = sqlx::query("VACUUM INTO ?")
        .bind(output.to_string_lossy().into_owned())
        .execute(&pool)
        .await;
    pool.close().await;
    result.map(|_| ())
}

/// Puts the database file at `input` in place at `target`, after checking it, and returns where
/// the previous one went.
pub async fn restore(input: &Path, target: &Path) -> Result<Option<PathBuf>, String> {
    use sqlx::ConnectOptions as _;
    let mut conn = SqliteConnectOptions::new()
        .filename(input)
        .read_only(true)
        .connect()
        .await
        .map_err(|e| format!("{}: {e}", input.display()))?;
    let verdict: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut conn)
        .await
        .map_err(|e| e.to_string())?;
    if verdict != "ok" {
        return Err(format!("{} is damaged: {verdict}", input.display()));
    }
    drop(conn);
    let kept = if target.exists() {
        let kept = target.with_extension(format!("before-restore-{}", crate::time::now_ms()));
        std::fs::rename(target, &kept).map_err(|e| e.to_string())?;
        Some(kept)
    } else {
        None
    };
    for suffix in ["wal", "shm"] {
        let _ = std::fs::remove_file(target.with_extension(format!("db-{suffix}")));
    }
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::copy(input, target).map_err(|e| e.to_string())?;
    Ok(kept)
}

/// Pragmas from PLAN.md section 7.3, plus `synchronous = NORMAL`, the usual choice with WAL:
/// a power loss may lose the last commits but cannot corrupt the database.
fn connect_options(path: &PathBuf) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_millis(5000))
        .synchronous(SqliteSynchronous::Normal)
        // A burst of writes must not leave a large write-ahead log behind.
        .pragma("journal_size_limit", JOURNAL_SIZE_LIMIT.to_string())
        // What is deleted is overwritten, not just marked free.
        .pragma("secure_delete", "ON")
}

/// The IDs as a JSON array for `json_each` (SQLite has no array parameters).
fn id_list(ids: &[Id]) -> String {
    serde_json::to_string(&ids.iter().map(|id| id.to_i64()).collect::<Vec<_>>())
        .unwrap_or_else(|_| "[]".into())
}

/// Converts an `INTEGER` column to a narrower type, failing on out-of-range values.
fn int<T: TryFrom<i64>>(value: i64, column: &str) -> Result<T, sqlx::Error> {
    T::try_from(value).map_err(|_| sqlx::Error::ColumnDecode {
        index: column.to_owned(),
        source: format!("value {value} out of range").into(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use sqlx::Row;

    use super::*;

    /// A fresh directory under the system temp dir, removed on drop.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("hermes-server-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn pragma(pool: &SqlitePool, name: &str) -> String {
        // Test-only: `name` is a constant from the test below.
        let row = sqlx::query(sqlx::AssertSqlSafe(format!("PRAGMA {name}")))
            .fetch_one(pool)
            .await
            .unwrap();
        row.try_get::<String, _>(0)
            .or_else(|_| row.try_get::<i64, _>(0).map(|v| v.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn open_creates_db_and_applies_pragmas() {
        let dir = TempDir::new("open");
        let data = dir.path().join("data");
        let db = Db::open(&data).await.unwrap();
        assert!(data.join(DB_FILE).is_file());
        for pool in [&db.read, &db.write] {
            assert_eq!(pragma(pool, "journal_mode").await, "wal");
            assert_eq!(pragma(pool, "foreign_keys").await, "1");
            assert_eq!(pragma(pool, "busy_timeout").await, "5000");
            assert_eq!(pragma(pool, "synchronous").await, "1"); // NORMAL
        }
        db.close().await;
    }

    #[tokio::test]
    async fn reopen_keeps_data_and_skips_applied_migrations() {
        let dir = TempDir::new("reopen");
        let db = Db::open(dir.path()).await.unwrap();
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, created_at) VALUES (1, 'a', 'x', 0)",
        )
        .execute(&db.write)
        .await
        .unwrap();
        db.close().await;

        let db = Db::open(dir.path()).await.unwrap();
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&db.read)
            .await
            .unwrap();
        assert_eq!(count, 1);
        let (applied,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&db.read)
            .await
            .unwrap();
        assert_eq!(applied, MIGRATOR.iter().count() as i64);
        db.close().await;
    }

    #[tokio::test]
    async fn foreign_keys_are_enforced_and_cascade() {
        let dir = TempDir::new("fk");
        let db = Db::open(dir.path()).await.unwrap();
        let w = &db.write;
        // Unknown owner is rejected.
        assert!(
            sqlx::query(
                "INSERT INTO spaces (id, name, owner_id, created_at) VALUES (10, 's', 99, 0)"
            )
            .execute(w)
            .await
            .is_err()
        );
        for sql in [
            "INSERT INTO users (id, username, password_hash, created_at) VALUES (1, 'a', 'x', 0)",
            "INSERT INTO spaces (id, name, owner_id, created_at) VALUES (10, 's', 1, 0)",
            "INSERT INTO channels (id, space_id, kind, name, created_at) VALUES (20, 10, 'text', 'general', 0)",
            "INSERT INTO messages (id, channel_id, author_id, content) VALUES (30, 20, 1, 'hi')",
            "DELETE FROM spaces WHERE id = 10",
        ] {
            sqlx::query(sql).execute(w).await.unwrap();
        }
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages")
            .fetch_one(w)
            .await
            .unwrap();
        assert_eq!(count, 0, "channel and messages cascade with the space");
        db.close().await;
    }

    #[tokio::test]
    async fn full_text_search_follows_messages() {
        let dir = TempDir::new("fts");
        let db = Db::open(dir.path()).await.unwrap();
        let w = &db.write;
        for sql in [
            "INSERT INTO users (id, username, password_hash, created_at) VALUES (1, 'a', 'x', 0)",
            "INSERT INTO channels (id, kind, created_at) VALUES (20, 'dm', 0)",
            "INSERT INTO messages (id, channel_id, author_id, content) VALUES (30, 20, 1, 'pumpkin soup tonight')",
            "INSERT INTO messages (id, channel_id, author_id, content) VALUES (31, 20, 1, 'pumpkin pie')",
        ] {
            sqlx::query(sql).execute(w).await.unwrap();
        }
        let search = |term: &'static str| async move {
            let rows: Vec<(i64,)> = sqlx::query_as(
                "SELECT rowid FROM messages_fts WHERE messages_fts MATCH ? ORDER BY rowid",
            )
            .bind(term)
            .fetch_all(w)
            .await
            .unwrap();
            rows.into_iter().map(|r| r.0).collect::<Vec<_>>()
        };
        assert_eq!(search("pumpkin").await, [30, 31]);

        sqlx::query("UPDATE messages SET content = 'ghost stories', edited_at = 1 WHERE id = 30")
            .execute(w)
            .await
            .unwrap();
        assert_eq!(search("pumpkin").await, [31]);
        assert_eq!(search("ghost").await, [30]);

        // Soft delete clears content.
        sqlx::query("UPDATE messages SET content = '', deleted = 1 WHERE id = 31")
            .execute(w)
            .await
            .unwrap();
        assert!(search("pumpkin").await.is_empty());

        sqlx::query("DELETE FROM messages WHERE id = 30")
            .execute(w)
            .await
            .unwrap();
        assert!(search("ghost").await.is_empty());
        db.close().await;
    }

    /// #86: retention and the 0021 migration move a channel's last message off deleted or gone
    /// messages; channels that are fine are left alone.
    #[tokio::test]
    async fn stale_last_messages_move_back() {
        let dir = TempDir::new("last-message");
        let db = Db::open(dir.path()).await.unwrap();
        let w = &db.write;
        for sql in [
            "INSERT INTO users (id, username, password_hash, created_at) VALUES (1, 'a', 'x', 0)",
            "INSERT INTO channels (id, kind, created_at, last_message_id) VALUES (20, 'dm', 0, 32)",
            "INSERT INTO channels (id, kind, created_at, last_message_id) VALUES (21, 'dm', 0, 40)",
            "INSERT INTO channels (id, kind, created_at, last_message_id) VALUES (22, 'dm', 0, 50)",
            "INSERT INTO messages (id, channel_id, author_id, content) VALUES (30, 20, 1, 'a')",
            "INSERT INTO messages (id, channel_id, author_id, content, deleted) VALUES (31, 20, 1, '', 1)",
            "INSERT INTO messages (id, channel_id, author_id, content, deleted) VALUES (32, 20, 1, '', 1)",
            "INSERT INTO messages (id, channel_id, author_id, content) VALUES (50, 22, 1, 'c')",
        ] {
            sqlx::query(sql).execute(w).await.unwrap();
        }
        // 20: newest is deleted -> 30. 21: its message is gone -> none. 22: fine.
        let mut changed = messages::refresh_stale_last_message_ids(w).await.unwrap();
        changed.sort();
        assert_eq!(changed, [(Id(20), Some(Id(30))), (Id(21), None)]);
        assert!(
            messages::refresh_stale_last_message_ids(w)
                .await
                .unwrap()
                .is_empty()
        );
        db.close().await;
    }

    #[test]
    fn int_rejects_out_of_range() {
        assert_eq!(int::<u32>(5, "x").unwrap(), 5);
        assert!(int::<u32>(-1, "x").is_err());
        assert!(int::<i32>(i64::MAX, "x").is_err());
    }
}
