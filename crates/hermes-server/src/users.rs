//! Admin changes made outside the running server (PLAN.md section 9.1): `hermes-server user …`
//! writes the database directly, and the server picks the changes up within 10 seconds.

use std::sync::Arc;
use std::time::Duration;

use hermes_proto::gateway::ServerEvent;
use hermes_proto::id::Id;
use hermes_proto::models::split_handle;

use crate::db;
use crate::gateway::bus::Audience;
use crate::state::AppState;

/// How often the database is compared with the cache.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// Copies `disabled` / `instance_admin` from the database into the cache and closes gateway
/// connections whose REST session was deleted (a password reset or a disabled account).
/// Returns how many users changed and how many connections were closed.
pub async fn refresh(state: &AppState) -> Result<(usize, usize), sqlx::Error> {
    let flags = db::users::flags(&state.db.read).await?;
    let mut changed = 0;
    let mut renamed = Vec::new();
    {
        let mut cache = state.cache_mut();
        for (id, disabled, instance_admin, handle) in flags {
            let Some(user) = cache.users.get_mut(&id) else {
                continue;
            };
            if user.disabled != disabled || user.instance_admin != instance_admin {
                user.disabled = disabled;
                user.instance_admin = instance_admin;
                changed += 1;
            }
            if user.user.handle() != handle {
                let (name, tag) = split_handle(&handle);
                user.user.username = name.to_owned();
                user.user.discriminator = tag.map(str::to_owned);
                renamed.push(user.user.clone());
                changed += 1;
            }
        }
    }
    for user in renamed {
        let audience = state.cache().profile_audience(user.id);
        state.publish(
            ServerEvent::UserUpdate(user),
            Audience::Users(audience.into_iter().collect()),
        );
    }
    let live: std::collections::HashSet<Id> = db::users::session_ids(&state.db.read)
        .await?
        .into_iter()
        .collect();
    let gone: Vec<Id> = state
        .gateway
        .auth_session_ids()
        .into_iter()
        .filter(|id| !live.contains(id))
        .collect();
    if !gone.is_empty() {
        state.gateway.revoke(&gone);
    }
    Ok((changed, gone.len()))
}

pub async fn refresh_task(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(REFRESH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        match refresh(&state).await {
            Ok((0, 0)) => {}
            Ok((users, sessions)) => tracing::info!(
                users,
                sessions,
                "applied account changes made with the user commands"
            ),
            Err(err) => tracing::warn!("could not check for account changes: {err}"),
        }
    }
}
