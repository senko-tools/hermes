//! `/spaces/{id}/roles`: create, update, delete and reorder (`MANAGE_ROLES` + hierarchy), and
//! their gateway events.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::ErrorCode;
use hermes_proto::api::{CreateRoleRequest, RolePosition, UpdateRoleRequest};
use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;
use hermes_proto::models::{AuditAction, Role};
use hermes_proto::permissions::Permissions;
use serde_json::{Map, Value, json};

use super::validate::{self, Fields};
use super::{ApiResult, Error, Json, Path, audit, perms};
use crate::auth::Auth;
use crate::db;
use crate::gateway::bus::Audience;
use crate::gateway::visibility::{self, Visible};
use crate::state::AppState;

/// Largest `0xRRGGBB` value.
const MAX_COLOR: u32 = 0xFF_FF_FF;

fn check_color(fields: &mut Fields, color: Option<u32>) {
    if color.is_some_and(|c| c > MAX_COLOR) {
        fields.add("color", "must be 0xRRGGBB (at most 16777215)");
    }
}

/// The new role goes in at position 1, right above `@everyone`; the other roles move up by one.
/// It can only get permission bits the caller has.
pub async fn create(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space_id): Path<Id>,
    Json(req): Json<CreateRoleRequest>,
) -> ApiResult<Json<Role>> {
    let mut fields = Fields::new();
    let name = fields.check("name", validate::name(&req.name, validate::ROLE_NAME_CHARS));
    check_color(&mut fields, req.color);
    fields.finish()?;
    let Some(name) = name else {
        unreachable!("finish() returned Ok, so every field is valid");
    };

    let _writes = state.writes.lock().await;
    let (role, shifted) = {
        let cache = state.cache();
        let access =
            perms::require_space(&cache, space_id, auth.user_id, Permissions::MANAGE_ROLES)?;
        let permissions = req.permissions.unwrap_or_default();
        if !access.may_grant(permissions) {
            return Err(perms::missing(
                permissions.difference(access.perms),
                "space",
            ));
        }
        let shifted: Vec<(Id, i32)> = access
            .space
            .roles
            .iter()
            .filter(|r| !r.is_everyone() && r.position >= 1)
            .map(|r| (r.id, r.position + 1))
            .collect();
        let role = Role {
            id: state.ids.next(),
            space_id,
            name,
            color: req.color,
            hoist: req.hoist.unwrap_or(false),
            mentionable: req.mentionable.unwrap_or(false),
            position: 1,
            permissions,
        };
        (role, shifted)
    };

    let mut tx = state.db.write.begin().await?;
    for &(id, position) in &shifted {
        db::roles::set_position(&mut tx, id, position).await?;
    }
    db::spaces::insert_role(&mut tx, &role).await?;
    audit::record(
        &mut tx,
        &state,
        space_id,
        auth.user_id,
        AuditAction::RoleCreate,
        Some(role.id),
        json!({ "name": role.name, "permissions": role.permissions }),
    )
    .await?;
    tx.commit().await?;
    if let Some(space) = state.cache_mut().spaces.get_mut(&space_id) {
        set_positions(&mut space.roles, &shifted);
        space.roles.push(role.clone());
        space.sort_roles();
    }
    // Clients shift the other roles' positions up by one themselves, as the server did.
    state.publish(
        ServerEvent::RoleCreate(role.clone()),
        Audience::space(space_id),
    );
    Ok(Json(role))
}

fn set_positions(roles: &mut [Role], positions: &[(Id, i32)]) {
    for role in roles {
        if let Some(&(_, position)) = positions.iter().find(|(id, _)| *id == role.id) {
            role.position = position;
        }
    }
}

/// Only roles below the caller's highest role. Added permission bits must be ones the caller has.
/// Of `@everyone`, only the permissions can change.
pub async fn update(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((space_id, role_id)): Path<(Id, Id)>,
    Json(req): Json<UpdateRoleRequest>,
) -> ApiResult<Json<Role>> {
    let _writes = state.writes.lock().await;
    let (role, details, before) = {
        let cache = state.cache();
        let access =
            perms::require_space(&cache, space_id, auth.user_id, Permissions::MANAGE_ROLES)?;
        let old = access
            .space
            .role(role_id)
            .ok_or_else(|| Error::not_found("role"))?;
        if !access.outranks_role(old) {
            return Err(access.hierarchy_error("roles"));
        }
        if old.is_everyone()
            && (req.name.is_some()
                || req.color.is_some()
                || req.hoist.is_some()
                || req.mentionable.is_some())
        {
            return Err(Error::new(
                ErrorCode::BadRequest,
                "Only the permissions of @everyone can be changed",
            ));
        }

        let mut role = old.clone();
        let mut fields = Fields::new();
        if let Some(name) = &req.name
            && let Some(name) =
                fields.check("name", validate::name(name, validate::ROLE_NAME_CHARS))
        {
            role.name = name;
        }
        if let Some(color) = req.color {
            check_color(&mut fields, color);
            role.color = color;
        }
        fields.finish()?;
        role.hoist = req.hoist.unwrap_or(role.hoist);
        role.mentionable = req.mentionable.unwrap_or(role.mentionable);
        if let Some(permissions) = req.permissions {
            let added = permissions.difference(old.permissions);
            if !access.may_grant(added) {
                return Err(perms::missing(added.difference(access.perms), "space"));
            }
            role.permissions = permissions;
        }

        let mut details = Map::new();
        if role.name != old.name {
            details.insert("name".into(), json!(role.name));
        }
        if role.color != old.color {
            details.insert("color".into(), json!(role.color));
        }
        if role.hoist != old.hoist {
            details.insert("hoist".into(), json!(role.hoist));
        }
        if role.mentionable != old.mentionable {
            details.insert("mentionable".into(), json!(role.mentionable));
        }
        if role.permissions != old.permissions {
            details.insert("permissions".into(), json!(role.permissions));
        }
        (role, details, Visible::of_space(&cache, space_id))
    };
    if details.is_empty() {
        return Ok(Json(role));
    }

    let mut tx = state.db.write.begin().await?;
    db::roles::update_role(&mut tx, &role).await?;
    audit::record(
        &mut tx,
        &state,
        space_id,
        auth.user_id,
        AuditAction::RoleUpdate,
        Some(role_id),
        Value::Object(details),
    )
    .await?;
    tx.commit().await?;
    if let Some(cached) = state
        .cache_mut()
        .spaces
        .get_mut(&space_id)
        .and_then(|s| s.roles.iter_mut().find(|r| r.id == role_id))
    {
        *cached = role.clone();
    }
    state.publish(
        ServerEvent::RoleUpdate(role.clone()),
        Audience::space(space_id),
    );
    visibility::publish_changes(&state, space_id, &before);
    Ok(Json(role))
}

/// Removes the role from every member and every channel override. `@everyone` cannot be deleted.
pub async fn delete(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((space_id, role_id)): Path<(Id, Id)>,
) -> ApiResult<StatusCode> {
    let _writes = state.writes.lock().await;
    let (name, before) = {
        let cache = state.cache();
        let access =
            perms::require_space(&cache, space_id, auth.user_id, Permissions::MANAGE_ROLES)?;
        let role = access
            .space
            .role(role_id)
            .ok_or_else(|| Error::not_found("role"))?;
        if role.is_everyone() {
            return Err(Error::new(
                ErrorCode::BadRequest,
                "The @everyone role cannot be deleted",
            ));
        }
        if !access.outranks_role(role) {
            return Err(access.hierarchy_error("roles"));
        }
        (role.name.clone(), Visible::of_space(&cache, space_id))
    };

    let mut tx = state.db.write.begin().await?;
    db::roles::delete_role(&mut tx, role_id).await?;
    audit::record(
        &mut tx,
        &state,
        space_id,
        auth.user_id,
        AuditAction::RoleDelete,
        Some(role_id),
        json!({ "name": name }),
    )
    .await?;
    tx.commit().await?;

    {
        let mut cache = state.cache_mut();
        if let Some(space) = cache.spaces.get_mut(&space_id) {
            space.roles.retain(|r| r.id != role_id);
            for member in space.members.values_mut() {
                member.role_ids.retain(|&id| id != role_id);
            }
        }
        for channel in cache.channels.values_mut() {
            if channel.channel.space_id == Some(space_id) {
                channel.channel.overrides.retain(|o| o.role_id != role_id);
            }
        }
    }
    // Clients drop the role from members and overrides themselves.
    state.publish(
        ServerEvent::RoleDelete {
            id: role_id,
            space_id,
        },
        Audience::space(space_id),
    );
    visibility::publish_changes(&state, space_id, &before);
    Ok(StatusCode::NO_CONTENT)
}

/// Moves roles. Only roles below the caller's highest role can move, and only to positions below
/// it. Afterwards the positions are renumbered 1..n in the new order (ties broken by ID).
/// Returns every role of the space.
pub async fn reorder(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space_id): Path<Id>,
    Json(req): Json<Vec<RolePosition>>,
) -> ApiResult<Json<Vec<Role>>> {
    let _writes = state.writes.lock().await;
    let changed = {
        let cache = state.cache();
        let access =
            perms::require_space(&cache, space_id, auth.user_id, Permissions::MANAGE_ROLES)?;
        let highest = access.highest();
        let mut requested: HashMap<Id, i32> = HashMap::new();
        for p in &req {
            let role = access
                .space
                .role(p.id)
                .filter(|r| !r.is_everyone())
                .ok_or_else(|| validate::field_error("roles", format!("unknown role {}", p.id)))?;
            if p.position < 1 {
                return Err(validate::field_error(
                    "roles",
                    "positions must be at least 1 (0 is @everyone)",
                ));
            }
            if requested.insert(p.id, p.position).is_some() {
                return Err(validate::field_error(
                    "roles",
                    format!("role {} is listed twice", p.id),
                ));
            }
            if role.position >= highest || p.position >= highest {
                return Err(access.hierarchy_error("roles"));
            }
        }
        new_positions(&access.space.roles, &requested)
    };

    if !changed.is_empty() {
        let mut tx = state.db.write.begin().await?;
        for &(id, position) in &changed {
            db::roles::set_position(&mut tx, id, position).await?;
        }
        let positions: Map<String, Value> = changed
            .iter()
            .map(|(id, position)| (id.to_string(), json!(position)))
            .collect();
        audit::record(
            &mut tx,
            &state,
            space_id,
            auth.user_id,
            AuditAction::RolesReorder,
            None,
            json!({ "positions": positions }),
        )
        .await?;
        tx.commit().await?;
    }
    let roles = {
        let mut cache = state.cache_mut();
        let space = cache
            .spaces
            .get_mut(&space_id)
            .ok_or_else(|| Error::not_found("space"))?;
        set_positions(&mut space.roles, &changed);
        space.sort_roles();
        space.roles.clone()
    };
    for role in roles
        .iter()
        .filter(|r| changed.iter().any(|(id, _)| *id == r.id))
    {
        state.publish(
            ServerEvent::RoleUpdate(role.clone()),
            Audience::space(space_id),
        );
    }
    Ok(Json(roles))
}

/// Sorts the roles other than `@everyone` by their requested (or current) position and ID,
/// numbers them 1..n and returns the roles whose position changed.
fn new_positions(roles: &[Role], requested: &HashMap<Id, i32>) -> Vec<(Id, i32)> {
    let mut order: Vec<(i32, Id, i32)> = roles
        .iter()
        .filter(|r| !r.is_everyone())
        .map(|r| {
            let position = requested.get(&r.id).copied().unwrap_or(r.position);
            (position, r.id, r.position)
        })
        .collect();
    order.sort();
    order
        .into_iter()
        .zip(1..)
        .filter(|&((_, _, old), new)| old != new)
        .map(|((_, id, _), new)| (id, new))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(id: u64, position: i32) -> Role {
        Role {
            id: Id(id),
            space_id: Id(1),
            name: format!("r{id}"),
            color: None,
            hoist: false,
            mentionable: false,
            position,
            permissions: Permissions::empty(),
        }
    }

    #[test]
    fn positions_are_renumbered_in_the_new_order() {
        let roles = [role(1, 0), role(2, 1), role(3, 2), role(4, 7)];
        // Nothing requested: only the gap is closed.
        assert_eq!(new_positions(&roles, &HashMap::new()), [(Id(4), 3)]);
        // Move role 2 to the top.
        let requested = HashMap::from([(Id(2), 10)]);
        assert_eq!(
            new_positions(&roles, &requested),
            [(Id(3), 1), (Id(4), 2), (Id(2), 3)]
        );
        // Ties are broken by ID.
        let requested = HashMap::from([(Id(4), 2)]);
        assert_eq!(new_positions(&roles, &requested), [(Id(4), 3)]);
    }
}
