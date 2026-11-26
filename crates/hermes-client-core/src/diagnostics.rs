//! Diagnostics: tests what can be tested from here and says how each part looks, in three
//! levels. [`Level::Working`] is verified end to end; [`Level::Warning`] might work, or works in
//! a reduced way, and the test cannot be sure; [`Level::NotWorking`] definitely failed.
//!
//! This holds the checks that need only the core: each server's addresses, HTTPS on each,
//! its certificate, the live connection, version compatibility, voice (QUIC over UDP) on each
//! address, this device's encryption keys, updates and the storage folders. The GUI adds the
//! ones that need the devices (microphone, speakers, screen share encoding, media playback).

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use hermes_proto::MIN_SERVER_PROTOCOL;
use hermes_proto::api::{InstanceResponse, QuicEndpoint};

use crate::core::{Core, InstanceId};
use crate::gateway::ConnStatus;
use crate::rest::Rest;
use crate::tls::Recorder;

/// How long one address gets to answer.
const EACH: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Working,
    Warning,
    NotWorking,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Working => "Working",
            Level::Warning => "Warning",
            Level::NotWorking => "Not working",
        }
    }
}

/// The result of one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub level: Level,
    /// What was found, in one line.
    pub detail: String,
    /// What to do about it, when it is not working.
    pub hint: Option<String>,
}

impl Check {
    pub fn new(name: impl Into<String>, level: Level, detail: impl Into<String>) -> Check {
        Check {
            name: name.into(),
            level,
            detail: detail.into(),
            hint: None,
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Check {
        self.hint = Some(hint.into());
        self
    }
}

/// A plain text report of `groups` (a heading and its checks), for a bug report. Holds no
/// secrets: addresses, versions and results only.
pub fn report(groups: &[(String, Vec<Check>)]) -> String {
    let mut out = format!(
        "Hermes {} diagnostics ({} {})\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    for (heading, checks) in groups {
        let _ = write!(out, "\n{heading}\n");
        for check in checks {
            let _ = writeln!(
                out,
                "  [{}] {}: {}",
                check.level.label(),
                check.name,
                check.detail
            );
            if let Some(hint) = &check.hint {
                let _ = writeln!(out, "      {hint}");
            }
        }
    }
    out
}

/// How long the system takes to look `host` up now, skipping the answers Hermes remembers.
async fn lookup_speed(host: &str) -> Check {
    const SLOW: std::time::Duration = std::time::Duration::from_secs(1);
    let (found, took) = crate::net::lookup_uncached(host).await;
    let ms = took.as_millis();
    match found {
        Ok(_) if took <= SLOW => Check::new("Lookup speed", Level::Working, format!("{ms} ms")),
        Ok(_) => Check::new(
            "Lookup speed",
            Level::Warning,
            format!("Looking up {host} took {ms} ms"),
        )
        .hint(
            "A DNS server answers slowly or not at all, often a VPN's. Hermes keeps using the \
             addresses it last found, but signing in after a restart can be slow. Check the DNS \
             servers of your network adapters.",
        ),
        Err(e) => Check::new(
            "Lookup speed",
            Level::Warning,
            format!("Looking up {host} again failed after {ms} ms: {}", short(e)),
        )
        .hint("The DNS server stopped answering. Hermes keeps using the addresses it found."),
    }
}

fn family(addr: &SocketAddr) -> &'static str {
    if addr.is_ipv6() { "IPv6" } else { "IPv4" }
}

/// Working when every address worked, a warning when some did (the others are fallen back
/// from), not working when none did.
fn by_address<T>(results: &[(SocketAddr, Result<T, String>)]) -> Level {
    let good = results.iter().filter(|(_, r)| r.is_ok()).count();
    if results.is_empty() || good == 0 {
        Level::NotWorking
    } else if good < results.len() {
        Level::Warning
    } else {
        Level::Working
    }
}

/// "IPv4 192.0.2.1: 40 ms; IPv6 2001:db8::1: timed out".
fn per_address<T>(
    results: &[(SocketAddr, Result<T, String>)],
    ok: impl Fn(&T) -> String,
) -> String {
    results
        .iter()
        .map(|(addr, r)| {
            let what = match r {
                Ok(v) => ok(v),
                Err(e) => e.clone(),
            };
            format!("{} {}: {what}", family(addr), addr.ip())
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// A hint for addresses of one family failing while the other works.
fn family_hint<T>(results: &[(SocketAddr, Result<T, String>)], what: &str) -> Option<String> {
    let failed_v6 = results.iter().any(|(a, r)| a.is_ipv6() && r.is_err());
    let failed_v4 = results.iter().any(|(a, r)| !a.is_ipv6() && r.is_err());
    match (failed_v4, failed_v6) {
        (false, true) => Some(format!(
            "The server's IPv6 address does not answer {what}. Hermes falls back to IPv4. The \
             server's owner can publish the port on IPv6 too (a Pterodactyl/Reviactyl server \
             needs an allocation on its IPv6 address and Bind set to ::), or remove the \
             domain's AAAA record."
        )),
        (true, false) => Some(format!(
            "The server's IPv4 address does not answer {what}; IPv6 does."
        )),
        (true, true) => Some(format!(
            "A firewall may block {what}, or the server is down. The server's owner can check \
             that the port is open."
        )),
        (false, false) => None,
    }
}

/// What one address answered over HTTPS.
struct Answer {
    info: InstanceResponse,
    ms: u128,
    cert: crate::core::CertStatus,
}

impl Core {
    /// Every check of one signed-in server, in order.
    pub async fn diagnose_instance(&self, inst: &InstanceId) -> Vec<Check> {
        let mut checks = Vec::new();
        let Some(entry) = self.config().instances.into_iter().find(|e| &e.id == inst) else {
            checks.push(Check::new(
                "Server",
                Level::NotWorking,
                "This server is not in the settings any more",
            ));
            return checks;
        };
        let host = entry.url;

        // 1. Addresses.
        let addrs = match crate::voice::resolve(&host.host, host.port).await {
            Ok(addrs) if !addrs.is_empty() => addrs,
            Ok(_) | Err(_) => {
                checks.push(
                    Check::new(
                        "Address lookup",
                        Level::NotWorking,
                        format!("{} did not resolve to any address", host.host),
                    )
                    .hint("Check the internet connection, or the server's address."),
                );
                return checks;
            }
        };
        let list = addrs
            .iter()
            .map(|a| format!("{} {}", family(a), a.ip()))
            .collect::<Vec<_>>()
            .join(", ");
        checks.push(Check::new("Address lookup", Level::Working, list));
        checks.push(lookup_speed(&host.host).await);

        // 2 and 3. HTTPS and the certificate, on each address.
        let mut https = Vec::new();
        for &addr in &addrs {
            https.push((addr, self.https_at(&host, addr).await));
        }
        let mut check = Check::new(
            "HTTPS (text, files)",
            by_address(&https),
            per_address(&https, |a| format!("{} ms", a.ms)),
        );
        if let Some(hint) = family_hint(&https, "HTTPS") {
            check = check.hint(hint);
        }
        checks.push(check);
        let answer = https.iter().find_map(|(_, r)| r.as_ref().ok());
        if let Some(answer) = answer {
            checks.push(certificate_check(&answer.cert));
        }

        // 4. The live connection.
        checks.push(live_check(&self.status(inst)));

        // 5. Version.
        if let Some(answer) = answer {
            let newer = self
                .store()
                .instance(inst)
                .is_some_and(crate::store::InstanceStore::server_is_newer);
            checks.push(version_check(&answer.info, newer));
        }

        // 6. Voice, on each address.
        let voice = self.store().instance(inst).and_then(|s| s.voice.clone());
        checks.push(match voice {
            None => Check::new(
                "Voice (UDP)",
                Level::Warning,
                "Not known yet: the live connection has not started",
            ),
            Some(v) if !v.enabled => Check::new(
                "Voice (UDP)",
                Level::Warning,
                "Voice is turned off on this server",
            ),
            Some(v) => match (v.quic_port, v.cert_sha256) {
                (Some(port), Some(cert)) => {
                    let quic = QuicEndpoint {
                        host: host.host.clone(),
                        port,
                        cert_sha256: cert,
                    };
                    voice_check(&quic).await
                }
                _ => Check::new(
                    "Voice (UDP)",
                    Level::Warning,
                    "The server did not say where voice connects",
                ),
            },
        });

        // 7. Encryption.
        checks.push(self.encryption_check(inst).await);
        checks
    }

    /// `GET /instance` at `addr` only, recording the certificate it shows.
    async fn https_at(
        &self,
        host: &crate::address::HostPort,
        addr: SocketAddr,
    ) -> Result<Answer, String> {
        let recorder = Recorder::new();
        let config = recorder.client_config().map_err(|e| e.to_string())?;
        let rest = Rest::with_tls_at((*config).clone(), host, addr, EACH).map_err(short)?;
        let started = Instant::now();
        let info = rest.instance().await.map_err(short)?;
        let ms = started.elapsed().as_millis();
        let seen = recorder.seen().ok_or("no certificate seen")?;
        let cert = self.cert_status(host, &seen);
        Ok(Answer { info, ms, cert })
    }

    /// This device's encryption keys are on the server.
    async fn encryption_check(&self, inst: &InstanceId) -> Check {
        const NAME: &str = "Encryption keys";
        let Some(me) = self.store().instance(inst).and_then(|s| s.me_id()) else {
            return Check::new(NAME, Level::Warning, "Not signed in yet");
        };
        let Some(keys) = self.account_keys(inst) else {
            return Check::new(NAME, Level::NotWorking, "This device has no keys");
        };
        let device = keys.identity.device_id().to_owned();
        let rest = match self.rest(inst) {
            Ok(rest) => rest,
            Err(e) => return Check::new(NAME, Level::Warning, format!("Could not ask: {e}")),
        };
        match rest.query_devices(&[me]).await {
            Ok(users) => {
                let known = users
                    .iter()
                    .filter(|u| u.user_id == me)
                    .flat_map(|u| &u.devices)
                    .any(|d| d.device_id == device);
                if known {
                    Check::new(NAME, Level::Working, "This device's keys are on the server")
                } else {
                    Check::new(
                        NAME,
                        Level::NotWorking,
                        "The server does not have this device's keys",
                    )
                    .hint(
                        "Others cannot send this device encrypted messages. Signing out and in \
                         again registers it.",
                    )
                }
            }
            Err(e) => Check::new(NAME, Level::Warning, format!("Could not ask: {}", short(e))),
        }
    }

    /// Whether this version is the newest.
    pub async fn diagnose_updates(&self) -> Check {
        const NAME: &str = "Updates";
        match crate::update::check(env!("CARGO_PKG_VERSION")).await {
            Ok(None) => Check::new(NAME, Level::Working, "This is the newest version"),
            Ok(Some(update)) => Check::new(
                NAME,
                Level::Working,
                format!("Version {} is available", update.version),
            ),
            Err(e) => Check::new(NAME, Level::Warning, format!("Could not check: {e}"))
                .hint("Offline, or a network that blocks GitHub."),
        }
    }

    /// The folders this client keeps its settings, keys and logs in can be written.
    pub fn diagnose_storage(&self) -> Check {
        const NAME: &str = "Storage";
        let Some(paths) = self.paths() else {
            return Check::new(NAME, Level::Warning, "Settings are kept in memory only");
        };
        let mut failed = Vec::new();
        for dir in [paths.dir.clone(), paths.dir.join("e2ee"), paths.logs()] {
            if dir.exists()
                && let Err(e) = writable(&dir)
            {
                failed.push(format!("{}: {e}", dir.display()));
            }
        }
        if failed.is_empty() {
            Check::new(
                NAME,
                Level::Working,
                format!("Writable: {}", paths.dir.display()),
            )
        } else {
            Check::new(NAME, Level::NotWorking, failed.join("; "))
                .hint("Settings, keys or logs cannot be saved. Check the folder's permissions.")
        }
    }
}

/// Writes and removes a small file in `dir`.
fn writable(dir: &Path) -> std::io::Result<()> {
    let probe = dir.join(format!(".hermes-diagnostics-{}", std::process::id()));
    std::fs::write(&probe, b"ok")?;
    std::fs::remove_file(&probe)
}

/// An error in one short line.
fn short(e: impl std::fmt::Display) -> String {
    let text = e.to_string();
    let lower = text.to_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        return "timed out".into();
    }
    if lower.contains("refused") {
        return "connection refused".into();
    }
    match text.char_indices().nth(120) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    }
}

fn certificate_check(cert: &crate::core::CertStatus) -> Check {
    use crate::core::CertStatus;
    const NAME: &str = "Certificate";
    match cert {
        CertStatus::Pinned => Check::new(NAME, Level::Working, "Matches the one saved for it"),
        CertStatus::Trusted => Check::new(NAME, Level::Working, "Valid (signed by a known CA)"),
        CertStatus::Unknown => Check::new(
            NAME,
            Level::Warning,
            "Self-signed and not saved on this device",
        ),
        CertStatus::Changed { .. } => Check::new(
            NAME,
            Level::NotWorking,
            "Different from the one saved for this server",
        )
        .hint(
            "Either the server made a new certificate, or someone is in between. Ask the \
             server's owner before trusting the new one.",
        ),
    }
}

fn live_check(status: &ConnStatus) -> Check {
    const NAME: &str = "Live connection";
    match status {
        ConnStatus::Connected => Check::new(NAME, Level::Working, "Connected"),
        ConnStatus::Connecting => Check::new(NAME, Level::Warning, "Connecting…"),
        ConnStatus::Reconnecting { attempt } => Check::new(
            NAME,
            Level::Warning,
            format!("Reconnecting (try {attempt})"),
        ),
        ConnStatus::Failed { reason } => Check::new(NAME, Level::NotWorking, reason.clone()),
        ConnStatus::LoggedOut => {
            Check::new(NAME, Level::NotWorking, "Signed out").hint("Sign in again.")
        }
        other => Check::new(NAME, Level::Warning, format!("{other:?}")),
    }
}

fn version_check(info: &InstanceResponse, server_is_newer: bool) -> Check {
    const NAME: &str = "Version";
    let versions = format!(
        "Server {}, this app {}",
        info.version,
        env!("CARGO_PKG_VERSION")
    );
    if info.protocol_version < MIN_SERVER_PROTOCOL {
        Check::new(NAME, Level::NotWorking, versions)
            .hint("The server is too old for this app. The server's owner needs to update it.")
    } else if server_is_newer {
        Check::new(NAME, Level::Warning, versions)
            .hint("The server is newer: update this app to use everything it offers.")
    } else {
        Check::new(NAME, Level::Working, versions)
    }
}

/// A full voice (QUIC) handshake on each address, then closed.
async fn voice_check(quic: &QuicEndpoint) -> Check {
    const NAME: &str = "Voice (UDP)";
    let addrs = match crate::voice::resolve(&quic.host, quic.port).await {
        Ok(addrs) if !addrs.is_empty() => addrs,
        _ => {
            return Check::new(
                NAME,
                Level::NotWorking,
                format!("{} did not resolve", quic.host),
            );
        }
    };
    let mut results = Vec::new();
    for addr in addrs {
        let started = Instant::now();
        let result = match crate::voice::connect_addr(quic, addr, EACH).await {
            Ok(conn) => {
                conn.close(0u32.into(), b"diagnostics");
                Ok(started.elapsed().as_millis())
            }
            Err(e) => Err(short(e)),
        };
        results.push((addr, result));
    }
    let mut check = Check::new(
        NAME,
        by_address(&results),
        per_address(&results, |ms| format!("{ms} ms (port {})", quic.port)),
    );
    if let Some(hint) = family_hint(&results, "voice (UDP)") {
        check = check.hint(hint);
    }
    check
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn addresses_sum_up_to_one_level() {
        let v4 = a("192.0.2.1:7700");
        let v6 = a("[2001:db8::1]:7700");
        let ok = |addr| (addr, Ok::<u32, String>(1));
        let bad = |addr| (addr, Err::<u32, String>("timed out".into()));
        assert_eq!(by_address(&[ok(v4), ok(v6)]), Level::Working);
        assert_eq!(by_address(&[ok(v4), bad(v6)]), Level::Warning);
        assert_eq!(by_address(&[bad(v4), bad(v6)]), Level::NotWorking);
        assert_eq!(by_address::<u32>(&[]), Level::NotWorking);
        // Only IPv6 failing points at the IPv6 setup.
        let hint = family_hint(&[ok(v4), bad(v6)], "voice (UDP)").unwrap();
        assert!(hint.contains("IPv6"), "{hint}");
        assert!(family_hint(&[ok(v4), ok(v6)], "HTTPS").is_none());
        assert_eq!(
            per_address(&[ok(v4), bad(v6)], |n| format!("{n} ms")),
            "IPv4 192.0.2.1: 1 ms; IPv6 2001:db8::1: timed out"
        );
    }

    #[test]
    fn the_report_lists_every_check() {
        let report = report(&[(
            "hermes.example".into(),
            vec![
                Check::new("HTTPS", Level::Working, "40 ms"),
                Check::new("Voice (UDP)", Level::Warning, "IPv6: timed out").hint("Publish it."),
            ],
        )]);
        assert!(report.contains("[Working] HTTPS: 40 ms"), "{report}");
        assert!(
            report.contains("[Warning] Voice (UDP): IPv6: timed out"),
            "{report}"
        );
        assert!(report.contains("      Publish it."), "{report}");
    }
}
