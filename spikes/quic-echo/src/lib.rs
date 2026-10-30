//! Spike B (PLAN.md section 20, Phase 0): QUIC datagram echo.
//!
//! `server` echoes every datagram back; `client` sends a steady stream of datagrams and
//! measures RTT, loss and reordering. The client pins the server's self-signed certificate
//! by its SHA-256 fingerprint, like the real voice transport will (PLAN.md section 13).

pub mod client;
pub mod pin;
pub mod server;

use std::sync::Arc;
use std::time::Duration;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

pub const ALPN: &[u8] = b"hermes-quic-echo/1";

/// Keep-alive and idle timeout from PLAN.md section 12.2.
fn transport_config() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(5)));
    t.max_idle_timeout(Some(
        Duration::from_secs(15)
            .try_into()
            .expect("valid idle timeout"),
    ));
    Arc::new(t)
}

fn crypto_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}
