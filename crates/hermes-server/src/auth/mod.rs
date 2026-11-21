//! Password hashing, session tokens and the [`Auth`] extractor (PLAN.md sections 9.4 and 13).

use std::sync::{Arc, OnceLock};

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hermes_proto::ErrorCode;
use hermes_proto::id::Id;
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, SemaphorePermit};

use crate::api::{ApiResult, Error};
use crate::db;
use crate::state::AppState;
use crate::time::{SECOND_MS, now_ms};

pub mod totp;

/// Prefix of every session token, so leaked tokens are easy to recognize.
pub const TOKEN_PREFIX: &str = "hms_";

/// `last_seen_at` is written at most this often per session.
const TOUCH_INTERVAL_MS: i64 = 60 * SECOND_MS;

/// Password hashes computed at once. Each takes 19 MiB, so a burst of logins or registrations
/// waits its turn instead of running the server out of memory.
static HASHING: Semaphore = Semaphore::const_new(4);

async fn hashing_turn() -> ApiResult<SemaphorePermit<'static>> {
    HASHING
        .acquire()
        .await
        .map_err(|err| Error::internal(format!("password hashing stopped: {err}")))
}

/// Hashes a password with argon2id (default parameters) on the blocking pool.
pub async fn hash_password(password: String) -> ApiResult<String> {
    let _turn = hashing_turn().await?;
    tokio::task::spawn_blocking(move || hash_password_blocking(&password))
        .await
        .map_err(|err| Error::internal(format!("password hashing task failed: {err}")))?
}

fn hash_password_blocking(password: &str) -> ApiResult<String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|err| Error::internal(format!("password hashing failed: {err}")))
}

/// Checks `password` against a stored PHC string on the blocking pool. With `hash = None` (unknown
/// user) a dummy hash is checked instead, so the response time does not reveal whether the
/// user exists; the result is then always `false`.
pub async fn verify_password(password: String, hash: Option<String>) -> ApiResult<bool> {
    let _turn = hashing_turn().await?;
    tokio::task::spawn_blocking(move || {
        let (hash, known) = match hash {
            Some(hash) => (hash, true),
            None => (dummy_hash()?.to_owned(), false),
        };
        let parsed = PasswordHash::new(&hash)
            .map_err(|err| Error::internal(format!("stored password hash is invalid: {err}")))?;
        let ok = Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();
        Ok(ok && known)
    })
    .await
    .map_err(|err| Error::internal(format!("password check task failed: {err}")))?
}

fn dummy_hash() -> ApiResult<&'static str> {
    static DUMMY: OnceLock<String> = OnceLock::new();
    if let Some(hash) = DUMMY.get() {
        return Ok(hash);
    }
    let hash = hash_password_blocking("not a real password")?;
    Ok(DUMMY.get_or_init(|| hash))
}

/// A new session token: `hms_` + 32 random bytes in base64url (no padding).
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// What the database stores instead of the token.
pub fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// A live session as the database had it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachedSession {
    pub id: Id,
    pub user_id: Id,
    pub last_seen_at: i64,
}

/// Sessions already looked up, by token hash, so a request does not ask the database who it is.
/// Revocations in this process forget their sessions at once ([`AppState::revoke_sessions`]);
/// ones made elsewhere (`hermes-server user …`) within [`crate::users::REFRESH_INTERVAL`], when
/// [`crate::users::refresh`] keeps only sessions still in the database.
#[derive(Debug, Default)]
pub struct SessionCache {
    inner: std::sync::Mutex<SessionCacheInner>,
}

#[derive(Debug, Default)]
struct SessionCacheInner {
    by_hash: std::collections::HashMap<[u8; 32], CachedSession>,
    /// Bumped by every forget, so a lookup that read the database before a revocation does not
    /// put the revoked session back.
    generation: u64,
}

/// Past this many sessions the cache starts over (they are looked up again as they come).
const SESSION_CACHE_MAX: usize = 50_000;

impl SessionCache {
    fn lock(&self) -> std::sync::MutexGuard<'_, SessionCacheInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The session of `hash`, from the cache or else the database.
    pub async fn lookup(
        &self,
        db: &sqlx::SqlitePool,
        hash: &[u8; 32],
    ) -> Result<Option<CachedSession>, sqlx::Error> {
        let generation = {
            let inner = self.lock();
            if let Some(session) = inner.by_hash.get(hash) {
                return Ok(Some(*session));
            }
            inner.generation
        };
        let Some(row) = db::auth::session_by_token_hash(db, hash).await? else {
            return Ok(None);
        };
        let session = CachedSession {
            id: row.id,
            user_id: row.user_id,
            last_seen_at: row.last_seen_at,
        };
        let mut inner = self.lock();
        if inner.generation == generation {
            if inner.by_hash.len() >= SESSION_CACHE_MAX {
                inner.by_hash.clear();
            }
            inner.by_hash.insert(*hash, session);
        }
        Ok(Some(session))
    }

    /// Records that `last_seen_at` of the session of `hash` was written.
    pub fn touched(&self, hash: &[u8; 32], now: i64) {
        if let Some(session) = self.lock().by_hash.get_mut(hash) {
            session.last_seen_at = now;
        }
    }

    /// Forgets the sessions `ids`.
    pub fn forget(&self, ids: &[Id]) {
        let mut inner = self.lock();
        inner.generation += 1;
        inner.by_hash.retain(|_, s| !ids.contains(&s.id));
    }

    /// Keeps only the sessions `live` has.
    pub fn retain_live(&self, live: &std::collections::HashSet<Id>) {
        let mut inner = self.lock();
        inner.generation += 1;
        inner.by_hash.retain(|_, s| live.contains(&s.id));
    }
}

/// An authenticated request: `Authorization: Bearer <token>` names a live session of an
/// enabled user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Auth {
    pub user_id: Id,
    pub session_id: Id,
}

impl FromRequestParts<Arc<AppState>> for Auth {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<AppState>) -> ApiResult<Self> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| Error::new(ErrorCode::Unauthorized, "Missing bearer token"))?;
        let hash = token_hash(token);
        let session = state
            .sessions
            .lookup(&state.db.read, &hash)
            .await?
            .ok_or_else(|| Error::new(ErrorCode::Unauthorized, "Invalid or revoked token"))?;

        let disabled = {
            let cache = state.cache();
            cache.users.get(&session.user_id).map(|u| u.disabled)
        };
        match disabled {
            None => return Err(Error::new(ErrorCode::Unauthorized, "Unknown user")),
            Some(true) => {
                return Err(Error::new(
                    ErrorCode::AccountDisabled,
                    "This account is disabled",
                ));
            }
            Some(false) => {}
        }

        let now = now_ms();
        if now - session.last_seen_at >= TOUCH_INTERVAL_MS {
            db::auth::touch_session(&state.db.write, session.id, now).await?;
            state.sessions.touched(&hash, now);
        }
        Ok(Auth {
            user_id: session.user_id,
            session_id: session.id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_and_prefixed() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert!(a.starts_with("hms_"));
        assert_eq!(a.len(), 4 + 43);
        assert!(
            a[4..]
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
        assert_ne!(token_hash(&a), token_hash(&b));
    }

    #[tokio::test]
    async fn password_round_trip() {
        let hash = hash_password("correct horse".into()).await.unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(
            verify_password("correct horse".into(), Some(hash.clone()))
                .await
                .unwrap()
        );
        assert!(!verify_password("wrong".into(), Some(hash)).await.unwrap());
        assert!(
            !verify_password("not a real password".into(), None)
                .await
                .unwrap()
        );
    }
}
