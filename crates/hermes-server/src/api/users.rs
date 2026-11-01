//! `/users/@me` (profile, password, mentions, notification settings) and `/users/{id}`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::ErrorCode;
use hermes_proto::api::{
    ChangePasswordRequest, NotificationSettingRequest, Pagination, UpdateMeRequest, UserProfile,
};
use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;
use hermes_proto::models::{CurrentUser, Message, NotificationSetting};
use hermes_proto::permissions::Permissions;

use super::validate::{self, Fields};
use super::{ApiResult, Error, Json, Path, Query};
use crate::auth::{self, Auth};
use crate::db;
use crate::gateway::bus::Audience;
use crate::messages;
use crate::state::{AppState, Cache, CachedUser};

pub async fn me(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<CurrentUser>> {
    state
        .cache()
        .users
        .get(&auth.user_id)
        .map(|u| Json(CachedUser::current_user(u)))
        .ok_or_else(|| Error::internal("authenticated user missing from the cache"))
}

/// Changes the password and revokes every other session of the user.
pub async fn change_password(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<ChangePasswordRequest>,
) -> ApiResult<StatusCode> {
    let mut fields = Fields::new();
    fields.check(
        "new_password",
        validate::password(
            &req.new_password,
            state.config.registration.min_password_length,
        ),
    );
    if req.current_password.chars().count() > validate::MAX_PASSWORD_CHARS {
        fields.add("current_password", "is too long");
    }
    fields.finish()?;

    let hash = db::auth::password_hash(&state.db.read, auth.user_id).await?;
    if !auth::verify_password(req.current_password, hash).await? {
        return Err(Error::new(
            ErrorCode::InvalidCredentials,
            "The current password is wrong",
        ));
    }
    let new_hash = auth::hash_password(req.new_password).await?;
    let mut tx = state.db.write.begin().await?;
    db::auth::set_password_hash(&mut tx, auth.user_id, &new_hash).await?;
    let revoked = db::auth::delete_other_sessions(&mut tx, auth.user_id, auth.session_id).await?;
    tx.commit().await?;
    state.gateway.revoke(&revoked);
    tracing::debug!(user_id = %auth.user_id, revoked = revoked.len(), "password changed");
    Ok(StatusCode::NO_CONTENT)
}

/// Updates my display name, avatar or status text; everyone who can see me gets `user_update`.
pub async fn update_me(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<UpdateMeRequest>,
) -> ApiResult<Json<CurrentUser>> {
    let _writes = state.writes.lock().await;
    let old = state
        .cache()
        .users
        .get(&auth.user_id)
        .cloned()
        .ok_or_else(|| Error::internal("authenticated user missing from the cache"))?;
    let mut user = old.user.clone();
    let mut fields = Fields::new();
    if let Some(display_name) = &req.display_name {
        let checked = match display_name {
            Some(name) if !name.trim().is_empty() => {
                validate::name(name, UpdateMeRequest::DISPLAY_NAME_CHARS).map(Some)
            }
            _ => Ok(None),
        };
        if let Some(name) = fields.check("display_name", checked) {
            user.display_name = name;
        }
    }
    if let Some(status) = &req.status_text {
        let checked = match status {
            // One line, like a name.
            Some(s) if s.contains(['\n', '\t']) => Err("must be one line".to_owned()),
            Some(s) => validate::optional_text(s, UpdateMeRequest::STATUS_TEXT_CHARS),
            None => Ok(None),
        };
        if let Some(status) = fields.check("status_text", checked) {
            user.status_text = status;
        }
    }
    fields.finish()?;
    match req.avatar_id {
        None => {}
        Some(None) => user.avatar = None,
        Some(Some(id)) if old.user.avatar.as_ref().is_some_and(|a| a.id == id) => {}
        Some(Some(id)) => {
            let avatar = db::spaces::own_image(&state.db.read, id, auth.user_id)
                .await?
                .ok_or_else(|| {
                    validate::field_error("avatar_id", "must be an image you uploaded")
                })?;
            user.avatar = Some(avatar);
        }
    }
    if user != old.user {
        let mut tx = state.db.write.begin().await?;
        db::users::update_profile(&mut tx, &user).await?;
        tx.commit().await?;
        let audience = {
            let mut cache = state.cache_mut();
            if let Some(cached) = cache.users.get_mut(&auth.user_id) {
                cached.user = user.clone();
            }
            cache.presence_audience(auth.user_id)
        };
        state.publish(
            ServerEvent::UserUpdate(user.clone()),
            Audience::Users(audience.into_iter().collect()),
        );
    }
    Ok(Json(CurrentUser {
        user,
        instance_admin: old.instance_admin,
    }))
}

/// A user I share a space or a DM with (or myself), with the spaces we share.
pub async fn get(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(user_id): Path<Id>,
) -> ApiResult<Json<UserProfile>> {
    let cache = state.cache();
    let visible =
        user_id == auth.user_id || cache.presence_audience(auth.user_id).contains(&user_id);
    let user = cache
        .users
        .get(&user_id)
        .filter(|_| visible)
        .ok_or_else(|| Error::not_found("user"))?;
    let mut mutual_space_ids: Vec<Id> = cache
        .spaces
        .values()
        .filter(|s| s.members.contains_key(&user_id) && s.members.contains_key(&auth.user_id))
        .map(|s| s.space.id)
        .collect();
    mutual_space_ids.sort();
    Ok(Json(UserProfile {
        user: user.user.clone(),
        mutual_space_ids,
    }))
}

/// Whether `target_id` is a space I am in or a channel I can see.
fn notification_target(cache: &Cache, user_id: Id, target_id: Id) -> bool {
    cache.is_member(target_id, user_id)
        || cache
            .channel_permissions(target_id, user_id)
            .contains(Permissions::VIEW_CHANNEL)
}

pub async fn put_notification_setting(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(target_id): Path<Id>,
    Json(req): Json<NotificationSettingRequest>,
) -> ApiResult<StatusCode> {
    if !notification_target(&state.cache(), auth.user_id, target_id) {
        return Err(Error::not_found("space or channel"));
    }
    if req.muted_until.is_some_and(|t| t < -1) {
        return Err(validate::field_error(
            "muted_until",
            "must be -1 (forever) or a time",
        ));
    }
    let setting = NotificationSetting {
        target_id,
        level: req.level,
        muted_until: req.muted_until,
    };
    let mut tx = state.db.write.begin().await?;
    db::users::set_notification_setting(&mut tx, auth.user_id, &setting).await?;
    tx.commit().await?;
    state.publish(
        ServerEvent::NotificationSettingsUpdate(setting),
        Audience::User(auth.user_id),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Back to the default (a channel follows its space again). Own sessions get the default
/// setting of a space (`mentions`) or DM (`all`) as `notification_settings_update` with
/// `muted_until = null`, and a channel of a space the space's setting is used.
pub async fn delete_notification_setting(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(target_id): Path<Id>,
) -> ApiResult<StatusCode> {
    let mut tx = state.db.write.begin().await?;
    db::users::delete_notification_setting(&mut tx, auth.user_id, target_id).await?;
    tx.commit().await?;
    state.publish(
        ServerEvent::NotificationSettingsDelete { target_id },
        Audience::User(auth.user_id),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /users/@me/mentions`: messages mentioning me, newest first.
pub async fn mentions(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Query(q): Query<Pagination>,
) -> ApiResult<Json<Vec<Message>>> {
    let (channels, roles) = {
        let cache = state.cache();
        let channels = super::search::readable_channels(&cache, auth.user_id, None, None);
        let roles: Vec<Id> = cache
            .spaces
            .values()
            .filter_map(|s| s.members.get(&auth.user_id))
            .flat_map(|m| m.role_ids.iter().copied())
            .collect();
        (channels, roles)
    };
    let rows = db::messages::mentioning(
        &state.db.read,
        auth.user_id,
        &roles,
        &channels,
        q.before,
        q.limit(),
    )
    .await?;
    Ok(Json(messages::to_wire(&state, rows, auth.user_id).await?))
}
