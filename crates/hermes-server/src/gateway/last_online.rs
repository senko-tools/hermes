//! When someone was last online, for their profile. Being Online or Do Not Disturb counts. Being
//! Idle or Invisible does not, except for what shows anyway: writing in a server or going into a
//! server's voice channel. DMs and DM calls never count.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use hermes_proto::id::Id;
use hermes_proto::models::Status;

use crate::db;
use crate::state::AppState;
use crate::time::now_ms;

/// A visible stretch shorter than this does not count: a client that comes back invisible looks
/// Online for a moment while it connects.
pub const MIN_VISIBLE_MS: i64 = 10_000;
/// Activity is written at most this often per person.
const ACTIVE_EVERY_MS: i64 = 60_000;
/// How often the people who are visible right now are written.
const FLUSH_EVERY: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Default)]
struct Inner {
    /// Since when each person has been visibly online.
    visible_since: HashMap<Id, i64>,
    /// When each person's activity was last written.
    active_at: HashMap<Id, i64>,
}

#[derive(Debug, Default)]
pub struct LastOnline(Mutex<Inner>);

impl LastOnline {
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn started(&self, user: Id, now: i64) {
        self.inner().visible_since.entry(user).or_insert(now);
    }

    /// `user` stopped being visible. Whether that stretch counts.
    fn stopped(&self, user: Id, now: i64) -> bool {
        self.inner()
            .visible_since
            .remove(&user)
            .is_some_and(|since| now - since >= MIN_VISIBLE_MS)
    }

    /// `user` did something that counts. Whether to write it now.
    fn active(&self, user: Id, now: i64) -> bool {
        let mut inner = self.inner();
        if inner
            .active_at
            .get(&user)
            .is_some_and(|at| now - at < ACTIVE_EVERY_MS)
        {
            return false;
        }
        inner.active_at.insert(user, now);
        true
    }

    /// Everyone who has been visible long enough to count.
    fn visible(&self, now: i64) -> Vec<Id> {
        self.inner()
            .visible_since
            .iter()
            .filter(|(_, since)| now - **since >= MIN_VISIBLE_MS)
            .map(|(user, _)| *user)
            .collect()
    }

    /// Makes `user`'s visible stretch older, so a test need not wait for it.
    pub fn backdate(&self, user: Id, ms: i64) {
        if let Some(since) = self.inner().visible_since.get_mut(&user) {
            *since -= ms;
        }
    }
}

fn counts(status: Status) -> bool {
    matches!(status, Status::Online | Status::Dnd)
}

fn write(state: &AppState, user: Id, now: i64) {
    let pool = state.db.write.clone();
    tokio::spawn(async move {
        if let Err(err) = db::users::set_last_online(&pool, &[user], now).await {
            tracing::debug!("could not note when {user} was last online: {err}");
        }
    });
}

/// What others see of `user` changed from `before` to `after`.
pub fn presence_changed(state: &AppState, user: Id, before: Status, after: Status) {
    let now = now_ms();
    match (counts(before), counts(after)) {
        (false, true) => state.last_online.started(user, now),
        (true, false) if state.last_online.stopped(user, now) => write(state, user, now),
        _ => {}
    }
}

/// `user` wrote in a server or went into a server's voice channel.
pub fn active(state: &AppState, user: Id) {
    let now = now_ms();
    if state.last_online.active(user, now) {
        write(state, user, now);
    }
}

/// Writes everyone who is visible now.
pub async fn flush(state: &AppState) -> Result<(), sqlx::Error> {
    let now = now_ms();
    let users = state.last_online.visible(now);
    if users.is_empty() {
        return Ok(());
    }
    db::users::set_last_online(&state.db.write, &users, now).await
}

/// Runs [`flush`] every few minutes, so a crash loses little.
pub async fn flush_task(state: std::sync::Arc<AppState>) {
    let mut interval = tokio::time::interval(FLUSH_EVERY);
    loop {
        interval.tick().await;
        if let Err(err) = flush(&state).await {
            tracing::error!("could not note who was last online: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: Id = Id(1);

    #[test]
    fn a_short_visible_stretch_does_not_count() {
        let last = LastOnline::default();
        last.started(ME, 1_000);
        assert!(!last.stopped(ME, 1_000 + MIN_VISIBLE_MS - 1));
        assert!(last.visible(1_000 + MIN_VISIBLE_MS).is_empty());
    }

    #[test]
    fn a_long_visible_stretch_counts_once() {
        let last = LastOnline::default();
        last.started(ME, 1_000);
        assert_eq!(last.visible(1_000 + MIN_VISIBLE_MS), vec![ME]);
        assert!(last.stopped(ME, 1_000 + MIN_VISIBLE_MS));
        assert!(!last.stopped(ME, 2_000 + MIN_VISIBLE_MS));
    }

    #[test]
    fn activity_is_written_at_most_once_a_minute() {
        let last = LastOnline::default();
        assert!(last.active(ME, 0));
        assert!(!last.active(ME, ACTIVE_EVERY_MS - 1));
        assert!(last.active(ME, ACTIVE_EVERY_MS));
    }

    #[test]
    fn only_online_and_do_not_disturb_count() {
        assert!(counts(Status::Online));
        assert!(counts(Status::Dnd));
        assert!(!counts(Status::Idle));
        assert!(!counts(Status::Offline));
    }
}
