//! The Hermes desktop client, on gpui.
//!
//! One root view, [`shell::Shell`], owns the state; its methods live in the module of the feature
//! they belong to. Everything async runs on a tokio runtime owned by `main` (the core needs one,
//! and iced no longer provides it), and comes back to the UI through `Shell::run`.

// No console window next to the GUI in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod actions;
mod add_instance;
mod assets;
mod channel;
mod collapsed;
mod command_bar;
mod desktop;
mod desktop_shell;
mod drag;
mod encryption;
#[cfg(test)]
mod fixtures;
mod form;
mod friends;
mod images;
mod keybinds;
mod keys;
mod markup;
mod members;
mod menu;
mod menu_entries;
mod modal;
mod new_dm;
mod open;
mod ptt;
mod results;
mod rows;
mod screen;
mod settings;
mod shell;
mod sidebar;
mod single_instance;
mod space_settings;
#[cfg(test)]
mod tests;
mod theme;
mod tree;
mod voice;
mod watchdog;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;

use clap::Parser;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use hermes_client_core::config::{ClientConfig, ConfigPaths};
use hermes_client_core::{Core, CoreOptions};

use crate::shell::{Boot, Shell};
use crate::single_instance::Endpoint;

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

impl Cli {
    /// The command line. Started for a `hermes://` link (by a browser, through the protocol
    /// handler), only the link counts: the rest of that command line comes from the link's
    /// author, so a crafted link that smuggles in flags (`--config-dir \\host\share`) changes
    /// nothing.
    fn from_args() -> Cli {
        Cli::from_raw(std::env::args_os().collect())
    }

    fn from_raw(args: Vec<std::ffi::OsString>) -> Cli {
        let link = args.iter().skip(1).map(|a| a.to_string_lossy()).find(|a| {
            a.get(..7)
                .is_some_and(|s| s.eq_ignore_ascii_case("hermes:"))
        });
        match link {
            Some(link) => Cli {
                config_dir: None,
                link: Some(link.into_owned()),
            },
            None => Cli::parse_from(args),
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::from_args();
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
    watchdog::start(paths.dir.join("logs"));
    for warning in &loaded.warnings {
        tracing::warn!("{warning}");
    }
    let options = match CoreOptions::with_paths("hermes-gui", paths) {
        Ok(options) => options,
        Err(err) => return fail(&format!("Could not load known hosts: {err}")),
    };

    // The core spawns tokio tasks, also from the UI thread: the guard makes this thread count as
    // inside the runtime for as long as the app runs.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("hermes-rt")
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => return fail(&format!("Could not start the runtime: {err}")),
    };
    let _guard = runtime.enter();
    let handle = runtime.handle().clone();
    let warning = loaded.warnings.first().cloned();
    let loaded_start_minimized = loaded.config.general.start_minimized;
    let (core, events) = Core::new(loaded.config, options);
    if let Err(err) = core.watch_config() {
        tracing::warn!("config hot reload unavailable: {err}");
    }
    core.start();

    let link = cli.link;
    let start_minimized = loaded_start_minimized;
    gpui_kit::application()
        .with_assets(assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            assets::load_fonts(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            let theme = Theme::global_mut(cx);
            theme.font_family = theme::FONT.into();
            theme.font_size = theme::body_size();
            cx.on_window_closed(|cx, _| cx.quit()).detach();

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1280.), px(800.)),
                    cx,
                ))),
                window_min_size: Some(size(px(theme::WINDOW_MIN_W), px(theme::WINDOW_MIN_H))),
                // The native title bar, which shows the space and page (`Shell::page_title`).
                titlebar: Some(TitlebarOptions {
                    title: Some("Hermes".into()),
                    appears_transparent: false,
                    ..Default::default()
                }),
                ..Default::default()
            };
            // The tray and the global keys live on this (the event loop's) thread.
            let tray = match desktop::tray::Tray::new() {
                Ok(tray) => Some(tray),
                Err(err) => {
                    tracing::warn!("no tray icon: {err}");
                    None
                }
            };
            let global_keys = match ptt::GlobalKeys::new() {
                Ok(keys) => Some(keys),
                Err(err) => {
                    tracing::warn!("no global keys: {err}");
                    None
                }
            };
            let start_hidden = start_minimized && tray.is_some();
            let boot = Boot {
                tray,
                global_keys,
                start_hidden,
                core,
                events,
                runtime: handle,
                endpoint,
                link,
                warning,
            };
            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| Shell::new(boot, window, cx))
            })
            .expect("failed to open the window");
            cx.activate(true);
        });
    ExitCode::SUCCESS
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

/// Logs go to `<config dir>/logs/latest.log`, older ones to `logs/logs_old/` (see
/// `hermes_client_core::logs`; `HERMES_LOG` sets the filter).
fn init_logging(paths: &ConfigPaths) {
    let dir = paths.dir.join("logs");
    hermes_client_core::crash::install(dir.clone(), "hermes");
    let Ok(file) = hermes_client_core::logs::DailyLog::open(&dir, "hermes") else {
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

#[cfg(test)]
mod cli_tests {
    use super::Cli;

    fn parse(args: &[&str]) -> Cli {
        Cli::from_raw(args.iter().map(Into::into).collect())
    }

    #[test]
    fn a_hermes_link_ignores_every_other_argument() {
        let cli = parse(&[
            "hermes.exe",
            "--config-dir",
            r"\\evil\share",
            "HERMES://example.org/invite/abc",
        ]);
        assert_eq!(cli.config_dir, None);
        assert_eq!(cli.link.as_deref(), Some("HERMES://example.org/invite/abc"));

        let cli = parse(&["hermes.exe", "--config-dir", "dev", "https://h/invite/x"]);
        assert_eq!(cli.config_dir, Some("dev".into()));
        assert_eq!(cli.link.as_deref(), Some("https://h/invite/x"));
    }
}
