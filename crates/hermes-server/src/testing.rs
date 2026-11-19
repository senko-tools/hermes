//! A real server for the clients' tests (feature `testing`): self-signed TLS on 127.0.0.1, its
//! own temporary data directory, and limits high enough never to get in a test's way.
//!
//! The server's own tests use `tests/common`, which runs without TLS and adds a JSON client.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Notify;

use crate::config::{Config, RegistrationMode, TlsMode};
use crate::state::AppState;

/// A running server; dropping it stops the server and removes its directory.
pub struct TestServer {
    pub port: u16,
    /// The first-start invite link (`https://127.0.0.1:<port>/invite/<code>#<fingerprint>`).
    pub link: String,
    pub state: Arc<AppState>,
    /// Its directory; the data is in `data` inside, and tests may put files here.
    pub dir: PathBuf,
    shutdown: Arc<Notify>,
}

impl TestServer {
    /// `127.0.0.1:<port>`.
    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// The code of [`TestServer::link`].
    pub fn setup_code(&self) -> &str {
        let path = self.link.split('#').next().unwrap_or_default();
        path.rsplit('/').next().unwrap_or_default()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shutdown.notify_one();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Starts a server named `name` (unique per test: it names the directory), with a voice
/// endpoint when `voice`. `configure` changes the config before it starts.
pub async fn spawn(name: &str, voice: bool, configure: impl FnOnce(&mut Config)) -> TestServer {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = std::env::temp_dir().join(format!("hermes-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a test port");
    let port = tcp.local_addr().expect("a bound port").port();

    let mut config = Config::default();
    config.instance.name = "Test".into();
    config.instance.public_host = "127.0.0.1".into();
    config.network.port = port;
    config.storage.data_dir = dir.join("data");
    config.tls.mode = TlsMode::SelfSigned;
    config.registration.mode = RegistrationMode::Open;
    config.limits.requests_per_second = 100_000;
    config.limits.login_attempts_per_minute = 100_000;
    config.limits.messages_per_5s = 100_000;
    configure(&mut config);

    let tls = crate::tls::load(&config).expect("test TLS");
    let fingerprint = *tls.fingerprint().expect("self-signed has a fingerprint");
    let quic = voice.then(|| {
        crate::voice::endpoint::bind(&config, "127.0.0.1:0".parse().expect("an address"))
            .expect("bind the voice endpoint")
    });
    let mut state = AppState::load(config).await.expect("load the test server");
    state.fingerprint = Some(fingerprint);
    if let Some((endpoint, cert_sha256)) = &quic {
        state.voice.set_endpoint(crate::voice::EndpointInfo {
            cert_sha256: cert_sha256.clone(),
            port: endpoint.local_addr().expect("a voice port").port(),
        });
    }
    let links = crate::setup::first_start(&state, Some(&fingerprint))
        .await
        .expect("first start")
        .expect("a new server has no users");

    let state = Arc::new(state);
    let shutdown = Arc::new(Notify::new());
    let signal = {
        let shutdown = shutdown.clone();
        async move { shutdown.notified().await }
    };
    tokio::spawn(crate::server::run(
        state.clone(),
        tcp,
        tls,
        quic.map(|(endpoint, _)| endpoint),
        signal,
    ));
    TestServer {
        port,
        link: links.web,
        state,
        shutdown,
        dir,
    }
}
