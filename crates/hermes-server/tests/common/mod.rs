//! Test harness: a real server on a random port with a temp data dir, TLS off, plus a small JSON
//! client on top of `reqwest` and a gateway client on top of `tokio-tungstenite`.

#![allow(dead_code)] // Each test binary uses a different subset.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use hermes_server::config::{Config, RegistrationMode, TlsMode};
use hermes_server::state::AppState;
use hermes_server::tls::Tls;
use hermes_server::{api, setup};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

/// A fresh directory under the system temp dir, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("hermes-server-it-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct TestServer {
    pub base: String,
    pub state: Arc<AppState>,
    /// Code of the first-start setup invite.
    pub setup_code: String,
    client: reqwest::Client,
    _dir: TempDir,
}

/// A response: status plus JSON body (`Value::Null` when empty).
#[derive(Debug)]
pub struct Res {
    pub status: StatusCode,
    pub body: Value,
}

impl Res {
    /// The `error.code` of an error body.
    pub fn code(&self) -> &str {
        self.body["error"]["code"].as_str().unwrap_or("")
    }

    #[track_caller]
    pub fn ok(self) -> Value {
        assert!(self.status.is_success(), "{} {}", self.status, self.body);
        self.body
    }

    #[track_caller]
    pub fn err(&self, status: u16, code: &str) {
        assert_eq!(
            (self.status.as_u16(), self.code()),
            (status, code),
            "{}",
            self.body
        );
    }
}

/// Starts a server; `name` must be unique per test (it names the temp dir).
pub async fn start(name: &str, mode: RegistrationMode) -> TestServer {
    start_with(name, mode, |_| {}).await
}

/// [`start`], with a chance to change the state (e.g. gateway timings) before serving.
pub async fn start_with(
    name: &str,
    mode: RegistrationMode,
    configure: impl FnOnce(&mut AppState),
) -> TestServer {
    // reqwest is built without a default crypto provider; HTTPS is not used, but building a
    // client still needs one.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = TempDir::new(name);
    let mut config = Config::default();
    config.instance.name = "Test Instance".into();
    config.storage.data_dir = dir.path().join("data");
    config.tls.mode = TlsMode::Off;
    config.registration.mode = mode;
    let mut state = AppState::load(config).await.unwrap();
    configure(&mut state);
    let state = Arc::new(state);
    let links = setup::first_start(&state, None).await.unwrap().unwrap();
    let setup_code = links.web.rsplit('/').next().unwrap().to_owned();

    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api/v1", tcp.local_addr().unwrap());
    let router = api::router(state.clone());
    tokio::spawn(async move { api::serve(tcp, &Tls::Off, router).await });
    TestServer {
        base,
        state,
        setup_code,
        client: reqwest::Client::new(),
        _dir: dir,
    }
}

impl TestServer {
    pub async fn request(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Res {
        let mut req = self.client.request(method, format!("{}{path}", self.base));
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req.send().await.unwrap();
        let status = res.status();
        let text = res.text().await.unwrap();
        let body = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or_else(|_| panic!("not JSON: {text}"))
        };
        Res { status, body }
    }

    pub async fn get(&self, path: &str, token: Option<&str>) -> Res {
        self.request(Method::GET, path, token, None).await
    }

    pub async fn post(&self, path: &str, token: Option<&str>, body: Value) -> Res {
        self.request(Method::POST, path, token, Some(body)).await
    }

    pub async fn delete(&self, path: &str, token: Option<&str>) -> Res {
        self.request(Method::DELETE, path, token, None).await
    }

    pub async fn patch(&self, path: &str, token: Option<&str>, body: Value) -> Res {
        self.request(Method::PATCH, path, token, Some(body)).await
    }

    pub async fn put(&self, path: &str, token: Option<&str>, body: Value) -> Res {
        self.request(Method::PUT, path, token, Some(body)).await
    }

    /// Registers `username` and joins `space_id` through a new invite created by `inviter`;
    /// returns `(token, user_id)`.
    pub async fn member(&self, inviter: &str, space_id: &str, username: &str) -> (String, String) {
        let invite = self
            .post(
                &format!("/spaces/{space_id}/invites"),
                Some(inviter),
                json!({}),
            )
            .await
            .ok();
        let code = invite["code"].as_str().unwrap();
        let (token, user_id) = self.register(username, Some(code)).await;
        (token, user_id)
    }

    /// The ID of the channel called `name` in `space_id`.
    pub fn channel_id(&self, space_id: &str, name: &str) -> String {
        let space_id = space_id.parse().unwrap();
        self.state
            .cache()
            .space_channels(space_id)
            .find(|c| c.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no channel {name}"))
            .id
            .to_string()
    }

    /// Creates a role with `permissions` (as `Permissions` bits); returns its ID.
    pub async fn role(&self, token: &str, space_id: &str, name: &str, permissions: u64) -> String {
        let role = self
            .post(
                &format!("/spaces/{space_id}/roles"),
                Some(token),
                json!({ "name": name, "permissions": permissions.to_string() }),
            )
            .await
            .ok();
        role["id"].as_str().unwrap().to_owned()
    }

    /// Sets a member's roles.
    pub async fn assign(&self, token: &str, space_id: &str, user_id: &str, role_ids: &[&str]) {
        self.patch(
            &format!("/spaces/{space_id}/members/{user_id}"),
            Some(token),
            json!({ "role_ids": role_ids }),
        )
        .await
        .ok();
    }

    /// The space's audit log actions, newest first.
    pub async fn audit_actions(&self, token: &str, space_id: &str) -> Vec<String> {
        let log = self
            .get(&format!("/spaces/{space_id}/audit-log"), Some(token))
            .await
            .ok();
        log.as_array()
            .unwrap()
            .iter()
            .map(|e| e["action"].as_str().unwrap().to_owned())
            .collect()
    }

    pub async fn register_raw(&self, username: &str, invite: Option<&str>) -> Res {
        let mut body = json!({
            "username": username,
            "password": "correct horse",
            "device_name": "Test PC",
        });
        if let Some(code) = invite {
            body["invite_code"] = json!(code);
        }
        self.post("/auth/register", None, body).await
    }

    /// Registers `username` (password `correct horse`) and returns `(token, user_id)`.
    pub async fn register(&self, username: &str, invite: Option<&str>) -> (String, String) {
        let body = self.register_raw(username, invite).await.ok();
        (
            body["token"].as_str().unwrap().to_owned(),
            body["user"]["id"].as_str().unwrap().to_owned(),
        )
    }

    /// Registers the owner with the setup invite; returns `(token, user_id, space_id)`.
    pub async fn owner(&self) -> (String, String, String) {
        let (token, user_id) = self.register("owner", Some(&self.setup_code.clone())).await;
        let space_id = self
            .state
            .cache()
            .spaces
            .keys()
            .next()
            .expect("default space")
            .to_string();
        (token, user_id, space_id)
    }

    pub async fn login(&self, username: &str, password: &str) -> Res {
        self.post(
            "/auth/login",
            None,
            json!({ "username": username, "password": password, "device_name": "Laptop" }),
        )
        .await
    }

    /// Opens a gateway connection and reads `hello`.
    pub async fn connect(&self) -> Ws {
        let url = format!("{}/gateway", self.base.replacen("http://", "ws://", 1));
        let (stream, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let mut ws = Ws { stream };
        let hello = ws.recv().await;
        assert_eq!(hello["op"], "hello", "{hello}");
        ws
    }

    /// Connects and identifies; returns the connection and the `ready` payload.
    pub async fn identify(&self, token: &str) -> (Ws, Value) {
        let mut ws = self.connect().await;
        ws.send_identify(token).await;
        let ready = ws.recv_op("ready").await["d"].clone();
        (ws, ready)
    }

    /// Runs a raw SQL statement on the write pool (for fixtures the API cannot create yet).
    pub async fn sql(&self, sql: &str) {
        sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(&self.state.db.write)
            .await
            .unwrap();
    }
}

/// How long a test waits for a gateway frame.
const WS_TIMEOUT: Duration = Duration::from_secs(5);

/// A gateway client.
pub struct Ws {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl Ws {
    pub async fn send(&mut self, frame: Value) {
        self.send_raw(&frame.to_string()).await;
    }

    pub async fn send_raw(&mut self, text: &str) {
        self.stream
            .send(WsMessage::Text(text.into()))
            .await
            .unwrap();
    }

    pub async fn send_identify(&mut self, token: &str) {
        self.send(json!({ "op": "identify", "d": {
            "token": token,
            "protocol_version": 1,
            "client": { "name": "test", "version": "0", "os": "test" },
        }}))
        .await;
    }

    pub async fn send_resume(&mut self, token: &str, session_id: &str, last_seq: u64) {
        self.send(json!({ "op": "resume", "d": {
            "token": token, "session_id": session_id, "last_seq": last_seq,
        }}))
        .await;
    }

    /// The next frame; panics on close or timeout.
    pub async fn recv(&mut self) -> Value {
        match self.next_message(WS_TIMEOUT).await {
            Some(WsMessage::Text(text)) => serde_json::from_str(&text).unwrap(),
            other => panic!("expected a text frame, got {other:?}"),
        }
    }

    /// The next frame that is not a `presence_update` (presences change whenever anyone
    /// connects, so most tests skip them).
    pub async fn next_event(&mut self) -> Value {
        loop {
            let frame = self.recv().await;
            if frame["op"] != "presence_update" {
                return frame;
            }
        }
    }

    /// Skips frames until one with `op` arrives and returns it.
    pub async fn recv_op(&mut self, op: &str) -> Value {
        loop {
            let frame = self.recv().await;
            if frame["op"] == op {
                return frame;
            }
        }
    }

    /// Asserts that no frame other than `presence_update` arrives within `ms`.
    pub async fn assert_silent(&mut self, ms: u64) {
        let until = tokio::time::Instant::now() + Duration::from_millis(ms);
        loop {
            let left = until.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return;
            }
            match self.next_message(left).await {
                None => return,
                Some(WsMessage::Text(text)) => {
                    let frame: Value = serde_json::from_str(&text).unwrap();
                    assert_eq!(frame["op"], "presence_update", "unexpected frame {frame}");
                }
                Some(other) => panic!("unexpected message {other:?}"),
            }
        }
    }

    /// Reads until the server closes the connection; returns the close code.
    pub async fn closed(&mut self) -> u16 {
        loop {
            match self.next_message(WS_TIMEOUT).await {
                Some(WsMessage::Close(frame)) => {
                    return frame.map_or(1005, |f| u16::from(f.code));
                }
                Some(_) => {}
                None => panic!("timed out waiting for close"),
            }
        }
    }

    /// The next message, or `None` on timeout. Pings are answered by tungstenite.
    async fn next_message(&mut self, timeout: Duration) -> Option<WsMessage> {
        loop {
            let msg = tokio::time::timeout(timeout, self.stream.next())
                .await
                .ok()?
                .expect("connection ended without a close frame")
                .unwrap();
            if !matches!(msg, WsMessage::Ping(_) | WsMessage::Pong(_)) {
                return Some(msg);
            }
        }
    }
}
