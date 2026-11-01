//! Outside events turned into messages: core events, theme file changes, window focus.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, Stream, StreamExt};
use hermes_client_core::CoreEvents;
use iced::{Event, Subscription, event, window};

use crate::message::Message;
use crate::single_instance::{self, Endpoint};

/// The core's event stream; the subscription takes it out once.
#[derive(Debug, Clone)]
pub struct CoreEventsHandle(Arc<Mutex<Option<CoreEvents>>>);

impl CoreEventsHandle {
    pub fn new(events: CoreEvents) -> Self {
        CoreEventsHandle(Arc::new(Mutex::new(Some(events))))
    }
}

impl Hash for CoreEventsHandle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "core-events".hash(state);
    }
}

pub fn core_events(handle: CoreEventsHandle) -> Subscription<Message> {
    Subscription::run_with(handle, |handle| {
        let events = handle.0.lock().expect("core events lock").take();
        futures_util::stream::iter(events)
            .flatten()
            .map(Message::Core)
    })
}

/// Reports changes in `<config dir>/themes/` (debounced by 200 ms).
pub fn themes(dir: PathBuf) -> Subscription<Message> {
    Subscription::run_with(dir, |dir| watch_dir(dir.clone()))
}

fn watch_dir(dir: PathBuf) -> impl Stream<Item = Message> {
    iced::stream::channel(8, async move |mut output| {
        use notify::Watcher;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let watcher = std::fs::create_dir_all(&dir)
            .map_err(notify::Error::io)
            .and_then(|()| {
                let mut watcher =
                    notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                        if res.is_ok_and(|e| !e.kind.is_access()) {
                            let _ = tx.send(());
                        }
                    })?;
                watcher.watch(&dir, notify::RecursiveMode::NonRecursive)?;
                Ok(watcher)
            });
        let _watcher = match watcher {
            Ok(watcher) => watcher,
            Err(err) => {
                tracing::warn!("theme hot reload unavailable: {err}");
                return;
            }
        };
        while rx.recv().await.is_some() {
            tokio::time::sleep(Duration::from_millis(200)).await;
            while rx.try_recv().is_ok() {}
            if output.send(Message::ThemesChanged).await.is_err() {
                break;
            }
        }
    })
}

/// Links forwarded by later launches (section 15.7).
pub fn single_instance(endpoint: Endpoint) -> Subscription<Message> {
    Subscription::run_with(endpoint, |endpoint| {
        let endpoint = endpoint.clone();
        iced::stream::channel(8, async move |mut output| {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let server = tokio::spawn(single_instance::serve(endpoint, tx));
            while let Some(link) = rx.recv().await {
                if output.send(Message::Forwarded(link)).await.is_err() {
                    break;
                }
            }
            // Ends here when `serve` failed (another client listens already).
            server.abort();
            if let Ok(Err(err)) = server.await {
                tracing::warn!("not receiving links from later launches: {err}");
            }
        })
    })
}

/// Window size (for placing menus), key presses (keybinds) and the left mouse button (drag and
/// drop).
pub fn window_and_keys() -> Subscription<Message> {
    event::listen_with(|event, status, _window| match event {
        Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
        Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
        Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key,
            physical_key,
            modifiers,
            ..
        }) => Some(Message::Key(
            key,
            physical_key,
            modifiers,
            status == event::Status::Captured,
        )),
        Event::Keyboard(iced::keyboard::Event::KeyReleased { key, .. }) => {
            Some(Message::KeyReleased(key))
        }
        Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) => {
            Some(Message::MousePressed)
        }
        Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
            Some(Message::MouseReleased)
        }
        _ => None,
    })
}

pub fn window_focus() -> Subscription<Message> {
    event::listen_with(|event, _status, _window| match event {
        Event::Window(window::Event::Focused) => Some(Message::WindowFocused(true)),
        Event::Window(window::Event::Unfocused) => Some(Message::WindowFocused(false)),
        _ => None,
    })
}
