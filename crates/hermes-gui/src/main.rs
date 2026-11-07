//! Hermes desktop client (iced), PLAN.md section 15.

// No console window next to the GUI in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod buffer;
mod desktop;
mod drag;
#[cfg(test)]
mod fixtures;
mod font;
mod keybinds;
mod message;
mod modal;
mod open;
mod pane;
mod panes;
mod ptt;
mod resize;
mod screen;
mod sidebar;
mod single_instance;
mod subscription;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_voice;
mod theme;
mod voice;
mod watchdog;
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
    prefer_direct3d12();
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
    let fonts = font::Fonts::from_config(&loaded.config.font);

    // The tray lives on this (the event loop's) thread; without it, closing quits.
    let tray = match desktop::tray::Tray::new() {
        Ok(tray) => Some(tray),
        Err(err) => {
            tracing::warn!("no tray icon: {err}");
            None
        }
    };
    // The global mute key also needs this thread (section 15.6).
    let global_keys = match ptt::GlobalKeys::new() {
        Ok(keys) => Some(keys),
        Err(err) => {
            tracing::warn!("no global keys: {err}");
            None
        }
    };
    let start_hidden = loaded.config.general.start_minimized && tray.is_some();
    let window_icon =
        iced::window::icon::from_rgba(desktop::tray::icon_rgba(64, false), 64, 64).ok();

    // `boot` runs once, inside iced's tokio runtime, which the core needs.
    let boot_state = std::cell::RefCell::new(Some((loaded, options, cli.link, tray, global_keys)));
    let result = iced::application(
        move || {
            let (loaded, options, link, tray, global_keys) =
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
            app.global_keys = global_keys;
            app.register_global_keys();
            app.window_visible = !start_hidden;
            if let Some(warning) = loaded.warnings.into_iter().next() {
                app.status = Some(app::Status {
                    text: format!("config: {warning}"),
                    level: app::Level::Warning,
                    since: iced::time::Instant::now(),
                });
            }
            let open = match link {
                Some(link) => app.open_link(link),
                None => Task::none(),
            };
            let restored = app.open_restored();
            let update = app.check_for_update();
            // The display's scale, for avatars (later changes arrive as `Rescaled` events).
            let scale = iced::window::oldest().then(|id| match id {
                Some(id) => iced::window::scale_factor(id).map(message::Message::WindowRescaled),
                None => Task::none(),
            });
            (
                app,
                Task::batch([app::load_fonts().chain(open), restored, update, scale]),
            )
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

/// On Windows, draws with Direct3D 12 unless `WGPU_BACKEND` says otherwise. Left to itself, wgpu
/// picks Vulkan first, and the Vulkan drivers of older (Intel) GPUs hang the window; Direct3D 12
/// is the API every Windows 10/11 GPU driver is built and tested for. Without a Direct3D 12 GPU,
/// iced falls back to drawing in software.
fn prefer_direct3d12() {
    #[cfg(windows)]
    if std::env::var_os("WGPU_BACKEND").is_none() {
        // SAFETY: the first thing `main` does, before any other thread exists.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("WGPU_BACKEND", "dx12");
        }
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

/// Logs go to `<config dir>/logs/latest.log`, older ones to `logs/logs_old/` (see
/// `hermes_client_core::logs`; `HERMES_LOG` sets the filter).
fn init_logging(paths: &ConfigPaths) {
    let dir = paths.dir.join("logs");
    hermes_client_core::crash::install(dir.clone(), "hermes");
    let Ok(file) = hermes_client_core::logs::DailyLog::open(&dir, "hermes") else {
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
