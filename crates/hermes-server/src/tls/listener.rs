//! An [`axum::serve::Listener`] that terminates TLS.
//!
//! Handshakes run in their own tasks with a timeout, so a slow or silent client never holds up
//! `accept` for everyone else. Failed handshakes are logged at debug level and dropped.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::ServerConfig;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Finished handshakes waiting for `accept`.
const QUEUE_LEN: usize = 64;

type Accepted = (TlsStream<TcpStream>, SocketAddr);

pub struct TlsListener {
    local_addr: SocketAddr,
    accepted: mpsc::Receiver<Accepted>,
    accept_task: JoinHandle<()>,
}

impl TlsListener {
    /// Must be called inside a tokio runtime.
    pub fn new(tcp: TcpListener, config: Arc<ServerConfig>) -> io::Result<Self> {
        let local_addr = tcp.local_addr()?;
        let (tx, accepted) = mpsc::channel(QUEUE_LEN);
        let accept_task = tokio::spawn(accept_loop(tcp, TlsAcceptor::from(config), tx));
        Ok(Self {
            local_addr,
            accepted,
            accept_task,
        })
    }
}

impl Drop for TlsListener {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

async fn accept_loop(mut tcp: TcpListener, acceptor: TlsAcceptor, tx: mpsc::Sender<Accepted>) {
    loop {
        // axum's impl retries accept errors (e.g. too many open files) with a short sleep.
        let (stream, addr) = axum::serve::Listener::accept(&mut tcp).await;
        let (acceptor, tx) = (acceptor.clone(), tx.clone());
        tokio::spawn(async move {
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                Ok(Ok(tls)) => {
                    let _ = tx.send((tls, addr)).await;
                }
                Ok(Err(err)) => tracing::debug!(%addr, "TLS handshake failed: {err}"),
                Err(_) => tracing::debug!(%addr, "TLS handshake timed out"),
            }
        });
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.accepted.recv().await {
            Some(accepted) => accepted,
            // The accept loop never ends on its own; only reachable if it panicked.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(self.local_addr)
    }
}

#[cfg(test)]
mod tests {
    use axum::serve::Listener;
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, ServerName};
    use rustls::{ClientConfig, RootCertStore};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::TlsConnector;

    use super::*;
    use crate::config::{Config, TlsMode};
    use crate::db::tests::TempDir;
    use crate::tls::{self, CERTS_DIR, Tls};

    /// Starts a listener for `tls` that echoes one 5-byte message per connection.
    async fn echo_server(tls: &Tls) -> SocketAddr {
        let Tls::Rustls { config, .. } = tls else {
            panic!("expected TLS");
        };
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut listener = TlsListener::new(tcp, config.clone()).unwrap();
        let addr = Listener::local_addr(&listener).unwrap();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = Listener::accept(&mut listener).await;
                tokio::spawn(async move {
                    let mut buf = [0; 5];
                    stream.read_exact(&mut buf).await.unwrap();
                    stream.write_all(&buf).await.unwrap();
                    stream.shutdown().await.unwrap();
                });
            }
        });
        addr
    }

    /// A client that trusts exactly `cert` (a self-signed certificate acts as its own root),
    /// with normal webpki checks of the name and validity period.
    fn connector(cert: CertificateDer<'static>) -> TlsConnector {
        let mut roots = RootCertStore::empty();
        roots.add(cert).unwrap();
        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth();
        TlsConnector::from(Arc::new(config))
    }

    async fn echo(connector: &TlsConnector, addr: SocketAddr, name: &str) -> io::Result<()> {
        let tcp = TcpStream::connect(addr).await?;
        let name = ServerName::try_from(name.to_owned()).unwrap();
        let mut stream = connector.connect(name, tcp).await?;
        stream.write_all(b"hello").await?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await?;
        assert_eq!(buf, b"hello");
        Ok(())
    }

    fn self_signed(dir: &TempDir) -> (Tls, CertificateDer<'static>) {
        let mut config = Config::default();
        config.instance.public_host = "hermes.example.org".into();
        config.network.bind = "127.0.0.1".parse().unwrap();
        config.storage.data_dir = dir.path().to_owned();
        let tls = tls::load(&config).unwrap();
        let cert = CertificateDer::from_pem_file(dir.path().join(CERTS_DIR).join("https-cert.pem"))
            .unwrap();
        (tls, cert)
    }

    #[tokio::test]
    async fn self_signed_handshake_checks_names_and_validity() {
        let dir = TempDir::new("tls-listener-self-signed");
        let (tls, cert) = self_signed(&dir);
        let addr = echo_server(&tls).await;
        let client = connector(cert);
        echo(&client, addr, "hermes.example.org").await.unwrap();
        echo(&client, addr, "127.0.0.1").await.unwrap();
        let err = echo(&client, addr, "other.example.org").await.unwrap_err();
        assert!(err.to_string().contains("certificate"), "{err}");
    }

    #[tokio::test]
    async fn files_mode_handshake() {
        let dir = TempDir::new("tls-listener-files");
        let pair = tls::load_or_generate(dir.path(), "own", &["localhost".to_owned()]).unwrap();
        let mut config = Config::default();
        config.tls.mode = TlsMode::Files;
        config.tls.cert_file = pair.cert_path;
        config.tls.key_file = pair.key_path;
        let tls = tls::load(&config).unwrap();
        let addr = echo_server(&tls).await;
        echo(&connector(pair.chain[0].clone()), addr, "localhost")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn bad_and_silent_clients_do_not_block_others() {
        let dir = TempDir::new("tls-listener-robust");
        let (tls, cert) = self_signed(&dir);
        let addr = echo_server(&tls).await;

        // Plain HTTP to the TLS port: the handshake fails and is dropped.
        let mut plain = TcpStream::connect(addr).await.unwrap();
        plain
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        // Connected but never sends a ClientHello.
        let _silent = TcpStream::connect(addr).await.unwrap();

        let client = connector(cert);
        tokio::time::timeout(Duration::from_secs(5), async {
            echo(&client, addr, "hermes.example.org").await.unwrap();
            echo(&client, addr, "hermes.example.org").await.unwrap();
        })
        .await
        .expect("good clients are served while others stall");
    }

    /// `files` mode on `<dir>/live-{cert,key}.pem`, starting with pair `a`; returns the TLS
    /// setup and the certificates of pairs `a` and `b` (`b` is in `<dir>/b-*.pem`).
    fn files_with_two_pairs(
        dir: &TempDir,
    ) -> (Tls, CertificateDer<'static>, CertificateDer<'static>) {
        let a = tls::load_or_generate(dir.path(), "a", &["localhost".to_owned()]).unwrap();
        let b = tls::load_or_generate(dir.path(), "b", &["localhost".to_owned()]).unwrap();
        let (cert, key) = (
            dir.path().join("live-cert.pem"),
            dir.path().join("live-key.pem"),
        );
        std::fs::copy(&a.cert_path, &cert).unwrap();
        std::fs::copy(&a.key_path, &key).unwrap();
        let mut config = Config::default();
        config.tls.mode = TlsMode::Files;
        config.tls.cert_file = cert;
        config.tls.key_file = key;
        (
            tls::load(&config).unwrap(),
            a.chain[0].clone(),
            b.chain[0].clone(),
        )
    }

    fn install(dir: &TempDir, name: &str) {
        for part in ["cert", "key"] {
            std::fs::copy(
                dir.path().join(format!("{name}-{part}.pem")),
                dir.path().join(format!("live-{part}.pem")),
            )
            .unwrap();
        }
    }

    #[tokio::test]
    async fn reload_switches_new_handshakes_and_keeps_open_connections() {
        let dir = TempDir::new("tls-reload");
        let (tls, a, b) = files_with_two_pairs(&dir);
        let reloader = tls.reloader().unwrap().clone();
        assert_eq!(reloader.fingerprint(), tls::Fingerprint::of(&a));
        let addr = echo_server(&tls).await;

        // A connection made before the reload.
        let tcp = TcpStream::connect(addr).await.unwrap();
        let name = ServerName::try_from("localhost").unwrap();
        let mut open = connector(a.clone()).connect(name, tcp).await.unwrap();

        install(&dir, "b");
        reloader.reload().unwrap();
        assert_eq!(reloader.fingerprint(), tls::Fingerprint::of(&b));

        // New handshakes use the new certificate only.
        echo(&connector(b.clone()), addr, "localhost")
            .await
            .unwrap();
        assert!(echo(&connector(a), addr, "localhost").await.is_err());
        // The old connection still works.
        open.write_all(b"still").await.unwrap();
        let mut buf = Vec::new();
        open.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"still");
    }

    #[tokio::test]
    async fn a_broken_new_pair_keeps_the_old_certificate() {
        let dir = TempDir::new("tls-reload-broken");
        let (tls, a, _) = files_with_two_pairs(&dir);
        let reloader = tls.reloader().unwrap().clone();
        let addr = echo_server(&tls).await;

        std::fs::write(dir.path().join("live-cert.pem"), "garbage").unwrap();
        assert!(matches!(
            reloader.reload(),
            Err(tls::TlsError::NoCertificate(_))
        ));
        // A certificate with the key of another one.
        std::fs::copy(
            dir.path().join("b-cert.pem"),
            dir.path().join("live-cert.pem"),
        )
        .unwrap();
        assert!(matches!(
            reloader.reload(),
            Err(tls::TlsError::Rustls { .. })
        ));
        std::fs::remove_file(dir.path().join("live-key.pem")).unwrap();
        assert!(matches!(reloader.reload(), Err(tls::TlsError::Io { .. })));

        assert_eq!(reloader.fingerprint(), tls::Fingerprint::of(&a));
        echo(&connector(a), addr, "localhost").await.unwrap();
    }

    #[test]
    fn reload_if_changed_follows_modification_times() {
        let dir = TempDir::new("tls-reload-mtime");
        let (tls, _, b) = files_with_two_pairs(&dir);
        let reloader = tls.reloader().unwrap().clone();
        assert!(!reloader.reload_if_changed().unwrap());

        let touch = |name: &str, secs: u64| {
            let file = std::fs::File::options()
                .write(true)
                .open(dir.path().join(name))
                .unwrap();
            let when = std::time::SystemTime::now() + Duration::from_secs(secs);
            file.set_modified(when).unwrap();
        };
        install(&dir, "b");
        touch("live-cert.pem", 10);
        touch("live-key.pem", 10);
        assert!(reloader.reload_if_changed().unwrap());
        assert_eq!(reloader.fingerprint(), tls::Fingerprint::of(&b));
        assert!(!reloader.reload_if_changed().unwrap());

        // A broken pair is reported once, then left alone until the files change again.
        std::fs::write(dir.path().join("live-key.pem"), "garbage").unwrap();
        touch("live-key.pem", 20);
        assert!(reloader.reload_if_changed().is_err());
        assert!(!reloader.reload_if_changed().unwrap());
        assert_eq!(reloader.fingerprint(), tls::Fingerprint::of(&b));
    }

    #[test]
    fn self_signed_and_off_do_not_reload() {
        let dir = TempDir::new("tls-reload-none");
        let (tls, _) = self_signed(&dir);
        assert!(tls.reloader().is_none());
        assert!(Tls::Off.reloader().is_none());
    }
}
