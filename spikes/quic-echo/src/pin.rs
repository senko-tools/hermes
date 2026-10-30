//! Certificate pinning: accept exactly one certificate, identified by its SHA-256 fingerprint.

use std::fmt::Write;
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

/// `sha256:` followed by 64 lowercase hex digits.
pub fn fingerprint(cert_der: &[u8]) -> String {
    let mut out = String::from("sha256:");
    for b in Sha256::digest(cert_der) {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Accepts `sha256:<hex>`, bare hex, upper or lower case, with or without `:` separators.
pub fn parse_fingerprint(s: &str) -> Result<[u8; 32], String> {
    let s = s.trim();
    let s = s
        .strip_prefix("sha256:")
        .or_else(|| s.strip_prefix("SHA256:"))
        .unwrap_or(s);
    let hex: String = s.chars().filter(|c| *c != ':').collect();
    if hex.len() != 64 {
        return Err(format!(
            "fingerprint must be 64 hex digits, got {}",
            hex.len()
        ));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| format!("fingerprint is not hex: {s}"))?;
    }
    Ok(out)
}

#[derive(Debug)]
pub struct PinnedCert {
    expected: [u8; 32],
    provider: Arc<CryptoProvider>,
}

impl PinnedCert {
    pub fn new(expected: [u8; 32], provider: Arc<CryptoProvider>) -> Self {
        Self { expected, provider }
    }
}

impl ServerCertVerifier for PinnedCert {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let actual: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if actual == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "server certificate fingerprint {} does not match the pinned one",
                fingerprint(end_entity.as_ref())
            )))
        }
    }

    // The handshake signature must still be checked: it proves the server owns the key.
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
            &self.provider.signature_verification_algorithms,
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
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_round_trips_in_all_spellings() {
        let fp = fingerprint(b"hello");
        let bytes = parse_fingerprint(&fp).unwrap();
        let hex = &fp["sha256:".len()..];
        assert_eq!(parse_fingerprint(hex).unwrap(), bytes);
        assert_eq!(parse_fingerprint(&hex.to_uppercase()).unwrap(), bytes);
        let colons: Vec<String> = (0..32).map(|i| hex[i * 2..i * 2 + 2].to_string()).collect();
        assert_eq!(parse_fingerprint(&colons.join(":")).unwrap(), bytes);
        assert!(parse_fingerprint("sha256:1234").is_err());
        assert!(parse_fingerprint(&"zz".repeat(32)).is_err());
    }
}
