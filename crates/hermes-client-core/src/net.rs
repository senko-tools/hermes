//! Which of a server's addresses to try first.
//!
//! Some networks lose IPv6 connections now and then while IPv4 works (measured 2026-11-19: 2 of
//! 110 IPv6 connects to the production server hung for 10 s, no IPv4 one did). After a connect
//! to a host fails, its IPv4 addresses go first for [`PREFER_IPV4_FOR`], for HTTPS (through
//! [`Resolver`]), the gateway and voice alike. Its IPv6 addresses stay in the list behind them,
//! so a host whose IPv4 is the broken one still answers.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long a host keeps its IPv4 addresses first after a failed connect.
pub const PREFER_IPV4_FOR: Duration = Duration::from_secs(10 * 60);

/// Host → until when its IPv4 addresses go first.
static PREFER_IPV4: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

fn preferences() -> std::sync::MutexGuard<'static, HashMap<String, Instant>> {
    PREFER_IPV4.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether `host`'s IPv4 addresses go first right now.
pub fn prefers_ipv4(host: &str) -> bool {
    prefers_ipv4_at(host, Instant::now())
}

fn prefers_ipv4_at(host: &str, now: Instant) -> bool {
    preferences()
        .get(&host.to_ascii_lowercase())
        .is_some_and(|until| now < *until)
}

/// A connect to `host` failed: try its IPv4 addresses first for a while. `why` goes in the log
/// line, which is written once each time the preference starts.
pub fn connect_failed(host: &str, why: &str) {
    if note_failure_at(host, Instant::now()) {
        tracing::info!(
            "connecting to {host} failed ({why}); trying its IPv4 addresses first for {} minutes",
            PREFER_IPV4_FOR.as_secs() / 60
        );
    }
}

/// Records the failure; `true` when the preference was not already on.
fn note_failure_at(host: &str, now: Instant) -> bool {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    let mut prefs = preferences();
    let was_on = prefs
        .get(&host.to_ascii_lowercase())
        .is_some_and(|until| now < *until);
    prefs.insert(host.to_ascii_lowercase(), now + PREFER_IPV4_FOR);
    !was_on
}

/// `addrs` without repeats, in the order to try them: as given (the system's preference), or
/// IPv4 first while `host` [`prefers_ipv4`].
pub fn order(host: &str, addrs: impl IntoIterator<Item = SocketAddr>) -> Vec<SocketAddr> {
    order_at(host, addrs, Instant::now())
}

fn order_at(
    host: &str,
    addrs: impl IntoIterator<Item = SocketAddr>,
    now: Instant,
) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = Vec::new();
    for addr in addrs {
        if !out.contains(&addr) {
            out.push(addr);
        }
    }
    if prefers_ipv4_at(host, now) {
        // Stable: each family keeps the system's order.
        out.sort_by_key(SocketAddr::is_ipv6);
    }
    out
}

/// The addresses of `host`:`port`, in [`order`].
pub async fn resolve(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    let found = tokio::net::lookup_host((host, port)).await?;
    let addrs = order(host, found);
    tracing::debug!("{host} resolves to {addrs:?}");
    Ok(addrs)
}

/// Connects a TCP stream to `host`:`port`, trying each address in [`order`] with `each` to
/// answer. Notes a failure when an address had to be skipped.
pub async fn connect_tcp(
    host: &str,
    port: u16,
    each: Duration,
) -> std::io::Result<tokio::net::TcpStream> {
    let addrs = resolve(host, port).await?;
    let mut failed: Vec<String> = Vec::new();
    for addr in &addrs {
        let attempt = tokio::time::timeout(each, tokio::net::TcpStream::connect(addr)).await;
        let why = match attempt {
            Ok(Ok(stream)) => {
                if !failed.is_empty() {
                    connect_failed(host, &failed.join("; "));
                }
                return Ok(stream);
            }
            Ok(Err(err)) => err.to_string(),
            Err(_) => "timed out".to_owned(),
        };
        failed.push(format!("{addr}: {why}"));
    }
    if !failed.is_empty() {
        connect_failed(host, &failed.join("; "));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        if failed.is_empty() {
            format!("{host} has no address")
        } else {
            failed.join("; ")
        },
    ))
}

/// reqwest's name lookup through [`resolve`], so HTTPS follows the same preference.
#[derive(Debug, Default, Clone, Copy)]
pub struct Resolver;

impl reqwest::dns::Resolve for Resolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            // reqwest fills in the URL's port.
            let addrs = resolve(&host, 0).await?;
            let addrs: reqwest::dns::Addrs = Box::new(addrs.into_iter());
            Ok(addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn system_order_until_a_failure_then_ipv4_first_until_it_expires() {
        let host = "order-test.example";
        let list = [
            addr("[2001:db8::1]:7700"),
            addr("[2001:db8::2]:7700"),
            addr("192.0.2.1:7700"),
            addr("[2001:db8::1]:7700"),
            addr("192.0.2.2:7700"),
        ];
        let now = Instant::now();
        assert_eq!(
            order_at(host, list, now),
            vec![list[0], list[1], list[2], list[4]],
            "the system's order, without repeats"
        );

        assert!(note_failure_at(host, now), "the first failure starts it");
        assert!(!note_failure_at(host, now), "a second one only extends it");
        assert_eq!(
            order_at(host, list, now),
            vec![list[2], list[4], list[0], list[1]],
            "IPv4 first, each family in its order"
        );
        assert!(
            prefers_ipv4_at("ORDER-test.example", now),
            "names ignore case"
        );

        let later = now + PREFER_IPV4_FOR + Duration::from_secs(1);
        assert_eq!(order_at(host, list, later)[0], list[0], "it wears off");
        assert!(note_failure_at(host, later), "and starts again");
    }

    #[test]
    fn addresses_are_not_marked() {
        assert!(!note_failure_at("192.0.2.9", Instant::now()));
        assert!(!prefers_ipv4("192.0.2.9"));
    }

    #[tokio::test]
    async fn tcp_skips_an_address_that_does_not_answer() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // localhost may resolve to ::1 first, where nothing listens: refused, then 127.0.0.1.
        let stream = connect_tcp("localhost", port, Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(stream.peer_addr().unwrap().port(), port);
    }
}
