//! Instance addresses, certificate fingerprints and invite links (PLAN.md section 9.6).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Port used when an address has none.
pub const DEFAULT_PORT: u16 = 7700;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressError {
    Empty,
    InvalidHost(String),
    InvalidPort(String),
    InvalidFingerprint(String),
    NotAnInvite(String),
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AddressError::Empty => f.write_str("the address is empty"),
            AddressError::InvalidHost(h) => write!(f, "{h:?} is not a valid host name or IP"),
            AddressError::InvalidPort(p) => write!(f, "{p:?} is not a valid port"),
            AddressError::InvalidFingerprint(fp) => {
                write!(f, "{fp:?} is not a SHA-256 fingerprint")
            }
            AddressError::NotAnInvite(s) => write!(f, "{s:?} is not an invite link"),
        }
    }
}

impl std::error::Error for AddressError {}

/// A server address: host name or IP (IPv6 without brackets) and port.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostPort {
    pub host: String,
    pub port: u16,
}

impl HostPort {
    /// Parses what a user types: `host`, `host:port`, `[v6]`, `[v6]:port`, a bare IPv6 address,
    /// or an `https://` URL (its path is ignored). The host is lowercased; the port defaults to
    /// 7700.
    pub fn parse(input: &str) -> Result<Self, AddressError> {
        let mut s = input.trim();
        if s.is_empty() {
            return Err(AddressError::Empty);
        }
        if let Some(rest) = strip_scheme(s, "https://").or_else(|| strip_scheme(s, "hermes://")) {
            s = rest;
        }
        // Only the authority: drop path, query and fragment.
        let end = s.find(['/', '?', '#']).unwrap_or(s.len());
        let authority = &s[..end];
        if authority.is_empty() {
            return Err(AddressError::Empty);
        }
        let (host, port) = split_authority(authority)?;
        let host = host.to_ascii_lowercase();
        if !valid_host(&host) {
            return Err(AddressError::InvalidHost(host));
        }
        Ok(HostPort { host, port })
    }

    fn is_ipv6(&self) -> bool {
        self.host.contains(':')
    }

    /// `host:port`, with brackets around IPv6 addresses.
    pub fn authority(&self) -> String {
        if self.is_ipv6() {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// The canonical stored form, `https://host:port`.
    pub fn url(&self) -> String {
        format!("https://{}", self.authority())
    }

    /// What users type and the instance ID in the config: `host`, plus `:port` when not 7700.
    pub fn display(&self) -> String {
        let host = if self.is_ipv6() {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == DEFAULT_PORT {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

impl fmt::Display for HostPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

impl FromStr for HostPort {
    type Err = AddressError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        HostPort::parse(s)
    }
}

impl Serialize for HostPort {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.url())
    }
}

impl<'de> Deserialize<'de> for HostPort {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        HostPort::parse(&s).map_err(serde::de::Error::custom)
    }
}

fn strip_scheme<'a>(s: &'a str, scheme: &str) -> Option<&'a str> {
    let head = s.get(..scheme.len())?;
    head.eq_ignore_ascii_case(scheme)
        .then(|| &s[scheme.len()..])
}

fn split_authority(authority: &str) -> Result<(&str, u16), AddressError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest
            .split_once(']')
            .ok_or_else(|| AddressError::InvalidHost(authority.to_owned()))?;
        let port = match after {
            "" => DEFAULT_PORT,
            p => parse_port(
                p.strip_prefix(':')
                    .ok_or_else(|| AddressError::InvalidHost(authority.to_owned()))?,
            )?,
        };
        return Ok((host, port));
    }
    match authority.matches(':').count() {
        0 => Ok((authority, DEFAULT_PORT)),
        1 => {
            let (host, port) = authority.split_once(':').expect("one colon");
            Ok((host, parse_port(port)?))
        }
        // A bare IPv6 address without brackets cannot carry a port.
        _ => Ok((authority, DEFAULT_PORT)),
    }
}

fn parse_port(s: &str) -> Result<u16, AddressError> {
    match s.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(AddressError::InvalidPort(s.to_owned())),
    }
}

fn valid_host(host: &str) -> bool {
    if host.contains(':') {
        return host.parse::<std::net::Ipv6Addr>().is_ok();
    }
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

/// SHA-256 of a certificate's DER encoding. Displays as `sha256:<64 lowercase hex digits>`, the
/// form of invite links (`#fp=sha256:…`) and of `known_hosts.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub [u8; 32]);

impl Fingerprint {
    pub fn of(der: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        Fingerprint(Sha256::digest(der).into())
    }

    /// Parses `sha256:<hex>`, plain hex, or colon-separated hex (`3F:A9:…`), in any case.
    pub fn parse(input: &str) -> Result<Self, AddressError> {
        let err = || AddressError::InvalidFingerprint(input.to_owned());
        let s = input.trim();
        let s = strip_scheme(s, "sha256:").unwrap_or(s);
        let hex: String = s.chars().filter(|&c| c != ':').collect();
        if hex.len() != 64 {
            return Err(err());
        }
        let mut bytes = [0u8; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(hex.get(2 * i..2 * i + 2).ok_or_else(err)?, 16)
                .map_err(|_| err())?;
        }
        Ok(Fingerprint(bytes))
    }

    /// `3F:A9:…`, easier to compare by eye in a trust prompt.
    pub fn colon_hex(&self) -> String {
        let parts: Vec<String> = self.0.iter().map(|b| format!("{b:02X}")).collect();
        parts.join(":")
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sha256:")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for Fingerprint {
    type Err = AddressError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Fingerprint::parse(s)
    }
}

impl Serialize for Fingerprint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Fingerprint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Fingerprint::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// An invite link in either form: `https://host:port/invite/<code>` or
/// `hermes://host[:port]/invite/<code>`, optionally with `#fp=sha256:…`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteLink {
    pub host: HostPort,
    pub code: String,
    pub fingerprint: Option<Fingerprint>,
}

impl InviteLink {
    pub fn parse(input: &str) -> Result<Self, AddressError> {
        let s = input.trim();
        let not_invite = || AddressError::NotAnInvite(s.to_owned());
        let (without_fragment, fragment) = match s.split_once('#') {
            Some((a, b)) => (a, Some(b)),
            None => (s, None),
        };
        let rest = strip_scheme(without_fragment, "https://")
            .or_else(|| strip_scheme(without_fragment, "hermes://"))
            .ok_or_else(not_invite)?;
        let (authority, path) = rest.split_once('/').ok_or_else(not_invite)?;
        let code = path
            .strip_prefix("invite/")
            .ok_or_else(not_invite)?
            .trim_end_matches('/');
        let code = code.split(['?', '/']).next().unwrap_or_default();
        if code.is_empty() || !code.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(not_invite());
        }
        let host = HostPort::parse(authority)?;
        let fingerprint = fragment
            .and_then(|f| f.split('&').find_map(|kv| kv.strip_prefix("fp=")))
            .map(Fingerprint::parse)
            .transpose()?;
        Ok(InviteLink {
            host,
            code: code.to_owned(),
            fingerprint,
        })
    }

    /// The app form, `hermes://host[:port]/invite/<code>[#fp=…]`.
    pub fn app_link(&self) -> String {
        let fragment = self
            .fingerprint
            .map_or_else(String::new, |fp| format!("#fp={fp}"));
        format!("hermes://{}/invite/{}{fragment}", self.host, self.code)
    }
}

/// What a user pasted into "Add instance": an address or an invite link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddTarget {
    Address(HostPort),
    Invite(InviteLink),
}

impl AddTarget {
    pub fn parse(input: &str) -> Result<Self, AddressError> {
        if input.contains("/invite/") {
            InviteLink::parse(input).map(AddTarget::Invite)
        } else {
            HostPort::parse(input).map(AddTarget::Address)
        }
    }

    pub fn host(&self) -> &HostPort {
        match self {
            AddTarget::Address(host) => host,
            AddTarget::Invite(link) => &link.host,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hp(host: &str, port: u16) -> HostPort {
        HostPort {
            host: host.into(),
            port,
        }
    }

    #[test]
    fn addresses() {
        assert_eq!(
            HostPort::parse("hermes.senko.tools").unwrap(),
            hp("hermes.senko.tools", 7700)
        );
        assert_eq!(
            HostPort::parse(" Example.COM:8443 ").unwrap(),
            hp("example.com", 8443)
        );
        assert_eq!(
            HostPort::parse("https://example.com:7701/invite/x").unwrap(),
            hp("example.com", 7701)
        );
        assert_eq!(HostPort::parse("[::1]:9000").unwrap(), hp("::1", 9000));
        assert_eq!(HostPort::parse("[::1]").unwrap(), hp("::1", 7700));
        assert_eq!(
            HostPort::parse("2001:db8::1").unwrap(),
            hp("2001:db8::1", 7700)
        );
        assert_eq!(HostPort::parse("127.0.0.1:1").unwrap(), hp("127.0.0.1", 1));

        for bad in [
            "",
            "  ",
            "host:0",
            "host:99999",
            "ho st",
            "a..b",
            "[::1",
            "user@host",
        ] {
            assert!(HostPort::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn address_forms() {
        let a = hp("hermes.senko.tools", 7700);
        assert_eq!(a.display(), "hermes.senko.tools");
        assert_eq!(a.url(), "https://hermes.senko.tools:7700");
        let b = hp("::1", 9000);
        assert_eq!(b.display(), "[::1]:9000");
        assert_eq!(b.url(), "https://[::1]:9000");
        assert_eq!(HostPort::parse(&b.url()).unwrap(), b);
    }

    #[test]
    fn fingerprints() {
        let hex = "ab".repeat(32);
        let fp = Fingerprint::parse(&format!("sha256:{hex}")).unwrap();
        assert_eq!(fp, Fingerprint([0xab; 32]));
        assert_eq!(fp.to_string(), format!("sha256:{hex}"));
        assert_eq!(Fingerprint::parse(&fp.colon_hex()).unwrap(), fp);
        assert_eq!(Fingerprint::parse(&hex.to_uppercase()).unwrap(), fp);
        assert!(Fingerprint::parse("sha256:abc").is_err());
        assert!(Fingerprint::parse(&"zz".repeat(32)).is_err());
    }

    #[test]
    fn invite_links() {
        let hex = "0f".repeat(32);
        let web = InviteLink::parse(&format!(
            "https://hermes.senko.tools:7700/invite/AbC123xYz9#fp=sha256:{hex}"
        ))
        .unwrap();
        assert_eq!(web.host, hp("hermes.senko.tools", 7700));
        assert_eq!(web.code, "AbC123xYz9");
        assert_eq!(web.fingerprint, Some(Fingerprint([0x0f; 32])));
        assert_eq!(
            web.app_link(),
            format!("hermes://hermes.senko.tools/invite/AbC123xYz9#fp=sha256:{hex}")
        );

        let app = InviteLink::parse("hermes://example.com:9000/invite/Code1").unwrap();
        assert_eq!(app.host, hp("example.com", 9000));
        assert_eq!(app.fingerprint, None);
        assert_eq!(InviteLink::parse(&app.app_link()).unwrap(), app);

        for bad in [
            "https://example.com/",
            "https://example.com/invite/",
            "ftp://example.com/invite/abc",
            "https://example.com/invite/a-b",
            "https://example.com/invite/abc#fp=nope",
        ] {
            assert!(InviteLink::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn add_targets() {
        assert!(matches!(
            AddTarget::parse("example.com").unwrap(),
            AddTarget::Address(_)
        ));
        assert!(matches!(
            AddTarget::parse("https://example.com:7700/invite/abc").unwrap(),
            AddTarget::Invite(_)
        ));
    }
}
