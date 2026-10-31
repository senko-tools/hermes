//! TLS for the HTTPS/WSS listener (PLAN.md sections 9.2, 9.3 and 13).
//!
//! - `self_signed`: a certificate generated once with `rcgen` into `<data_dir>/certs/` and reused
//!   forever, so its fingerprint (pinned by clients, appended to invite links) stays stable.
//! - `files`: a PEM certificate chain and key from `tls.cert_file` / `tls.key_file`.
//! - `off`: plain HTTP, for running behind a TLS-terminating reverse proxy.

mod listener;
mod reload;

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rcgen::{CertificateParams, DnType, KeyPair};
use rustls::ServerConfig;
use rustls::pki_types::pem::{self, PemObject};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

use crate::config::{Config, TlsMode};

pub use listener::TlsListener;
pub use reload::CertReloader;

/// Subdirectory of `storage.data_dir` holding the generated certificates.
pub const CERTS_DIR: &str = "certs";
/// Base name of the self-signed HTTPS certificate: `https-cert.pem` + `https-key.pem`.
pub const HTTPS_CERT_NAME: &str = "https";

/// Self-signed certificates are valid for this many years.
const VALIDITY_YEARS: i32 = 10;

/// How the HTTPS/WSS listener is secured, built from `[tls]` by [`load`].
#[derive(Debug, Clone)]
pub enum Tls {
    /// Plain HTTP/WS (`tls.mode = "off"`).
    Off,
    Rustls {
        config: Arc<ServerConfig>,
        /// Set for `self_signed` only: clients pin it and invite links carry it.
        fingerprint: Option<Fingerprint>,
        /// Set for `files` only: reloads the certificate on SIGHUP and file changes.
        reloader: Option<Arc<CertReloader>>,
    },
}

impl Tls {
    pub fn fingerprint(&self) -> Option<&Fingerprint> {
        match self {
            Tls::Off => None,
            Tls::Rustls { fingerprint, .. } => fingerprint.as_ref(),
        }
    }

    pub fn reloader(&self) -> Option<&Arc<CertReloader>> {
        match self {
            Tls::Off => None,
            Tls::Rustls { reloader, .. } => reloader.as_ref(),
        }
    }
}

/// SHA-256 of a certificate's DER encoding. Displays as `sha256:<64 lowercase hex digits>`,
/// the form used in invite links (`#fp=sha256:…`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub [u8; 32]);

impl Fingerprint {
    pub fn of(cert: &CertificateDer<'_>) -> Self {
        Self(Sha256::digest(cert.as_ref()).into())
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

#[derive(Debug)]
pub enum TlsError {
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    NoCertificate(PathBuf),
    NoPrivateKey(PathBuf),
    Pem {
        path: PathBuf,
        source: pem::Error,
    },
    /// One of a self-signed certificate/key pair exists without the other.
    Incomplete {
        present: PathBuf,
        missing: PathBuf,
    },
    Generate(rcgen::Error),
    /// rustls rejected the pair, e.g. because the key does not belong to the certificate.
    Rustls {
        cert: PathBuf,
        key: PathBuf,
        source: Box<rustls::Error>,
    },
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlsError::Io {
                action,
                path,
                source,
            } => write!(f, "could not {action} {}: {source}", path.display()),
            TlsError::NoCertificate(path) => {
                write!(f, "{} contains no PEM certificate", path.display())
            }
            TlsError::NoPrivateKey(path) => {
                write!(f, "{} contains no PEM private key", path.display())
            }
            TlsError::Pem { path, source } => {
                write!(f, "{} is not a valid PEM file: {source}", path.display())
            }
            TlsError::Incomplete { present, missing } => write!(
                f,
                "{} exists but {} is missing; restore it from a backup, or delete {} to \
                 generate a new certificate (clients will then warn that the fingerprint changed)",
                present.display(),
                missing.display(),
                present.display()
            ),
            TlsError::Generate(err) => {
                write!(f, "could not generate a self-signed certificate: {err}")
            }
            TlsError::Rustls { cert, key, source } => write!(
                f,
                "certificate {} and key {} cannot be used: {source}",
                cert.display(),
                key.display()
            ),
        }
    }
}

impl std::error::Error for TlsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TlsError::Io { source, .. } => Some(source),
            TlsError::Pem { source, .. } => Some(source),
            TlsError::Generate(err) => Some(err),
            TlsError::Rustls { source, .. } => Some(&**source),
            TlsError::NoCertificate(_)
            | TlsError::NoPrivateKey(_)
            | TlsError::Incomplete { .. } => None,
        }
    }
}

/// Builds the TLS setup for `config.tls`, generating the self-signed certificate on first start.
pub fn load(config: &Config) -> Result<Tls, TlsError> {
    match config.tls.mode {
        TlsMode::Off => Ok(Tls::Off),
        TlsMode::SelfSigned => {
            let dir = config.storage.data_dir.join(CERTS_DIR);
            let pair = load_or_generate(&dir, HTTPS_CERT_NAME, &self_signed_sans(config))?;
            let fingerprint = Fingerprint::of(&pair.chain[0]);
            Ok(Tls::Rustls {
                config: server_config(pair.chain, pair.key, &pair.cert_path, &pair.key_path)?,
                fingerprint: Some(fingerprint),
                reloader: None,
            })
        }
        TlsMode::Files => {
            let (cert, key) = (&config.tls.cert_file, &config.tls.key_file);
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let reloader = Arc::new(CertReloader::new(cert, key, provider.clone())?);
            let mut server = ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|source| TlsError::Rustls {
                    cert: cert.clone(),
                    key: key.clone(),
                    source: Box::new(source),
                })?
                .with_no_client_auth()
                .with_cert_resolver(reloader.clone());
            server.alpn_protocols = vec![b"http/1.1".to_vec()];
            Ok(Tls::Rustls {
                config: Arc::new(server),
                fingerprint: None,
                reloader: Some(reloader),
            })
        }
    }
}

/// rustls server config with the `ring` provider and HTTP/1.1 ALPN.
pub fn server_config(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    cert_path: &Path,
    key_path: &Path,
) -> Result<Arc<ServerConfig>, TlsError> {
    let rustls_err = |source| TlsError::Rustls {
        cert: cert_path.to_owned(),
        key: key_path.to_owned(),
        source: Box::new(source),
    };
    let mut config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(rustls_err)?
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .map_err(rustls_err)?;
    // Only HTTP/1.1: WebSocket upgrades use it, and axum is built without HTTP/2.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// A certificate chain and its key, loaded from or written to `cert_path` / `key_path`.
#[derive(Debug)]
pub struct CertPair {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

/// Loads `<dir>/<name>-cert.pem` and `<dir>/<name>-key.pem`, or generates them if neither exists.
/// `sans` (DNS names or IP addresses) are only used when generating.
pub fn load_or_generate(dir: &Path, name: &str, sans: &[String]) -> Result<CertPair, TlsError> {
    let cert_path = dir.join(format!("{name}-cert.pem"));
    let key_path = dir.join(format!("{name}-key.pem"));
    let exists = |path: &Path| {
        path.try_exists().map_err(|source| TlsError::Io {
            action: "check",
            path: path.to_owned(),
            source,
        })
    };
    let (chain, key) = match (exists(&cert_path)?, exists(&key_path)?) {
        (true, true) => (read_chain(&cert_path)?, read_key(&key_path)?),
        (false, false) => generate(&cert_path, &key_path, sans)?,
        (true, false) => {
            return Err(TlsError::Incomplete {
                present: cert_path,
                missing: key_path,
            });
        }
        (false, true) => {
            return Err(TlsError::Incomplete {
                present: key_path,
                missing: cert_path,
            });
        }
    };
    Ok(CertPair {
        chain,
        key,
        cert_path,
        key_path,
    })
}

/// SANs of the self-signed HTTPS certificate: `public_host` and the bind IP (when it is a
/// specific address). Clients pin the fingerprint, so these are informational.
pub fn self_signed_sans(config: &Config) -> Vec<String> {
    let mut sans = Vec::new();
    let host = config.instance.public_host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if !host.is_empty() {
        sans.push(host.to_owned());
    }
    let bind = config.network.bind;
    if !bind.is_unspecified() && !sans.contains(&bind.to_string()) {
        sans.push(bind.to_string());
    }
    if sans.is_empty() {
        sans.push("localhost".to_owned());
    }
    sans
}

type ChainAndKey = (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>);

fn generate(cert_path: &Path, key_path: &Path, sans: &[String]) -> Result<ChainAndKey, TlsError> {
    let key_pair = KeyPair::generate().map_err(TlsError::Generate)?;
    let mut params = CertificateParams::new(sans.to_vec()).map_err(TlsError::Generate)?;
    params
        .distinguished_name
        .push(DnType::CommonName, format!("Hermes ({})", sans.join(", ")));
    let today = days_since_epoch();
    // Start yesterday so clients with a slightly late clock accept it too.
    let (y, m, d) = civil_from_days(today - 1);
    params.not_before = rcgen::date_time_ymd(y, m, d);
    let (y, m, d) = civil_from_days(today);
    let d = if m == 2 { d.min(28) } else { d };
    params.not_after = rcgen::date_time_ymd(y + VALIDITY_YEARS, m, d);
    let cert = params.self_signed(&key_pair).map_err(TlsError::Generate)?;

    let dir = cert_path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(|source| TlsError::Io {
        action: "create",
        path: dir.to_owned(),
        source,
    })?;
    // Key first: a crash in between leaves no certificate, so no fingerprint was ever shown.
    write_file(key_path, &key_pair.serialize_pem(), true)?;
    write_file(cert_path, &cert.pem(), false)?;
    tracing::info!(
        "generated a new self-signed certificate {} for {}",
        cert_path.display(),
        sans.join(", ")
    );

    let key = PrivatePkcs8KeyDer::from(key_pair.serialize_der()).into();
    Ok((vec![cert.der().clone()], key))
}

/// Writes through a temporary file and a rename, so a crash never leaves a half-written file.
/// `private` files are readable by the owner only (on unix; on Windows the data_dir ACLs apply).
fn write_file(path: &Path, contents: &str, private: bool) -> Result<(), TlsError> {
    let tmp = path.with_extension("pem.tmp");
    let io_err = |action, path: &Path| {
        let path = path.to_owned();
        move |source| TlsError::Io {
            action,
            path,
            source,
        }
    };
    // A leftover from a crash may have other permissions; `mode` only applies on creation.
    let _ = fs::remove_file(&tmp);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options.open(&tmp).map_err(io_err("create", &tmp))?;
    file.write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(io_err("write", &tmp))?;
    drop(file);
    fs::rename(&tmp, path).map_err(io_err("write", path))
}

fn read_chain(path: &Path) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    let chain = CertificateDer::pem_file_iter(path)
        .and_then(|iter| iter.collect::<Result<Vec<_>, _>>())
        .map_err(|err| pem_error(path, err))?;
    if chain.is_empty() {
        return Err(TlsError::NoCertificate(path.to_owned()));
    }
    Ok(chain)
}

fn read_key(path: &Path) -> Result<PrivateKeyDer<'static>, TlsError> {
    PrivateKeyDer::from_pem_file(path).map_err(|err| match err {
        pem::Error::NoItemsFound => TlsError::NoPrivateKey(path.to_owned()),
        err => pem_error(path, err),
    })
}

fn pem_error(path: &Path, err: pem::Error) -> TlsError {
    match err {
        pem::Error::Io(source) => TlsError::Io {
            action: "read",
            path: path.to_owned(),
            source,
        },
        source => TlsError::Pem {
            path: path.to_owned(),
            source,
        },
    }
}

fn days_since_epoch() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    i64::try_from(secs / 86_400).unwrap_or(0)
}

/// `(year, month, day)` of a day number counted from 1970-01-01
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year as i32, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::TempDir;

    fn self_signed_config(dir: &TempDir) -> Config {
        let mut config = Config::default();
        config.instance.public_host = "hermes.example.org".into();
        config.storage.data_dir = dir.path().join("data");
        config
    }

    fn fingerprint(tls: &Tls) -> Fingerprint {
        *tls.fingerprint().expect("self_signed has a fingerprint")
    }

    /// Writes a fresh self-signed pair to `<dir>/<name>-{cert,key}.pem` without using `generate`.
    fn write_pair(dir: &Path, name: &str) -> (PathBuf, PathBuf) {
        let key = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec!["localhost".to_owned()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let (cert_path, key_path) = (
            dir.join(format!("{name}-cert.pem")),
            dir.join(format!("{name}-key.pem")),
        );
        fs::write(&cert_path, cert.pem()).unwrap();
        fs::write(&key_path, key.serialize_pem()).unwrap();
        (cert_path, key_path)
    }

    fn files_config(cert: &Path, key: &Path) -> Config {
        let mut config = Config::default();
        config.tls.mode = TlsMode::Files;
        config.tls.cert_file = cert.to_owned();
        config.tls.key_file = key.to_owned();
        config
    }

    #[test]
    fn off_has_no_tls() {
        let mut config = Config::default();
        config.tls.mode = TlsMode::Off;
        assert!(matches!(load(&config), Ok(Tls::Off)));
    }

    #[test]
    fn self_signed_is_generated_once_and_reused() {
        let dir = TempDir::new("tls-self-signed");
        let config = self_signed_config(&dir);
        let first = load(&config).unwrap();
        let certs = config.storage.data_dir.join(CERTS_DIR);
        assert!(certs.join("https-cert.pem").is_file());
        assert!(certs.join("https-key.pem").is_file());
        assert!(!certs.join("https-cert.pem.tmp").exists());

        let second = load(&config).unwrap();
        assert_eq!(fingerprint(&first), fingerprint(&second));
        let fp = fingerprint(&first).to_string();
        assert!(fp.starts_with("sha256:"));
        assert_eq!(fp.len(), "sha256:".len() + 64);
        assert!(fp[7..].chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    }

    #[test]
    fn deleting_the_pair_changes_the_fingerprint() {
        let dir = TempDir::new("tls-regenerate");
        let config = self_signed_config(&dir);
        let first = fingerprint(&load(&config).unwrap());
        fs::remove_dir_all(config.storage.data_dir.join(CERTS_DIR)).unwrap();
        assert_ne!(first, fingerprint(&load(&config).unwrap()));
    }

    #[test]
    fn half_a_pair_is_an_error_not_a_new_certificate() {
        let dir = TempDir::new("tls-incomplete");
        let config = self_signed_config(&dir);
        load(&config).unwrap();
        let certs = config.storage.data_dir.join(CERTS_DIR);
        let cert = fs::read(certs.join("https-cert.pem")).unwrap();

        fs::remove_file(certs.join("https-key.pem")).unwrap();
        let err = load(&config).unwrap_err();
        assert!(
            matches!(&err, TlsError::Incomplete { missing, .. } if missing.ends_with("https-key.pem")),
            "{err}"
        );
        // The remaining certificate is left alone.
        assert_eq!(fs::read(certs.join("https-cert.pem")).unwrap(), cert);

        fs::remove_file(certs.join("https-cert.pem")).unwrap();
        fs::write(certs.join("https-key.pem"), "x").unwrap();
        let err = load(&config).unwrap_err();
        assert!(
            matches!(&err, TlsError::Incomplete { missing, .. } if missing.ends_with("https-cert.pem")),
            "{err}"
        );
    }

    #[test]
    fn sans_are_public_host_and_specific_bind_ip() {
        let mut config = Config::default();
        config.instance.public_host = " hermes.example.org ".into();
        config.network.bind = "192.0.2.10".parse().unwrap();
        assert_eq!(
            self_signed_sans(&config),
            ["hermes.example.org", "192.0.2.10"]
        );
    }

    #[test]
    fn sans_skip_unspecified_bind_and_fall_back_to_localhost() {
        let mut config = Config::default();
        assert_eq!(self_signed_sans(&config), ["localhost"]);
        config.network.bind = "::".parse().unwrap();
        config.instance.public_host = "[2001:db8::1]".into();
        assert_eq!(self_signed_sans(&config), ["2001:db8::1"]);
        config.network.bind = "2001:db8::1".parse().unwrap();
        assert_eq!(self_signed_sans(&config), ["2001:db8::1"]);
    }

    #[cfg(unix)]
    #[test]
    fn generated_key_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("tls-perms");
        let config = self_signed_config(&dir);
        load(&config).unwrap();
        let key = config
            .storage
            .data_dir
            .join(CERTS_DIR)
            .join("https-key.pem");
        let mode = fs::metadata(key).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn files_mode_loads_the_given_pair_without_fingerprint() {
        let dir = TempDir::new("tls-files");
        let (cert, key) = write_pair(dir.path(), "own");
        let tls = load(&files_config(&cert, &key)).unwrap();
        assert!(matches!(
            tls,
            Tls::Rustls {
                fingerprint: None,
                ..
            }
        ));
    }

    #[test]
    fn files_mode_reports_missing_files() {
        let dir = TempDir::new("tls-files-missing");
        let (cert, _) = write_pair(dir.path(), "own");
        let missing = dir.path().join("nope.pem");
        let err = load(&files_config(&cert, &missing)).unwrap_err();
        assert!(
            matches!(&err, TlsError::Io { path, .. } if *path == missing),
            "{err}"
        );
        assert!(err.to_string().contains("nope.pem"), "{err}");
    }

    #[test]
    fn files_mode_rejects_files_without_pem_items() {
        let dir = TempDir::new("tls-files-garbage");
        let (cert, key) = write_pair(dir.path(), "own");
        let garbage = dir.path().join("garbage.pem");
        fs::write(&garbage, "not a certificate").unwrap();
        let err = load(&files_config(&garbage, &key)).unwrap_err();
        assert!(matches!(err, TlsError::NoCertificate(_)), "{err}");
        // A certificate is not a key.
        let err = load(&files_config(&cert, &cert)).unwrap_err();
        assert!(matches!(err, TlsError::NoPrivateKey(_)), "{err}");
    }

    #[test]
    fn files_mode_rejects_a_key_of_another_certificate() {
        let dir = TempDir::new("tls-files-mismatch");
        let (cert, _) = write_pair(dir.path(), "a");
        let (_, other_key) = write_pair(dir.path(), "b");
        let err = load(&files_config(&cert, &other_key)).unwrap_err();
        assert!(
            matches!(&err, TlsError::Rustls { source, .. }
                if matches!(**source, rustls::Error::InconsistentKeys(_))),
            "{err}"
        );
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(20_757), (2026, 10, 31));
    }
}
