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
pub mod channels;
pub mod dms;
pub mod gateway;
pub mod invites;
pub mod members;
pub mod messages;
pub mod roles;
pub mod spaces;
pub mod state;

use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

/// Name of the database file inside `storage.data_dir`.
pub const DB_FILE: &str = "hermes.db";

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

    /// Closes both pools, checkpointing the WAL.
    pub async fn close(&self) {
        self.read.close().await;
        self.write.close().await;
    }
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

    #[test]
    fn int_rejects_out_of_range() {
        assert_eq!(int::<u32>(5, "x").unwrap(), 5);
        assert!(int::<u32>(-1, "x").is_err());
        assert!(int::<i32>(i64::MAX, "x").is_err());
    }
}
