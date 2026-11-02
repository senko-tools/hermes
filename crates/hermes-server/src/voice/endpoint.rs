//! The voice QUIC endpoint (PLAN.md 12.2): its own self-signed certificate, one task per
//! connection, the control stream, incoming datagrams and screen share streams.

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hermes_proto::ErrorCode;
use hermes_proto::voice::{
    self, ALPN, ClientControl, IDLE_TIMEOUT_MS, KEEP_ALIVE_MS, ServerControl,
};
use quinn::crypto::rustls::QuicServerConfig;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use super::{ConnId, JoinError, Link, ShareError};
use crate::config::Config;
use crate::state::AppState;
use crate::tls::{self, CERTS_DIR, TlsError};

/// Base name of the voice certificate: `voice-cert.pem` + `voice-key.pem`.
pub const VOICE_CERT_NAME: &str = "voice";

/// A new connection must send `auth` within this time.
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
/// A screen share frame must arrive completely within this time.
const FRAME_TIMEOUT: Duration = Duration::from_secs(5);
/// Screen share frames waiting to be written to one client.
const VIDEO_QUEUE: usize = 8;
/// Unidirectional streams a peer may have open at once (one per share frame).
pub const MAX_UNI_STREAMS: u32 = 8;

#[derive(Debug)]
pub enum EndpointError {
    Tls(TlsError),
    Rustls(rustls::Error),
    Quic(String),
    Bind(SocketAddr, std::io::Error),
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EndpointError::Tls(err) => write!(f, "voice certificate: {err}"),
            EndpointError::Rustls(err) => write!(f, "voice TLS setup: {err}"),
            EndpointError::Quic(err) => write!(f, "voice QUIC setup: {err}"),
            EndpointError::Bind(addr, err) => write!(f, "could not listen on udp {addr}: {err}"),
        }
    }
}

impl std::error::Error for EndpointError {}

/// QUIC transport settings shared by the server and clients.
pub fn transport_config() -> quinn::TransportConfig {
    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(
        quinn::IdleTimeout::try_from(Duration::from_millis(IDLE_TIMEOUT_MS))
            .expect("15 s fits a VarInt"),
    ));
    transport.keep_alive_interval(Some(Duration::from_millis(KEEP_ALIVE_MS)));
    transport.max_concurrent_uni_streams(MAX_UNI_STREAMS.into());
    transport
}

/// Loads (or generates) the voice certificate in `storage.data_dir/certs` and binds the UDP
/// socket. Returns the endpoint and the certificate fingerprint as `"3F:A9:…"`.
pub fn bind(config: &Config, addr: SocketAddr) -> Result<(quinn::Endpoint, String), EndpointError> {
    let dir = config.storage.data_dir.join(CERTS_DIR);
    let pair = tls::load_or_generate(&dir, VOICE_CERT_NAME, &tls::self_signed_sans(config))
        .map_err(EndpointError::Tls)?;
    let fingerprint = voice::colon_hex(&Sha256::digest(pair.chain[0].as_ref()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut crypto = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(EndpointError::Rustls)?
        .with_no_client_auth()
        .with_single_cert(pair.chain, pair.key)
        .map_err(EndpointError::Rustls)?;
    crypto.alpn_protocols = vec![ALPN.to_vec()];
    let crypto =
        QuicServerConfig::try_from(crypto).map_err(|e| EndpointError::Quic(e.to_string()))?;
    let mut server = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    server.transport_config(Arc::new(transport_config()));
    let endpoint =
        quinn::Endpoint::server(server, addr).map_err(|err| EndpointError::Bind(addr, err))?;
    Ok((endpoint, fingerprint))
}

/// Accepts connections until the endpoint is closed.
pub async fn serve(state: Arc<AppState>, endpoint: quinn::Endpoint) {
    while let Some(incoming) = endpoint.accept().await {
        let state = state.clone();
        tokio::spawn(async move {
            let remote = incoming.remote_address();
            match incoming.await {
                Ok(conn) => connection(state, conn).await,
                Err(err) => tracing::debug!("voice handshake from {remote} failed: {err}"),
            }
        });
    }
}

async fn connection(state: Arc<AppState>, conn: quinn::Connection) {
    let (control, mut outbox) = mpsc::unbounded_channel();
    let (video, video_out) = mpsc::channel(VIDEO_QUEUE);
    let conn_id = state.voice.register(Link {
        control,
        conn: conn.clone(),
        video,
    });
    let writer = tokio::spawn(write_video(conn.clone(), video_out));
    let reader = tokio::spawn(read_video(state.clone(), conn_id, conn.clone()));
    let reason = run(&state, conn_id, &conn, &mut outbox).await;
    writer.abort();
    reader.abort();
    state.voice.unregister(&state, conn_id);
    tracing::debug!(
        "voice connection from {} ended: {reason}",
        conn.remote_address().ip().to_canonical()
    );
    conn.close(0u32.into(), b"bye");
}

/// Serves one connection; returns why it ended.
async fn run(
    state: &AppState,
    conn_id: ConnId,
    conn: &quinn::Connection,
    outbox: &mut mpsc::UnboundedReceiver<ServerControl>,
) -> String {
    let (mut send, recv) = match tokio::time::timeout(AUTH_TIMEOUT, conn.accept_bi()).await {
        Ok(Ok(streams)) => streams,
        Ok(Err(err)) => return err.to_string(),
        Err(_) => return "no control stream".into(),
    };
    // Reading frames is not cancel-safe, so it gets its own task.
    let (inbox_tx, mut inbox) = mpsc::channel(16);
    let reader = tokio::spawn(read_frames(recv, inbox_tx));
    let auth_deadline = tokio::time::sleep(AUTH_TIMEOUT);
    tokio::pin!(auth_deadline);
    let mut authed = false;

    let reason = loop {
        tokio::select! {
            msg = inbox.recv() => {
                let msg = match msg {
                    Some(Ok(msg)) => msg,
                    Some(Err(reason)) => break reason,
                    None => break "control stream closed".into(),
                };
                let reply = handle(state, conn_id, &mut authed, msg);
                let fatal = matches!(reply, Some((_, true)));
                if let Some((reply, _)) = reply
                    && write(&mut send, &reply).await.is_err()
                {
                    break "control stream write failed".into();
                }
                if fatal {
                    // Let the error frame reach the client before the connection closes.
                    let _ = send.finish();
                    let _ = tokio::time::timeout(Duration::from_secs(1), send.stopped()).await;
                    break "authentication failed".into();
                }
            }
            msg = outbox.recv() => {
                let Some(msg) = msg else { break "closed".into() };
                if write(&mut send, &msg).await.is_err() {
                    break "control stream write failed".into();
                }
            }
            datagram = conn.read_datagram() => match datagram {
                Ok(data) => state.voice.forward(state, conn_id, data),
                Err(err) => break err.to_string(),
            },
            () = &mut auth_deadline, if !authed => break "no auth in time".into(),
        }
    };
    reader.abort();
    reason
}

/// Writes screen share frames to the client, one unidirectional stream per frame, in order.
async fn write_video(conn: quinn::Connection, mut frames: mpsc::Receiver<bytes::Bytes>) {
    while let Some(frame) = frames.recv().await {
        let Ok(mut stream) = conn.open_uni().await else {
            return;
        };
        if stream.write_all(&frame).await.is_err() {
            continue;
        }
        let _ = stream.finish();
    }
}

/// Reads the client's screen share frames, one stream at a time in the order they were opened
/// (so frames stay in order), each at most [`voice::MAX_VIDEO_FRAME`] bytes.
async fn read_video(state: Arc<AppState>, conn_id: ConnId, conn: quinn::Connection) {
    loop {
        let Ok(mut stream) = conn.accept_uni().await else {
            return;
        };
        let read =
            tokio::time::timeout(FRAME_TIMEOUT, stream.read_to_end(voice::MAX_VIDEO_FRAME)).await;
        match read {
            Ok(Ok(frame)) => state.voice.forward_video(&state, conn_id, frame),
            // Too large, cut off or too slow: drop it.
            _ => {
                let _ = stream.stop(0u32.into());
            }
        }
    }
}

async fn write(send: &mut quinn::SendStream, msg: &ServerControl) -> Result<(), quinn::WriteError> {
    send.write_all(&voice::encode_frame(msg)).await
}

/// Reads length-prefixed frames into `inbox` until the stream ends or a frame is invalid.
async fn read_frames(
    mut recv: quinn::RecvStream,
    inbox: mpsc::Sender<Result<ClientControl, String>>,
) {
    loop {
        let mut prefix = [0u8; 4];
        if recv.read_exact(&mut prefix).await.is_err() {
            return;
        }
        let Some(len) = voice::frame_len(prefix) else {
            let _ = inbox.send(Err("control frame too large".into())).await;
            return;
        };
        let mut body = vec![0u8; len];
        if recv.read_exact(&mut body).await.is_err() {
            return;
        }
        let msg = serde_json::from_slice(&body).map_err(|e| format!("bad control frame: {e}"));
        let stop = msg.is_err();
        if inbox.send(msg).await.is_err() || stop {
            return;
        }
    }
}

fn error(code: ErrorCode, message: impl Into<String>) -> ServerControl {
    ServerControl::Error {
        code: code.as_str().into(),
        message: message.into(),
    }
}

/// Handles one client message. Returns a reply, and whether the connection must close after it.
fn handle(
    state: &AppState,
    conn_id: ConnId,
    authed: &mut bool,
    msg: ClientControl,
) -> Option<(ServerControl, bool)> {
    let voice = &state.voice;
    match msg {
        ClientControl::Auth {
            token,
            self_mute,
            self_deaf,
        } => {
            if *authed {
                return Some((error(ErrorCode::BadRequest, "Already authenticated"), false));
            }
            let Some(mut ticket) = voice.redeem(&token, None) else {
                return Some((
                    error(ErrorCode::Unauthorized, "Invalid or expired voice token"),
                    true,
                ));
            };
            ticket.self_mute = self_mute;
            ticket.self_deaf = self_deaf;
            match voice.join(state, conn_id, ticket) {
                Ok(()) => {
                    *authed = true;
                    None
                }
                Err(err) => Some((join_error(err), true)),
            }
        }
        _ if !*authed => Some((error(ErrorCode::Unauthorized, "Send auth first"), true)),
        ClientControl::Switch { token } => {
            let user_id = voice.conn_user(conn_id)?;
            let Some(ticket) = voice.redeem(&token, Some(user_id)) else {
                return Some((
                    error(ErrorCode::Unauthorized, "Invalid or expired voice token"),
                    false,
                ));
            };
            voice
                .join(state, conn_id, ticket)
                .err()
                .map(|err| (join_error(err), false))
        }
        ClientControl::SetState {
            self_mute,
            self_deaf,
        } => {
            let user_id = voice.conn_user(conn_id)?;
            voice.set_self_state(state, user_id, Some(self_mute), Some(self_deaf));
            None
        }
        ClientControl::Ping { t } => Some((ServerControl::Pong { t }, false)),
        ClientControl::Leave {} => {
            voice.leave_conn(state, conn_id);
            None
        }
        ClientControl::ShareStart {} => match voice.share_start(state, conn_id) {
            Ok(()) => None,
            Err(ShareError::NotInRoom) => Some((
                error(ErrorCode::BadRequest, "Join a voice room before sharing"),
                false,
            )),
            Err(ShareError::NoPermission) => Some((
                error(
                    ErrorCode::MissingPermission,
                    "You cannot share your screen in this channel",
                ),
                false,
            )),
        },
        ClientControl::ShareStop {} => {
            voice.share_stop(conn_id);
            None
        }
        ClientControl::Watch {
            participant_id,
            watching,
        } => {
            voice.watch(conn_id, participant_id, watching);
            None
        }
        ClientControl::RequestKeyframe { participant_id } => {
            voice.keyframe_wanted(conn_id, participant_id);
            None
        }
    }
}

fn join_error(err: JoinError) -> ServerControl {
    match err {
        JoinError::Full => error(ErrorCode::VoiceFull, "This voice channel is full"),
        JoinError::NoAccess => error(
            ErrorCode::MissingPermission,
            "You can no longer connect to this channel",
        ),
        JoinError::OtherUser => error(
            ErrorCode::BadRequest,
            "This connection belongs to another user",
        ),
    }
}
