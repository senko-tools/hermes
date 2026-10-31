//! Hermes desktop client (iced), PLAN.md section 15.

// No console window next to the GUI in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod buffer;
mod font;
mod message;
mod sidebar;
mod subscription;
mod theme;
mod widget;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;

use clap::Parser;
use hermes_client_core::config::{ClientConfig, ConfigPaths};
use hermes_client_core::{Core, CoreOptions};

use crate::app::App;
use crate::subscription::CoreEventsHandle;

#[derive(Debug, Parser)]
#[command(name = "hermes", version, about = "Hermes desktop client")]
struct Cli {
    /// Directory with config.toml, known_hosts.toml, themes and logs (default: the platform
    /// config directory, or the executable's directory in portable mode).
    #[arg(long)]
    config_dir: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let paths = match cli.config_dir {
        Some(dir) => ConfigPaths::in_dir(dir),
        None => match ConfigPaths::detect() {
            Some(paths) => paths,
            None => return fail("Could not find a config directory; pass --config-dir"),
        },
    };
    let loaded = match ClientConfig::load(&paths.config()) {
        Ok(loaded) => loaded,
        Err(err) => return fail(&format!("Invalid config: {err}")),
    };
    init_logging(&paths);
    for warning in &loaded.warnings {
        tracing::warn!("{warning}");
    }
    let options = match CoreOptions::with_paths("hermes-gui", paths) {
        Ok(options) => options,
        Err(err) => return fail(&format!("Could not load known hosts: {err}")),
    };
    let fonts = font::Fonts::from_config(&loaded.config.font);

    // `boot` runs once, inside iced's tokio runtime, which the core needs.
    let boot_state = Mutex::new(Some((loaded, options)));
    let result = iced::application(
        move || {
            let (loaded, options) = boot_state
                .lock()
                .expect("boot lock")
                .take()
                .expect("boot runs once");
            let (core, events) = Core::new(loaded.config, options);
            if let Err(err) = core.watch_config() {
                tracing::warn!("config hot reload unavailable: {err}");
            }
            core.start();
            let mut app = App::new(core, Some(CoreEventsHandle::new(events)));
            if let Some(warning) = loaded.warnings.into_iter().next() {
                app.status = Some(app::Status {
                    text: format!("config: {warning}"),
                    error: false,
                });
            }
            (app, app::load_fonts())
        },
        App::update,
        App::view,
    )
    .title(App::title)
    .theme(App::theme)
    .scale_factor(App::scale_factor)
    .subscription(App::subscription)
    .default_font(fonts.ui)
    .window_size((1200.0, 800.0))
    .run();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => fail(&format!("hermes: {err}")),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    tracing::error!("{message}");
    ExitCode::FAILURE
}

/// Logs go to `<config dir>/logs/hermes.log` (`HERMES_LOG` sets the filter).
fn init_logging(paths: &ConfigPaths) {
    let dir = paths.dir.join("logs");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("hermes.log"))
    else {
        return;
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("HERMES_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,wgpu=warn,naga=warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(Mutex::new(file))
        .init();
}
