use std::time::Duration;

use hermes_client_core::CoreEvents;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tokio::sync::mpsc;

use crate::app::{App, TaskMsg};
use crate::ui;

/// Feeds core events and task results to the app until `done` holds.
pub(crate) async fn pump(
    app: &mut App,
    events: &mut CoreEvents,
    tasks: &mut mpsc::UnboundedReceiver<TaskMsg>,
    what: &str,
    done: impl Fn(&App) -> bool,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !done(app) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out: {what} (status: {:?})",
            app.status
        );
        tokio::select! {
            Some(e) = events.recv() => app.on_core_event(e),
            Some(m) = tasks.recv() => app.on_task(m),
            () = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }
}

pub(crate) fn screen(app: &mut App, width: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}
