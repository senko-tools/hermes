//! Roles (PLAN.md section 7.3). Inserting is in `spaces`, next to the space it belongs to.

use hermes_proto::id::Id;
use hermes_proto::models::Role;
use sqlx::SqliteConnection;

/// Writes every column of `role` except `space_id`.
pub async fn update_role(conn: &mut SqliteConnection, role: &Role) -> Result<(), sqlx::Error> {
    let id = role.id.to_i64();
    let color = role.color.map(i64::from);
    let permissions = role.permissions.bits() as i64;
    sqlx::query!(
        "UPDATE roles SET name = ?, color = ?, hoist = ?, mentionable = ?, position = ?,
                          permissions = ?
         WHERE id = ?",
        role.name,
        color,
        role.hoist,
        role.mentionable,
        role.position,
        permissions,
        id
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn set_position(
    conn: &mut SqliteConnection,
    role_id: Id,
    position: i32,
) -> Result<(), sqlx::Error> {
    let id = role_id.to_i64();
    sqlx::query!("UPDATE roles SET position = ? WHERE id = ?", position, id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Deletes a role; member roles and overrides go with it (`ON DELETE CASCADE`).
pub async fn delete_role(conn: &mut SqliteConnection, role_id: Id) -> Result<(), sqlx::Error> {
    let id = role_id.to_i64();
    sqlx::query!("DELETE FROM roles WHERE id = ?", id)
        .execute(conn)
        .await?;
    Ok(())
}
