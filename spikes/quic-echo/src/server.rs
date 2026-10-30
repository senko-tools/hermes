//! Echo server: a self-signed certificate kept on disk, every datagram sent straight back.

use std::fs;
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use quinn::crypto::rustls::QuicServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::{ALPN, Result, crypto_provider, pin, transport_config};

const CERT_FILE: &str = "quic-echo-cert.der";
const KEY_FILE: &str = "quic-echo-key.der";

pub struct Server {
    endpoint: quinn::Endpoint,
    fingerprint: String,
}

impl Server {
    /// Load (or create on first start) the certificate in `dir` and bind the UDP socket.
    pub fn start(bind: SocketAddr, dir: &Path) -> Result<Self> {
        let (cert, key) = load_or_create_cert(dir)?;
        let fingerprint = pin::fingerprint(cert.as_ref());

        let mut tls = rustls::ServerConfig::builder_with_provider(crypto_provider())
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)?;
        tls.alpn_protocols = vec![ALPN.to_vec()];
        let mut config =
            quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls)?));
        config.transport_config(transport_config());

        let endpoint = quinn::Endpoint::server(config, bind)?;
        Ok(Self {
            endpoint,
            fingerprint,
        })
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.endpoint.local_addr()?)
    }

    /// Accept connections forever, one task per connection.
    pub async fn run(self) {
        while let Some(incoming) = self.endpoint.accept().await {
            tokio::spawn(async move {
                let remote = incoming.remote_address();
                match incoming.await {
                    Ok(conn) => echo(conn).await,
                    Err(e) => println!("{remote}: handshake failed: {e}"),
                }
            });
        }
    }
}

async fn echo(conn: quinn::Connection) {
    let remote = conn.remote_address();
    let started = Instant::now();
    println!("{remote}: connected");
    let mut echoed = 0u64;
    let reason = loop {
        match conn.read_datagram().await {
            Ok(data) => {
                // Never block: if the send buffer is full the datagram is dropped.
                if conn.send_datagram(data).is_ok() {
                    echoed += 1;
                }
            }
            Err(e) => break e,
        }
    };
    let stats = conn.stats();
    println!(
        "{remote}: closed after {:.1} s ({reason}); echoed {echoed} datagrams, \
         path rtt {:.1} ms, lost packets {}",
        started.elapsed().as_secs_f64(),
        stats.path.rtt.as_secs_f64() * 1000.0,
        stats.path.lost_packets,
    );
}

fn load_or_create_cert(dir: &Path) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
    let cert_path = dir.join(CERT_FILE);
    let key_path = dir.join(KEY_FILE);
    if cert_path.exists() && key_path.exists() {
        let cert = CertificateDer::from(fs::read(&cert_path)?);
        let key = PrivatePkcs8KeyDer::from(fs::read(&key_path)?);
        println!("loaded certificate from {}", cert_path.display());
        return Ok((cert, key.into()));
    }

    let generated = rcgen::generate_simple_self_signed(vec!["quic-echo".to_string()])?;
    let cert_der = generated.cert.der().to_vec();
    let key_der = generated.signing_key.serialize_der();
    fs::create_dir_all(dir)?;
    write_private(&key_path, &key_der)?;
    fs::write(&cert_path, &cert_der)?;
    println!("generated new certificate in {}", cert_path.display());
    Ok((
        CertificateDer::from(cert_der),
        PrivatePkcs8KeyDer::from(key_der).into(),
    ))
}

/// Write a file readable only by its owner (0600 on Unix).
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(data)?;
    Ok(())
}
