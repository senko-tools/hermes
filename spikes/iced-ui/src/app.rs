//! App state, update, view and subscriptions.

use std::path::PathBuf;
use std::time::Instant;

use iced::event::{self, Event};
use iced::keyboard::{self, key};
use iced::widget::{
    Column, button, column, container, pane_grid, row, scrollable, text, text_editor,
};
use iced::{Color, Element, Fill, Subscription, Task, Theme, window};

use crate::chat::{self, ChatRow};
use crate::ptt::{self, PttEvent, Source};

const ROWS: usize = 10_000;
const LOG_LIMIT: usize = 500;

#[derive(Clone)]
pub struct Config {
    /// Shown in the header; `None` if no global hotkey was registered.
    pub hotkey: Option<String>,
    /// Name and virtual-key code for the polling fallback.
    pub poll: Option<(String, u16)>,
}

enum Pane {
    Chat {
        id: iced::widget::Id,
        offset_y: f32,
        height: f32,
        editor: text_editor::Content,
    },
    Log,
}

impl Pane {
    fn chat() -> Self {
        Self::Chat {
            id: iced::widget::Id::unique(),
            offset_y: 0.0,
            // Until the first scroll event reports the real height.
            height: 1200.0,
            editor: text_editor::Content::new(),
        }
    }

    fn same_kind(&self) -> Self {
        match self {
            Self::Chat { .. } => Self::chat(),
            Self::Log => Self::Log,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Clicked(pane_grid::Pane),
    Dragged(pane_grid::DragEvent),
    Resized(pane_grid::ResizeEvent),
    Split(pane_grid::Axis, pane_grid::Pane),
    Close(pane_grid::Pane),
    Scrolled(pane_grid::Pane, scrollable::Viewport),
    Edit(pane_grid::Pane, text_editor::Action),
    Send(pane_grid::Pane),
    FileDropped(PathBuf),
    Focus(bool),
    Ptt(PttEvent),
}

pub struct App {
    config: Config,
    started: Instant,
    panes: pane_grid::State<Pane>,
    focus: Option<pane_grid::Pane>,
    rows: Vec<ChatRow>,
    log: Vec<String>,
    held_since: [Option<Instant>; 2],
    ptt_down: [bool; 2],
}

impl App {
    pub fn new(config: Config) -> Self {
        let (mut panes, first) = pane_grid::State::new(Pane::chat());
        let _ = panes.split(pane_grid::Axis::Vertical, first, Pane::Log);
        let mut app = Self {
            config,
            started: Instant::now(),
            panes,
            focus: Some(first),
            rows: chat::fake_rows(ROWS),
            log: Vec::new(),
            held_since: [None; 2],
            ptt_down: [false; 2],
        };
        match &app.config.hotkey {
            Some(k) => app.log(format!("global-hotkey registered: {k}")),
            None => app.log("global-hotkey disabled".to_string()),
        }
        if let Some((name, vk)) = &app.config.poll {
            let msg = format!("polling fallback on: {name} (VK 0x{vk:02X}, every 10 ms)");
            app.log(msg);
        }
        app.log("drop files onto the window to see their paths".to_string());
        app
    }

    pub fn title(&self) -> String {
        "Hermes Spike C: iced UI risks".to_string()
    }

    pub fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn stamp(&self, at: Instant) -> String {
        format!("{:9.3} s", at.duration_since(self.started).as_secs_f64())
    }

    fn log(&mut self, line: String) {
        let line = format!("[{}] {line}", self.stamp(Instant::now()));
        println!("{line}");
        self.log.push(line);
        if self.log.len() > LOG_LIMIT {
            self.log.remove(0);
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Clicked(pane) => self.focus = Some(pane),
            Message::Dragged(pane_grid::DragEvent::Dropped { pane, target }) => {
                self.panes.drop(pane, target);
            }
            Message::Dragged(_) => {}
            Message::Resized(pane_grid::ResizeEvent { split, ratio }) => {
                self.panes.resize(split, ratio);
            }
            Message::Split(axis, pane) => {
                if let Some(kind) = self.panes.get(pane).map(Pane::same_kind)
                    && let Some((new, _)) = self.panes.split(axis, pane, kind)
                {
                    self.focus = Some(new);
                }
            }
            Message::Close(pane) => {
                if let Some((_, sibling)) = self.panes.close(pane) {
                    self.focus = Some(sibling);
                }
            }
            Message::Scrolled(pane, viewport) => {
                if let Some(Pane::Chat {
                    offset_y, height, ..
                }) = self.panes.get_mut(pane)
                {
                    *offset_y = viewport.absolute_offset().y;
                    *height = viewport.bounds().height;
                }
            }
            Message::Edit(pane, action) => {
                if let Some(Pane::Chat { editor, .. }) = self.panes.get_mut(pane) {
                    editor.perform(action);
                }
            }
            Message::Send(pane) => {
                if let Some(Pane::Chat { editor, id, .. }) = self.panes.get_mut(pane) {
                    let body = editor.text().trim_end().replace('\n', " ⏎ ");
                    if body.trim().is_empty() {
                        return Task::none();
                    }
                    *editor = text_editor::Content::new();
                    let id = id.clone();
                    self.rows.push(ChatRow {
                        time: "now".to_string(),
                        author: "you",
                        color: Color::from_rgb(1.0, 1.0, 1.0),
                        body,
                    });
                    return iced::widget::operation::snap_to_end(id);
                }
            }
            Message::FileDropped(path) => self.log(format!("file dropped: {}", path.display())),
            Message::Focus(focused) => self.log(format!(
                "window {}",
                if focused { "focused" } else { "unfocused" }
            )),
            Message::Ptt(ev) => self.on_ptt(ev),
        }
        Task::none()
    }

    fn on_ptt(&mut self, ev: PttEvent) {
        let i = match ev.source {
            Source::Hotkey => 0,
            Source::Poll => 1,
        };
        let name = match ev.source {
            Source::Hotkey => "hotkey",
            Source::Poll => "poll",
        };
        // Windows sends WM_HOTKEY once per press (MOD_NOREPEAT); still ignore repeats.
        if ev.down == self.ptt_down[i] {
            return;
        }
        self.ptt_down[i] = ev.down;
        let at = self.stamp(ev.at);
        if ev.down {
            self.held_since[i] = Some(ev.at);
            println!("PTT DOWN ({name}) at {at}");
            self.log.push(format!("[{at}] PTT DOWN ({name})"));
        } else {
            let held = self.held_since[i].take().map_or(String::new(), |t| {
                format!(
                    ", held {:.0} ms",
                    ev.at.duration_since(t).as_secs_f64() * 1000.0
                )
            });
            println!("PTT UP   ({name}) at {at}{held}");
            self.log.push(format!("[{at}] PTT UP   ({name}){held}"));
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let window_events = event::listen_with(|event, _status, _window| match event {
            Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
            Event::Window(window::Event::Focused) => Some(Message::Focus(true)),
            Event::Window(window::Event::Unfocused) => Some(Message::Focus(false)),
            _ => None,
        });
        let mut subs = vec![window_events];
        if self.config.hotkey.is_some() {
            subs.push(ptt::hotkey_events().map(Message::Ptt));
        }
        if let Some((_, vk)) = self.config.poll {
            subs.push(ptt::poll_events(vk).map(Message::Ptt));
        }
        Subscription::batch(subs)
    }

    pub fn view(&self) -> Element<'_, Message> {
        let total = self.panes.len();
        let grid = pane_grid(&self.panes, |id, pane, _maximized| {
            let focused = self.focus == Some(id);
            let (title, body) = match pane {
                Pane::Chat {
                    id: list_id,
                    offset_y,
                    height,
                    editor,
                } => {
                    let range = chat::visible_range(self.rows.len(), *offset_y, *height);
                    let title = format!(
                        "Chat: {} rows, {} built ({}..{})",
                        self.rows.len(),
                        range.len(),
                        range.start,
                        range.end
                    );
                    let list = chat::view_list(&self.rows, range, list_id.clone(), move |vp| {
                        Message::Scrolled(id, vp)
                    });
                    let input = text_editor(editor)
                        .placeholder("Message (Enter sends, Shift+Enter new line)")
                        .on_action(move |a| Message::Edit(id, a))
                        .key_binding(move |kp| {
                            let enter = matches!(kp.key, keyboard::Key::Named(key::Named::Enter));
                            if enter && !kp.modifiers.shift() {
                                Some(text_editor::Binding::Custom(Message::Send(id)))
                            } else {
                                text_editor::Binding::from_key_press(kp)
                            }
                        })
                        .height(90);
                    let body: Element<'_, Message> =
                        column![list, input].spacing(6).padding(6).into();
                    (title, body)
                }
                Pane::Log => {
                    let lines = Column::with_children(
                        self.log
                            .iter()
                            .rev()
                            .map(|l| text(l).size(13).font(iced::Font::MONOSPACE).into()),
                    )
                    .spacing(2);
                    let header = text(format!(
                        "PTT: hotkey {} | poll {}",
                        self.config.hotkey.as_deref().unwrap_or("off"),
                        self.config.poll.as_ref().map_or("off", |(n, _)| n.as_str()),
                    ))
                    .size(14);
                    let body: Element<'_, Message> =
                        column![header, scrollable(lines).height(Fill).width(Fill)]
                            .spacing(6)
                            .padding(6)
                            .into();
                    ("Event log (newest first)".to_string(), body)
                }
            };
            let small = |label| button(text(label).size(13)).padding([2, 6]);
            let controls = row![
                small("split →").on_press(Message::Split(pane_grid::Axis::Vertical, id)),
                small("split ↓").on_press(Message::Split(pane_grid::Axis::Horizontal, id)),
                small("close")
                    .style(button::danger)
                    .on_press_maybe((total > 1).then_some(Message::Close(id))),
            ]
            .spacing(4);
            let title_bar = pane_grid::TitleBar::new(text(title).size(14))
                .controls(pane_grid::Controls::new(controls))
                .always_show_controls()
                .padding(6)
                .style(if focused {
                    style::title_focused
                } else {
                    style::title
                });
            pane_grid::Content::new(body)
                .title_bar(title_bar)
                .style(if focused {
                    style::pane_focused
                } else {
                    style::pane
                })
        })
        .width(Fill)
        .height(Fill)
        .spacing(6)
        .on_click(Message::Clicked)
        .on_drag(Message::Dragged)
        .on_resize(10, Message::Resized);

        container(grid).padding(6).into()
    }
}

mod style {
    use iced::widget::container;
    use iced::{Border, Theme};

    pub fn title(theme: &Theme) -> container::Style {
        let p = theme.extended_palette();
        container::Style {
            text_color: Some(p.background.strong.text),
            background: Some(p.background.strong.color.into()),
            ..Default::default()
        }
    }

    pub fn title_focused(theme: &Theme) -> container::Style {
        let p = theme.extended_palette();
        container::Style {
            text_color: Some(p.primary.strong.text),
            background: Some(p.primary.strong.color.into()),
            ..Default::default()
        }
    }

    fn bordered(theme: &Theme, focused: bool) -> container::Style {
        let p = theme.extended_palette();
        container::Style {
            background: Some(p.background.weak.color.into()),
            border: Border {
                width: 2.0,
                color: if focused {
                    p.primary.strong.color
                } else {
                    p.background.strong.color
                },
                ..Border::default()
            },
            ..Default::default()
        }
    }

    pub fn pane(theme: &Theme) -> container::Style {
        bordered(theme, false)
    }

    pub fn pane_focused(theme: &Theme) -> container::Style {
        bordered(theme, true)
    }
}
