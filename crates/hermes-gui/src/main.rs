//! Hermes desktop client (iced), PLAN.md section 15.

// No console window next to the GUI in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod buffer;
mod desktop;
#[cfg(test)]
mod fixtures;
mod font;
mod keybinds;
mod message;
mod modal;
mod open;
mod pane;
mod panes;
mod sidebar;
mod single_instance;
mod subscription;
#[cfg(test)]
mod tests;
mod theme;
mod widget;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;

use clap::Parser;
use hermes_client_core::config::{ClientConfig, ConfigPaths};
use hermes_client_core::{Core, CoreOptions};
use iced::Task;

use crate::app::App;
use crate::single_instance::Endpoint;
use crate::subscription::CoreEventsHandle;

#[derive(Debug, Parser)]
#[command(name = "hermes", version, about = "Hermes desktop client")]
struct Cli {
    /// Directory with config.toml, known_hosts.toml, themes and logs (default: the platform
    /// config directory, or the executable's directory in portable mode).
    #[arg(long)]
    config_dir: Option<PathBuf>,
    /// An invite link or address to add (also `hermes://` links). If Hermes is already running,
    /// the link is handed to it.
    link: Option<String>,
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
    // Single instance (section 15.7): hand the link to a running client and exit.
    let endpoint = Endpoint::for_config_dir(&paths.dir);
    if forward_to_running(&endpoint, cli.link.as_deref()) {
        return ExitCode::SUCCESS;
    }
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

    // The tray lives on this (the event loop's) thread; without it, closing quits.
    let tray = match desktop::tray::Tray::new() {
        Ok(tray) => Some(tray),
        Err(err) => {
            tracing::warn!("no tray icon: {err}");
            None
        }
    };
    let start_hidden = loaded.config.general.start_minimized && tray.is_some();
    let window_icon =
        iced::window::icon::from_rgba(desktop::tray::icon_rgba(64, false), 64, 64).ok();

    // `boot` runs once, inside iced's tokio runtime, which the core needs.
    let boot_state = std::cell::RefCell::new(Some((loaded, options, cli.link, tray)));
    let result = iced::application(
        move || {
            let (loaded, options, link, tray) =
                boot_state.borrow_mut().take().expect("boot runs once");
            let (core, events) = Core::new(loaded.config, options);
            if let Err(err) = core.watch_config() {
                tracing::warn!("config hot reload unavailable: {err}");
            }
            core.start();
            let mut app = App::new(
                core,
                Some(CoreEventsHandle::new(events)),
                Some(endpoint.clone()),
            );
            app.tray = tray;
            app.window_visible = !start_hidden;
            if let Some(warning) = loaded.warnings.into_iter().next() {
                app.status = Some(app::Status {
                    text: format!("config: {warning}"),
                    level: app::Level::Warning,
                });
            }
            let open = match link {
                Some(link) => app.open_link(link),
                None => Task::none(),
            };
            let restored = app.open_restored();
            (app, Task::batch([app::load_fonts().chain(open), restored]))
        },
        App::update,
        App::view,
    )
    .title(App::title)
    .theme(App::theme)
    .scale_factor(App::scale_factor)
    .subscription(App::subscription)
    .default_font(fonts.ui)
    .window(iced::window::Settings {
        size: iced::Size::new(1200.0, 800.0),
        visible: !start_hidden,
        icon: window_icon,
        exit_on_close_request: false,
        ..iced::window::Settings::default()
    })
    .run();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => fail(&format!("hermes: {err}")),
    }
}

/// `true` if another client for this config directory is running and took `link`.
fn forward_to_running(endpoint: &Endpoint, link: Option<&str>) -> bool {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return false;
    };
    runtime
        .block_on(single_instance::forward(endpoint, link))
        .is_ok()
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
