//! Queries that load everything the state cache holds (PLAN.md section 4: "State cache").

use hermes_proto::id::Id;
use hermes_proto::models::{AttachmentRef, Channel, ChannelKind, Override, Role, Space, User};
use hermes_proto::permissions::Permissions;
use sqlx::SqlitePool;

use super::int;

/// A user row: the wire `User` plus the server-only flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRow {
    pub user: User,
    pub instance_admin: bool,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRow {
    pub space_id: Id,
    pub user_id: Id,
    pub nickname: Option<String>,
    pub joined_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberRoleRow {
    pub space_id: Id,
    pub user_id: Id,
    pub role_id: Id,
}

/// Every cached table, as loaded at startup.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub users: Vec<UserRow>,
    pub spaces: Vec<Space>,
    pub roles: Vec<Role>,
    pub members: Vec<MemberRow>,
    pub member_roles: Vec<MemberRoleRow>,
    /// `overrides` and `recipients` are empty; they come in `overrides` and `dm_recipients`.
    pub channels: Vec<Channel>,
    pub overrides: Vec<(Id, Override)>,
    /// `(channel_id, user_id)`.
    pub dm_recipients: Vec<(Id, Id)>,
}

pub async fn load(pool: &SqlitePool) -> Result<Snapshot, sqlx::Error> {
    Ok(Snapshot {
        users: users(pool).await?,
        spaces: spaces(pool).await?,
        roles: roles(pool).await?,
        members: members(pool).await?,
        member_roles: member_roles(pool).await?,
        channels: channels(pool).await?,
        overrides: overrides(pool).await?,
        dm_recipients: dm_recipients(pool).await?,
    })
}

fn attachment(id: Option<i64>, filename: Option<String>) -> Option<AttachmentRef> {
    Some(AttachmentRef {
        id: Id::from_i64(id?),
        filename: filename?,
    })
}

fn permissions(bits: i64) -> Permissions {
    // Unknown bits are kept so that writing a row back never loses them.
    Permissions::from_bits_retain(bits as u64)
}

async fn users(pool: &SqlitePool) -> Result<Vec<UserRow>, sqlx::Error> {
    sqlx::query!(
        r#"SELECT u.id, u.username, u.display_name, u.status_text,
                  u.instance_admin AS "instance_admin: bool", u.disabled AS "disabled: bool",
                  u.avatar_id, a.filename AS "avatar_filename?"
           FROM users u LEFT JOIN attachments a ON a.id = u.avatar_id"#
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| {
        Ok(UserRow {
            user: User {
                id: Id::from_i64(r.id),
                username: r.username,
                display_name: r.display_name,
                avatar: attachment(r.avatar_id, r.avatar_filename),
                status_text: r.status_text,
            },
            instance_admin: r.instance_admin,
            disabled: r.disabled,
        })
    })
    .collect()
}

async fn spaces(pool: &SqlitePool) -> Result<Vec<Space>, sqlx::Error> {
    sqlx::query!(
        r#"SELECT s.id, s.name, s.description, s.owner_id, s.icon_id, a.filename AS "icon_filename?"
           FROM spaces s LEFT JOIN attachments a ON a.id = s.icon_id"#
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| {
        Ok(Space {
            id: Id::from_i64(r.id),
            name: r.name,
            description: r.description,
            icon: attachment(r.icon_id, r.icon_filename),
            owner_id: Id::from_i64(r.owner_id),
        })
    })
    .collect()
}

async fn roles(pool: &SqlitePool) -> Result<Vec<Role>, sqlx::Error> {
    sqlx::query!(
        r#"SELECT id, space_id, name, color, hoist AS "hoist: bool",
                  mentionable AS "mentionable: bool", position, permissions
           FROM roles"#
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| {
        Ok(Role {
            id: Id::from_i64(r.id),
            space_id: Id::from_i64(r.space_id),
            name: r.name,
            color: r.color.map(|c| int(c, "roles.color")).transpose()?,
            hoist: r.hoist,
            mentionable: r.mentionable,
            position: int(r.position, "roles.position")?,
            permissions: permissions(r.permissions),
        })
    })
    .collect()
}

async fn members(pool: &SqlitePool) -> Result<Vec<MemberRow>, sqlx::Error> {
    let rows = sqlx::query!("SELECT space_id, user_id, nickname, joined_at FROM space_members")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| MemberRow {
            space_id: Id::from_i64(r.space_id),
            user_id: Id::from_i64(r.user_id),
            nickname: r.nickname,
            joined_at: r.joined_at,
        })
        .collect())
}

async fn member_roles(pool: &SqlitePool) -> Result<Vec<MemberRoleRow>, sqlx::Error> {
    let rows = sqlx::query!("SELECT space_id, user_id, role_id FROM member_roles")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| MemberRoleRow {
            space_id: Id::from_i64(r.space_id),
            user_id: Id::from_i64(r.user_id),
            role_id: Id::from_i64(r.role_id),
        })
        .collect())
}

fn channel_kind(kind: &str) -> Result<ChannelKind, sqlx::Error> {
    Ok(match kind {
        "text" => ChannelKind::Text,
        "voice" => ChannelKind::Voice,
        "category" => ChannelKind::Category,
        "dm" => ChannelKind::Dm,
        "group_dm" => ChannelKind::GroupDm,
        other => {
            return Err(sqlx::Error::ColumnDecode {
                index: "channels.kind".into(),
                source: format!("unknown channel kind {other:?}").into(),
            });
        }
    })
}

async fn channels(pool: &SqlitePool) -> Result<Vec<Channel>, sqlx::Error> {
    sqlx::query!(
        "SELECT id, space_id, kind, parent_id, name, topic, position, bitrate, user_limit,
                slowmode_secs, owner_id, last_message_id
         FROM channels"
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| {
        Ok(Channel {
            id: Id::from_i64(r.id),
            space_id: r.space_id.map(Id::from_i64),
            kind: channel_kind(&r.kind)?,
            parent_id: r.parent_id.map(Id::from_i64),
            name: r.name,
            topic: r.topic,
            position: int(r.position, "channels.position")?,
            bitrate: r.bitrate.map(|b| int(b, "channels.bitrate")).transpose()?,
            user_limit: r
                .user_limit
                .map(|l| int(l, "channels.user_limit"))
                .transpose()?,
            slowmode_secs: int(r.slowmode_secs, "channels.slowmode_secs")?,
            overrides: Vec::new(),
            recipients: Vec::new(),
            owner_id: r.owner_id.map(Id::from_i64),
            last_message_id: r.last_message_id.map(Id::from_i64),
        })
    })
    .collect()
}

async fn overrides(pool: &SqlitePool) -> Result<Vec<(Id, Override)>, sqlx::Error> {
    let rows = sqlx::query!("SELECT channel_id, role_id, allow, deny FROM channel_overrides")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let o = Override {
                role_id: Id::from_i64(r.role_id),
                allow: permissions(r.allow),
                deny: permissions(r.deny),
            };
            (Id::from_i64(r.channel_id), o)
        })
        .collect())
}

async fn dm_recipients(pool: &SqlitePool) -> Result<Vec<(Id, Id)>, sqlx::Error> {
    let rows = sqlx::query!("SELECT channel_id, user_id FROM dm_recipients")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| (Id::from_i64(r.channel_id), Id::from_i64(r.user_id)))
        .collect())
}
