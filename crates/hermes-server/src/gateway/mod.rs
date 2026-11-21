//! The gateway WebSocket (PLAN.md section 11): connection lifecycle, the EventBus and the `ready`
//! snapshot. One task per connection reads client ops and writes queued frames with `select!`.

pub mod bus;
pub mod last_online;
pub mod presence;
pub mod ready;
pub mod visibility;

use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use hermes_proto::gateway::{
    ClientMessage, CloseCode, Identify, Resume, ServerEvent, ServerMessage,
};
use hermes_proto::id::Id;
use hermes_proto::models::PresenceStatus;
use tokio::sync::watch;
use tokio::time::{Instant, sleep_until};

use crate::api::ClientAddr;
use crate::auth::token_hash;
use crate::logging::{self, GATEWAY};
use crate::ratelimit::{self, Bucket};
use crate::state::AppState;
use bus::{Attached, ResumeError};

/// A client that takes longer than this to accept a frame has stopped reading.
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Presence changes per connection: at most [`PRESENCE_BURST`] per [`PRESENCE_PERIOD`]. Each one
/// goes to everyone who sees the person, so more are held back and only the latest is applied.
const PRESENCE_BURST: u32 = 5;
const PRESENCE_PERIOD: std::time::Duration = std::time::Duration::from_secs(10);

/// Client frames are small; anything bigger is not a valid op.
const MAX_CLIENT_FRAME: usize = 16 * 1024;

/// `GET /api/v1/gateway`: upgrades to a WebSocket.
pub async fn handler(
    State(state): State<Arc<AppState>>,
    ClientAddr(ip): ClientAddr,
    ws: WebSocketUpgrade,
) -> Response {
    ws.max_message_size(MAX_CLIENT_FRAME)
        .max_frame_size(MAX_CLIENT_FRAME)
        .on_upgrade(move |socket| connection(state, socket, ip))
}

/// Why the connection loop stops.
enum End {
    /// Close the WebSocket with this code.
    Close(CloseCode),
    /// The client went away (or a write failed).
    Gone,
}

struct Connection {
    state: Arc<AppState>,
    socket: WebSocket,
    attached: Option<Attached>,
    /// The connection is closed with 4009 when this passes without a heartbeat (or identify).
    deadline: Instant,
    /// Client ops: `limits.gateway_ops_per_minute` per [`ratelimit::GATEWAY_OPS_PERIOD`], else 4008.
    ops: Bucket,
    /// Presence changes, see [`PRESENCE_BURST`].
    presence: Bucket,
    /// A presence change held back by `presence`, applied at the instant given.
    held_presence: Option<(PresenceStatus, Instant)>,
    /// The client's address, for the log.
    ip: IpAddr,
}

async fn connection(state: Arc<AppState>, socket: WebSocket, ip: IpAddr) {
    let opened = Instant::now();
    let interval = state.gateway.timings.heartbeat_interval;
    let identify_within = state.gateway.timings.identify_within;
    let shutdown = state.gateway.shutting_down();
    let gateway_ops = state.config.limits.gateway_ops_per_minute;
    let mut conn = Connection {
        deadline: Instant::now() + (interval * 2).min(identify_within),
        state,
        socket,
        attached: None,
        ops: Bucket::full(gateway_ops, std::time::Instant::now()),
        presence: Bucket::full(PRESENCE_BURST, std::time::Instant::now()),
        held_presence: None,
        ip,
    };
    let hello = ServerEvent::Hello {
        heartbeat_interval_ms: u64::try_from(interval.as_millis()).unwrap_or(u64::MAX),
    };
    let end = match conn.send_control(hello).await {
        Ok(()) => tokio::select! {
            end = conn.run() => end,
            () = shutdown => End::Close(CloseCode::ServerRestarting),
        },
        Err(end) => end,
    };
    if let End::Close(code) = end {
        let frame = CloseFrame {
            code: code.code(),
            reason: format!("{code:?}").into(),
        };
        let _ = conn.socket.send(Message::Close(Some(frame))).await;
    }
    if let Some(attached) = conn.attached.take() {
        let why = match &end {
            End::Close(code) => format!("{code:?}"),
            End::Gone => "the client went away".to_owned(),
        };
        tracing::info!(
            target: GATEWAY,
            "{} left the gateway ({ip}) after {}: {why}",
            logging::who(&conn.state, attached.session.user_id),
            logging::duration(opened.elapsed())
        );
        conn.state.gateway.detach(&attached);
        presence::disconnected(&conn.state, attached.session.user_id);
    }
}

/// Something the attached session wants written.
enum Outgoing {
    Frame(Arc<str>),
    Close(CloseCode),
}

/// The next queued frame or close request; never when not attached.
async fn outgoing(attached: &mut Option<Attached>) -> Outgoing {
    let Some(a) = attached else {
        return std::future::pending().await;
    };
    let frame = tokio::select! {
        biased;
        code = wait_close(&mut a.close) => return Outgoing::Close(code),
        frame = a.frames.recv() => frame,
    };
    match frame {
        Some(frame) => Outgoing::Frame(frame),
        // The queue is dropped only together with a close request.
        None => Outgoing::Close(wait_close(&mut a.close).await),
    }
}

/// Sleeps until `at`, or forever without one.
async fn sleep_until_some(at: Option<Instant>) {
    match at {
        Some(at) => sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn wait_close(rx: &mut watch::Receiver<Option<CloseCode>>) -> CloseCode {
    loop {
        if let Some(code) = *rx.borrow_and_update() {
            return code;
        }
        if rx.changed().await.is_err() {
            return rx.borrow().unwrap_or(CloseCode::SessionTimedOut);
        }
    }
}

/// The REST session and user of `token`; 4004 if it is invalid or the user is disabled, 4008 if
/// `ip` or the user signed in too often ([`AppState::sign_in_from`], [`AppState::sign_in_as`]).
async fn check_token(state: &AppState, ip: IpAddr, token: &str) -> Result<(Id, Id), End> {
    let limited = End::Close(CloseCode::RateLimited);
    if state.sign_in_from(ip).is_err() {
        tracing::info!(target: GATEWAY, "{ip} signed in to the gateway too often");
        return Err(limited);
    }
    let failed = End::Close(CloseCode::AuthenticationFailed);
    let session = state
        .sessions
        .lookup(&state.db.read, &token_hash(token))
        .await
        .map_err(|err| {
            tracing::error!("gateway: database: {err}");
            End::Close(CloseCode::UnknownError)
        })?
        .ok_or(failed)?;
    let enabled = state
        .cache()
        .users
        .get(&session.user_id)
        .is_some_and(|u| !u.disabled);
    if !enabled {
        return Err(End::Close(CloseCode::AuthenticationFailed));
    }
    if state.sign_in_as(session.user_id).is_err() {
        tracing::info!(
            target: GATEWAY,
            "{} signed in to the gateway too often ({ip})",
            logging::who(state, session.user_id)
        );
        return Err(limited);
    }
    Ok((session.user_id, session.id))
}

impl Connection {
    async fn run(&mut self) -> End {
        loop {
            let held_until = self.held_presence.map(|(_, at)| at);
            tokio::select! {
                msg = self.socket.recv() => {
                    let text = match msg {
                        Some(Ok(Message::Text(text))) => text,
                        Some(Ok(Message::Binary(_))) => return End::Close(CloseCode::DecodeError),
                        Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                        Some(Ok(Message::Close(_)) | Err(_)) | None => return End::Gone,
                    };
                    let now = std::time::Instant::now();
                    if self
                        .ops
                        .take(
                            self.state.config.limits.gateway_ops_per_minute,
                            ratelimit::GATEWAY_OPS_PERIOD,
                            now,
                        )
                        .is_err()
                    {
                        return End::Close(CloseCode::RateLimited);
                    }
                    let op = match ClientMessage::decode(text.as_str()) {
                        Ok(op) => op,
                        Err(err) => return End::Close(err.close_code()),
                    };
                    if let Err(end) = self.handle(op).await {
                        return end;
                    }
                }
                out = outgoing(&mut self.attached) => match out {
                    Outgoing::Frame(frame) => {
                        if let Err(end) = self.send_text(&frame).await {
                            return end;
                        }
                    }
                    Outgoing::Close(code) => return End::Close(code),
                },
                () = sleep_until(self.deadline) => return End::Close(CloseCode::SessionTimedOut),
                () = sleep_until_some(held_until) => {
                    if let Some((status, _)) = self.held_presence.take() {
                        self.set_presence(status);
                    }
                }
            }
        }
    }

    async fn handle(&mut self, op: ClientMessage) -> Result<(), End> {
        match op {
            ClientMessage::Identify(identify) => self.identify(identify).await,
            ClientMessage::Resume(resume) => self.resume(resume).await,
            ClientMessage::Heartbeat { .. } => {
                self.authenticated()?;
                self.heard_from();
                self.send_control(ServerEvent::HeartbeatAck {}).await
            }
            ClientMessage::PresenceUpdate { status } => {
                self.authenticated()?;
                self.set_presence(status);
                Ok(())
            }
        }
    }

    /// Applies `status` now, or holds it back (replacing one held before) until the presence
    /// bucket has room.
    fn set_presence(&mut self, status: PresenceStatus) {
        let Some(user_id) = self.attached.as_ref().map(|a| a.session.user_id) else {
            return;
        };
        let now = std::time::Instant::now();
        match self.presence.take(PRESENCE_BURST, PRESENCE_PERIOD, now) {
            Ok(()) => {
                self.held_presence = None;
                presence::set(&self.state, user_id, status);
            }
            Err(wait) => {
                let at = self
                    .held_presence
                    .map_or_else(|| Instant::now() + wait, |(_, at)| at);
                self.held_presence = Some((status, at));
            }
        }
    }

    /// The client is alive: it has two heartbeat intervals until the next heartbeat. Also after
    /// identify and resume, so the short wait for identifying does not carry over.
    fn heard_from(&mut self) {
        self.deadline = Instant::now() + self.state.gateway.timings.heartbeat_interval * 2;
    }

    /// The user of the attached session; 4003 before identify/resume.
    fn authenticated(&self) -> Result<Id, End> {
        self.attached
            .as_ref()
            .map(|a| a.session.user_id)
            .ok_or(End::Close(CloseCode::NotAuthenticated))
    }

    async fn identify(&mut self, identify: Identify) -> Result<(), End> {
        if self.attached.is_some() {
            return Err(End::Close(CloseCode::AlreadyAuthenticated));
        }
        // Newer clients are welcome: they hide what this server's `features` lack.
        if identify.protocol_version < hermes_proto::MIN_CLIENT_PROTOCOL {
            return Err(End::Close(CloseCode::UnsupportedProtocolVersion));
        }
        let (user_id, auth_session_id) = check_token(&self.state, self.ip, &identify.token).await?;
        let rows = ready::load_rows(&self.state, user_id)
            .await
            .map_err(|err| {
                tracing::error!("gateway: database: {err}");
                End::Close(CloseCode::UnknownError)
            })?;
        // Attach before taking the snapshot, so no event between the two is lost (one may
        // arrive twice; events are idempotent by ID).
        let attached = self.state.gateway.create(user_id, auth_session_id);
        let session_id = attached.session.id.clone();
        self.attached = Some(attached);
        self.heard_from();
        presence::connected(&self.state, user_id);
        let ready = ready::build(&self.state, user_id, session_id, rows)
            .ok_or(End::Close(CloseCode::AuthenticationFailed))?;
        tracing::info!(
            target: GATEWAY,
            "{} joined the gateway from {} ({} {})",
            logging::who(&self.state, user_id),
            self.ip,
            identify.client.name,
            identify.client.version
        );
        self.send_control(ServerEvent::Ready(Box::new(ready))).await
    }

    async fn resume(&mut self, resume: Resume) -> Result<(), End> {
        if self.attached.is_some() {
            return Err(End::Close(CloseCode::AlreadyAuthenticated));
        }
        let (user_id, auth_session_id) = check_token(&self.state, self.ip, &resume.token).await?;
        match self
            .state
            .gateway
            .resume(&resume.session_id, auth_session_id, resume.last_seq)
        {
            Ok((attached, replay)) => {
                tracing::info!(
                    target: GATEWAY,
                    "{} resumed their gateway session from {}",
                    logging::who(&self.state, user_id),
                    self.ip
                );
                self.attached = Some(attached);
                self.heard_from();
                presence::connected(&self.state, user_id);
                for frame in replay {
                    self.send_text(&frame).await?;
                }
                self.send_control(ServerEvent::Resumed {}).await
            }
            Err(ResumeError::SeqAhead) => Err(End::Close(CloseCode::InvalidSeq)),
            Err(ResumeError::Invalid) => {
                self.send_control(ServerEvent::InvalidSession { resumable: false })
                    .await
            }
        }
    }

    async fn send_control(&mut self, event: ServerEvent) -> Result<(), End> {
        let json = serde_json::to_string(&ServerMessage::control(event))
            .expect("server messages always serialize");
        self.send_text(&json).await
    }

    async fn send_text(&mut self, text: &str) -> Result<(), End> {
        match tokio::time::timeout(SEND_TIMEOUT, self.socket.send(Message::Text(text.into()))).await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) | Err(_) => Err(End::Gone),
        }
    }
}
