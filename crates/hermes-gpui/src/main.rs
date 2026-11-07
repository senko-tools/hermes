//! The Hermes desktop client on gpui, replacing the iced client in `hermes-gui`.
//!
//! So far this is the layout shell, running on sample data: nothing talks to a server yet.

mod assets;
mod channel;
mod friends;
mod menu;
mod sample;
mod shell;
mod sidebar;
mod theme;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

use crate::sample::Data;
use crate::shell::Shell;

fn main() {
    let no_instances = std::env::args().any(|arg| arg == "--no-instances");
    gpui_kit::application()
        .with_assets(assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            assets::load_fonts(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            let theme = Theme::global_mut(cx);
            theme.font_family = theme::FONT.into();
            theme.font_size = px(theme::TEXT);
            // One window for now: closing it ends the app.
            cx.on_window_closed(|cx, _| cx.quit()).detach();

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1280.), px(800.)),
                    cx,
                ))),
                window_min_size: Some(size(px(940.), px(500.))),
                // The native title bar; the shell keeps its title on the current page.
                titlebar: Some(TitlebarOptions {
                    title: Some("Hermes".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| Shell::new(Data::sample(no_instances), window, cx))
            })
            .expect("failed to open the window");
            cx.activate(true);
        });
}
