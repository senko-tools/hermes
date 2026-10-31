//! A raw QUIC voice client for the tests: pins the certificate from `/voice/join`, speaks the
//! control stream and sends/receives audio datagrams.

use std::sync::Arc;
use std::time::Duration;

use hermes_proto::voice::{self, ALPN, AudioHeader};
use quinn::crypto::rustls::QuicClientConfig;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::TestServer;

const TIMEOUT: Duration = Duration::from_secs(5);

/// Accepts only the certificate with this `"3F:A9:…"` fingerprint.
#[derive(Debug)]
struct Pinned {
    fingerprint: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if voice::colon_hex(&Sha256::digest(end_entity.as_ref())) == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("fingerprint mismatch".into()))
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

pub struct VoiceClient {
    pub conn: quinn::Connection,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    _endpoint: quinn::Endpoint,
}

impl TestServer {
    /// `POST /voice/join` for `channel_id`; panics unless it succeeds.
    pub async fn voice_token(&self, token: &str, channel_id: &str) -> Value {
        self.post(
            "/voice/join",
            Some(token),
            json!({ "channel_id": channel_id }),
        )
        .await
        .ok()
    }

    /// Joins `channel_id` and authenticates; returns the client and the `ready` payload.
    pub async fn voice_join(&self, token: &str, channel_id: &str) -> (VoiceClient, Value) {
        let join = self.voice_token(token, channel_id).await;
        let mut client = VoiceClient::connect(&join).await;
        client
            .send(json!({ "op": "auth", "d": {
                "token": join["token"], "self_mute": false, "self_deaf": false,
            }}))
            .await;
        let ready = client.recv_op("ready").await["d"].clone();
        (client, ready)
    }
}

impl VoiceClient {
    /// Connects to the endpoint of a `/voice/join` response and opens the control stream.
    pub async fn connect(join: &Value) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Arc::new(Pinned {
            fingerprint: join["quic"]["cert_sha256"].as_str().unwrap().to_owned(),
            provider: provider.clone(),
        });
        let mut crypto = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        crypto.alpn_protocols = vec![ALPN.to_vec()];
        let mut config =
            quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(crypto).unwrap()));
        config.transport_config(Arc::new(hermes_server::voice::endpoint::transport_config()));
        let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(config);
        let host = join["quic"]["host"].as_str().unwrap();
        let port = join["quic"]["port"].as_u64().unwrap();
        let addr = format!("{host}:{port}").parse().unwrap();
        let conn = endpoint.connect(addr, "hermes").unwrap().await.unwrap();
        let (send, recv) = conn.open_bi().await.unwrap();
        VoiceClient {
            conn,
            send,
            recv,
            _endpoint: endpoint,
        }
    }

    pub async fn send(&mut self, msg: Value) {
        let body = msg.to_string().into_bytes();
        let mut frame = (body.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&body);
        self.send.write_all(&frame).await.unwrap();
    }

    /// The next control message, or `None` when the stream or connection ends.
    pub async fn try_recv(&mut self) -> Option<Value> {
        let read = async {
            let mut prefix = [0u8; 4];
            self.recv.read_exact(&mut prefix).await.ok()?;
            let mut body = vec![0u8; u32::from_be_bytes(prefix) as usize];
            self.recv.read_exact(&mut body).await.ok()?;
            Some(serde_json::from_slice(&body).unwrap())
        };
        tokio::time::timeout(TIMEOUT, read)
            .await
            .expect("timed out waiting for a control message")
    }

    pub async fn recv(&mut self) -> Value {
        self.try_recv().await.expect("control stream ended")
    }

    /// Skips control messages until one with `op` arrives.
    pub async fn recv_op(&mut self, op: &str) -> Value {
        loop {
            let msg = self.recv().await;
            if msg["op"] == op {
                return msg;
            }
        }
    }

    /// Sends an audio datagram with a 3-byte fake Opus payload.
    pub fn send_audio(&self, sequence: u16) {
        let packet = AudioHeader {
            voice_active: true,
            sender_id: 999,
            timestamp: u32::from(sequence) * voice::FRAME_SAMPLES,
            sequence,
            audio_level: 20,
        }
        .encode(&[1, 2, 3]);
        self.conn.send_datagram(packet.into()).unwrap();
    }

    /// The next audio datagram within `ms`, if any.
    pub async fn recv_audio(&self, ms: u64) -> Option<(AudioHeader, Vec<u8>)> {
        let data = tokio::time::timeout(Duration::from_millis(ms), self.conn.read_datagram())
            .await
            .ok()?
            .ok()?;
        let (header, opus) = AudioHeader::decode(&data).unwrap();
        Some((header, opus.to_vec()))
    }

    /// Sends a few packets and returns how many arrive at `to`, with the sender ID they carry.
    pub async fn deliver(&self, to: &VoiceClient, from_seq: u16) -> (usize, Option<u16>) {
        for i in 0..5 {
            self.send_audio(from_seq + i);
        }
        let mut got = 0;
        let mut sender = None;
        while let Some((header, _)) = to.recv_audio(300).await {
            got += 1;
            sender = Some(header.sender_id);
        }
        (got, sender)
    }
}
