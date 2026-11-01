//! Certificate reload for `tls.mode = "files"` (PLAN.md section 9.1): a rustls
//! [`ResolvesServerCert`] backed by an [`ArcSwap`], so a renewed certificate is used by new
//! handshakes while open connections keep going.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use rustls::crypto::CryptoProvider;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

use super::{Fingerprint, TlsError, read_chain, read_key};

/// How often the files' modification times are checked.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Serves the current certificate of `cert_file` / `key_file` and swaps in a new one on reload.
pub struct CertReloader {
    cert_path: PathBuf,
    key_path: PathBuf,
    provider: Arc<CryptoProvider>,
    current: ArcSwap<CertifiedKey>,
    /// Modification times of the pair currently served.
    mtimes: Mutex<(Option<SystemTime>, Option<SystemTime>)>,
}

impl fmt::Debug for CertReloader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CertReloader")
            .field("cert_path", &self.cert_path)
            .field("key_path", &self.key_path)
            .field("fingerprint", &self.fingerprint().to_string())
            .finish_non_exhaustive()
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl CertReloader {
    /// Loads the pair; fails like the server's start would.
    pub fn new(
        cert_path: &Path,
        key_path: &Path,
        provider: Arc<CryptoProvider>,
    ) -> Result<Self, TlsError> {
        let mtimes = (mtime(cert_path), mtime(key_path));
        let key = load(cert_path, key_path, &provider)?;
        Ok(Self {
            cert_path: cert_path.to_owned(),
            key_path: key_path.to_owned(),
            provider,
            current: ArcSwap::from_pointee(key),
            mtimes: Mutex::new(mtimes),
        })
    }

    /// Fingerprint of the leaf certificate currently served.
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(&self.current.load().cert[0])
    }

    /// Reads both files again and serves them from now on. On error the old pair stays.
    pub fn reload(&self) -> Result<(), TlsError> {
        let mtimes = (mtime(&self.cert_path), mtime(&self.key_path));
        let key = load(&self.cert_path, &self.key_path, &self.provider)?;
        self.current.store(Arc::new(key));
        *self.mtimes.lock().unwrap_or_else(|e| e.into_inner()) = mtimes;
        Ok(())
    }

    /// [`reload`](Self::reload) if either file's modification time changed since the last
    /// (attempted) load. `Ok(true)` when a new pair was loaded.
    pub fn reload_if_changed(&self) -> Result<bool, TlsError> {
        let now = (mtime(&self.cert_path), mtime(&self.key_path));
        {
            let mut seen = self.mtimes.lock().unwrap_or_else(|e| e.into_inner());
            if *seen == now {
                return Ok(false);
            }
            // Remember the attempt, so a broken pair is reported once, not every minute.
            *seen = now;
        }
        self.reload().map(|()| true)
    }

    /// Logs the outcome of a reload.
    fn log(&self, result: Result<bool, TlsError>, why: &str) {
        match result {
            Ok(true) => tracing::info!(
                "{why}: reloaded the TLS certificate {} ({})",
                self.cert_path.display(),
                self.fingerprint()
            ),
            Ok(false) => {}
            Err(err) => tracing::error!(
                "{why}: could not reload the TLS certificate, still using the old one: {err}"
            ),
        }
    }

    /// Reloads on SIGHUP (unix) and when the files change (checked every [`CHECK_INTERVAL`]),
    /// forever. The SIGHUP handler is installed when this is called, not when the returned
    /// future first runs, so a SIGHUP right after startup cannot hit the default action
    /// (terminating the process). Must be called inside a tokio runtime.
    // Not an `async fn`: the handler must be installed before the first poll.
    #[allow(clippy::manual_async_fn)]
    pub fn watch(self: Arc<Self>) -> impl std::future::Future<Output = ()> + Send + 'static {
        #[cfg(unix)]
        let mut hangup = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        {
            Ok(signal) => Some(signal),
            Err(err) => {
                tracing::error!("cannot listen for SIGHUP: {err}");
                None
            }
        };
        async move {
            let mut interval = tokio::time::interval(CHECK_INTERVAL);
            interval.tick().await;
            self.watch_loop(
                &mut interval,
                #[cfg(unix)]
                &mut hangup,
            )
            .await;
        }
    }

    async fn watch_loop(
        &self,
        interval: &mut tokio::time::Interval,
        #[cfg(unix)] hangup: &mut Option<tokio::signal::unix::Signal>,
    ) {
        loop {
            #[cfg(unix)]
            let sighup = async {
                match hangup.as_mut() {
                    Some(signal) => {
                        signal.recv().await;
                    }
                    None => std::future::pending().await,
                }
            };
            #[cfg(not(unix))]
            let sighup = std::future::pending::<()>();
            tokio::select! {
                _ = interval.tick() => self.log(self.reload_if_changed(), "file change"),
                () = sighup => self.log(self.reload().map(|()| true), "SIGHUP"),
            }
        }
    }
}

impl ResolvesServerCert for CertReloader {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current.load_full())
    }
}

fn load(
    cert_path: &Path,
    key_path: &Path,
    provider: &CryptoProvider,
) -> Result<CertifiedKey, TlsError> {
    let chain = read_chain(cert_path)?;
    let key = read_key(key_path)?;
    CertifiedKey::from_der(chain, key, provider).map_err(|source| TlsError::Rustls {
        cert: cert_path.to_owned(),
        key: key_path.to_owned(),
        source: Box::new(source),
    })
}
