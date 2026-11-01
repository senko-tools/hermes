//! The gateway WebSocket (PLAN.md section 11): connection lifecycle, the EventBus and the `ready`
//! snapshot. One task per connection reads client ops and writes queued frames with `select!`.

pub mod bus;
pub mod presence;
pub mod ready;
pub mod visibility;

use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use hermes_proto::PROTOCOL_VERSION;
use hermes_proto::gateway::{
    ClientMessage, CloseCode, Identify, Resume, ServerEvent, ServerMessage,
};
use hermes_proto::id::Id;
use tokio::sync::watch;
use tokio::time::{Instant, sleep_until};

use crate::auth::token_hash;
use crate::db;
use crate::ratelimit::{self, Bucket};
use crate::state::AppState;
use bus::{Attached, ResumeError};

/// Client frames are small; anything bigger is not a valid op.
const MAX_CLIENT_FRAME: usize = 16 * 1024;

/// `GET /api/v1/gateway`: upgrades to a WebSocket.
pub async fn handler(State(state): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(MAX_CLIENT_FRAME)
        .on_upgrade(move |socket| connection(state, socket))
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
    /// Client ops: [`ratelimit::GATEWAY_OPS`] per [`ratelimit::GATEWAY_OPS_PERIOD`], else 4008.
    ops: Bucket,
}

async fn connection(state: Arc<AppState>, socket: WebSocket) {
    let interval = state.gateway.timings.heartbeat_interval;
    let shutdown = state.gateway.shutting_down();
    let mut conn = Connection {
        deadline: Instant::now() + interval * 2,
        state,
        socket,
        attached: None,
        ops: Bucket::full(ratelimit::GATEWAY_OPS, std::time::Instant::now()),
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
            reason: "".into(),
        };
        let _ = conn.socket.send(Message::Close(Some(frame))).await;
    }
    if let Some(attached) = conn.attached.take() {
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

/// The REST session and user of `token`; 4004 if it is invalid or the user is disabled.
async fn check_token(state: &AppState, token: &str) -> Result<(Id, Id), End> {
    let failed = End::Close(CloseCode::AuthenticationFailed);
    let session = db::auth::session_by_token_hash(&state.db.read, &token_hash(token))
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
    Ok((session.user_id, session.id))
}

impl Connection {
    async fn run(&mut self) -> End {
        loop {
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
                        .take(ratelimit::GATEWAY_OPS, ratelimit::GATEWAY_OPS_PERIOD, now)
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
            }
        }
    }

    async fn handle(&mut self, op: ClientMessage) -> Result<(), End> {
        match op {
            ClientMessage::Identify(identify) => self.identify(identify).await,
            ClientMessage::Resume(resume) => self.resume(resume).await,
            ClientMessage::Heartbeat { .. } => {
                self.authenticated()?;
                self.deadline = Instant::now() + self.state.gateway.timings.heartbeat_interval * 2;
                self.send_control(ServerEvent::HeartbeatAck {}).await
            }
            ClientMessage::PresenceUpdate { status } => {
                let user_id = self.authenticated()?;
                presence::set(&self.state, user_id, status);
                Ok(())
            }
        }
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
        if identify.protocol_version != PROTOCOL_VERSION {
            return Err(End::Close(CloseCode::UnsupportedProtocolVersion));
        }
        let (user_id, auth_session_id) = check_token(&self.state, &identify.token).await?;
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
        presence::connected(&self.state, user_id);
        let ready = ready::build(&self.state, user_id, session_id, rows)
            .ok_or(End::Close(CloseCode::AuthenticationFailed))?;
        tracing::debug!(
            user_id = %user_id,
            client = %identify.client.name,
            version = %identify.client.version,
            "gateway: identified"
        );
        self.send_control(ServerEvent::Ready(Box::new(ready))).await
    }

    async fn resume(&mut self, resume: Resume) -> Result<(), End> {
        if self.attached.is_some() {
            return Err(End::Close(CloseCode::AlreadyAuthenticated));
        }
        let (user_id, auth_session_id) = check_token(&self.state, &resume.token).await?;
        match self
            .state
            .gateway
            .resume(&resume.session_id, auth_session_id, resume.last_seq)
        {
            Ok((attached, replay)) => {
                self.attached = Some(attached);
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
        self.socket
            .send(Message::Text(text.into()))
            .await
            .map_err(|_| End::Gone)
    }
}
