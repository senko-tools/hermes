//! Runs the server and client in-process over localhost.

use std::time::Duration;

use quic_echo::{client, server::Server};

fn options(port: u16, fingerprint: String) -> client::Options {
    client::Options {
        server: format!("127.0.0.1:{port}"),
        fingerprint,
        rate: 200,
        size: 200,
        duration: Duration::from_secs(1),
        grace: Duration::from_millis(300),
        prefer_ipv4: true,
        prefer_ipv6: false,
        verbose: false,
    }
}

fn start_server(dir: &std::path::Path) -> (u16, String) {
    let server = Server::start("127.0.0.1:0".parse().unwrap(), dir).unwrap();
    let port = server.local_addr().unwrap().port();
    let fingerprint = server.fingerprint().to_string();
    tokio::spawn(server.run());
    (port, fingerprint)
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("quic-echo-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[tokio::test]
async fn echoes_every_datagram_over_localhost() {
    let dir = temp_dir("echo");
    let (port, fingerprint) = start_server(&dir);
    let report = client::run(options(port, fingerprint)).await.unwrap();
    assert_eq!(report.sent, 200);
    assert_eq!(report.received, 200, "{report}");
    assert_eq!(report.duplicates, 0);
    assert!(report.rtts_us.iter().all(|&r| r < 1_000_000));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn certificate_is_reused_across_restarts() {
    let dir = temp_dir("reuse");
    let first = Server::start("127.0.0.1:0".parse().unwrap(), &dir).unwrap();
    let second = Server::start("127.0.0.1:0".parse().unwrap(), &dir).unwrap();
    assert_eq!(first.fingerprint(), second.fingerprint());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn wrong_fingerprint_is_refused() {
    let dir = temp_dir("pin");
    let (port, _) = start_server(&dir);
    let wrong = format!("sha256:{}", "00".repeat(32));
    let err = client::run(options(port, wrong)).await.unwrap_err();
    assert!(err.to_string().contains("does not match"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
