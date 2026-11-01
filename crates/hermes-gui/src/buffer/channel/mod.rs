//! The Channel buffer (PLAN.md section 15.2): a virtualized message list anchored to the bottom,
//! with grouping, day separators, the "New messages" divider, reply previews, pending messages,
//! the typing line and "Jump to present".

pub mod input;
pub mod list;
pub mod markup;
pub mod message;
pub mod rows;

use std::collections::HashSet;

use hermes_client_core::config::ClientConfig;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Attachment;
use hermes_client_core::store::{InstanceStore, LocalId, MessageWindow, PendingState};
use hermes_client_core::{Core, InstanceId, now_ms, time};
use iced::widget::{Space, button, column, container, row, scrollable, sensor, stack};
use iced::{Alignment, Element, Fill, Length, Size};

use self::list::Heights;
use self::rows::{Row, RowKey};
use crate::font::Fonts;
use crate::theme::{Theme, style};
use crate::widget::{label, small};

/// Within this many pixels of the bottom counts as "at the bottom" (live, marks read).
pub const AT_BOTTOM: f32 = 4.0;
/// Older history loads when the top is this close.
const LOAD_OLDER_AT: f32 = 400.0;

/// Per-pane state of a channel buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelView {
    pub scroll_id: iced::widget::Id,
    /// Pixels above the bottom.
    pub from_bottom: f32,
    pub viewport: Size,
    /// Total content height (for "near the top").
    pub content_height: f32,
    pub heights: Heights,
    /// My read marker when the channel was opened (the divider position).
    pub read_marker: Option<Id>,
    pub loading_older: bool,
    /// The message under the pointer (shows the toolbar).
    pub hovered: Option<Id>,
    /// The message whose quick-reaction palette is open.
    pub palette: Option<Id>,
    /// Messages whose spoilers were clicked.
    pub revealed: HashSet<Id>,
}

impl Default for ChannelView {
    fn default() -> Self {
        ChannelView {
            scroll_id: iced::widget::Id::unique(),
            from_bottom: 0.0,
            viewport: Size::new(800.0, 600.0),
            content_height: 0.0,
            heights: Heights::default(),
            read_marker: None,
            loading_older: false,
            hovered: None,
            palette: None,
            revealed: HashSet::new(),
        }
    }
}

impl ChannelView {
    pub fn new(read_marker: Option<Id>) -> ChannelView {
        ChannelView {
            read_marker,
            ..ChannelView::default()
        }
    }

    pub fn at_bottom(&self) -> bool {
        self.from_bottom <= AT_BOTTOM
    }

    pub fn near_top(&self) -> bool {
        self.content_height - self.viewport.height - self.from_bottom < LOAD_OLDER_AT
    }

    /// Takes a scroll position; `true` if the width changed (heights were reset).
    pub fn scrolled(&mut self, viewport: &scrollable::Viewport) -> bool {
        self.from_bottom = viewport.absolute_offset().y;
        let bounds = viewport.bounds();
        self.content_height = viewport.content_bounds().height;
        let width_changed = (bounds.width - self.viewport.width).abs() > 1.0;
        self.viewport = bounds.size();
        if width_changed {
            self.heights.clear();
        }
        width_changed
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Scrolled(scrollable::Viewport),
    Measured(RowKey, f32),
    JumpToPresent,
    OlderLoaded,
    Retry(LocalId),
    Discard(LocalId),
    /// The pointer entered (`true`) or left a message.
    Hover(Id, bool),
    /// Opens (or closes) the quick-reaction palette of a message.
    Palette(Option<Id>),
    /// The `⋯` of a message: its context menu.
    More(Id),
    /// Adds (`true`) or removes a reaction.
    React(Id, String, bool),
    Link(Id, markup::Link),
    OpenImage(Attachment),
    Download(Attachment),
    Input(input::Message),
    /// Reply to a message (toolbar).
    Reply(Id),
    /// Edit my message (toolbar).
    StartEdit(Id),
    /// A message author's name or avatar was clicked: their menu.
    Author(Id),
}

/// What the view needs besides the pane state.
pub struct Context<'a> {
    pub core: &'a Core,
    pub config: &'a ClientConfig,
    pub fonts: &'a Fonts,
    pub theme: &'a Theme,
    pub utc_offset: i64,
    pub images: &'a message::Images,
    pub composer: Option<&'a input::Composer>,
}

pub fn view<'a>(
    ctx: &Context<'a>,
    inst: &InstanceId,
    channel: Id,
    state: &'a ChannelView,
) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let store = ctx.core.store();
    let Some(s) = store.instance(inst) else {
        return centered(small("Not connected to this instance.", fonts));
    };
    let Some(window) = s.window(channel) else {
        return centered(small("Loading…", fonts).color(theme.text.tertiary.0));
    };
    if !window.loaded {
        return centered(small("Loading…", fonts).color(theme.text.tertiary.0));
    }
    let rows = rows::build(
        s,
        window,
        &rows::Options {
            group_minutes: ctx.config.buffer.group_minutes,
            utc_offset: ctx.utc_offset,
            read_marker: state.read_marker,
        },
    );
    if rows.is_empty() && window.pending.is_empty() {
        let title = s
            .channel(channel)
            .map(|c| hermes_client_core::view::channel_title(s, c))
            .unwrap_or_default();
        return column![
            centered(
                small(format!("This is the start of {title}."), fonts).color(theme.text.tertiary.0)
            ),
            typing_line(ctx, s, channel),
            composer(ctx, s, channel),
        ]
        .into();
    }
    let heights: Vec<f32> = rows
        .iter()
        .map(|r| {
            state
                .heights
                .get(&r.key())
                .unwrap_or_else(|| estimate(r, window, state.viewport.width, fonts.size))
        })
        .collect();
    let visible = list::visible(&heights, state.from_bottom, state.viewport.height);

    let mut list = column![Space::new().height(visible.above)];
    for r in &rows[visible.range.clone()] {
        let key = r.key();
        list = list.push(
            sensor(row_view(ctx, s, inst, window, r, state))
                .key(key)
                .on_show(move |size| Message::Measured(key, size.height))
                .on_resize(move |size| Message::Measured(key, size.height)),
        );
    }
    list = list.push(Space::new().height(visible.below));

    let messages = scrollable(list.width(Fill))
        .id(state.scroll_id.clone())
        .anchor_bottom()
        .on_scroll(Message::Scrolled)
        .height(Fill)
        .width(Fill);

    let mut layers = stack![messages];
    if !state.at_bottom() || !window.reached_end {
        layers = layers.push(
            container(
                button(small("Jump to present", fonts))
                    .padding([4, 12])
                    .style(move |_, st| style::primary(theme, st))
                    .on_press(Message::JumpToPresent),
            )
            .align_right(Fill)
            .align_bottom(Fill)
            .padding(12),
        );
    }

    column![
        layers,
        typing_line(ctx, s, channel),
        composer(ctx, s, channel)
    ]
    .into()
}

fn composer<'a>(ctx: &Context<'a>, s: &InstanceStore, channel: Id) -> Element<'a, Message> {
    let title = s
        .channel(channel)
        .map(|c| hermes_client_core::view::channel_title(s, c))
        .unwrap_or_default();
    let placeholder = if title.starts_with('#') {
        format!("Message {title}")
    } else {
        format!("Message @{title}")
    };
    let can_send = hermes_client_core::view::can(
        s,
        channel,
        hermes_client_core::proto::permissions::Permissions::SEND_MESSAGES,
    );
    let reply_name = ctx.composer.and_then(|c| c.reply_to).map(|id| {
        s.window(channel)
            .and_then(|w| w.get(id))
            .map(|m| s.display_name(s.space_of(channel), m.author.id))
            .unwrap_or_else(|| "a message".into())
    });
    input::view(ctx, ctx.composer, placeholder, can_send, reply_name).map(Message::Input)
}

fn centered<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content).center(Fill).into()
}

fn estimate(r: &Row, window: &MessageWindow, width: f32, font_size: f32) -> f32 {
    match r {
        Row::Older => 32.0,
        Row::Day { .. } => 30.0,
        Row::NewMessages => 24.0,
        Row::Message(m) => match window.get(m.id) {
            Some(msg) => list::estimate_message(
                &msg.content,
                m.grouped,
                &msg.attachments
                    .iter()
                    .map(|a| (a.width, a.height, a.content_type.starts_with("image/")))
                    .collect::<Vec<_>>(),
                !msg.reactions.is_empty(),
                m.reply.is_some(),
                width,
                font_size,
            ),
            None => 30.0,
        },
        Row::Pending(p) => window.pending(p.local_id).map_or(30.0, |pm| {
            list::estimate_message(&pm.content, p.grouped, &[], false, false, width, font_size)
        }),
    }
}

/// `Bob is typing…`, `Bob and Carol are typing…`, `Several people are typing…`.
pub fn typing_text(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [a] => Some(format!("{a} is typing…")),
        [a, b] => Some(format!("{a} and {b} are typing…")),
        [a, b, c] => Some(format!("{a}, {b} and {c} are typing…")),
        _ => Some("Several people are typing…".into()),
    }
}

fn typing_line<'a>(ctx: &Context<'a>, s: &InstanceStore, channel: Id) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let names: Vec<String> = if ctx.config.buffer.show_typing {
        let space = s.space_of(channel);
        s.typing(channel, now_ms())
            .into_iter()
            .map(|u| s.display_name(space, u))
            .collect()
    } else {
        Vec::new()
    };
    container(
        small(typing_text(&names).unwrap_or_default(), fonts).color(ctx.theme.text.secondary.0),
    )
    .padding([2, 12])
    .height(Length::Fixed(fonts.small() * 1.4 + 4.0))
    .into()
}

fn row_view<'a>(
    ctx: &Context<'a>,
    s: &InstanceStore,
    inst: &InstanceId,
    window: &MessageWindow,
    r: &Row,
    state: &'a ChannelView,
) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    match r {
        Row::Older => {
            container(small("Loading older messages…", fonts).color(theme.text.tertiary.0))
                .center_x(Fill)
                .padding(8)
                .into()
        }
        Row::Day { label: day, .. } => row![
            rule_line(theme, theme.general.horizontal_rule.0),
            small(day.clone(), fonts).color(theme.text.tertiary.0),
            rule_line(theme, theme.general.horizontal_rule.0),
        ]
        .spacing(8)
        .padding([6, 12])
        .align_y(Alignment::Center)
        .into(),
        Row::NewMessages => row![
            rule_line(theme, theme.text.error.0),
            small("New messages", fonts).color(theme.text.error.0),
        ]
        .spacing(8)
        .padding([2, 12])
        .align_y(Alignment::Center)
        .into(),
        Row::Message(m) => match window.get(m.id) {
            Some(msg) => message::view(ctx, s, inst, m, msg, state),
            None => Space::new().into(),
        },
        Row::Pending(p) => match window.pending(p.local_id) {
            Some(pm) => {
                let (color, note) = match &pm.state {
                    PendingState::Sending => (theme.text.tertiary.0, None),
                    PendingState::Queued => (theme.text.tertiary.0, Some("waiting for connection")),
                    PendingState::Failed(_) => (theme.text.error.0, Some("not sent")),
                };
                let mut body = column![label(pm.content.clone(), fonts).color(color)].spacing(2);
                if !p.grouped {
                    let me = s
                        .me_id()
                        .map(|id| s.display_name(None, id))
                        .unwrap_or_default();
                    body = column![
                        row![
                            label(me, fonts).font(fonts.bold()),
                            small(
                                time::format(
                                    pm.created_at,
                                    &ctx.config.buffer.timestamp_format,
                                    ctx.utc_offset
                                ),
                                fonts
                            )
                            .color(theme.text.tertiary.0),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                        body
                    ]
                    .spacing(2);
                }
                if let Some(note) = note {
                    let mut line = row![small(note, fonts).color(color)]
                        .spacing(8)
                        .align_y(Alignment::Center);
                    if let PendingState::Failed(reason) = &pm.state {
                        line = line
                            .push(small(reason.clone(), fonts).color(theme.text.tertiary.0))
                            .push(
                                button(small("Retry", fonts))
                                    .padding([1, 8])
                                    .style(move |_, st| style::secondary(theme, st))
                                    .on_press(Message::Retry(p.local_id)),
                            )
                            .push(
                                button(small("Delete", fonts))
                                    .padding([1, 8])
                                    .style(move |_, st| style::danger(theme, st))
                                    .on_press(Message::Discard(p.local_id)),
                            );
                    }
                    body = body.push(line);
                }
                container(body)
                    .padding(row_padding(p.grouped))
                    .width(Fill)
                    .into()
            }
            None => Space::new().into(),
        },
    }
}

pub(super) fn row_padding(grouped: bool) -> iced::Padding {
    iced::Padding {
        top: if grouped { 1.0 } else { 8.0 },
        bottom: 1.0,
        left: 16.0,
        right: 16.0,
    }
}

fn rule_line<'a>(_theme: &'a Theme, color: iced::Color) -> Element<'a, Message> {
    container(Space::new().height(1))
        .width(Fill)
        .style(move |_| container::Style {
            background: Some(color.into()),
            ..container::Style::default()
        })
        .into()
}

/// The color of the member's highest role that has one.
pub fn author_color(s: &InstanceStore, space: Id, user: Id) -> Option<u32> {
    let state = s.spaces.get(&space)?;
    let member = state.members.get(&user)?;
    state
        .roles
        .iter()
        .filter(|r| member.role_ids.contains(&r.id))
        .find_map(|r| r.color)
}

pub(super) fn rgb(c: u32) -> iced::Color {
    iced::Color::from_rgb8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_texts() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(typing_text(&n(&[])), None);
        assert_eq!(typing_text(&n(&["Bob"])).unwrap(), "Bob is typing…");
        assert_eq!(
            typing_text(&n(&["Bob", "Carol"])).unwrap(),
            "Bob and Carol are typing…"
        );
        assert_eq!(
            typing_text(&n(&["a", "b", "c", "d"])).unwrap(),
            "Several people are typing…"
        );
    }

    #[test]
    fn author_color_is_the_highest_colored_role() {
        let s = crate::fixtures::store();
        assert_eq!(author_color(&s, Id(10), Id(3)), None, "only @everyone");
    }
}
