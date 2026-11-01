//! `hermes-server healthcheck`: `GET /healthz` on the local server, for Docker's `HEALTHCHECK`
//! (the distroless image has no curl).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::config::{Config, TlsMode};

/// The whole check must finish within this time.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Responses are tiny; anything longer is not our `/healthz`.
const MAX_RESPONSE: u64 = 16 * 1024;

/// Where the check connects: the loopback address of the bind address's family when the server
/// listens on all addresses, otherwise the bind address itself.
pub fn target(config: &Config) -> SocketAddr {
    let ip = match config.network.bind {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, config.network.port)
}

/// Runs the check; `Err` describes why the server is not healthy.
pub async fn check(config: &Config) -> Result<(), String> {
    tokio::time::timeout(TIMEOUT, run(config))
        .await
        .map_err(|_| format!("no answer within {} s", TIMEOUT.as_secs()))?
}

async fn run(config: &Config) -> Result<(), String> {
    let addr = target(config);
    let tcp = TcpStream::connect(addr)
        .await
        .map_err(|err| format!("could not connect to {addr}: {err}"))?;
    let response = match config.tls.mode {
        TlsMode::Off => request(tcp).await?,
        TlsMode::SelfSigned | TlsMode::Files => {
            let connector = TlsConnector::from(Arc::new(client_config()?));
            let name = ServerName::IpAddress(addr.ip().into());
            let tls = connector
                .connect(name, tcp)
                .await
                .map_err(|err| format!("TLS handshake with {addr} failed: {err}"))?;
            request(tls).await?
        }
    };
    let (status, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let status = status.lines().next().unwrap_or_default();
    if status.split_whitespace().nth(1) == Some("200") && body.trim() == "ok" {
        Ok(())
    } else {
        Err(format!("unexpected answer from {addr}: {status:?}"))
    }
}

async fn request<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S) -> Result<String, String> {
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .map_err(|err| format!("could not send the request: {err}"))?;
    let mut response = Vec::new();
    // A TLS peer that closes without close_notify is still a complete answer here.
    let _ = (&mut stream)
        .take(MAX_RESPONSE)
        .read_to_end(&mut response)
        .await;
    if response.is_empty() {
        return Err("empty answer".to_owned());
    }
    Ok(String::from_utf8_lossy(&response).into_owned())
}

fn client_config() -> Result<ClientConfig, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|err| err.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
        .with_no_client_auth();
    Ok(config)
}

/// Accepts any certificate: the check talks to the server on the same machine and only wants to
/// know that it answers, and a `files` certificate is not valid for a loopback address.
#[derive(Debug)]
struct AnyCertificate(Arc<CryptoProvider>);

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_uses_loopback_for_unspecified_binds() {
        let mut config = Config::default();
        config.network.port = 7701;
        config.network.bind = "0.0.0.0".parse().unwrap();
        assert_eq!(target(&config), "127.0.0.1:7701".parse().unwrap());
        config.network.bind = "::".parse().unwrap();
        assert_eq!(target(&config), "[::1]:7701".parse().unwrap());
        config.network.bind = "10.0.0.5".parse().unwrap();
        assert_eq!(target(&config), "10.0.0.5:7701".parse().unwrap());
    }
}
