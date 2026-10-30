//! Spike B: QUIC datagram echo. See the library docs in `lib.rs`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use quic_echo::{client, server};

#[derive(Parser)]
#[command(about = "Hermes Spike B: QUIC datagram echo server and client")]
struct Args {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Subcommand)]
enum Mode {
    /// Echo every datagram back to its sender.
    Server {
        /// Address to listen on ([::] accepts IPv4 and IPv6 on Linux).
        #[arg(long, default_value = "[::]:7700")]
        bind: SocketAddr,
        /// Directory for the certificate and key (created on first start).
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Send datagrams to a server and report RTT, loss and reordering.
    Client {
        /// Server as host:port.
        #[arg(long)]
        server: String,
        /// The server's certificate fingerprint, as printed by the server (sha256:...).
        #[arg(long)]
        fingerprint: String,
        /// Datagrams per second.
        #[arg(long, default_value_t = 50)]
        rate: u32,
        /// Datagram size in bytes.
        #[arg(long, default_value_t = 200)]
        size: usize,
        /// Test length in seconds.
        #[arg(long, default_value_t = 60)]
        duration: u64,
        /// Use the server's IPv4 address.
        #[arg(long, conflicts_with = "ipv6")]
        ipv4: bool,
        /// Use the server's IPv6 address.
        #[arg(long)]
        ipv6: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let result = match Args::parse().mode {
        Mode::Server { bind, dir } => run_server(bind, dir).await,
        Mode::Client {
            server,
            fingerprint,
            rate,
            size,
            duration,
            ipv4,
            ipv6,
        } => {
            let opts = client::Options {
                server,
                fingerprint,
                rate,
                size,
                duration: Duration::from_secs(duration),
                grace: Duration::from_secs(2),
                prefer_ipv4: ipv4,
                prefer_ipv6: ipv6,
                verbose: true,
            };
            client::run(opts).await.map(|report| println!("\n{report}"))
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run_server(bind: SocketAddr, dir: PathBuf) -> quic_echo::Result<()> {
    let server = match server::Server::start(bind, &dir) {
        // Hosts without IPv6 cannot bind [::]; fall back to IPv4 only.
        Err(e) if bind.ip().is_unspecified() && bind.is_ipv6() => {
            println!("cannot listen on {bind} ({e}); falling back to IPv4 only");
            server::Server::start(SocketAddr::from(([0, 0, 0, 0], bind.port())), &dir)?
        }
        other => other?,
    };
    println!("listening on udp {}", server.local_addr()?);
    println!("fingerprint: {}", server.fingerprint());
    println!("Press Ctrl+C to stop.");
    tokio::select! {
        () = server.run() => {}
        _ = tokio::signal::ctrl_c() => println!("stopping"),
    }
    Ok(())
}
