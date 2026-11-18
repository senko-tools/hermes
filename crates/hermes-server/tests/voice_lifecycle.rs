//! Voice edge cases around the life of a connection: a client that stops reading its control
//! stream, and calls that are still running when the server stops.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::start;
use common::voice::VoiceClient;
use hermes_proto::voice::ALPN;
use hermes_server::config::RegistrationMode;
use quinn::crypto::rustls::QuicClientConfig;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde_json::{Value, json};

/// Accepts any certificate: these tests are about the control stream, not about pinning (which
/// `tests/voice.rs` covers).
#[derive(Debug)]
struct AnyCert(Arc<CryptoProvider>);

impl ServerCertVerifier for AnyCert {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
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

/// A voice client that authenticates and then never reads its control stream. The streams stay
/// open (dropping the receiving side would tell the server to stop sending).
struct Stalled {
    conn: quinn::Connection,
    _streams: (quinn::SendStream, quinn::RecvStream),
    _endpoint: quinn::Endpoint,
}

/// Connects to the endpoint of a `/voice/join` response with a tiny stream receive window and
/// authenticates.
async fn connect_without_reading(join: &Value) -> Stalled {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut crypto = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCert(provider)))
        .with_no_client_auth();
    crypto.alpn_protocols = vec![ALPN.to_vec()];
    let mut transport = hermes_server::voice::endpoint::transport_config();
    // The server can write only this much before its writes to us block.
    transport.stream_receive_window(1024u32.into());
    let mut config =
        quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(crypto).unwrap()));
    config.transport_config(Arc::new(transport));
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    endpoint.set_default_client_config(config);
    let host = join["quic"]["host"].as_str().unwrap();
    let port = join["quic"]["port"].as_u64().unwrap();
    let addr = format!("{host}:{port}").parse().unwrap();
    let conn = endpoint.connect(addr, "hermes").unwrap().await.unwrap();
    let (mut send, recv) = conn.open_bi().await.unwrap();
    let auth = json!({ "op": "auth", "d": {
        "token": join["token"], "self_mute": false, "self_deaf": false,
    }});
    let body = auth.to_string().into_bytes();
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(&body);
    send.write_all(&frame).await.unwrap();
    Stalled {
        conn,
        _streams: (send, recv),
        _endpoint: endpoint,
    }
}

#[tokio::test]
async fn a_client_that_stops_reading_its_control_stream_is_closed() {
    let server = start("voice-stalled-control", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, _) = server.member(&owner, &space_id, "alice").await;
    let (bob, _) = server.member(&owner, &space_id, "bob").await;
    let general = server.channel_id(&space_id, "General");
    let (mut a, _) = server.voice_join(&alice, &general).await;
    let join = server.voice_token(&bob, &general).await;
    let stalled = connect_without_reading(&join).await;
    a.recv_op("participant_joined").await;

    // Every toggle sends `share_started` / `share_stopped` to Bob, who reads none of them.
    for _ in 0..hermes_server::voice::CONTROL_QUEUE {
        a.send(json!({ "op": "share_start", "d": {} })).await;
        a.send(json!({ "op": "share_stop", "d": {} })).await;
    }
    let reason = tokio::time::timeout(Duration::from_secs(10), stalled.conn.closed())
        .await
        .expect("the stalled client is closed instead of piling up messages");
    match reason {
        quinn::ConnectionError::ApplicationClosed(close) => {
            assert_eq!(&close.reason[..], b"control queue full");
        }
        other => panic!("unexpected close: {other}"),
    }
    // The others see Bob leave and carry on.
    a.recv_op("participant_left").await;
    let (_b, ready) = server.voice_join(&bob, &general).await;
    assert_eq!(ready["participants"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_call_running_at_shutdown_gets_its_end_message() {
    let server = start("voice-shutdown-call", RegistrationMode::Open).await;
    let (owner, _, space_id) = server.owner().await;
    let (alice, alice_id) = server.member(&owner, &space_id, "alice").await;
    let dm = server
        .post("/dms", Some(&owner), json!({ "recipient_ids": [alice_id] }))
        .await
        .ok();
    let dm_id = dm["id"].as_str().unwrap().to_owned();
    let (mut ows, _) = server.identify(&owner).await;
    let (_o, _): (VoiceClient, _) = server.voice_join(&owner, &dm_id).await;
    let started = ows.recv_op("message_create").await;
    assert_eq!(started["d"]["content"], "owner started a call");
    let (_a, _) = server.voice_join(&alice, &dm_id).await;

    server.shut_down().await.unwrap();

    let db = hermes_server::db::Db::open(&server.state.config.storage.data_dir)
        .await
        .unwrap();
    let contents: Vec<String> =
        sqlx::query_scalar("SELECT content FROM messages WHERE channel_id = ? ORDER BY id")
            .bind(dm_id.parse::<i64>().unwrap())
            .fetch_all(&db.read)
            .await
            .unwrap();
    db.close().await;
    assert_eq!(contents, ["owner started a call", "Call ended · 1 min"]);
}
