//! Looking up a server's addresses, and which of them to try first.
//!
//! **Name lookups.** A DNS server that does not answer (measured 2026-11-21: a VPN adapter's DNS
//! server made Windows take 16 s for one lookup, then fall back to the next server for a while)
//! used to fail requests with "operation timed out", because the lookup ran inside the 5 s
//! connect timeout. Now every lookup of a name goes through [`lookup`], which
//! - answers from memory for [`FRESH_FOR`] after a lookup,
//! - then asks the system again, but when that takes longer than [`SLOW_LOOKUP`] it answers with
//!   the last addresses it knew while the lookup finishes in the background (the certificate is
//!   still checked, so an old address can only fail, never fool us),
//! - runs one system lookup per name at a time, whoever asks.
//!
//! Callers with a tight connect timeout look the name up first ([`warm`]), so the time a first
//! lookup takes (up to [`LOOKUP_TIMEOUT`]) is never counted against the connect.
//!
//! **Address order.** Some networks lose IPv6 connections now and then while IPv4 works. After a
//! connect to a host fails, its IPv4 addresses go first for [`PREFER_IPV4_FOR`], for HTTPS
//! (through [`Resolver`]), the gateway and voice alike. Its IPv6 addresses stay in the list
//! behind them, so a host whose IPv4 is the broken one still answers.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};

/// How long a lookup's answer is used without asking the system again.
pub const FRESH_FOR: Duration = Duration::from_secs(60);
/// How long a lookup may take before the last known addresses are used instead.
pub const SLOW_LOOKUP: Duration = Duration::from_secs(1);
/// How long a lookup may take when no addresses are known yet.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_secs(30);

type Lookup = Shared<BoxFuture<'static, Result<Vec<IpAddr>, String>>>;

#[derive(Clone)]
struct Known {
    addrs: Vec<IpAddr>,
    at: Instant,
}

#[derive(Default)]
struct Names {
    /// Name → its last answer.
    known: HashMap<String, Known>,
    /// Name → the system lookup running now, and when it started.
    running: HashMap<String, (Lookup, Instant)>,
}

static NAMES: LazyLock<Mutex<Names>> = LazyLock::new(Default::default);

fn lock_names() -> std::sync::MutexGuard<'static, Names> {
    NAMES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The system's answer for `host` now, and how long it took. Nothing is remembered: for
/// diagnostics, which want to see the lookup as it is.
pub async fn lookup_uncached(host: &str) -> (std::io::Result<Vec<IpAddr>>, Duration) {
    let started = Instant::now();
    let found = tokio::net::lookup_host((host, 0))
        .await
        .map(|addrs| dedup(addrs.map(|a| a.ip())));
    (found, started.elapsed())
}

fn dedup(ips: impl IntoIterator<Item = IpAddr>) -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = Vec::new();
    for ip in ips {
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    out
}

/// The system lookup of `key` that is running, or a new one. It runs in its own task, so it
/// finishes (and is remembered) also when nobody waits for it any more.
fn running_lookup(key: &str) -> (Lookup, Instant) {
    let mut names = lock_names();
    if let Some(running) = names.running.get(key) {
        return running.clone();
    }
    let host = key.to_owned();
    let task = tokio::spawn(async move {
        let (found, took) = lookup_uncached(&host).await;
        if took > SLOW_LOOKUP {
            tracing::info!(
                "looking up {host} took {} ms (a slow DNS server?)",
                took.as_millis()
            );
        }
        let mut names = lock_names();
        names.running.remove(&host);
        match found {
            Ok(addrs) if !addrs.is_empty() => {
                names.known.insert(
                    host,
                    Known {
                        addrs: addrs.clone(),
                        at: Instant::now(),
                    },
                );
                Ok(addrs)
            }
            Ok(_) => Err(format!("{host} has no address")),
            Err(err) => Err(err.to_string()),
        }
    });
    let lookup: Lookup = async move {
        task.await
            .unwrap_or_else(|err| Err(format!("the lookup stopped: {err}")))
    }
    .boxed()
    .shared();
    let running = (lookup, Instant::now());
    names.running.insert(key.to_owned(), running.clone());
    running
}

/// The addresses of the name `host` (an IP address is its own answer), in the system's order.
pub async fn lookup(host: &str) -> std::io::Result<Vec<IpAddr>> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Ok(vec![ip]);
    }
    let key = host.to_ascii_lowercase();
    let known = lock_names().known.get(&key).cloned();
    if let Some(known) = &known
        && known.at.elapsed() < FRESH_FOR
    {
        return Ok(known.addrs.clone());
    }
    let (lookup, started) = running_lookup(&key);
    let Some(known) = known else {
        return match tokio::time::timeout(LOOKUP_TIMEOUT, lookup).await {
            Ok(Ok(addrs)) => Ok(addrs),
            Ok(Err(err)) => Err(std::io::Error::other(err)),
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "looking up {host} took longer than {} s",
                    LOOKUP_TIMEOUT.as_secs()
                ),
            )),
        };
    };
    // Whoever asks while a lookup is already slow gets the known addresses at once.
    let wait = SLOW_LOOKUP.saturating_sub(started.elapsed());
    match tokio::time::timeout(wait, lookup).await {
        Ok(Ok(addrs)) => Ok(addrs),
        Ok(Err(err)) => {
            tracing::info!("looking up {host} failed ({err}); using its last known addresses");
            Ok(known.addrs)
        }
        Err(_) => {
            tracing::debug!("looking up {host} is slow; using its last known addresses");
            Ok(known.addrs)
        }
    }
}

/// Looks `host` up now, so that a connect right after it finds the answer in memory instead of
/// spending its timeout on the lookup.
pub async fn warm(host: &str) -> std::io::Result<()> {
    lookup(host).await.map(|_| ())
}

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

/// The addresses of `host`:`port` (see [`lookup`]), in [`order`].
pub async fn resolve(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    let found = lookup(host).await?;
    let addrs = order(host, found.into_iter().map(|ip| SocketAddr::new(ip, port)));
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

    /// `host` was last found at `ips` `age` ago, and a lookup that never answers started
    /// `running_for` ago.
    fn stuck(host: &str, ips: &[IpAddr], age: Duration, running_for: Duration) {
        let never: Lookup = std::future::pending().boxed().shared();
        let now = Instant::now();
        let mut names = lock_names();
        names.known.insert(
            host.to_owned(),
            Known {
                addrs: ips.to_vec(),
                at: now - age,
            },
        );
        names
            .running
            .insert(host.to_owned(), (never, now - running_for));
    }

    #[tokio::test]
    async fn a_fresh_answer_is_used_without_asking() {
        let ip: IpAddr = "192.0.2.10".parse().unwrap();
        stuck("fresh.example", &[ip], Duration::ZERO, Duration::ZERO);
        let started = Instant::now();
        assert_eq!(lookup("FRESH.example").await.unwrap(), vec![ip]);
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn a_slow_lookup_falls_back_to_the_last_known_addresses() {
        let ip: IpAddr = "192.0.2.11".parse().unwrap();
        let old = FRESH_FOR * 2;
        stuck("slow.example", &[ip], old, Duration::ZERO);
        let started = Instant::now();
        assert_eq!(lookup("slow.example").await.unwrap(), vec![ip]);
        let took = started.elapsed();
        assert!(took >= SLOW_LOOKUP - Duration::from_millis(50), "{took:?}");
        assert!(took < SLOW_LOOKUP * 3, "{took:?}");

        // Once the lookup is already slow, the next caller does not wait again.
        stuck("slower.example", &[ip], old, SLOW_LOOKUP * 2);
        let started = Instant::now();
        assert_eq!(lookup("slower.example").await.unwrap(), vec![ip]);
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn addresses_are_their_own_answer() {
        assert_eq!(
            resolve("[2001:db8::5]", 7700).await.unwrap(),
            vec![addr("[2001:db8::5]:7700")]
        );
        assert_eq!(
            resolve("192.0.2.5", 7700).await.unwrap(),
            vec![addr("192.0.2.5:7700")]
        );
    }

    #[tokio::test]
    async fn a_real_lookup_is_remembered() {
        let first = lookup("localhost").await.unwrap();
        assert!(!first.is_empty());
        assert!(lock_names().known.contains_key("localhost"));
        assert!(!lock_names().running.contains_key("localhost"));
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
