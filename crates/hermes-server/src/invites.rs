//! Invite codes and links (PLAN.md section 9.6).

use std::net::IpAddr;

use hermes_proto::id::Id;
use sqlx::SqliteExecutor;

use crate::config::Config;
use crate::db;
use crate::db::invites::InviteRow;
use crate::tls::Fingerprint;

/// Length of an invite code.
pub const CODE_LEN: usize = 10;
/// Port clients use when an address has none; links omit it in the `hermes://` form.
pub const DEFAULT_PORT: u16 = 7700;

const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// A random 10-character base62 code (~59 bits).
pub fn new_code() -> String {
    (0..CODE_LEN)
        .map(|_| char::from(BASE62[rand::random_range(0..BASE62.len())]))
        .collect()
}

/// Whether `code` could be an invite code (checked before touching the database).
pub fn is_valid_code(code: &str) -> bool {
    code.len() == CODE_LEN && code.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The two equivalent forms of an invite link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteLinks {
    /// `https://host:port/invite/<code>`, what people share.
    pub web: String,
    /// `hermes://host[:port]/invite/<code>`, opened by the client.
    pub app: String,
}

/// Builds the links for `code`. With a self-signed certificate (`fingerprint` is `Some`) both
/// carry `#fp=sha256:<hex>`.
pub fn links(config: &Config, fingerprint: Option<&Fingerprint>, code: &str) -> InviteLinks {
    let host = link_host(config);
    let port = config.public_port();
    let fragment = fingerprint.map_or_else(String::new, |fp| format!("#fp={fp}"));
    let app_authority = if port == DEFAULT_PORT {
        host.clone()
    } else {
        format!("{host}:{port}")
    };
    InviteLinks {
        web: format!("https://{host}:{port}/invite/{code}{fragment}"),
        app: format!("hermes://{app_authority}/invite/{code}{fragment}"),
    }
}

/// The address users type in "Add instance": `host`, or `host:port` when the port is not 7700.
pub fn address(config: &Config) -> String {
    let host = link_host(config);
    match config.public_port() {
        DEFAULT_PORT => host,
        port => format!("{host}:{port}"),
    }
}

/// `instance.public_host`, or the bind address when it is empty (`localhost` if that is a
/// wildcard). IPv6 addresses get brackets.
fn link_host(config: &Config) -> String {
    let host = config.instance.public_host.trim();
    let host = if host.is_empty() {
        let bind = config.network.bind;
        if bind.is_unspecified() {
            "localhost".to_owned()
        } else {
            bind.to_string()
        }
    } else {
        host.to_owned()
    };
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) => format!("[{v6}]"),
        _ => host,
    }
}

/// Inserts a new invite with a fresh code, retrying on the (unlikely) code collision.
pub async fn create(
    e: impl SqliteExecutor<'_> + Copy,
    space_id: Option<Id>,
    creator_id: Option<Id>,
    max_uses: Option<u32>,
    expires_at: Option<i64>,
    now: i64,
) -> Result<InviteRow, sqlx::Error> {
    let mut attempt = 0;
    loop {
        let invite = InviteRow {
            code: new_code(),
            space_id,
            creator_id,
            max_uses,
            uses: 0,
            expires_at,
            created_at: now,
        };
        match db::invites::insert(e, &invite).await {
            Ok(()) => return Ok(invite),
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() && attempt < 3 => {
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_base62() {
        let code = new_code();
        assert!(is_valid_code(&code), "{code}");
        assert_ne!(new_code(), code);
        assert!(!is_valid_code("short"));
        assert!(!is_valid_code("AbC123xYz!"));
        assert!(!is_valid_code("AbC123xYzé"));
    }

    #[test]
    fn links_with_fingerprint_and_default_port() {
        let mut config = Config::default();
        config.instance.public_host = "hermes.senko.tools".into();
        let fp = Fingerprint([0xab; 32]);
        let links = links(&config, Some(&fp), "AbC123xYz9");
        let hex = "ab".repeat(32);
        assert_eq!(
            links.web,
            format!("https://hermes.senko.tools:7700/invite/AbC123xYz9#fp=sha256:{hex}")
        );
        assert_eq!(
            links.app,
            format!("hermes://hermes.senko.tools/invite/AbC123xYz9#fp=sha256:{hex}")
        );
    }

    #[test]
    fn links_with_public_port_and_no_fingerprint() {
        let mut config = Config::default();
        config.instance.public_host = "example.org".into();
        config.instance.public_port = 443;
        let links = links(&config, None, "AbC123xYz9");
        assert_eq!(links.web, "https://example.org:443/invite/AbC123xYz9");
        assert_eq!(links.app, "hermes://example.org:443/invite/AbC123xYz9");
    }

    #[test]
    fn links_fall_back_to_bind_address() {
        let mut config = Config::default();
        assert!(
            links(&config, None, "x")
                .web
                .starts_with("https://localhost:7700/")
        );
        config.network.bind = "2001:db8::1".parse().unwrap();
        assert!(
            links(&config, None, "x")
                .web
                .starts_with("https://[2001:db8::1]:7700/")
        );
        config.instance.public_host = "::1".into();
        assert!(links(&config, None, "x").app.starts_with("hermes://[::1]/"));
    }
}
