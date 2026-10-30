//! Echo client: steady datagram stream, then RTT / loss / reordering statistics.

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use quinn::crypto::rustls::QuicClientConfig;
use tokio::sync::Mutex;
use tokio::time::{MissedTickBehavior, interval, sleep, timeout};

use crate::pin::{PinnedCert, parse_fingerprint};
use crate::{ALPN, Result, crypto_provider, transport_config};

/// Sequence number (u32) + send time in microseconds since start (u64).
const HEADER: usize = 12;

pub struct Options {
    /// `host:port`.
    pub server: String,
    pub fingerprint: String,
    pub rate: u32,
    pub size: usize,
    pub duration: Duration,
    /// How long to wait for late echoes after the last datagram was sent.
    pub grace: Duration,
    pub prefer_ipv4: bool,
    pub prefer_ipv6: bool,
    /// Print progress every 10 s.
    pub verbose: bool,
}

#[derive(Debug)]
pub struct Report {
    pub remote: SocketAddr,
    pub sent: u64,
    pub received: u64,
    pub duplicates: u64,
    pub reordered: u64,
    /// RTTs in microseconds, sorted.
    pub rtts_us: Vec<u64>,
    /// Mean absolute RTT difference between consecutive received packets, microseconds.
    pub jitter_us: f64,
    pub max_datagram_size: Option<usize>,
    pub path_rtt: Duration,
    pub path_lost_packets: u64,
    pub congestion_events: u64,
}

impl Report {
    pub fn loss_percent(&self) -> f64 {
        if self.sent == 0 {
            0.0
        } else {
            (self.sent - self.received) as f64 * 100.0 / self.sent as f64
        }
    }

    fn percentile_ms(&self, p: f64) -> f64 {
        if self.rtts_us.is_empty() {
            return f64::NAN;
        }
        let idx = ((self.rtts_us.len() - 1) as f64 * p).round() as usize;
        self.rtts_us[idx] as f64 / 1000.0
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let avg = if self.rtts_us.is_empty() {
            f64::NAN
        } else {
            self.rtts_us.iter().sum::<u64>() as f64 / self.rtts_us.len() as f64 / 1000.0
        };
        writeln!(f, "server      {}", self.remote)?;
        writeln!(
            f,
            "datagrams   sent {}, received {}, loss {:.2} %, reordered {}, duplicates {}",
            self.sent,
            self.received,
            self.loss_percent(),
            self.reordered,
            self.duplicates
        )?;
        writeln!(
            f,
            "rtt ms      min {:.1} / avg {:.1} / p50 {:.1} / p95 {:.1} / p99 {:.1} / max {:.1}",
            self.percentile_ms(0.0),
            avg,
            self.percentile_ms(0.5),
            self.percentile_ms(0.95),
            self.percentile_ms(0.99),
            self.percentile_ms(1.0),
        )?;
        writeln!(f, "jitter ms   {:.2}", self.jitter_us / 1000.0)?;
        write!(
            f,
            "quic path   rtt {:.1} ms, lost packets {}, congestion events {}, max datagram {} bytes",
            self.path_rtt.as_secs_f64() * 1000.0,
            self.path_lost_packets,
            self.congestion_events,
            self.max_datagram_size
                .map_or("n/a".to_string(), |s| s.to_string()),
        )
    }
}

/// Per-sequence results collected by the receive task.
struct Received {
    rtt_us: Vec<Option<u64>>,
    duplicates: u64,
    reordered: u64,
    highest: Option<u32>,
}

pub async fn run(opts: Options) -> Result<Report> {
    if opts.size < HEADER {
        return Err(format!("--size must be at least {HEADER} bytes").into());
    }
    let expected = parse_fingerprint(&opts.fingerprint)?;
    let remote = resolve(&opts).await?;
    let host = opts
        .server
        .rsplit_once(':')
        .map_or(opts.server.as_str(), |(h, _)| h)
        .trim_matches(['[', ']']);

    let provider = crypto_provider();
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedCert::new(expected, provider)))
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut config = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls)?));
    config.transport_config(transport_config());

    let bind: SocketAddr = if remote.is_ipv4() {
        "0.0.0.0:0".parse()?
    } else {
        "[::]:0".parse()?
    };
    let mut endpoint = quinn::Endpoint::client(bind)?;
    endpoint.set_default_client_config(config);

    // The pinned fingerprint authenticates the server; the name is only sent as SNI.
    let conn = timeout(Duration::from_secs(10), endpoint.connect(remote, host)?)
        .await
        .map_err(|_| format!("timed out connecting to {remote} (is UDP blocked?)"))??;
    if opts.verbose {
        println!(
            "connected to {remote}, sending {} datagrams/s of {} bytes for {} s",
            opts.rate,
            opts.size,
            opts.duration.as_secs()
        );
    }

    let total = (u64::from(opts.rate) * opts.duration.as_millis() as u64 / 1000) as usize;
    let received = Arc::new(Mutex::new(Received {
        rtt_us: vec![None; total],
        duplicates: 0,
        reordered: 0,
        highest: None,
    }));
    let received_count = Arc::new(AtomicU64::new(0));
    let start = Instant::now();

    let recv_task = {
        let conn = conn.clone();
        let received = received.clone();
        let count = received_count.clone();
        tokio::spawn(async move {
            while let Ok(data) = conn.read_datagram().await {
                let now_us = start.elapsed().as_micros() as u64;
                if data.len() < HEADER {
                    continue;
                }
                let seq = u32::from_be_bytes(data[0..4].try_into().expect("4 bytes"));
                let sent_us = u64::from_be_bytes(data[4..12].try_into().expect("8 bytes"));
                let mut r = received.lock().await;
                let Some(slot) = r.rtt_us.get_mut(seq as usize) else {
                    continue;
                };
                if slot.is_some() {
                    r.duplicates += 1;
                    continue;
                }
                *slot = Some(now_us.saturating_sub(sent_us));
                count.fetch_add(1, Ordering::Relaxed);
                match r.highest {
                    Some(h) if seq < h => r.reordered += 1,
                    _ => r.highest = Some(seq),
                }
            }
        })
    };

    let mut tick = interval(Duration::from_secs_f64(1.0 / f64::from(opts.rate)));
    tick.set_missed_tick_behavior(MissedTickBehavior::Burst);
    let mut buf = vec![0u8; opts.size];
    let mut sent = 0u64;
    let mut last_progress = Instant::now();
    for seq in 0..total as u32 {
        tick.tick().await;
        buf[0..4].copy_from_slice(&seq.to_be_bytes());
        buf[4..12].copy_from_slice(&(start.elapsed().as_micros() as u64).to_be_bytes());
        conn.send_datagram(buf.clone().into())?;
        sent += 1;
        if opts.verbose && last_progress.elapsed() >= Duration::from_secs(10) {
            let got = received_count.load(Ordering::Relaxed);
            println!(
                "{:>4.0} s: sent {sent}, received {got}, rtt {:.1} ms",
                start.elapsed().as_secs_f64(),
                conn.rtt().as_secs_f64() * 1000.0
            );
            last_progress = Instant::now();
        }
    }

    sleep(opts.grace).await;
    let stats = conn.stats();
    let max_datagram_size = conn.max_datagram_size();
    conn.close(0u32.into(), b"done");
    recv_task.abort();
    let _ = recv_task.await;
    endpoint.wait_idle().await;

    let r = received.lock().await;
    let in_order: Vec<u64> = r.rtt_us.iter().flatten().copied().collect();
    let jitter_us = if in_order.len() > 1 {
        in_order
            .windows(2)
            .map(|w| w[0].abs_diff(w[1]) as f64)
            .sum::<f64>()
            / (in_order.len() - 1) as f64
    } else {
        0.0
    };
    let mut rtts_us = in_order;
    rtts_us.sort_unstable();
    Ok(Report {
        remote,
        sent,
        received: rtts_us.len() as u64,
        duplicates: r.duplicates,
        reordered: r.reordered,
        rtts_us,
        jitter_us,
        max_datagram_size,
        path_rtt: stats.path.rtt,
        path_lost_packets: stats.path.lost_packets,
        congestion_events: stats.path.congestion_events,
    })
}

async fn resolve(opts: &Options) -> Result<SocketAddr> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&opts.server)
        .await
        .map_err(|e| format!("cannot resolve {}: {e}", opts.server))?
        .collect();
    let pick = if opts.prefer_ipv4 {
        addrs.iter().find(|a| a.is_ipv4())
    } else if opts.prefer_ipv6 {
        addrs.iter().find(|a| a.is_ipv6())
    } else {
        addrs.first()
    };
    pick.copied()
        .ok_or_else(|| format!("{} has no matching address", opts.server).into())
}
