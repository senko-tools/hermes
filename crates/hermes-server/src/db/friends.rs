//! The `relationships` table: friend requests and friends.

use hermes_proto::id::Id;
use sqlx::{SqliteConnection, SqlitePool};

/// One row, as it is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub user_low: Id,
    pub user_high: Id,
    pub requester: Id,
    pub created_at: i64,
    pub accepted_at: Option<i64>,
}

fn ordered(a: Id, b: Id) -> (i64, i64) {
    let (a, b) = (a.to_i64(), b.to_i64());
    (a.min(b), a.max(b))
}

pub async fn all(pool: &SqlitePool) -> Result<Vec<Row>, sqlx::Error> {
    let rows = sqlx::query!(
        "SELECT user_low, user_high, requester_id, created_at, accepted_at FROM relationships"
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Row {
            user_low: Id::from_i64(r.user_low),
            user_high: Id::from_i64(r.user_high),
            requester: Id::from_i64(r.requester_id),
            created_at: r.created_at,
            accepted_at: r.accepted_at,
        })
        .collect())
}

pub async fn insert_request(
    conn: &mut SqliteConnection,
    requester: Id,
    other: Id,
    now: i64,
) -> Result<(), sqlx::Error> {
    let (low, high) = ordered(requester, other);
    let requester = requester.to_i64();
    sqlx::query!(
        "INSERT INTO relationships (user_low, user_high, requester_id, created_at)
         VALUES (?, ?, ?, ?)",
        low,
        high,
        requester,
        now
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn accept(
    conn: &mut SqliteConnection,
    a: Id,
    b: Id,
    now: i64,
) -> Result<(), sqlx::Error> {
    let (low, high) = ordered(a, b);
    sqlx::query!(
        "UPDATE relationships SET accepted_at = ? WHERE user_low = ? AND user_high = ?",
        now,
        low,
        high
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Whether there was anything to delete.
pub async fn delete(conn: &mut SqliteConnection, a: Id, b: Id) -> Result<bool, sqlx::Error> {
    let (low, high) = ordered(a, b);
    let done = sqlx::query!(
        "DELETE FROM relationships WHERE user_low = ? AND user_high = ?",
        low,
        high
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected() > 0)
}
