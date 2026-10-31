//! Command line interface (PLAN.md section 9.1).

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use hermes_proto::id::Id;

use crate::config::{self, Config, LogConfig, LogFormat, TlsMode};
use crate::state::AppState;
use crate::time::{SECOND_MS, now_ms};
use crate::tls::{self, Tls};
use crate::{api, invites, setup};

#[derive(Debug, Parser)]
#[command(name = "hermes-server", version, about = "Hermes chat server")]
pub struct Cli {
    /// Path to the configuration file.
    #[arg(
        long,
        global = true,
        env = "HERMES_CONFIG",
        default_value = "./hermes.toml"
    )]
    pub config: PathBuf,

    /// Defaults to `run`.
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the server.
    Run,
    /// Write a commented default config file and exit.
    Init,
    /// Validate the config and print the effective values.
    CheckConfig,
    /// Manage invites.
    #[command(subcommand)]
    Invite(InviteCommand),
}

#[derive(Debug, Subcommand)]
pub enum InviteCommand {
    /// Create an invite and print its link. Without `--space` it only allows creating an
    /// account (and, if nobody has registered yet, the first account becomes the instance admin).
    Create {
        /// ID of the space the invite joins.
        #[arg(long)]
        space: Option<Id>,
        /// How many times it can be used; unlimited if omitted.
        #[arg(long)]
        uses: Option<u32>,
        /// Lifetime: a number with s, m, h or d (e.g. `30m`, `7d`), or `never`.
        #[arg(long, default_value = "7d")]
        expires: Lifetime,
    },
}

/// `--expires` of `invite create`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    Never,
    Secs(u64),
}

impl FromStr for Lifetime {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("never") {
            return Ok(Lifetime::Never);
        }
        let err = || format!("{s:?} is not a lifetime like 30m, 24h, 7d or never");
        let unit = s.chars().last().ok_or_else(err)?;
        let factor = match unit {
            's' => 1,
            'm' => 60,
            'h' => 60 * 60,
            'd' => 24 * 60 * 60,
            _ => return Err(err()),
        };
        let n: u64 = s[..s.len() - 1].parse().map_err(|_| err())?;
        // Capped at 100 years, so it always fits into the millisecond timestamps.
        match n.checked_mul(factor) {
            Some(secs) if secs > 0 && secs <= 100 * 365 * 24 * 60 * 60 => Ok(Lifetime::Secs(secs)),
            _ => Err(err()),
        }
    }
}

impl fmt::Display for Lifetime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Lifetime::Never => f.write_str("never"),
            Lifetime::Secs(secs) => {
                for (unit, size) in [("d", 86_400), ("h", 3600), ("m", 60)] {
                    if secs % size == 0 {
                        return write!(f, "{}{unit}", secs / size);
                    }
                }
                write!(f, "{secs}s")
            }
        }
    }
}

/// Entry point of the `hermes-server` binary.
pub fn run() -> ExitCode {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Run) {
        Command::Run => run_server(&cli.config),
        Command::Init => init(&cli.config),
        Command::CheckConfig => check_config(&cli.config),
        Command::Invite(InviteCommand::Create {
            space,
            uses,
            expires,
        }) => invite_create(&cli.config, space, uses, expires),
    }
}

fn run_server(path: &Path) -> ExitCode {
    let Some(loaded) = load_or_report(path) else {
        return ExitCode::FAILURE;
    };
    init_logging(&loaded.config.log);
    if !loaded.file_found {
        tracing::info!(
            "config file {} not found, using defaults and environment variables",
            path.display()
        );
    }
    for warning in &loaded.warnings {
        tracing::warn!("{warning}");
    }
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::error!("could not start the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(serve(loaded.config))
}

async fn serve(config: Config) -> ExitCode {
    let Some(tls) = load_tls(&config) else {
        return ExitCode::FAILURE;
    };
    let addr = SocketAddr::new(config.network.bind, config.network.port);
    let Some(state) = open_state(config).await else {
        return ExitCode::FAILURE;
    };
    if let Err(err) = setup::first_start(&state, tls.fingerprint()).await {
        tracing::error!("could not create the setup invite: {err}");
        return ExitCode::FAILURE;
    }
    let tcp = match tokio::net::TcpListener::bind(addr).await {
        Ok(tcp) => tcp,
        Err(err) => {
            tracing::error!("could not listen on {addr}: {err}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!("listening on {addr}");
    // Graceful shutdown, `/healthz` and the "Hermes is ready" line come with their own
    // PLAN.md section 20 item.
    let state = Arc::new(state);
    tokio::spawn(crate::files::cleanup_task(state.clone()));
    let result = api::serve(tcp, &tls, api::router(state.clone())).await;
    state.db.close().await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("server error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Opens the database and loads the cache, logging what was loaded.
async fn open_state(config: Config) -> Option<AppState> {
    let data_dir = config.storage.data_dir.clone();
    let state = match AppState::load(config).await {
        Ok(state) => state,
        Err(err) => {
            tracing::error!(
                "could not open the database in {}: {err}",
                data_dir.display()
            );
            return None;
        }
    };
    {
        let cache = state.cache();
        tracing::info!(
            users = cache.users.len(),
            spaces = cache.spaces.len(),
            channels = cache.channels.len(),
            "database {} loaded",
            data_dir.join(crate::db::DB_FILE).display()
        );
    }
    Some(state)
}

fn invite_create(path: &Path, space: Option<Id>, uses: Option<u32>, expires: Lifetime) -> ExitCode {
    let Some(loaded) = load_or_report(path) else {
        return ExitCode::FAILURE;
    };
    let config = loaded.config;
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("Could not start the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async move {
        // Only a self-signed certificate puts a fingerprint into the link.
        let fingerprint = match config.tls.mode {
            TlsMode::SelfSigned => match tls::load(&config) {
                Ok(tls) => tls.fingerprint().copied(),
                Err(err) => {
                    eprintln!("TLS setup failed: {err}");
                    return ExitCode::FAILURE;
                }
            },
            TlsMode::Files | TlsMode::Off => None,
        };
        let data_dir = config.storage.data_dir.clone();
        let state = match AppState::load(config).await {
            Ok(state) => state,
            Err(err) => {
                eprintln!(
                    "Could not open the database in {}: {err}",
                    data_dir.display()
                );
                return ExitCode::FAILURE;
            }
        };
        let space_name = match space {
            Some(id) => {
                let name = state.cache().spaces.get(&id).map(|s| s.space.name.clone());
                if name.is_none() {
                    eprintln!("There is no space with ID {id}");
                    state.db.close().await;
                    return ExitCode::FAILURE;
                }
                name
            }
            None => None,
        };
        let now = now_ms();
        let expires_at = match expires {
            Lifetime::Never => None,
            // `Lifetime` caps the value at 100 years, so this fits into i64.
            Lifetime::Secs(secs) => Some(now + secs as i64 * SECOND_MS),
        };
        let max_uses = uses.filter(|&n| n > 0);
        let result = invites::create(&state.db.write, space, None, max_uses, expires_at, now).await;
        state.db.close().await;
        let invite = match result {
            Ok(invite) => invite,
            Err(err) => {
                eprintln!("Could not create the invite: {err}");
                return ExitCode::FAILURE;
            }
        };
        let links = invites::links(&state.config, fingerprint.as_ref(), &invite.code);
        match space_name {
            Some(name) => println!("Invite {} for the space {name:?}", invite.code),
            None => println!(
                "Invite {} (creates an account, joins no space)",
                invite.code
            ),
        }
        match max_uses {
            Some(n) => println!("Uses: {n}"),
            None => println!("Uses: unlimited"),
        }
        match expires {
            Lifetime::Never => println!("Expires: never"),
            lifetime => println!("Expires: in {lifetime}"),
        }
        println!("\n{}\n{}", links.web, links.app);
        ExitCode::SUCCESS
    })
}

/// Loads (or on first start generates) the TLS certificate and logs what is used.
fn load_tls(config: &Config) -> Option<Tls> {
    let tls = match tls::load(config) {
        Ok(tls) => tls,
        Err(err) => {
            tracing::error!("TLS setup failed: {err}");
            return None;
        }
    };
    match config.tls.mode {
        TlsMode::SelfSigned => tracing::info!(
            "HTTPS uses the self-signed certificate in {}, fingerprint {}",
            config.storage.data_dir.join(tls::CERTS_DIR).display(),
            tls.fingerprint().expect("self_signed has a fingerprint")
        ),
        TlsMode::Files => tracing::info!(
            "HTTPS uses the certificate {}",
            config.tls.cert_file.display()
        ),
        TlsMode::Off => tracing::warn!(
            "tls.mode = \"off\": serving plain HTTP; only do this behind a TLS-terminating \
             reverse proxy"
        ),
    }
    Some(tls)
}

fn init(path: &Path) -> ExitCode {
    match write_template(path) {
        Ok(()) => {
            println!("Wrote the default configuration to {}", path.display());
            println!(
                "Set instance.public_host, then check it with `hermes-server check-config --config {}`",
                path.display()
            );
            ExitCode::SUCCESS
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            eprintln!(
                "{} already exists; not overwriting it. Delete it first to start over.",
                path.display()
            );
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("Could not write {}: {err}", path.display());
            ExitCode::FAILURE
        }
    }
}

fn write_template(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    // `create_new` fails if the file exists, without a race between checking and writing.
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(config::DEFAULT_TEMPLATE.as_bytes())
}

fn check_config(path: &Path) -> ExitCode {
    let Some(loaded) = load_or_report(path) else {
        return ExitCode::FAILURE;
    };
    if loaded.file_found {
        println!("# Config file: {}", path.display());
    } else {
        println!(
            "# Config file: {} (not found, using defaults and environment variables)",
            path.display()
        );
    }
    println!("# Effective values (file + environment):\n");
    print!("{}", effective_toml(&loaded.config));
    if !loaded.warnings.is_empty() {
        eprintln!("\nWarnings:");
        for warning in &loaded.warnings {
            eprintln!("- {warning}");
        }
    }
    eprintln!("\nThe configuration is valid.");
    ExitCode::SUCCESS
}

fn effective_toml(config: &Config) -> String {
    toml::to_string_pretty(config).expect("Config always serializes to TOML")
}

/// Loads the config, printing every error to stderr on failure.
fn load_or_report(path: &Path) -> Option<config::Loaded> {
    match config::load(path) {
        Ok(loaded) => Some(loaded),
        Err(err) => {
            eprintln!("Invalid configuration ({}):\n{err}", path.display());
            None
        }
    }
}

fn init_logging(log: &LogConfig) {
    let filter = tracing_subscriber::EnvFilter::try_new(&log.level)
        .expect("log.level is checked by config validation");
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match log.format {
        LogFormat::Pretty => builder.init(),
        LogFormat::Json => builder.json().init(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifetimes() {
        assert_eq!("never".parse(), Ok(Lifetime::Never));
        assert_eq!("7d".parse(), Ok(Lifetime::Secs(7 * 86_400)));
        assert_eq!("90m".parse(), Ok(Lifetime::Secs(5400)));
        assert_eq!("45s".parse(), Ok(Lifetime::Secs(45)));
        for bad in ["", "d", "0h", "-1d", "7w", "1.5h", "99999999999d"] {
            assert!(bad.parse::<Lifetime>().is_err(), "{bad}");
        }
        assert_eq!(Lifetime::Secs(7 * 86_400).to_string(), "7d");
        assert_eq!(Lifetime::Secs(5400).to_string(), "90m");
        assert_eq!(Lifetime::Secs(45).to_string(), "45s");
    }
}
