//! TLS for REST and the gateway (PLAN.md section 13): normal CA validation through the OS trust
//! store, or pinning to a known certificate fingerprint for self-signed instances.

use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};

use crate::address::Fingerprint;

/// How a server certificate is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// CA validation with the OS trust store (`tls.mode = "files"` or a reverse proxy).
    Ca,
    /// Only this exact certificate is accepted (`tls.mode = "self_signed"`).
    Pinned(Fingerprint),
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A rustls client config for `trust`. `http` sets the HTTP/1.1 ALPN used by REST.
pub fn client_config(trust: Trust, http: bool) -> Result<Arc<ClientConfig>, rustls::Error> {
    let provider = provider();
    let verifier: Arc<dyn ServerCertVerifier> = match trust {
        Trust::Ca => Arc::new(rustls_platform_verifier::Verifier::new(provider.clone())?),
        Trust::Pinned(fingerprint) => Arc::new(Pinned {
            fingerprint,
            provider: provider.clone(),
        }),
    };
    build(provider, verifier, http)
}

/// The rustls config of the voice QUIC connection (PLAN.md 12.2): TLS 1.3, ALPN
/// `hermes-voice/1`, the certificate pinned to the fingerprint from `/voice/join`.
pub fn voice_config(fingerprint: Fingerprint) -> Result<ClientConfig, rustls::Error> {
    let provider = provider();
    let verifier = Arc::new(Pinned {
        fingerprint,
        provider: provider.clone(),
    });
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = vec![hermes_proto::voice::ALPN.to_vec()];
    Ok(config)
}

fn build(
    provider: Arc<CryptoProvider>,
    verifier: Arc<dyn ServerCertVerifier>,
    http: bool,
) -> Result<Arc<ClientConfig>, rustls::Error> {
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    if http {
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
    }
    Ok(Arc::new(config))
}

/// Accepts exactly one certificate, whatever its name or issuer.
#[derive(Debug)]
struct Pinned {
    fingerprint: Fingerprint,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if Fingerprint::of(end_entity) == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "certificate fingerprint changed (expected {})",
                self.fingerprint
            )))
        }
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

/// What a probe saw of the server certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeenCertificate {
    pub fingerprint: Fingerprint,
    /// Whether normal CA validation accepted it.
    pub ca_valid: bool,
}

/// A verifier for probing an unknown instance: it records the certificate and whether CA
/// validation accepts it, then accepts the connection either way. Only used to read the public
/// `GET /instance`, so the user can be shown the fingerprint to trust.
#[derive(Debug)]
pub struct Recorder {
    ca: Option<rustls_platform_verifier::Verifier>,
    provider: Arc<CryptoProvider>,
    seen: Mutex<Option<SeenCertificate>>,
}

impl Recorder {
    pub fn new() -> Arc<Self> {
        let provider = provider();
        Arc::new(Recorder {
            ca: rustls_platform_verifier::Verifier::new(provider.clone()).ok(),
            provider,
            seen: Mutex::new(None),
        })
    }

    pub fn client_config(self: &Arc<Self>) -> Result<Arc<ClientConfig>, rustls::Error> {
        build(self.provider.clone(), self.clone(), true)
    }

    pub fn seen(&self) -> Option<SeenCertificate> {
        *self.seen.lock().expect("recorder lock")
    }
}

impl ServerCertVerifier for Recorder {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let ca_valid = self.ca.as_ref().is_some_and(|ca| {
            ca.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
                .is_ok()
        });
        *self.seen.lock().expect("recorder lock") = Some(SeenCertificate {
            fingerprint: Fingerprint::of(end_entity),
            ca_valid,
        });
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
