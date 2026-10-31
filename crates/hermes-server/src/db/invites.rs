//! Invites (PLAN.md sections 7.3 and 10).

use hermes_proto::id::Id;
use sqlx::{SqliteConnection, SqliteExecutor};

use super::int;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteRow {
    pub code: String,
    /// `None` = instance-only invite (registration without joining a space).
    pub space_id: Option<Id>,
    /// `None` = created by the server (setup invite, CLI).
    pub creator_id: Option<Id>,
    pub max_uses: Option<u32>,
    pub uses: u32,
    pub expires_at: Option<i64>,
    pub created_at: i64,
}

impl InviteRow {
    /// Not expired and not used up.
    pub fn is_usable(&self, now: i64) -> bool {
        self.expires_at.is_none_or(|at| at > now) && self.max_uses.is_none_or(|max| self.uses < max)
    }
}

pub async fn insert(e: impl SqliteExecutor<'_>, invite: &InviteRow) -> Result<(), sqlx::Error> {
    let space_id = invite.space_id.map(Id::to_i64);
    let creator_id = invite.creator_id.map(Id::to_i64);
    let max_uses = invite.max_uses.map(i64::from);
    let uses = i64::from(invite.uses);
    sqlx::query!(
        "INSERT INTO invites (code, space_id, creator_id, max_uses, uses, expires_at, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        invite.code,
        space_id,
        creator_id,
        max_uses,
        uses,
        invite.expires_at,
        invite.created_at
    )
    .execute(e)
    .await?;
    Ok(())
}

struct Raw {
    code: String,
    space_id: Option<i64>,
    creator_id: Option<i64>,
    max_uses: Option<i64>,
    uses: i64,
    expires_at: Option<i64>,
    created_at: i64,
}

impl Raw {
    fn into_row(self) -> Result<InviteRow, sqlx::Error> {
        Ok(InviteRow {
            code: self.code,
            space_id: self.space_id.map(Id::from_i64),
            creator_id: self.creator_id.map(Id::from_i64),
            max_uses: self
                .max_uses
                .map(|m| int(m, "invites.max_uses"))
                .transpose()?,
            uses: int(self.uses, "invites.uses")?,
            expires_at: self.expires_at,
            created_at: self.created_at,
        })
    }
}

pub async fn get(e: impl SqliteExecutor<'_>, code: &str) -> Result<Option<InviteRow>, sqlx::Error> {
    sqlx::query_as!(
        Raw,
        "SELECT code, space_id, creator_id, max_uses, uses, expires_at, created_at
         FROM invites WHERE code = ?",
        code
    )
    .fetch_optional(e)
    .await?
    .map(Raw::into_row)
    .transpose()
}

/// All invites of a space, oldest first.
pub async fn list_for_space(
    e: impl SqliteExecutor<'_>,
    space_id: Id,
) -> Result<Vec<InviteRow>, sqlx::Error> {
    let space_id = space_id.to_i64();
    sqlx::query_as!(
        Raw,
        "SELECT code, space_id, creator_id, max_uses, uses, expires_at, created_at
         FROM invites WHERE space_id = ? ORDER BY created_at, code",
        space_id
    )
    .fetch_all(e)
    .await?
    .into_iter()
    .map(Raw::into_row)
    .collect()
}

/// The newest usable instance-only invite created by the server, if any (the setup invite of a
/// previous start that nobody used yet).
pub async fn usable_server_instance_invite(
    e: impl SqliteExecutor<'_>,
    now: i64,
) -> Result<Option<InviteRow>, sqlx::Error> {
    sqlx::query_as!(
        Raw,
        "SELECT code, space_id, creator_id, max_uses, uses, expires_at, created_at
         FROM invites
         WHERE space_id IS NULL AND creator_id IS NULL
           AND (expires_at IS NULL OR expires_at > ?)
           AND (max_uses IS NULL OR uses < max_uses)
         ORDER BY created_at DESC LIMIT 1",
        now
    )
    .fetch_optional(e)
    .await?
    .map(Raw::into_row)
    .transpose()
}

pub async fn delete(e: impl SqliteExecutor<'_>, code: &str) -> Result<bool, sqlx::Error> {
    let done = sqlx::query!("DELETE FROM invites WHERE code = ?", code)
        .execute(e)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// Takes one use of an invite if it is still usable. Returns `None` if the invite does not exist,
/// has expired or is used up; otherwise `Some(space_id)`.
pub async fn take_use(
    conn: &mut SqliteConnection,
    code: &str,
    now: i64,
) -> Result<Option<Option<Id>>, sqlx::Error> {
    let row = sqlx::query!(
        "UPDATE invites SET uses = uses + 1
         WHERE code = ?
           AND (expires_at IS NULL OR expires_at > ?)
           AND (max_uses IS NULL OR uses < max_uses)
         RETURNING space_id",
        code,
        now
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|r| r.space_id.map(Id::from_i64)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(max_uses: Option<u32>, uses: u32, expires_at: Option<i64>) -> InviteRow {
        InviteRow {
            code: "AbC123xYz9".into(),
            space_id: None,
            creator_id: None,
            max_uses,
            uses,
            expires_at,
            created_at: 0,
        }
    }

    #[test]
    fn usable() {
        assert!(row(None, 1000, None).is_usable(5));
        assert!(row(Some(2), 1, Some(6)).is_usable(5));
        assert!(!row(Some(2), 2, None).is_usable(5));
        assert!(!row(None, 0, Some(5)).is_usable(5));
    }
}
