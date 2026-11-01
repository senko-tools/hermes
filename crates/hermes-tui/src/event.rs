//! The event loop: terminal input, core events, task results and a 250 ms tick, one at a time.

use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use hermes_client_core::CoreEvents;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::app::{App, TaskMsg};
use crate::ui;

pub async fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    mut core_events: CoreEvents,
    mut tasks: mpsc::UnboundedReceiver<TaskMsg>,
) -> std::io::Result<()> {
    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        tokio::select! {
            event = input.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => app.on_key(key),
                Some(Ok(Event::Mouse(mouse))) => app.on_mouse(mouse),
                Some(Ok(Event::Paste(text))) => app.on_paste(&text),
                Some(Ok(Event::FocusGained)) => app.core.set_app_focused(true),
                Some(Ok(Event::FocusLost)) => app.core.set_app_focused(false),
                Some(Ok(_)) => {}
                Some(Err(err)) => return Err(err),
                None => return Ok(()),
            },
            Some(event) = core_events.recv() => {
                app.on_core_event(event);
                // Drain the burst before redrawing.
                while let Some(event) = core_events.try_recv() {
                    app.on_core_event(event);
                }
            }
            Some(msg) = tasks.recv() => app.on_task(msg),
            _ = tick.tick() => app.on_tick(),
        }
        if app.quit {
            return Ok(());
        }
    }
}
