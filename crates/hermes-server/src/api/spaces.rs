//! `/spaces`: create, update, delete and transfer, and their gateway events.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::ErrorCode;
use hermes_proto::api::{CreateSpaceRequest, TransferSpaceRequest, UpdateSpaceRequest};
use hermes_proto::gateway::{ServerEvent, SpaceSnapshot};
use hermes_proto::id::Id;
use hermes_proto::models::{AuditAction, Space};
use hermes_proto::permissions::Permissions;
use serde_json::{Map, Value, json};

use super::validate::{self, Fields};
use super::{ApiResult, Error, Json, Path, audit, perms};
use crate::auth::Auth;
use crate::db;
use crate::gateway::bus::Audience;
use crate::gateway::visibility::{self, Visible};
use crate::setup::DefaultSpace;
use crate::state::AppState;
use crate::time::now_ms;

/// Creates a space owned by the caller, with `@everyone`, `#general` and `General`.
/// `limits.max_spaces_per_user` counts owned spaces; instance admins have no limit.
pub async fn create(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<CreateSpaceRequest>,
) -> ApiResult<Json<SpaceSnapshot>> {
    let mut fields = Fields::new();
    let name = fields.check(
        "name",
        validate::name(&req.name, validate::SPACE_NAME_CHARS),
    );
    fields.finish()?;
    let Some(name) = name else {
        unreachable!("finish() returned Ok, so every field is valid");
    };

    let _writes = state.writes.lock().await;
    {
        let cache = state.cache();
        let admin = cache
            .users
            .get(&auth.user_id)
            .is_some_and(|u| u.instance_admin);
        let max = state.config.limits.max_spaces_per_user as usize;
        let owned = cache
            .spaces
            .values()
            .filter(|s| s.space.owner_id == auth.user_id)
            .count();
        if !admin && max == 0 {
            return Err(Error::new(
                ErrorCode::MissingPermission,
                "Only instance admins can create spaces",
            ));
        }
        if !admin && owned >= max {
            return Err(Error::new(
                ErrorCode::BadRequest,
                format!("You already own {owned} spaces, the most allowed on this instance"),
            ));
        }
    }

    let space = DefaultSpace::new(&state, auth.user_id, name);
    let space_id = space.space.id;
    let now = now_ms();
    let mut tx = state.db.write.begin().await?;
    space.insert(&mut tx, now).await?;
    tx.commit().await?;
    space.cache(&state, now);
    let snapshot = state
        .cache()
        .space_snapshot(space_id, auth.user_id)
        .ok_or_else(|| Error::internal("new space missing from the cache"))?;
    state.publish(
        ServerEvent::SpaceCreate(Box::new(snapshot.clone())),
        Audience::User(auth.user_id),
    );
    Ok(Json(snapshot))
}

/// `MANAGE_SPACE`. `icon_id` must be an image the caller uploaded.
pub async fn update(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space_id): Path<Id>,
    Json(req): Json<UpdateSpaceRequest>,
) -> ApiResult<Json<Space>> {
    let _writes = state.writes.lock().await;
    let old = {
        let cache = state.cache();
        let access =
            perms::require_space(&cache, space_id, auth.user_id, Permissions::MANAGE_SPACE)?;
        access.space.space.clone()
    };

    let mut space = old.clone();
    let mut fields = Fields::new();
    if let Some(name) = &req.name
        && let Some(name) = fields.check("name", validate::name(name, validate::SPACE_NAME_CHARS))
    {
        space.name = name;
    }
    if let Some(description) = &req.description {
        let checked = match description {
            Some(d) => validate::optional_text(d, validate::SPACE_DESCRIPTION_CHARS),
            None => Ok(None),
        };
        if let Some(description) = fields.check("description", checked) {
            space.description = description;
        }
    }
    fields.finish()?;
    match req.icon_id {
        None => {}
        Some(None) => space.icon = None,
        Some(Some(id)) if old.icon.as_ref().is_some_and(|icon| icon.id == id) => {}
        Some(Some(id)) => {
            let icon = db::spaces::own_image(&state.db.read, id, auth.user_id)
                .await?
                .ok_or_else(|| validate::field_error("icon_id", "must be an image you uploaded"))?;
            space.icon = Some(icon);
        }
    }

    let mut details = Map::new();
    if space.name != old.name {
        details.insert("name".into(), json!(space.name));
    }
    if space.description != old.description {
        details.insert("description".into(), json!(space.description));
    }
    if space.icon != old.icon {
        details.insert("icon_id".into(), json!(space.icon.as_ref().map(|i| i.id)));
    }
    if details.is_empty() {
        return Ok(Json(space));
    }

    let mut tx = state.db.write.begin().await?;
    db::spaces::update_space(&mut tx, &space).await?;
    audit::record(
        &mut tx,
        &state,
        space_id,
        auth.user_id,
        AuditAction::SpaceUpdate,
        Some(space_id),
        Value::Object(details),
    )
    .await?;
    tx.commit().await?;
    if let Some(state) = state.cache_mut().spaces.get_mut(&space_id) {
        state.space = space.clone();
    }
    state.publish(
        ServerEvent::SpaceUpdate(space.clone()),
        Audience::space(space_id),
    );
    Ok(Json(space))
}

fn owner_only(action: &str) -> Error {
    Error::new(
        ErrorCode::MissingPermission,
        format!("Only the owner can {action} the space"),
    )
}

/// Owner only. Deletes the space with all its channels, messages, roles and invites.
pub async fn delete(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space_id): Path<Id>,
) -> ApiResult<StatusCode> {
    let _writes = state.writes.lock().await;
    let members: Vec<Id> = {
        let cache = state.cache();
        let access = perms::space(&cache, space_id, auth.user_id)?;
        if !access.is_owner() {
            return Err(owner_only("delete"));
        }
        access.space.members.keys().copied().collect()
    };
    let mut tx = state.db.write.begin().await?;
    db::spaces::delete_space(&mut tx, space_id).await?;
    tx.commit().await?;
    state.cache_mut().remove_space(space_id);
    state.publish(
        ServerEvent::SpaceDelete { id: space_id },
        Audience::Users(members),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Owner only. The new owner must be a member.
pub async fn transfer(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(space_id): Path<Id>,
    Json(req): Json<TransferSpaceRequest>,
) -> ApiResult<StatusCode> {
    let _writes = state.writes.lock().await;
    let before = {
        let cache = state.cache();
        if !perms::space(&cache, space_id, auth.user_id)?.is_owner() {
            return Err(owner_only("transfer"));
        }
        if req.user_id == auth.user_id {
            return Err(validate::field_error(
                "user_id",
                "you already own the space",
            ));
        }
        if !cache.is_member(space_id, req.user_id) {
            return Err(validate::field_error(
                "user_id",
                "must be a member of the space",
            ));
        }
        Visible::of_space(&cache, space_id)
    };
    let mut tx = state.db.write.begin().await?;
    db::spaces::set_owner(&mut tx, space_id, req.user_id).await?;
    audit::record(
        &mut tx,
        &state,
        space_id,
        auth.user_id,
        AuditAction::SpaceTransfer,
        Some(req.user_id),
        json!({ "previous_owner_id": auth.user_id }),
    )
    .await?;
    tx.commit().await?;
    let space = {
        let mut cache = state.cache_mut();
        cache.spaces.get_mut(&space_id).map(|s| {
            s.space.owner_id = req.user_id;
            s.space.clone()
        })
    };
    if let Some(space) = space {
        state.publish(ServerEvent::SpaceUpdate(space), Audience::space(space_id));
    }
    visibility::publish_changes(&state, space_id, &before);
    Ok(StatusCode::NO_CONTENT)
}
