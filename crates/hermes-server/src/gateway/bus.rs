//! The EventBus (PLAN.md section 9.4): gateway sessions, their resume buffers, and delivering
//! dispatch events to the sessions of an [`Audience`].
//!
//! An event is serialized once. Each session numbers it with its own sequence, so a frame is
//! `{"s":N,` followed by the shared body, stored as an `Arc<str>` in the resume buffer and pushed
//! into the session's bounded queue. A full queue closes the connection with 4009; the client
//! then resumes from the buffer.
//!
//! Sessions are indexed by ID (for `resume`) and by user (for delivering events). Lock order:
//! the session map, then a session's own lock.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use hermes_proto::gateway::{CloseCode, HEARTBEAT_INTERVAL_MS, ServerEvent};
use hermes_proto::id::Id;
use hermes_proto::permissions::Permissions;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::state::Cache;

/// Who receives an event. Resolved to users with the state cache at publish time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audience {
    /// Members of a space with `perm`: space-level, or in the channel when one is given.
    Space(Id, Permissions, Option<Id>),
    /// DM recipients, or the space members with `VIEW_CHANNEL` in the channel.
    Channel(Id),
    /// Every session of one user.
    User(Id),
    Users(Vec<Id>),
}

impl Audience {
    /// Every member of a space.
    pub const fn space(space_id: Id) -> Self {
        Audience::Space(space_id, Permissions::empty(), None)
    }

    /// The users this audience means right now.
    pub fn resolve(&self, cache: &Cache) -> HashSet<Id> {
        match self {
            Audience::Space(space_id, perm, channel_id) => {
                let Some(space) = cache.spaces.get(space_id) else {
                    return HashSet::new();
                };
                space
                    .members
                    .keys()
                    .copied()
                    .filter(|&user_id| {
                        let have = match channel_id {
                            Some(channel_id) => cache.channel_permissions(*channel_id, user_id),
                            None => cache.space_permissions(*space_id, user_id),
                        };
                        have.contains(*perm)
                    })
                    .collect()
            }
            Audience::Channel(channel_id) => cache.channel_viewers(*channel_id),
            Audience::User(user_id) => HashSet::from([*user_id]),
            Audience::Users(user_ids) => user_ids.iter().copied().collect(),
        }
    }
}

/// Gateway timings and sizes (PLAN.md section 11.2). Tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    /// Announced in `hello`; a connection that sends no heartbeat (or, before that, no identify)
    /// for two intervals is closed with 4009.
    pub heartbeat_interval: Duration,
    /// A new connection that has not identified or resumed by then is closed with 4009. Once
    /// it has, only heartbeats count.
    pub identify_within: Duration,
    /// How long a disconnected session stays resumable, and the age limit of buffered events.
    pub resume_window: Duration,
    /// Events kept per session for resuming.
    pub resume_buffer: usize,
    /// Frames queued per connection before it is closed with 4009.
    pub queue_capacity: usize,
}

impl Default for Timings {
    fn default() -> Self {
        Timings {
            heartbeat_interval: Duration::from_millis(HEARTBEAT_INTERVAL_MS),
            identify_within: Duration::from_secs(15),
            resume_window: Duration::from_secs(5 * 60),
            resume_buffer: 2000,
            queue_capacity: 1024,
        }
    }
}

/// Gateway sessions one person may have at once.
const MAX_SESSIONS_PER_USER: usize = 32;

/// What one session may hold for resuming, however few events that is.
const RESUME_BUFFER_BYTES: usize = 32 << 20;

/// All gateway sessions, connected or waiting to be resumed.
#[derive(Debug, Default)]
pub struct Gateway {
    pub timings: Timings,
    sessions: Mutex<Sessions>,
    /// Set once on shutdown; every connection then closes with 4100.
    shutdown: watch::Sender<bool>,
}

/// The sessions by ID and by user; the two always hold the same sessions.
#[derive(Debug, Default)]
struct Sessions {
    by_id: HashMap<String, Arc<Session>>,
    by_user: HashMap<Id, Vec<Arc<Session>>>,
}

impl Sessions {
    fn insert(&mut self, session: Arc<Session>) {
        self.by_user
            .entry(session.user_id)
            .or_default()
            .push(session.clone());
        self.by_id.insert(session.id.clone(), session);
    }

    fn remove(&mut self, session_id: &str) -> Option<Arc<Session>> {
        let session = self.by_id.remove(session_id)?;
        if let Some(of_user) = self.by_user.get_mut(&session.user_id) {
            of_user.retain(|s| !Arc::ptr_eq(s, &session));
            if of_user.is_empty() {
                self.by_user.remove(&session.user_id);
            }
        }
        Some(session)
    }

    /// Removes every session for which `drop` is true.
    fn remove_where(&mut self, mut drop: impl FnMut(&Session) -> bool) -> Vec<Arc<Session>> {
        let ids: Vec<String> = self
            .by_id
            .values()
            .filter(|s| drop(s))
            .map(|s| s.id.clone())
            .collect();
        ids.iter().filter_map(|id| self.remove(id)).collect()
    }

    /// The sessions of `users`, except those of `except`. Walks whichever side is smaller: the
    /// audience or the users that have sessions.
    fn of_users(&self, users: &HashSet<Id>, except: Option<Id>) -> Vec<Arc<Session>> {
        let wanted = |user_id: &Id| Some(*user_id) != except;
        if users.len() <= self.by_user.len() {
            users
                .iter()
                .filter(|u| wanted(u))
                .filter_map(|u| self.by_user.get(u))
                .flatten()
                .cloned()
                .collect()
        } else {
            self.by_user
                .iter()
                .filter(|(u, _)| wanted(u) && users.contains(u))
                .flat_map(|(_, of_user)| of_user)
                .cloned()
                .collect()
        }
    }
}

/// One gateway session: created by `identify`, survives disconnects for
/// [`Timings::resume_window`].
#[derive(Debug)]
pub struct Session {
    /// Random, sent in `ready` and used by `resume`.
    pub id: String,
    pub user_id: Id,
    /// The REST session (token) it was opened with; revoking that closes it with 4011.
    pub auth_session_id: Id,
    inner: Mutex<SessionInner>,
}

#[derive(Debug)]
struct SessionInner {
    /// Last sequence number used; 0 = none yet.
    seq: u64,
    buffer: VecDeque<Buffered>,
    /// What `buffer` holds, in bytes.
    buffered_bytes: usize,
    conn: Option<Conn>,
    /// Set while no connection is attached.
    disconnected_at: Option<Instant>,
    next_conn_id: u64,
}

#[derive(Debug)]
struct Buffered {
    seq: u64,
    frame: Arc<str>,
    at: Instant,
}

#[derive(Debug)]
struct Conn {
    id: u64,
    frames: mpsc::Sender<Arc<str>>,
    close: watch::Sender<Option<CloseCode>>,
}

/// The receiving side of an attached connection.
#[derive(Debug)]
pub struct Attached {
    pub session: Arc<Session>,
    pub conn_id: u64,
    pub frames: mpsc::Receiver<Arc<str>>,
    /// Becomes `Some(code)` when the server wants the connection closed.
    pub close: watch::Receiver<Option<CloseCode>>,
}

/// Why a session cannot be resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeError {
    /// Unknown, expired, of another token, or the buffer no longer reaches back far enough:
    /// the client must identify again (`invalid_session`).
    Invalid,
    /// `last_seq` is ahead of the session (close 4007).
    SeqAhead,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A random session ID: 16 bytes in hex.
fn new_session_id() -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    let mut id = String::with_capacity(2 * bytes.len());
    for b in bytes {
        let _ = write!(id, "{b:02x}");
    }
    id
}

/// The part of a serialized event after its opening `{`, ready to be prefixed with `{"s":N,`.
pub fn event_body(event: &ServerEvent) -> String {
    let json = serde_json::to_string(event).expect("server events always serialize");
    debug_assert!(json.starts_with('{'), "events serialize to objects");
    json[1..].to_owned()
}

impl Gateway {
    pub fn new(timings: Timings) -> Self {
        Gateway {
            timings,
            sessions: Mutex::default(),
            shutdown: watch::Sender::new(false),
        }
    }

    /// Closes every connection, now and later ones, with 4100 ("server restarting").
    pub fn shut_down(&self) {
        self.shutdown.send_replace(true);
    }

    /// Completes once [`shut_down`](Self::shut_down) was called.
    pub fn shutting_down(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut rx = self.shutdown.subscribe();
        async move {
            let _ = rx.wait_for(|down| *down).await;
        }
    }

    /// Creates a session for `identify` and attaches the connection to it. Also drops sessions
    /// whose resume window has passed.
    pub fn create(&self, user_id: Id, auth_session_id: Id) -> Attached {
        let now = Instant::now();
        let session = Arc::new(Session {
            id: new_session_id(),
            user_id,
            auth_session_id,
            inner: Mutex::new(SessionInner {
                seq: 0,
                buffer: VecDeque::new(),
                buffered_bytes: 0,
                conn: None,
                disconnected_at: Some(now),
                next_conn_id: 0,
            }),
        });
        let attached = session.attach(self.timings.queue_capacity);
        let mut sessions = lock(&self.sessions);
        let window = self.timings.resume_window;
        sessions.remove_where(|s| s.expired(now, window));
        // One person's sockets stay few: the oldest one goes to make room (`by_user` keeps
        // creation order).
        let oldest = sessions
            .by_user
            .get(&user_id)
            .filter(|mine| mine.len() >= MAX_SESSIONS_PER_USER)
            .and_then(|mine| mine.first())
            .map(|s| s.id.clone());
        if let Some(oldest) = oldest.and_then(|id| sessions.remove(&id))
            && let Some(conn) = lock(&oldest.inner).conn.take()
        {
            conn.close.send_replace(Some(CloseCode::SessionTimedOut));
        }
        sessions.insert(session);
        attached
    }

    /// Attaches a new connection to an existing session and returns the frames after
    /// `last_seq` to replay first. A connection still attached to it is closed with 4009.
    pub fn resume(
        &self,
        session_id: &str,
        auth_session_id: Id,
        last_seq: u64,
    ) -> Result<(Attached, Vec<Arc<str>>), ResumeError> {
        let now = Instant::now();
        let window = self.timings.resume_window;
        let session = {
            let mut sessions = lock(&self.sessions);
            let session = sessions.by_id.get(session_id).cloned();
            match session {
                Some(s) if s.expired(now, window) => {
                    sessions.remove(session_id);
                    return Err(ResumeError::Invalid);
                }
                Some(s) if s.auth_session_id == auth_session_id => s,
                _ => return Err(ResumeError::Invalid),
            }
        };
        let mut inner = lock(&session.inner);
        if last_seq > inner.seq {
            return Err(ResumeError::SeqAhead);
        }
        inner.prune(now, &self.timings);
        let oldest_kept = inner.buffer.front().map_or(inner.seq + 1, |b| b.seq);
        if last_seq + 1 < oldest_kept {
            drop(inner);
            lock(&self.sessions).remove(session_id);
            return Err(ResumeError::Invalid);
        }
        let replay = inner
            .buffer
            .iter()
            .filter(|b| b.seq > last_seq)
            .map(|b| b.frame.clone())
            .collect();
        if let Some(old) = inner.conn.take() {
            old.close.send_replace(Some(CloseCode::SessionTimedOut));
        }
        let attached = inner.attach(&session, self.timings.queue_capacity);
        drop(inner);
        Ok((attached, replay))
    }

    /// Called when a connection ends. The session stays resumable for the resume window.
    pub fn detach(&self, attached: &Attached) {
        let mut inner = lock(&attached.session.inner);
        if inner
            .conn
            .as_ref()
            .is_some_and(|c| c.id == attached.conn_id)
        {
            inner.conn = None;
            inner.disconnected_at = Some(Instant::now());
        }
    }

    /// Whether `user_id` has at least one connected session.
    pub fn is_connected(&self, user_id: Id) -> bool {
        lock(&self.sessions)
            .by_user
            .get(&user_id)
            .is_some_and(|of_user| of_user.iter().any(|s| lock(&s.inner).conn.is_some()))
    }

    /// The REST sessions that have a gateway session.
    pub fn auth_session_ids(&self) -> HashSet<Id> {
        lock(&self.sessions)
            .by_id
            .values()
            .map(|s| s.auth_session_id)
            .collect()
    }

    /// Closes (4011) and forgets every session opened with one of these REST sessions.
    pub fn revoke(&self, auth_session_ids: &[Id]) {
        let revoked =
            lock(&self.sessions).remove_where(|s| auth_session_ids.contains(&s.auth_session_id));
        for session in revoked {
            if let Some(conn) = lock(&session.inner).conn.take() {
                conn.close.send_replace(Some(CloseCode::SessionRevoked));
            }
        }
    }

    /// Sends a dispatch event to every session of `users`, except those of `except`. Sessions
    /// whose resume window has passed get nothing and are dropped.
    pub fn send(&self, users: &HashSet<Id>, except: Option<Id>, event: &ServerEvent) {
        debug_assert!(event.is_dispatch(), "control op published on the bus");
        let targets = lock(&self.sessions).of_users(users, except);
        if targets.is_empty() {
            return;
        }
        let body = event_body(event);
        let now = Instant::now();
        let expired: Vec<&Arc<Session>> = targets
            .iter()
            .filter(|session| !session.push(&body, now, &self.timings))
            .collect();
        if !expired.is_empty() {
            let mut sessions = lock(&self.sessions);
            for session in expired {
                // An expired session never comes back (`resume` refuses it), so it can go.
                if sessions
                    .by_id
                    .get(&session.id)
                    .is_some_and(|s| Arc::ptr_eq(s, session))
                {
                    sessions.remove(&session.id);
                }
            }
        }
    }

    /// Number of known sessions (connected or resumable).
    pub fn session_count(&self) -> usize {
        lock(&self.sessions).by_id.len()
    }
}

impl Session {
    fn attach(self: &Arc<Self>, capacity: usize) -> Attached {
        lock(&self.inner).attach(self, capacity)
    }

    fn expired(&self, now: Instant, window: Duration) -> bool {
        lock(&self.inner).expired(now, window)
    }

    /// Numbers, buffers and queues one event. Returns `false` (and does nothing) if the
    /// session's resume window has passed.
    fn push(&self, body: &str, now: Instant, timings: &Timings) -> bool {
        let mut inner = lock(&self.inner);
        if inner.expired(now, timings.resume_window) {
            return false;
        }
        inner.seq += 1;
        let frame: Arc<str> = format!("{{\"s\":{},{body}", inner.seq).into();
        let seq = inner.seq;
        inner.buffered_bytes += frame.len();
        inner.buffer.push_back(Buffered {
            seq,
            frame: frame.clone(),
            at: now,
        });
        inner.prune(now, timings);
        let overflowed = inner
            .conn
            .as_ref()
            .is_some_and(|conn| conn.frames.try_send(frame).is_err());
        if overflowed && let Some(conn) = inner.conn.take() {
            conn.close.send_replace(Some(CloseCode::SessionTimedOut));
            inner.disconnected_at = Some(now);
        }
        true
    }
}

impl SessionInner {
    /// Disconnected for at least `window`: it can no longer be resumed.
    fn expired(&self, now: Instant, window: Duration) -> bool {
        self.disconnected_at
            .is_some_and(|at| now.duration_since(at) >= window)
    }

    fn attach(&mut self, session: &Arc<Session>, capacity: usize) -> Attached {
        let (frames_tx, frames_rx) = mpsc::channel(capacity);
        let (close_tx, close_rx) = watch::channel(None);
        self.next_conn_id += 1;
        let conn_id = self.next_conn_id;
        self.conn = Some(Conn {
            id: conn_id,
            frames: frames_tx,
            close: close_tx,
        });
        self.disconnected_at = None;
        Attached {
            session: session.clone(),
            conn_id,
            frames: frames_rx,
            close: close_rx,
        }
    }

    /// Keeps at most `resume_buffer` events, none older than `resume_window`.
    fn prune(&mut self, now: Instant, timings: &Timings) {
        while self.buffer.len() > timings.resume_buffer
            || self.buffered_bytes > RESUME_BUFFER_BYTES
            || self
                .buffer
                .front()
                .is_some_and(|b| now.duration_since(b.at) >= timings.resume_window)
        {
            let Some(dropped) = self.buffer.pop_front() else {
                break;
            };
            self.buffered_bytes -= dropped.frame.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_proto::gateway::ServerMessage;

    const ALICE: Id = Id(1);
    const BOB: Id = Id(2);

    fn event(n: u64) -> ServerEvent {
        ServerEvent::DmDelete { id: Id(n) }
    }

    fn seq_of(frame: &str) -> u64 {
        let msg: ServerMessage = serde_json::from_str(frame).unwrap();
        msg.s.unwrap()
    }

    #[test]
    fn frames_are_numbered_per_session() {
        let gw = Gateway::default();
        let mut a = gw.create(ALICE, Id(10));
        let mut b = gw.create(BOB, Id(11));
        gw.send(&HashSet::from([ALICE]), None, &event(1));
        gw.send(&HashSet::from([ALICE, BOB]), None, &event(2));
        let frames: Vec<_> = std::iter::from_fn(|| a.frames.try_recv().ok()).collect();
        assert_eq!(frames.iter().map(|f| seq_of(f)).collect::<Vec<_>>(), [1, 2]);
        let msg: ServerMessage = serde_json::from_str(&frames[1]).unwrap();
        assert_eq!(msg, ServerMessage::dispatch(2, event(2)));
        assert_eq!(seq_of(&b.frames.try_recv().unwrap()), 1);
        // `except` skips a user.
        gw.send(&HashSet::from([ALICE, BOB]), Some(BOB), &event(3));
        assert!(b.frames.try_recv().is_err());
        assert_eq!(seq_of(&a.frames.try_recv().unwrap()), 3);
    }

    #[test]
    fn resume_replays_missed_frames() {
        let gw = Gateway::default();
        let a = gw.create(ALICE, Id(10));
        let id = a.session.id.clone();
        gw.send(&HashSet::from([ALICE]), None, &event(1));
        gw.detach(&a);
        assert!(!gw.is_connected(ALICE));
        gw.send(&HashSet::from([ALICE]), None, &event(2));
        gw.send(&HashSet::from([ALICE]), None, &event(3));

        assert_eq!(
            gw.resume(&id, Id(99), 1).unwrap_err(),
            ResumeError::Invalid,
            "another token"
        );
        assert_eq!(
            gw.resume(&id, Id(10), 4).unwrap_err(),
            ResumeError::SeqAhead
        );
        let (mut b, replay) = gw.resume(&id, Id(10), 1).unwrap();
        assert_eq!(replay.iter().map(|f| seq_of(f)).collect::<Vec<_>>(), [2, 3]);
        assert!(gw.is_connected(ALICE));
        gw.send(&HashSet::from([ALICE]), None, &event(4));
        assert_eq!(seq_of(&b.frames.try_recv().unwrap()), 4);
        // The old connection's detach does not detach the new one.
        gw.detach(&a);
        assert!(gw.is_connected(ALICE));
    }

    #[test]
    fn resume_fails_when_the_buffer_does_not_reach_back() {
        let gw = Gateway::new(Timings {
            resume_buffer: 2,
            ..Timings::default()
        });
        let a = gw.create(ALICE, Id(10));
        let id = a.session.id.clone();
        for n in 1..=4 {
            gw.send(&HashSet::from([ALICE]), None, &event(n));
        }
        gw.detach(&a);
        // Frames 3 and 4 are kept: resuming from 2 works, from 1 does not.
        assert!(gw.resume(&id, Id(10), 2).is_ok());
        assert_eq!(gw.resume(&id, Id(10), 1).unwrap_err(), ResumeError::Invalid);
        // A failed resume forgets the session.
        assert_eq!(gw.session_count(), 0);
    }

    #[test]
    fn full_queue_closes_the_connection() {
        let gw = Gateway::new(Timings {
            queue_capacity: 2,
            ..Timings::default()
        });
        let a = gw.create(ALICE, Id(10));
        for n in 1..=3 {
            gw.send(&HashSet::from([ALICE]), None, &event(n));
        }
        assert_eq!(*a.close.borrow(), Some(CloseCode::SessionTimedOut));
        assert!(!gw.is_connected(ALICE));
        // Still resumable from the buffer.
        assert_eq!(gw.resume(&a.session.id, Id(10), 0).unwrap().1.len(), 3);
    }

    #[test]
    fn revoke_closes_with_4011() {
        let gw = Gateway::default();
        let a = gw.create(ALICE, Id(10));
        let b = gw.create(ALICE, Id(11));
        gw.revoke(&[Id(10)]);
        assert_eq!(*a.close.borrow(), Some(CloseCode::SessionRevoked));
        assert_eq!(*b.close.borrow(), None);
        assert_eq!(
            gw.resume(&a.session.id, Id(10), 0).unwrap_err(),
            ResumeError::Invalid
        );
        assert_eq!(gw.session_count(), 1);
    }

    #[test]
    fn events_reach_the_sessions_of_small_and_large_audiences() {
        let gw = Gateway::default();
        let mut a1 = gw.create(ALICE, Id(10));
        let mut a2 = gw.create(ALICE, Id(11));
        let mut b = gw.create(BOB, Id(12));
        // Fewer users than have sessions, then many users who mostly have none.
        gw.send(&HashSet::from([ALICE]), None, &event(1));
        let crowd: HashSet<Id> = (1..=100).map(Id).collect();
        gw.send(&crowd, Some(BOB), &event(2));
        gw.send(&crowd, None, &event(3));
        for a in [&mut a1, &mut a2] {
            let seqs: Vec<u64> = std::iter::from_fn(|| a.frames.try_recv().ok())
                .map(|f| seq_of(&f))
                .collect();
            assert_eq!(seqs, [1, 2, 3]);
        }
        let seqs: Vec<u64> = std::iter::from_fn(|| b.frames.try_recv().ok())
            .map(|f| seq_of(&f))
            .collect();
        assert_eq!(seqs, [1], "only event 3, as bob's first");
        assert!(gw.is_connected(ALICE));
        gw.revoke(&[Id(10), Id(11)]);
        assert!(!gw.is_connected(ALICE));
        assert!(gw.is_connected(BOB));
        assert_eq!(gw.session_count(), 1);
    }

    #[test]
    fn sending_drops_expired_sessions() {
        let gw = Gateway::new(Timings {
            resume_window: Duration::ZERO,
            ..Timings::default()
        });
        let a = gw.create(ALICE, Id(10));
        let _b = gw.create(BOB, Id(11));
        gw.detach(&a);
        assert_eq!(gw.session_count(), 2);
        gw.send(&HashSet::from([ALICE, BOB]), None, &event(1));
        assert_eq!(gw.session_count(), 1, "alice's expired session is gone");
        assert!(gw.is_connected(BOB));
        assert!(!gw.is_connected(ALICE));
    }

    #[test]
    fn session_ids_are_32_hex_digits() {
        let id = new_session_id();
        assert_eq!(id.len(), 32);
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_ne!(id, new_session_id());
    }

    #[test]
    fn expired_sessions_cannot_resume() {
        let gw = Gateway::new(Timings {
            resume_window: Duration::ZERO,
            ..Timings::default()
        });
        let a = gw.create(ALICE, Id(10));
        gw.detach(&a);
        assert_eq!(
            gw.resume(&a.session.id, Id(10), 0).unwrap_err(),
            ResumeError::Invalid
        );
    }
}
