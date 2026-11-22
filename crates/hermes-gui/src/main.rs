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
mod crop;
mod desktop;
mod desktop_shell;
mod diagnostics;
mod drag;
mod emoji_picker;
mod encryption;
mod form;
mod friends;
mod hover;
mod images;
mod key_settings;
mod keybinds;
mod keys;
mod link_card;
mod markup;
mod media;
mod members;
mod menu;
use hermes_client_core::menus as menu_entries;
mod modal;
mod new_dm;
mod notice;
mod open;
mod overlay;
mod ptt;
mod results;
mod rows;
mod screen;
mod selection;
mod settings;
mod share_picker;
mod shell;
mod sidebar;
mod single_instance;
mod smooth_scroll;
mod space_settings;
mod syntax;
#[cfg(test)]
mod tests;
mod theme;
mod time_codes;
mod tree;
mod ui;
mod unencrypted;
mod update_notes;
mod updater_window;
mod user_menu;
mod voice;
mod watchdog;
mod webp_anim;

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
    /// Started by the updater: the old version may still be closing.
    #[arg(long, hide = true)]
    after_update: bool,
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
                after_update: false,
            },
            None => Cli::parse_from(args),
        }
    }
}

/// Hidden: deletes every secret Hermes keeps in the credential store, then exits (the uninstaller's
/// "delete my data").
const FORGET_SECRETS_FLAG: &str = "--forget-secrets";

fn main() -> ExitCode {
    let launched = std::time::Instant::now();
    // The old version, renamed, watching the new one come up after an update.
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if args
        .first()
        .is_some_and(|a| a == hermes_client_core::updater::WATCH_FLAG)
    {
        hermes_client_core::updater::watch(&args[1..]);
        return ExitCode::SUCCESS;
    }
    // Run by the uninstaller when the user deletes their data: Hermes' sign-ins and keys in the
    // system's credential store go too.
    if args.first().is_some_and(|a| a == FORGET_SECRETS_FLAG) {
        return match hermes_client_core::tokens::forget_all() {
            Ok(n) => {
                println!("deleted {n} secrets");
                ExitCode::SUCCESS
            }
            Err(err) => fail(&format!("could not delete the secrets: {err}")),
        };
    }
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
    // Just updated: the old version is closing, and must not be handed this start.
    if cli.after_update {
        for _ in 0..80 {
            match single_instance::lock(&paths.dir) {
                Ok(Some(lock)) => {
                    drop(lock);
                    break;
                }
                _ => std::thread::sleep(std::time::Duration::from_millis(250)),
            }
        }
    } else if forward_to_running(&endpoint, cli.link.as_deref()) {
        return ExitCode::SUCCESS;
    }
    // Nobody answered, but one may be starting: the lock tells. It is held until `main` returns.
    let _lock = match single_instance::lock(&paths.dir) {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => {
            for _ in 0..40 {
                std::thread::sleep(std::time::Duration::from_millis(250));
                if forward_to_running(&endpoint, cli.link.as_deref()) {
                    return ExitCode::SUCCESS;
                }
            }
            return fail("Hermes is already running");
        }
        Err(err) => {
            eprintln!("no single-instance lock: {err}");
            None
        }
    };
    let loaded = match ClientConfig::load(&paths.config()) {
        Ok(loaded) => loaded,
        Err(err) => return fail(&format!("Invalid config: {err}")),
    };
    init_logging(&paths);
    watchdog::start(paths.dir.join("logs"));
    for warning in &loaded.warnings {
        tracing::warn!("{warning}");
    }
    let portable = paths.portable;
    let options = match CoreOptions::with_paths("hermes-gui", paths) {
        Ok(options) if portable && loaded.config.general.sign_ins_in_folder => {
            options.sign_ins_in_folder()
        }
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
    let link = cli.link;
    let link_ready =
        core.start_desktop_accounts(link.as_deref().and_then(add_instance::launch_instance));
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

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1280.), px(800.)),
                    cx,
                ))),
                window_min_size: Some(size(px(theme::WINDOW_MIN_W), px(theme::WINDOW_MIN_H))),
                // Draws at the display's rate also while another window has focus, so scrolling and
                // animations beside it stay smooth (#66). Frames are still drawn only when something
                // changed; a window hidden behind another one is slowed down in vendor/gpui-pre-windows.
                inactive_frame_interval: None,
                // The native title bar, which shows the space and page (`Shell::page_title`).
                titlebar: Some(TitlebarOptions {
                    title: Some("Hermes".into()),
                    appears_transparent: false,
                    ..Default::default()
                }),
                ..Default::default()
            };
            // The tray and the global keys live on this (the event loop's) thread.
            let tray = start_minimized.then(desktop_shell::create_tray).flatten();
            let start_hidden = start_minimized && tray.is_some();
            let boot = Boot {
                tray,
                launched,
                link_ready,
                start_hidden,
                core,
                events,
                runtime: handle,
                endpoint,
                link,
                warning,
            };
            let (main, _) = gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| Shell::new(boot, window, cx))
            })
            .expect("failed to open the window");
            // Up and running: an update that started this one is done. It looks through the
            // install folder, so not on this thread: the first events wait for it (#28).
            let _ = std::thread::Builder::new()
                .name("update-finish".into())
                .spawn(hermes_client_core::updater::finish_started);
            // Only the main window ends the app: a popped-out share's window just closes.
            let main = main.window_id();
            cx.on_window_closed(move |cx, closed| {
                if closed == main {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    // A portable folder leaves nothing in the computer's temp folder (also cleared on start).
    if portable {
        open::clear_pasted();
    }
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
