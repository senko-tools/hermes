//! `/auth/*`: register, login, logout, sessions.

use std::sync::Arc;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use hermes_proto::ErrorCode;
use hermes_proto::api::{AuthResponse, LoginRequest, RegisterRequest, SessionInfo};
use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;
use hermes_proto::models::{PrivacySettings, User};
use sqlx::SqliteConnection;

use super::validate::{self, Fields};
use super::{ApiResult, Error, Json, Path, PeerAddr};
use crate::auth::{self, Auth};
use crate::config::RegistrationMode;
use crate::db;
use crate::gateway::bus::Audience;
use crate::invites;
use crate::setup::DefaultSpace;
use crate::state::{AppState, CachedUser};
use crate::time::now_ms;

fn registration_closed(message: &str) -> Error {
    Error::new(ErrorCode::RegistrationClosed, message)
}

fn invite_invalid() -> Error {
    Error::new(
        ErrorCode::InviteInvalid,
        "This invite is invalid, expired or used up",
    )
}

/// Whether registering is allowed at all, given whether the instance has users yet and whether
/// an invite was given. The very first account always needs the setup invite, whatever the mode,
/// so a random visitor cannot become the instance admin.
fn check_mode(mode: RegistrationMode, first_user: bool, has_invite: bool) -> ApiResult<()> {
    if first_user {
        return if has_invite {
            Ok(())
        } else {
            Err(registration_closed(
                "Use the setup invite printed in the server console to create the first account",
            ))
        };
    }
    match mode {
        RegistrationMode::Open => Ok(()),
        RegistrationMode::InviteOnly if has_invite => Ok(()),
        RegistrationMode::InviteOnly => Err(registration_closed(
            "This instance requires an invite to register",
        )),
        RegistrationMode::Closed => Err(registration_closed("Registration is closed")),
    }
}

pub async fn register(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<PeerAddr>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> ApiResult<Json<AuthResponse>> {
    let mut fields = Fields::new();
    let username = fields.check("username", validate::username(&req.username));
    fields.check(
        "password",
        validate::password(&req.password, state.config.registration.min_password_length),
    );
    let device_name = fields.check("device_name", validate::device_name(&req.device_name));
    let invite_code = req
        .invite_code
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    fields.finish()?;
    let (Some(username), Some(device_name)) = (username, device_name) else {
        unreachable!("finish() returned Ok, so every field is valid");
    };

    // Cheap checks before hashing; repeated authoritatively inside the transaction.
    let first_user = state.cache().users.is_empty();
    check_mode(
        state.config.registration.mode,
        first_user,
        invite_code.is_some(),
    )?;
    if invite_code.is_some_and(|c| !invites::is_valid_code(c)) {
        return Err(invite_invalid());
    }
    let password_hash = auth::hash_password(req.password).await?;

    let now = now_ms();
    let user_id = state.ids.next();
    let token = auth::new_token();
    let ip = state.client_ip(peer.0, &headers).to_string();
    let mut tx = state.db.write.begin().await?;

    let first_user = db::auth::count_users(&mut *tx).await? == 0;
    check_mode(
        state.config.registration.mode,
        first_user,
        invite_code.is_some(),
    )?;
    let join_space = match invite_code {
        Some(code) => db::invites::take_use(&mut tx, code, now)
            .await?
            .ok_or_else(invite_invalid)?,
        None => None,
    };
    // The instance's first account, its owner, goes without a tag; everyone else gets a random
    // free one. Two registrations can pick the same tag, so it tries again on that.
    let mut discriminator = None;
    for _ in 0..TAG_ATTEMPTS {
        discriminator = if first_user {
            None
        } else {
            Some(
                state
                    .cache()
                    .free_tag(&username)
                    .ok_or_else(username_taken)?,
            )
        };
        let handle = match &discriminator {
            Some(tag) => format!("{username}#{tag}"),
            None => username.clone(),
        };
        let new_user = db::auth::NewUser {
            id: user_id,
            username: &handle,
            password_hash: &password_hash,
            instance_admin: first_user,
            created_at: now,
        };
        match db::auth::insert_user(&mut tx, &new_user).await {
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
                discriminator = None;
                if first_user {
                    return Err(username_taken());
                }
                continue;
            }
            result => {
                result?;
                break;
            }
        }
    }
    if discriminator.is_none() && !first_user {
        return Err(username_taken());
    }
    let default_space = if first_user {
        let space = DefaultSpace::new(&state, user_id, state.config.instance.name.clone());
        space.insert(&mut tx, now).await?;
        Some(space)
    } else {
        None
    };
    if let Some(space_id) = join_space {
        db::spaces::insert_member(&mut tx, space_id, user_id, now).await?;
    }
    let session_id = insert_session(&mut tx, &state, user_id, &token, &device_name, &ip).await?;
    tx.commit().await?;

    let user = CachedUser {
        user: User {
            id: user_id,
            username,
            discriminator,
            display_name: None,
            avatar: None,
            status_text: None,
        },
        instance_admin: first_user,
        disabled: false,
        privacy: PrivacySettings::default(),
    };
    let current = user.current_user();
    state.cache_mut().insert_user(user);
    if let Some(space) = default_space {
        tracing::info!(
            user = %current.user.username,
            "first account created; it is the instance admin and owns the space {:?}",
            space.space.name
        );
        space.cache(&state, now);
    }
    if let Some(space_id) = join_space {
        let member = {
            let mut cache = state.cache_mut();
            cache.insert_member(space_id, user_id, now);
            cache.member(space_id, user_id)
        };
        if let Some(member) = member {
            state.publish(ServerEvent::MemberAdd(member), Audience::space(space_id));
        }
    }
    tracing::debug!(user_id = %user_id, session_id = %session_id, "registered");
    Ok(Json(AuthResponse {
        token,
        user: current,
    }))
}

fn username_taken() -> Error {
    Error::new(ErrorCode::Conflict, "This username is taken")
}

/// How many random tags registering tries before giving up.
const TAG_ATTEMPTS: usize = 5;

/// Inserts a session for `token` and returns its ID.
async fn insert_session(
    conn: &mut SqliteConnection,
    state: &AppState,
    user_id: Id,
    token: &str,
    device_name: &str,
    ip: &str,
) -> ApiResult<Id> {
    let id = state.ids.next();
    let session = db::auth::NewSession {
        id,
        user_id,
        token_hash: &auth::token_hash(token),
        device_name,
        created_at: now_ms(),
        ip: Some(ip),
    };
    db::auth::insert_session(conn, &session).await?;
    Ok(id)
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<PeerAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Json<AuthResponse>> {
    let mut fields = Fields::new();
    let device_name = fields.check("device_name", validate::device_name(&req.device_name));
    if req.password.chars().count() > validate::MAX_PASSWORD_CHARS {
        fields.add("password", "is too long");
    }
    fields.finish()?;
    let device_name = device_name.unwrap_or_default();

    let login = req.username.trim().trim_start_matches('@').to_lowercase();
    let handle = db::auth::resolve_handle(&state.db.read, &login)
        .await?
        .map_err(|()| {
            Error::new(
                ErrorCode::InvalidCredentials,
                "Several accounts have this name: add your # number, like name#1234",
            )
        })?;
    let credentials = match &handle {
        Some(handle) => db::auth::credentials_by_username(&state.db.read, handle).await?,
        None => None,
    };
    let hash = credentials.as_ref().map(|c| c.password_hash.clone());
    if !auth::verify_password(req.password, hash).await? {
        return Err(Error::new(
            ErrorCode::InvalidCredentials,
            "Wrong username or password",
        ));
    }
    let credentials = credentials.expect("verify_password is false for unknown users");
    if credentials.disabled {
        return Err(Error::new(
            ErrorCode::AccountDisabled,
            "This account is disabled",
        ));
    }
    let user = state
        .cache()
        .users
        .get(&credentials.user_id)
        .map(CachedUser::current_user)
        .ok_or_else(|| Error::internal("user missing from the cache"))?;

    let token = auth::new_token();
    let ip = state.client_ip(peer.0, &headers).to_string();
    let mut conn = state.db.write.acquire().await?;
    insert_session(
        &mut conn,
        &state,
        credentials.user_id,
        &token,
        &device_name,
        &ip,
    )
    .await?;
    Ok(Json(AuthResponse { token, user }))
}

/// Revokes the current session and closes its gateway connections (4011).
pub async fn logout(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<StatusCode> {
    db::auth::delete_session(&state.db.write, auth.user_id, auth.session_id).await?;
    state.gateway.revoke(&[auth.session_id]);
    super::devices::revoke_sessions(&state, &[auth.session_id]).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    auth: Auth,
) -> ApiResult<Json<Vec<SessionInfo>>> {
    let sessions = db::auth::list_sessions(&state.db.read, auth.user_id).await?;
    Ok(Json(
        sessions
            .into_iter()
            .map(|s| SessionInfo {
                id: s.id,
                device_name: s.device_name,
                created_at: s.created_at,
                last_seen_at: s.last_seen_at,
                current: s.id == auth.session_id,
            })
            .collect(),
    ))
}

/// Revokes one of the caller's own sessions and closes its gateway connections (4011).
pub async fn delete_session(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(session_id): Path<Id>,
) -> ApiResult<StatusCode> {
    if db::auth::delete_session(&state.db.write, auth.user_id, session_id).await? {
        state.gateway.revoke(&[session_id]);
        super::devices::revoke_sessions(&state, &[session_id]).await?;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(Error::not_found("session"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_modes() {
        use RegistrationMode::*;
        let code = |r: ApiResult<()>| r.err().map(|e| e.0.code);
        // First user: invite required in every mode.
        for mode in [Open, InviteOnly, Closed] {
            assert_eq!(code(check_mode(mode, true, true)), None);
            assert_eq!(
                code(check_mode(mode, true, false)),
                Some(ErrorCode::RegistrationClosed)
            );
        }
        assert_eq!(code(check_mode(Open, false, false)), None);
        assert_eq!(code(check_mode(Open, false, true)), None);
        assert_eq!(code(check_mode(InviteOnly, false, true)), None);
        assert_eq!(
            code(check_mode(InviteOnly, false, false)),
            Some(ErrorCode::RegistrationClosed)
        );
        assert_eq!(
            code(check_mode(Closed, false, true)),
            Some(ErrorCode::RegistrationClosed)
        );
    }
}
