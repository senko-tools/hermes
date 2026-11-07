//! Device keys for end-to-end encryption (docs/e2ee.md): clients publish their public keys here
//! and read each other's. The server cannot tell whether a key is genuine, so clients check
//! signatures and pin what they have seen; this only stores and hands out.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use hermes_proto::ErrorCode;
use hermes_proto::e2ee::{
    DeviceKeys, DeviceQuery, MAX_DEVICE_QUERY, MAX_DEVICES, RegisterDeviceRequest, UserDevices,
};
use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;

use super::{ApiResult, Error, Json};
use crate::auth::Auth;
use crate::db;
use crate::gateway::bus::Audience;
use crate::state::AppState;
use crate::time::now_ms;

/// Longest base64 value a key field can hold (an Ed25519 signature is the longest).
const MAX_KEY_CHARS: usize = 128;

fn valid_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_KEY_CHARS
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_'))
}

fn valid_device_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Publishes or brings back the device of the current session.
pub async fn register(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<RegisterDeviceRequest>,
) -> ApiResult<StatusCode> {
    if !valid_device_id(&req.device_id)
        || !valid_key(&req.sign_key)
        || !valid_key(&req.dh_key)
        || !valid_key(&req.signature)
    {
        return Err(Error::new(ErrorCode::BadRequest, "Malformed device keys"));
    }
    // A client registers on every connect: nothing changes, nobody hears about it.
    if db::devices::is_live(&state.db.read, auth.user_id, &req, auth.session_id).await? {
        return Ok(StatusCode::NO_CONTENT);
    }
    if db::devices::count_live(&state.db.read, auth.user_id, &req.device_id).await?
        >= MAX_DEVICES as i64
    {
        return Err(Error::new(
            ErrorCode::BadRequest,
            "This account has too many devices: sign some out",
        ));
    }
    let mut conn = state.db.write.acquire().await?;
    let stored =
        db::devices::register(&mut conn, auth.user_id, auth.session_id, &req, now_ms()).await?;
    drop(conn);
    if !stored {
        return Err(Error::new(
            ErrorCode::Conflict,
            "This device is already known with other keys",
        ));
    }
    devices_changed(&state, auth.user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The devices of up to [`MAX_DEVICE_QUERY`] users. Public keys are not secret, so any signed-in
/// user may ask, also about someone they only have a message request with.
pub async fn query(
    State(state): State<Arc<AppState>>,
    _auth: Auth,
    Json(req): Json<DeviceQuery>,
) -> ApiResult<Json<Vec<UserDevices>>> {
    if req.user_ids.len() > MAX_DEVICE_QUERY {
        return Err(Error::new(
            ErrorCode::BadRequest,
            "Asking about too many users",
        ));
    }
    let mut out = Vec::with_capacity(req.user_ids.len());
    for user_id in req.user_ids {
        out.push(UserDevices {
            user_id,
            devices: db::devices::of_user(&state.db.read, user_id).await?,
        });
    }
    Ok(Json(out))
}

/// Tells everyone who could need `user_id`'s keys that they changed. Clients may also ask at any
/// time, so one who misses this catches up on their next look.
pub async fn devices_changed(state: &AppState, user_id: Id) -> Result<(), sqlx::Error> {
    let devices: Vec<DeviceKeys> = db::devices::of_user(&state.db.read, user_id).await?;
    let audience = {
        let cache = state.cache();
        let mut users = cache.profile_audience(user_id);
        // DMs in every state, also message requests and closed ones: they can still be written to.
        users.extend(
            cache
                .dm_ids(user_id)
                .filter_map(|id| cache.channels.get(&id))
                .flat_map(|c| c.recipient_ids.iter().copied()),
        );
        users
    };
    state.publish(
        ServerEvent::DevicesUpdate { user_id, devices },
        Audience::Users(audience.into_iter().collect()),
    );
    Ok(())
}

/// Revokes what these sessions registered, once they are signed out.
pub async fn revoke_sessions(state: &AppState, session_ids: &[Id]) -> Result<(), sqlx::Error> {
    if session_ids.is_empty() {
        return Ok(());
    }
    let mut conn = state.db.write.acquire().await?;
    let users = db::devices::revoke_sessions(&mut conn, session_ids, now_ms()).await?;
    drop(conn);
    for user in users {
        devices_changed(state, user).await?;
    }
    Ok(())
}
