//! Spike C (PLAN.md section 20, Phase 0): iced UI risks.
//!
//! One window with a pane_grid (split/close/drag/resize), a text_editor input, a virtualized
//! list of 10,000 chat rows with color emoji, file-drop events, and a global push-to-talk test
//! (global-hotkey, plus the GetAsyncKeyState polling fallback from section 21 behind --poll).

mod app;
mod chat;
mod ptt;

use std::process::ExitCode;
use std::str::FromStr;

use clap::Parser;
use global_hotkey::GlobalHotKeyManager;
use global_hotkey::hotkey::HotKey;

#[derive(Parser)]
#[command(about = "Hermes Spike C: iced UI risks")]
struct Args {
    /// Global hotkey for push-to-talk, in global-hotkey syntax (e.g. F8, CapsLock,
    /// Ctrl+Shift+KeyV). Use "none" to disable.
    #[arg(long, default_value = "F8")]
    key: String,
    /// Also poll this key with GetAsyncKeyState every 10 ms (Windows only), e.g. F8, CapsLock,
    /// Mouse4, Mouse5, V. Does not take the key away from other apps.
    #[arg(long, value_name = "KEY")]
    poll: Option<String>,
}

fn main() -> ExitCode {
    let args = Args::parse();

    // The manager must be created, and stay alive, on the thread that runs the event loop.
    let mut manager = None;
    let hotkey = if args.key.eq_ignore_ascii_case("none") {
        None
    } else {
        match register(&args.key) {
            Ok(m) => {
                manager = Some(m);
                Some(args.key.clone())
            }
            Err(e) => {
                eprintln!("error: cannot register global hotkey \"{}\": {e}", args.key);
                return ExitCode::FAILURE;
            }
        }
    };

    let poll = match args.poll {
        None => None,
        Some(_) if !cfg!(windows) => {
            eprintln!("error: --poll is only available on Windows");
            return ExitCode::FAILURE;
        }
        Some(name) => match ptt::virtual_key(&name) {
            Some(vk) => Some((name, vk)),
            None => {
                eprintln!("error: unknown key for --poll: {name}");
                return ExitCode::FAILURE;
            }
        },
    };

    let config = app::Config { hotkey, poll };
    let result = iced::application(
        move || app::App::new(config.clone()),
        app::App::update,
        app::App::view,
    )
    .title(app::App::title)
    .theme(app::App::theme)
    .subscription(app::App::subscription)
    .window_size((1200.0, 800.0))
    .run();
    drop(manager);

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn register(key: &str) -> Result<GlobalHotKeyManager, String> {
    let hotkey = HotKey::from_str(key).map_err(|e| e.to_string())?;
    let manager = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
    manager.register(hotkey).map_err(|e| e.to_string())?;
    Ok(manager)
}
