//! Command line interface (PLAN.md section 9.1).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::config::{self, Config, LogConfig, LogFormat};

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
}

/// Entry point of the `hermes-server` binary.
pub fn run() -> ExitCode {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Run) {
        Command::Run => run_server(&cli.config),
        Command::Init => init(&cli.config),
        Command::CheckConfig => check_config(&cli.config),
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
    // The server itself is built in the next PLAN.md section 20 items.
    tracing::error!("the server is not implemented yet");
    ExitCode::FAILURE
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
