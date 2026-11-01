//! Gateway connection (PLAN.md section 11): hello, identify or resume, heartbeats, and
//! reconnecting with backoff. One task per instance; it reports to the core over a channel.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use hermes_proto::PROTOCOL_VERSION;
use hermes_proto::gateway::{
    ClientInfo, ClientMessage, CloseAction, CloseCode, Identify, Ready, Resume, ServerEvent,
    ServerMessage,
};
use hermes_proto::models::PresenceStatus;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until, timeout};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as WsCloseCode;

use crate::address::HostPort;
use crate::tls::{self, Trust};

/// How long the server has to send `hello` (and `ready`/`resumed`) before the attempt counts as
/// failed.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Connection status reported to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnStatus {
    Connecting,
    Connected,
    /// Waiting before reconnect attempt `attempt` (1 = first retry).
    Reconnecting {
        attempt: u32,
    },
    /// Not reconnecting: the user must act (update the client, check the address).
    Failed {
        reason: String,
    },
    /// The session was revoked or the token is invalid.
    LoggedOut,
    /// Disconnected on request (`set_online(false)`).
    Offline,
}

/// What the gateway task tells the core.
#[derive(Debug)]
pub enum GatewayEvent {
    Status(ConnStatus),
    /// A fresh session: the store must be replaced.
    Ready(Box<Ready>),
    /// Missed events were replayed; the store is current again.
    Resumed,
    Dispatch(Box<ServerEvent>),
}

#[derive(Debug)]
pub(crate) enum Command {
    Presence(PresenceStatus),
    /// Close the connection but keep the session for a later resume.
    Offline,
    Online,
    Shutdown,
}

/// Handle to a running gateway task. Dropping it stops the task.
#[derive(Debug)]
pub struct Gateway {
    tx: mpsc::UnboundedSender<Command>,
}

impl Gateway {
    pub(crate) fn spawn(
        host: HostPort,
        trust: Trust,
        token: String,
        client: ClientInfo,
        events: mpsc::UnboundedSender<GatewayEvent>,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let task = Task {
            url: format!("wss://{}/api/v1/gateway", host.authority()),
            trust,
            token,
            client,
            events,
            commands: rx,
            session_id: None,
            last_seq: 0,
            presence: None,
        };
        tokio::spawn(task.run());
        Gateway { tx }
    }

    pub(crate) fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

/// Backoff before reconnect attempt `attempt` (1-based): 1 s, 2 s, 4 s, … capped at 30 s,
/// with ±20 % jitter.
pub fn backoff(attempt: u32) -> Duration {
    let base = Duration::from_secs(1u64 << attempt.saturating_sub(1).min(5)).min(MAX_BACKOFF);
    let jitter = rand::random_range(0.8..1.2);
    base.mul_f64(jitter)
}

/// Heartbeat interval with ±10 % jitter.
fn jittered(interval: Duration) -> Duration {
    interval.mul_f64(rand::random_range(0.9..1.1))
}

/// How one connection ended.
enum End {
    /// Reconnect after the normal backoff.
    Retry,
    /// Reconnect after at least this long.
    RetryAfter(Duration),
    /// Stop reporting this status.
    Stop(ConnStatus),
    /// `Command::Offline`.
    Offline,
    Shutdown,
}

struct Task {
    url: String,
    trust: Trust,
    token: String,
    client: ClientInfo,
    events: mpsc::UnboundedSender<GatewayEvent>,
    commands: mpsc::UnboundedReceiver<Command>,
    session_id: Option<String>,
    last_seq: u64,
    presence: Option<PresenceStatus>,
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

impl Task {
    fn emit(&self, event: GatewayEvent) -> bool {
        self.events.send(event).is_ok()
    }

    async fn run(mut self) {
        let mut attempt = 0u32;
        let mut min_wait = Duration::ZERO;
        loop {
            if attempt > 0 {
                if !self.emit(GatewayEvent::Status(ConnStatus::Reconnecting { attempt })) {
                    return;
                }
                let wait = backoff(attempt).max(min_wait);
                if self.wait(wait).await {
                    return;
                }
            } else if !self.emit(GatewayEvent::Status(ConnStatus::Connecting)) {
                return;
            }
            min_wait = Duration::ZERO;
            let mut connected = false;
            match self.connection(&mut connected).await {
                End::Retry => {}
                End::RetryAfter(d) => min_wait = d,
                End::Stop(status) => {
                    self.emit(GatewayEvent::Status(status));
                    // Wait for an explicit `Online` (e.g. after logging in again) or shutdown.
                    if !self.wait_online().await {
                        return;
                    }
                    attempt = 0;
                    continue;
                }
                End::Offline => {
                    self.emit(GatewayEvent::Status(ConnStatus::Offline));
                    if !self.wait_online().await {
                        return;
                    }
                    attempt = 0;
                    continue;
                }
                End::Shutdown => return,
            }
            attempt = if connected { 1 } else { attempt + 1 };
        }
    }

    /// Sleeps for `d` while handling commands; `true` on shutdown.
    async fn wait(&mut self, d: Duration) -> bool {
        let deadline = Instant::now() + d;
        loop {
            tokio::select! {
                () = sleep_until(deadline) => return false,
                cmd = self.commands.recv() => match cmd {
                    None | Some(Command::Shutdown) => return true,
                    Some(Command::Presence(p)) => self.presence = Some(p),
                    // Retry right away.
                    Some(Command::Online) => return false,
                    Some(Command::Offline) => {
                        self.emit(GatewayEvent::Status(ConnStatus::Offline));
                        if !self.wait_online().await {
                            return true;
                        }
                        return false;
                    }
                },
            }
        }
    }

    /// Waits for `Online`; `false` on shutdown.
    async fn wait_online(&mut self) -> bool {
        loop {
            match self.commands.recv().await {
                None | Some(Command::Shutdown) => return false,
                Some(Command::Online) => return true,
                Some(Command::Presence(p)) => self.presence = Some(p),
                Some(Command::Offline) => {}
            }
        }
    }

    async fn connect(&self) -> Result<Ws, String> {
        let config = tls::client_config(self.trust, false).map_err(|e| e.to_string())?;
        let connector = tokio_tungstenite::Connector::Rustls(config);
        let (ws, _) = timeout(
            HANDSHAKE_TIMEOUT,
            tokio_tungstenite::connect_async_tls_with_config(
                self.url.as_str(),
                None,
                true,
                Some(connector),
            ),
        )
        .await
        .map_err(|_| "connection timed out".to_owned())?
        .map_err(|e| e.to_string())?;
        Ok(ws)
    }

    async fn send(ws: &mut Ws, msg: &ClientMessage) -> bool {
        let text = serde_json::to_string(msg).expect("client messages serialize");
        ws.send(WsMessage::text(text)).await.is_ok()
    }

    fn identify(&self) -> ClientMessage {
        match &self.session_id {
            Some(session_id) => ClientMessage::Resume(Resume {
                token: self.token.clone(),
                session_id: session_id.clone(),
                last_seq: self.last_seq,
            }),
            None => ClientMessage::Identify(Identify {
                token: self.token.clone(),
                protocol_version: PROTOCOL_VERSION,
                client: self.client.clone(),
            }),
        }
    }

    /// One connection, from connecting to its end. Sets `connected` once `ready`/`resumed`
    /// arrived, so the next attempt starts with a short backoff.
    async fn connection(&mut self, connected: &mut bool) -> End {
        let mut ws = match self.connect().await {
            Ok(ws) => ws,
            Err(err) => {
                tracing::info!("gateway {}: {err}", self.url);
                return End::Retry;
            }
        };
        // hello
        let interval = match timeout(HANDSHAKE_TIMEOUT, ws.next()).await {
            Ok(Some(Ok(WsMessage::Text(text)))) => {
                match serde_json::from_str::<ServerMessage>(&text) {
                    Ok(ServerMessage {
                        event:
                            ServerEvent::Hello {
                                heartbeat_interval_ms,
                            },
                        ..
                    }) => Duration::from_millis(heartbeat_interval_ms.max(1000)),
                    _ => return End::Retry,
                }
            }
            Ok(Some(Ok(WsMessage::Close(frame)))) => return self.closed(frame),
            _ => return End::Retry,
        };
        if !Self::send(&mut ws, &self.identify()).await {
            return End::Retry;
        }

        let mut next_beat = Instant::now() + jittered(interval);
        let mut last_ack = Instant::now();
        let handshake_deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        loop {
            let deadline = if *connected {
                next_beat
            } else {
                next_beat.min(handshake_deadline)
            };
            tokio::select! {
                frame = ws.next() => {
                    let text = match frame {
                        Some(Ok(WsMessage::Text(text))) => text,
                        Some(Ok(WsMessage::Close(frame))) => return self.closed(frame),
                        Some(Ok(_)) => continue,
                        Some(Err(_)) | None => return End::Retry,
                    };
                    let msg = match serde_json::from_str::<ServerMessage>(&text) {
                        Ok(msg) => msg,
                        Err(err) => {
                            // Newer servers may send events this client does not know.
                            tracing::debug!("ignoring gateway frame: {err}");
                            continue;
                        }
                    };
                    if let Some(s) = msg.s {
                        self.last_seq = self.last_seq.max(s);
                    }
                    match msg.event {
                        ServerEvent::Hello { .. } => {}
                        ServerEvent::HeartbeatAck {} => last_ack = Instant::now(),
                        ServerEvent::Ready(ready) => {
                            self.session_id = Some(ready.session_id.clone());
                            *connected = true;
                            if !self.emit(GatewayEvent::Ready(ready))
                                || !self.emit(GatewayEvent::Status(ConnStatus::Connected))
                            {
                                return End::Shutdown;
                            }
                            if let Some(p) = self.presence
                                && !Self::send(&mut ws, &ClientMessage::PresenceUpdate { status: p }).await
                            {
                                return End::Retry;
                            }
                        }
                        ServerEvent::Resumed {} => {
                            *connected = true;
                            if !self.emit(GatewayEvent::Resumed)
                                || !self.emit(GatewayEvent::Status(ConnStatus::Connected))
                            {
                                return End::Shutdown;
                            }
                        }
                        ServerEvent::InvalidSession { resumable } => {
                            if !resumable {
                                self.session_id = None;
                                self.last_seq = 0;
                            }
                            if !Self::send(&mut ws, &self.identify()).await {
                                return End::Retry;
                            }
                        }
                        event => {
                            if !self.emit(GatewayEvent::Dispatch(Box::new(event))) {
                                return End::Shutdown;
                            }
                        }
                    }
                }
                () = sleep_until(deadline) => {
                    if !*connected && Instant::now() >= handshake_deadline {
                        return End::Retry;
                    }
                    // Two intervals without an ack: the connection is dead.
                    if last_ack.elapsed() > interval * 2 + Duration::from_secs(5) {
                        tracing::info!("gateway {}: no heartbeat ack, reconnecting", self.url);
                        return End::Retry;
                    }
                    if !Self::send(&mut ws, &ClientMessage::Heartbeat { last_seq: self.last_seq }).await {
                        return End::Retry;
                    }
                    next_beat = Instant::now() + jittered(interval);
                }
                cmd = self.commands.recv() => match cmd {
                    None | Some(Command::Shutdown) => {
                        Self::close(&mut ws).await;
                        return End::Shutdown;
                    }
                    Some(Command::Offline) => {
                        Self::close(&mut ws).await;
                        return End::Offline;
                    }
                    Some(Command::Online) => {}
                    Some(Command::Presence(status)) => {
                        self.presence = Some(status);
                        if *connected
                            && !Self::send(&mut ws, &ClientMessage::PresenceUpdate { status }).await
                        {
                            return End::Retry;
                        }
                    }
                },
            }
        }
    }

    async fn close(ws: &mut Ws) {
        let frame = CloseFrame {
            code: WsCloseCode::Normal,
            reason: "".into(),
        };
        let _ = timeout(Duration::from_secs(2), ws.close(Some(frame))).await;
    }

    /// What to do after the server closed the connection.
    fn closed(&mut self, frame: Option<CloseFrame>) -> End {
        let code = frame.as_ref().map(|f| u16::from(f.code));
        let Some(code) = code.and_then(CloseCode::from_code) else {
            return End::Retry;
        };
        tracing::info!("gateway {} closed with {code:?}", self.url);
        match code.action() {
            CloseAction::Reconnect | CloseAction::Resume => End::Retry,
            CloseAction::ReconnectAfterMs(ms) => End::RetryAfter(Duration::from_millis(ms)),
            CloseAction::IdentifyFresh => {
                self.session_id = None;
                self.last_seq = 0;
                End::Retry
            }
            CloseAction::ShowLogin => End::Stop(ConnStatus::LoggedOut),
            CloseAction::UpdateClient => End::Stop(ConnStatus::Failed {
                reason: "This server needs a newer version of Hermes.".into(),
            }),
        }
    }
}

/// `ClientInfo` for this process.
pub fn client_info(name: &str) -> ClientInfo {
    ClientInfo {
        name: name.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        os: std::env::consts::OS.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        for (attempt, secs) in [
            (1, 1.0),
            (2, 2.0),
            (3, 4.0),
            (4, 8.0),
            (5, 16.0),
            (6, 30.0),
            (40, 30.0),
        ] {
            let d = backoff(attempt).as_secs_f64();
            assert!(d >= secs * 0.8 && d <= secs * 1.2, "attempt {attempt}: {d}");
        }
    }
}
