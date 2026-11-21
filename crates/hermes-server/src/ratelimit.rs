//! Rate limits from `[limits]` (PLAN.md section 13).
//!
//! - REST: `requests_per_second` per session token (per IP without a token) and
//!   `login_attempts_per_minute` per IP, both with `tower_governor` layers.
//! - Message sends: `messages_per_5s` per user per channel, and gateway ops: 120 per minute per
//!   connection, both with the small [`Bucket`] / [`KeyedLimiter`] below, which also report how
//!   long to wait to the millisecond.

use std::collections::HashMap;
use std::hash::Hash;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::header::{AUTHORIZATION, HeaderValue, RETRY_AFTER};
use axum::http::{Request, Response};
use axum::response::IntoResponse;
use axum::routing::MethodRouter;
use hermes_proto::ApiError;
use sha2::{Digest, Sha256};
use tower_governor::GovernorError;
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::KeyExtractor;

use crate::api::{Error, PeerAddr};
use crate::config::ProxyRange;
use crate::state::client_ip;

pub const GATEWAY_OPS_PERIOD: Duration = Duration::from_secs(60);
/// The window of `limits.messages_per_5s`.
pub const MESSAGES_PERIOD: Duration = Duration::from_secs(5);
/// What a person can do too often; each kind has its own bucket of a fraction of
/// `limits.actions_per_minute` ([`Action::divisor`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Edit,
    React,
    Typing,
    /// Opening someone's profile card (`GET /users/{id}`).
    Profile,
    Search,
    Upload,
    Friend,
    Ring,
    Password,
    /// `GET /users/@me/export`: everything of mine at once.
    Export,
}

impl Action {
    /// `limits.actions_per_minute` divided by this is the burst per minute.
    pub fn divisor(self) -> u32 {
        match self {
            Action::Edit => 1,
            Action::React | Action::Profile => 2,
            Action::Typing => 3,
            Action::Search => 4,
            Action::Upload => 5,
            Action::Friend => 6,
            Action::Ring => 12,
            Action::Password => 24,
            Action::Export => 120,
        }
    }
}

/// What was turned away since the last [`report_task`] line.
static TURNED_AWAY: [std::sync::atomic::AtomicU64; 4] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 4];
const AWAY_REST: usize = 0;
const AWAY_ACTIONS: usize = 1;
const AWAY_SIGN_INS: usize = 2;
const AWAY_BUSY: usize = 3;

fn note(kind: usize) {
    TURNED_AWAY[kind].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// A request turned away by `limits.max_inflight_requests`.
pub fn note_busy() {
    note(AWAY_BUSY);
}

/// How often [`report_task`] says what the limits turned away.
const REPORT_INTERVAL: Duration = Duration::from_secs(60);

/// Once a minute, if any limit turned something away, says how much, so a flood shows in the
/// console without a line per request.
pub async fn report_task() {
    let mut interval = tokio::time::interval(REPORT_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        let [rest, actions, sign_ins, busy] =
            std::array::from_fn(|i| TURNED_AWAY[i].swap(0, std::sync::atomic::Ordering::Relaxed));
        if rest + actions + sign_ins + busy > 0 {
            tracing::warn!(
                "rate limits in the last minute turned away {rest} REST requests, {actions}                  actions, {sign_ins} gateway sign-ins and {busy} requests while busy"
            );
        }
    }
}

/// One address may make this many times the REST requests of one session.
const REST_IP_FACTOR: u32 = 10;
/// How often idle REST limiter entries are dropped.
const PRUNE_INTERVAL: Duration = Duration::from_secs(60);

/// A token bucket holding up to `burst` tokens, refilled at `burst` per `period`.
#[derive(Debug, Clone, Copy)]
pub struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Bucket {
    pub fn full(burst: u32, now: Instant) -> Self {
        Self {
            tokens: f64::from(burst),
            last: now,
        }
    }

    /// Takes a token, or says how long until one is available.
    pub fn take(&mut self, burst: u32, period: Duration, now: Instant) -> Result<(), Duration> {
        let burst = f64::from(burst.max(1));
        let per_token = period.as_secs_f64() / burst;
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed / per_token).min(burst);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - self.tokens) * per_token))
        }
    }

    /// Whether the bucket is full again by `now` (so forgetting it changes nothing).
    fn is_full(&self, burst: u32, period: Duration, now: Instant) -> bool {
        let per_token = period.as_secs_f64() / f64::from(burst.max(1));
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens + elapsed / per_token >= f64::from(burst)
    }
}

/// One [`Bucket`] per key. The limit is passed on each call, so it follows the config.
#[derive(Debug)]
pub struct KeyedLimiter<K> {
    buckets: Mutex<HashMap<K, Bucket>>,
}

impl<K> Default for KeyedLimiter<K> {
    fn default() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
        }
    }
}

/// Full buckets are dropped once the map grows this large.
const PRUNE_AT: usize = 10_000;

impl<K: Hash + Eq + Clone> KeyedLimiter<K> {
    pub fn check(&self, key: &K, burst: u32, period: Duration) -> Result<(), Duration> {
        self.check_at(key, burst, period, Instant::now())
    }

    pub fn check_at(
        &self,
        key: &K,
        burst: u32,
        period: Duration,
        now: Instant,
    ) -> Result<(), Duration> {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        if buckets.len() >= PRUNE_AT {
            buckets.retain(|_, b| !b.is_full(burst, period, now));
        }
        buckets
            .entry(key.clone())
            .or_insert_with(|| Bucket::full(burst, now))
            .take(burst, period, now)
    }
}

/// `wait` as `retry_after_ms`: whole milliseconds, rounded up, at least 1.
pub fn retry_ms(wait: Duration) -> u64 {
    u64::try_from(wait.as_micros().div_ceil(1000))
        .unwrap_or(u64::MAX)
        .max(1)
}

/// `RATE_LIMITED` with `retry_after_ms` and a `Retry-After` header (whole seconds, rounded up).
pub fn rate_limited(wait: Duration) -> axum::response::Response {
    let ms = retry_ms(wait);
    let mut response = Error(ApiError::rate_limited(ms)).into_response();
    response
        .headers_mut()
        .insert(RETRY_AFTER, HeaderValue::from(ms.div_ceil(1000)));
    response
}

/// The REST limiter key: the hashed bearer token, or the client IP without one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RestKey {
    Token([u8; 32]),
    Ip(IpAddr),
}

/// Keys requests by session token, falling back to the client IP.
#[derive(Debug, Clone)]
pub struct SessionOrIp {
    trusted_proxies: Arc<[ProxyRange]>,
}

/// Keys requests by client IP (`network.trusted_proxies` aware).
#[derive(Debug, Clone)]
pub struct ClientIp {
    trusted_proxies: Arc<[ProxyRange]>,
}

fn request_ip<T>(trusted: &[ProxyRange], req: &Request<T>) -> IpAddr {
    match req.extensions().get::<ConnectInfo<PeerAddr>>() {
        Some(ConnectInfo(peer)) => client_ip(trusted, peer.0, req.headers()),
        // Only when serving without connect info (never in the real server).
        None => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
    }
}

impl KeyExtractor for SessionOrIp {
    type Key = RestKey;

    fn extract<T>(&self, req: &Request<T>) -> Result<RestKey, GovernorError> {
        let token = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty());
        Ok(match token {
            Some(token) => RestKey::Token(Sha256::digest(token.as_bytes()).into()),
            None => RestKey::Ip(request_ip(&self.trusted_proxies, req)),
        })
    }
}

impl KeyExtractor for ClientIp {
    type Key = IpAddr;

    fn extract<T>(&self, req: &Request<T>) -> Result<IpAddr, GovernorError> {
        Ok(request_ip(&self.trusted_proxies, req))
    }
}

/// A `tower_governor` layer allowing `burst` requests per key, refilled evenly over `period`.
/// A macro, because the layer's middleware type cannot be named outside `tower_governor`.
macro_rules! governor_layer {
    ($extractor:expr, $burst:expr, $period:expr) => {{
        let burst: u32 = $burst.max(1);
        // At least 1 ns: more than a billion requests per second would round the refill period
        // down to zero, which `finish` refuses.
        let per_request = ($period / burst).max(Duration::from_nanos(1));
        let config = Arc::new(
            GovernorConfigBuilder::default()
                .key_extractor($extractor)
                .period(per_request)
                .burst_size(burst)
                .finish()
                .expect("burst and period are not zero"),
        );
        // Drop idle keys now and then; the task ends together with the router.
        let weak = Arc::downgrade(&config);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(PRUNE_INTERVAL);
            loop {
                interval.tick().await;
                let Some(config) = weak.upgrade() else { return };
                config.limiter().retain_recent();
            }
        });
        GovernorLayer::new(config).error_handler(governor_error)
    }};
}

fn governor_error(err: GovernorError) -> Response<Body> {
    match err {
        GovernorError::TooManyRequests { wait_time, .. } => {
            note(AWAY_REST);
            // tower_governor only reports whole seconds; at least 1 s.
            rate_limited(Duration::from_secs(wait_time.max(1)))
        }
        other => Error::internal(format!("rate limiter: {other}")).into_response(),
    }
}

/// Applies `limits.requests_per_second` per session (or IP) to every route of `router`.
/// Must run inside a tokio runtime.
pub fn limit_rest<S: Clone + Send + Sync + 'static>(
    router: Router<S>,
    trusted_proxies: &[ProxyRange],
    per_second: u32,
) -> Router<S> {
    let extractor = SessionOrIp {
        trusted_proxies: trusted_proxies.into(),
    };
    let by_ip = ClientIp {
        trusted_proxies: trusted_proxies.into(),
    };
    router
        .layer(governor_layer!(
            extractor,
            per_second,
            Duration::from_secs(1)
        ))
        // The session's limit trusts any token it is shown, verified or not; made-up ones
        // would each get a bucket of their own. Several people may share an address.
        .layer(governor_layer!(
            by_ip,
            per_second.saturating_mul(REST_IP_FACTOR),
            Duration::from_secs(1)
        ))
}

impl crate::state::AppState {
    /// One more `action` of `user`, unless they have done too many this minute.
    pub fn throttle(&self, user: hermes_proto::id::Id, action: Action) -> Result<(), Error> {
        let burst = (self.config.limits.actions_per_minute / action.divisor()).max(1);
        self.action_limiter
            .check(&(user, action), burst, Duration::from_secs(60))
            .map_err(|wait| {
                note(AWAY_ACTIONS);
                Error(ApiError::rate_limited(retry_ms(wait)))
            })
    }
}

/// Who signs in to the gateway, for `limits.gateway_identifies_per_minute`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignInKey {
    User(hermes_proto::id::Id),
    Ip(IpAddr),
}

/// One address may sign in this many times as often as one person (several may share it).
const SIGN_IN_IP_FACTOR: u32 = 10;

impl crate::state::AppState {
    /// One more gateway sign-in (identify or resume) from `ip`, unless it has made too many.
    /// Checked before the token, so guessing tokens costs too.
    pub fn sign_in_from(&self, ip: IpAddr) -> Result<(), Duration> {
        let burst = self
            .config
            .limits
            .gateway_identifies_per_minute
            .saturating_mul(SIGN_IN_IP_FACTOR);
        self.sign_in_limiter
            .check(&SignInKey::Ip(ip), burst, GATEWAY_OPS_PERIOD)
            .inspect_err(|_| note(AWAY_SIGN_INS))
    }

    /// One more gateway sign-in of `user`. Each one builds a `ready` or replays events and tells
    /// everyone who sees them that they came online, so they are rarer than other ops.
    pub fn sign_in_as(&self, user: hermes_proto::id::Id) -> Result<(), Duration> {
        let burst = self.config.limits.gateway_identifies_per_minute;
        self.sign_in_limiter
            .check(&SignInKey::User(user), burst, GATEWAY_OPS_PERIOD)
            .inspect_err(|_| note(AWAY_SIGN_INS))
    }
}

/// Requests per second one address may make to the web pages and `/healthz`, which need no
/// session.
pub const PUBLIC_PER_SECOND: u32 = 10;

/// Applies [`PUBLIC_PER_SECOND`] per IP to every route of `router`. Must run inside a tokio
/// runtime.
pub fn limit_public<S: Clone + Send + Sync + 'static>(
    router: Router<S>,
    trusted_proxies: &[ProxyRange],
) -> Router<S> {
    let extractor = ClientIp {
        trusted_proxies: trusted_proxies.into(),
    };
    router.layer(governor_layer!(
        extractor,
        PUBLIC_PER_SECOND,
        Duration::from_secs(1)
    ))
}

/// Applies `limits.login_attempts_per_minute` per IP to one route.
/// Must run inside a tokio runtime.
pub fn limit_login<S: Clone + Send + Sync + 'static>(
    route: MethodRouter<S>,
    trusted_proxies: &[ProxyRange],
    per_minute: u32,
) -> MethodRouter<S> {
    let extractor = ClientIp {
        trusted_proxies: trusted_proxies.into(),
    };
    route.layer(governor_layer!(
        extractor,
        per_minute,
        Duration::from_secs(60)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_allows_burst_then_refills_evenly() {
        let t0 = Instant::now();
        let period = Duration::from_secs(5);
        let mut b = Bucket::full(5, t0);
        for _ in 0..5 {
            b.take(5, period, t0).unwrap();
        }
        let wait = b.take(5, period, t0).unwrap_err();
        assert_eq!(wait, Duration::from_secs(1));
        let wait = b
            .take(5, period, t0 + Duration::from_millis(400))
            .unwrap_err();
        assert!((wait.as_millis() as i64 - 600).abs() <= 1, "{wait:?}");
        b.take(5, period, t0 + Duration::from_secs(1)).unwrap();
        // Never more than the burst, however long it waited.
        let later = t0 + Duration::from_secs(3600);
        for _ in 0..5 {
            b.take(5, period, later).unwrap();
        }
        assert!(b.take(5, period, later).is_err());
    }

    #[test]
    fn keyed_limiter_keeps_keys_apart() {
        let limiter = KeyedLimiter::default();
        let (now, period) = (Instant::now(), Duration::from_secs(60));
        limiter.check_at(&"a", 1, period, now).unwrap();
        assert!(limiter.check_at(&"a", 1, period, now).is_err());
        limiter.check_at(&"b", 1, period, now).unwrap();
    }

    #[tokio::test]
    async fn huge_limits_do_not_panic() {
        // 1 s / u32::MAX rounds down to 0 ns.
        let _ = limit_rest(Router::<()>::new(), &[], u32::MAX);
        let _ = limit_login::<()>(axum::routing::get(|| async {}), &[], u32::MAX);
    }

    #[test]
    fn keyed_limiter_prunes_full_buckets() {
        let limiter = KeyedLimiter::default();
        let (now, period) = (Instant::now(), Duration::from_secs(1));
        for i in 0..PRUNE_AT {
            limiter.check_at(&i, 2, period, now).unwrap();
        }
        limiter
            .check_at(&usize::MAX, 2, period, now + Duration::from_secs(10))
            .unwrap();
        assert_eq!(limiter.buckets.lock().unwrap().len(), 1);
    }
}
