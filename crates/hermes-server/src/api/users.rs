//! `/users/@me` and `/users/@me/password`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::ErrorCode;
use hermes_proto::api::ChangePasswordRequest;
use hermes_proto::models::CurrentUser;

use super::validate::{self, Fields};
use super::{ApiResult, Error, Json};
use crate::auth::{self, Auth};
use crate::db;
use crate::state::{AppState, CachedUser};

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
    tracing::debug!(user_id = %auth.user_id, revoked = revoked.len(), "password changed");
    Ok(StatusCode::NO_CONTENT)
}
