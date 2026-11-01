//! Hermes terminal client (ratatui), PLAN.md section 16.

mod app;
mod commands;
mod event;
mod input;
mod keymap;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_voice;
mod ui;
mod voice;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;

use clap::Parser;
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture,
};
use crossterm::execute;
use hermes_client_core::config::{ClientConfig, ConfigPaths};
use hermes_client_core::{Core, CoreOptions, time};

#[derive(Debug, Parser)]
#[command(name = "hermes-tui", version, about = "Hermes terminal client")]
struct Cli {
    /// Directory with config.toml, known_hosts.toml and logs (default: the platform config
    /// directory, or the executable's directory in portable mode).
    #[arg(long)]
    config_dir: Option<PathBuf>,
    /// An invite link or address to add on start (also `hermes://` links).
    link: Option<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let paths = match cli.config_dir {
        Some(dir) => ConfigPaths::in_dir(dir),
        None => match ConfigPaths::detect() {
            Some(paths) => paths,
            None => {
                eprintln!("Could not find a config directory; pass --config-dir");
                return ExitCode::FAILURE;
            }
        },
    };
    let loaded = match ClientConfig::load(&paths.config()) {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("Invalid config: {err}");
            return ExitCode::FAILURE;
        }
    };
    init_logging(&paths);
    for warning in &loaded.warnings {
        tracing::warn!("{warning}");
    }
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(err) => {
            eprintln!("Could not start the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    let utc_offset = time::local_offset_secs();
    let result = runtime.block_on(async move {
        let options = CoreOptions::with_paths("hermes-tui", paths)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let (core, events) = Core::new(loaded.config, options);
        if let Err(err) = core.watch_config() {
            tracing::warn!("config hot reload unavailable: {err}");
        }
        core.start();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = app::App::new(core, tx, utc_offset);
        if let Some(warning) = loaded.warnings.first() {
            app.error(format!("config: {warning}"));
        }
        if let Some(link) = cli.link {
            app.run(commands::Command::AddInstance(link));
        }
        // `ratatui::init` also restores the terminal when a panic happens; the crash log hook
        // runs that first.
        let mut terminal = ratatui::init();
        if let Some(paths) = app.core.paths() {
            hermes_client_core::crash::install(paths.dir.join("logs"), "hermes-tui");
        }
        let _ = execute!(
            std::io::stdout(),
            EnableMouseCapture,
            EnableFocusChange,
            EnableBracketedPaste
        );
        let result = event::run(&mut terminal, &mut app, events, rx).await;
        let _ = execute!(
            std::io::stdout(),
            DisableMouseCapture,
            DisableFocusChange,
            DisableBracketedPaste
        );
        ratatui::restore();
        // Leave voice properly, so others do not see you until the connection times out.
        if app.core.voice_channel().is_some() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), app.core.leave_voice())
                .await;
        }
        result
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("hermes-tui: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Logs go to `<config dir>/logs/hermes-tui.log`; the terminal is busy with the UI.
fn init_logging(paths: &ConfigPaths) {
    let dir = paths.dir.join("logs");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("hermes-tui.log"))
    else {
        return;
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("HERMES_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(Mutex::new(file))
        .init();
}
