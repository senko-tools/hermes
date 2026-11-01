//! One message (PLAN.md section 15.2): author line, rendered markup, attachments, reactions and
//! the hover toolbar.

use std::collections::HashMap;

use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::{Attachment, Message as ChatMessage};
use hermes_client_core::store::InstanceStore;
use hermes_client_core::{InstanceId, time};
use iced::font::{Style as FontStyle, Weight};
use iced::widget::text::Span;
use iced::widget::{
    Space, button, column, container, image, mouse_area, rich_text, row, stack, text,
};
use iced::{Alignment, Background, Border, Color, Element, Fill, Font, Length};

use super::markup::{self, Block, Link, Piece};
use super::rows::MessageRow;
use super::{ChannelView, Context, Message};
use crate::theme::{Theme, style};
use crate::widget::{label, small};

/// Quick reactions offered by the toolbar.
pub const QUICK_REACTIONS: [&str; 8] = ["👍", "❤️", "😂", "😮", "😢", "🎉", "👀", "🔥"];
const GUTTER: f32 = 44.0;
const MAX_IMAGE_WIDTH: f32 = 400.0;
const MAX_IMAGE_HEIGHT: f32 = 300.0;

/// A loaded (or loading) image, keyed by instance and attachment.
#[derive(Debug, Clone)]
pub enum Img {
    Loading,
    Ready(image::Handle),
    Failed,
}

pub type Images = HashMap<(InstanceId, Id), Img>;

/// Whether an attachment is shown as an inline image.
pub fn is_image(a: &Attachment) -> bool {
    a.has_thumb && a.content_type.starts_with("image/")
}

/// The size an inline image is shown at: scaled into 400×300, never enlarged.
pub fn display_size(a: &Attachment) -> (f32, f32) {
    match (a.width, a.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => {
            let (w, h) = (w as f32, h as f32);
            let scale = (MAX_IMAGE_WIDTH / w).min(MAX_IMAGE_HEIGHT / h).min(1.0);
            (w * scale, h * scale)
        }
        _ => (300.0, 200.0),
    }
}

/// `1.4 MB`, `820 KB`, `12 B`.
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB * KB {
        format!("{:.1} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[allow(clippy::too_many_arguments)]
pub fn view<'a>(
    ctx: &Context<'a>,
    s: &InstanceStore,
    inst: &InstanceId,
    m: &MessageRow,
    msg: &ChatMessage,
    state: &'a ChannelView,
) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let space = s.space_of(msg.channel_id);
    let hovered = state.hovered == Some(msg.id);
    let id = msg.id;

    let mut body = column![].spacing(3).width(Fill);
    if let Some(reply) = &m.reply {
        let excerpt = if reply.excerpt.is_empty() {
            "original message deleted".to_owned()
        } else {
            reply.excerpt.replace('\n', " ")
        };
        body = body.push(
            mouse_area(
                row![
                    small("↪", fonts).color(theme.text.tertiary.0),
                    small(reply.author.clone(), fonts)
                        .font(fonts.bold())
                        .color(theme.text.secondary.0),
                    small(excerpt, fonts)
                        .color(theme.text.tertiary.0)
                        .wrapping(text::Wrapping::None),
                ]
                .spacing(6),
            )
            .on_press(Message::JumpTo(reply.id))
            .interaction(iced::mouse::Interaction::Pointer),
        );
    }
    let time_text = time::format(
        msg.created_at() as i64,
        &ctx.config.buffer.timestamp_format,
        ctx.utc_offset,
    );
    if !m.grouped {
        let name = s.display_name(space, msg.author.id);
        let color = space
            .and_then(|sp| super::author_color(s, sp, msg.author.id))
            .map_or(theme.roles.default_color.0, super::rgb);
        let author = msg.author.id;
        body = body.push(
            row![
                mouse_area(label(name, fonts).font(fonts.bold()).color(color))
                    .on_press(Message::Author(author))
                    .on_right_press(Message::Author(author))
                    .interaction(iced::mouse::Interaction::Pointer),
                small(time_text.clone(), fonts).color(theme.text.tertiary.0),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }

    let revealed = state.revealed.contains(&id);
    let blocks = markup::blocks(&msg.content, s, space);
    let last = blocks.len().saturating_sub(1);
    for (i, block) in blocks.into_iter().enumerate() {
        let edited = i == last && msg.edited_at.is_some();
        body = body.push(block_view(ctx, block, id, m.system, revealed, edited));
    }
    if msg.content.is_empty() && msg.edited_at.is_some() {
        body = body.push(small("(edited)", fonts).color(theme.text.tertiary.0));
    }
    for a in &msg.attachments {
        body = body.push(attachment_view(ctx, inst, a));
    }
    if !msg.reactions.is_empty() {
        let mut chips = row![].spacing(4);
        for r in &msg.reactions {
            let mine = r.me;
            chips = chips.push(
                button(small(format!("{} {}", r.emoji, r.count), fonts))
                    .padding([1, 8])
                    .style(move |_, st| reaction_style(theme, st, mine))
                    .on_press(Message::React(id, r.emoji.clone(), !mine)),
            );
        }
        body = body.push(chips.wrap());
    }

    // Gutter: avatar on the first message of a group, the time on hover for the others.
    let gutter: Element<'a, Message> = if !m.grouped && ctx.config.buffer.show_avatars {
        mouse_area(avatar(&s.display_name(space, msg.author.id), fonts))
            .on_press(Message::Author(msg.author.id))
            .on_right_press(Message::Author(msg.author.id))
            .interaction(iced::mouse::Interaction::Pointer)
            .into()
    } else if m.grouped && hovered {
        container(small(time_text, fonts).color(theme.text.tertiary.0))
            .padding([3, 0])
            .into()
    } else {
        Space::new().into()
    };
    let line = row![container(gutter).width(Length::Fixed(GUTTER)), body];

    let highlight = m.mentions_me;
    let jumped = state.highlight == Some(id);
    let content = container(line)
        .padding(super::row_padding(m.grouped))
        .width(Fill)
        .style(move |_| container::Style {
            border: if jumped {
                iced::Border {
                    color: theme.pane.focused_border.0,
                    width: 1.0,
                    radius: 4.0.into(),
                }
            } else {
                iced::Border::default()
            },
            background: if jumped {
                Some(theme.pane.focused_border.0.scale_alpha(0.15).into())
            } else if highlight {
                Some(theme.text.mention_bg.0.into())
            } else if hovered {
                Some(theme.sidebar.hover.0.scale_alpha(0.35).into())
            } else {
                None
            },
            ..container::Style::default()
        });

    let mut layers = stack![content];
    if hovered {
        layers = layers.push(
            container(toolbar(
                ctx,
                id,
                state.palette == Some(id),
                s.me_id() == Some(msg.author.id) && !m.system,
            ))
            .align_right(Fill)
            .padding([0, 12]),
        );
    }
    // Right-click anywhere on the message opens its menu (the author has their own).
    mouse_area(layers)
        .on_enter(Message::Hover(id, true))
        .on_exit(Message::Hover(id, false))
        .on_right_press(Message::More(id))
        .into()
}

fn toolbar<'a>(ctx: &Context<'a>, id: Id, palette: bool, own: bool) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let mut bar = row![].spacing(2).align_y(Alignment::Center);
    let tool = |label_text: &'a str| {
        button(small(label_text, fonts))
            .padding([2, 8])
            .style(move |_, st| style::sidebar_item(theme, st, false))
    };
    if palette {
        for emoji in QUICK_REACTIONS {
            bar = bar.push(tool(emoji).on_press(Message::React(id, emoji.to_owned(), true)));
        }
        bar = bar.push(tool("×").on_press(Message::Palette(None)));
    } else {
        bar = bar
            .push(tool("React").on_press(Message::Palette(Some(id))))
            .push(tool("Reply").on_press(Message::Reply(id)));
        if own {
            bar = bar.push(tool("Edit").on_press(Message::StartEdit(id)));
        }
        bar = bar.push(tool("⋯").on_press(Message::More(id)));
    }
    container(bar)
        .padding(2)
        .style(move |_| container::Style {
            background: Some(theme.sidebar.background.0.into()),
            border: Border {
                color: theme.general.border.0,
                width: 1.0,
                radius: 6.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn avatar<'a>(name: &str, fonts: &crate::font::Fonts) -> Element<'a, Message> {
    let initial: String = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into());
    // A stable color per name.
    let hue = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    let palette = [
        Color::from_rgb8(0x58, 0x65, 0xf2),
        Color::from_rgb8(0x3b, 0xa5, 0x5d),
        Color::from_rgb8(0xfa, 0xa6, 0x1a),
        Color::from_rgb8(0xed, 0x42, 0x45),
        Color::from_rgb8(0xeb, 0x45, 0x9e),
        Color::from_rgb8(0x1a, 0xbc, 0x9c),
    ];
    let bg = palette[(hue % palette.len() as u32) as usize];
    container(
        text(initial)
            .font(fonts.bold())
            .size(fonts.size + 1.0)
            .color(Color::WHITE),
    )
    .center_x(32)
    .center_y(32)
    .style(move |_| container::Style {
        background: Some(bg.into()),
        border: Border {
            radius: 16.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    })
    .into()
}

fn block_view<'a>(
    ctx: &Context<'a>,
    block: Block,
    id: Id,
    system: bool,
    revealed: bool,
    edited: bool,
) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    match block {
        Block::Text(pieces) => {
            let mut spans: Vec<Span<'a, Link, Font>> = pieces
                .into_iter()
                .map(|p| piece_span(ctx, p, system, revealed))
                .collect();
            if edited {
                spans.push(
                    iced::widget::span(" (edited)")
                        .font(fonts.ui)
                        .size(fonts.small())
                        .color(theme.text.tertiary.0),
                );
            }
            rich_text(spans)
                .size(fonts.size)
                .on_link_click(move |link| Message::Link(id, link))
                .into()
        }
        Block::Code { code, .. } => {
            let mut content = column![
                container(text(code).font(fonts.mono).size(fonts.small()))
                    .padding(8)
                    .width(Fill)
                    .style(move |_| container::Style {
                        background: Some(theme.text.code_bg.0.into()),
                        border: Border {
                            color: theme.general.border.0,
                            width: 1.0,
                            radius: 4.0.into(),
                        },
                        ..container::Style::default()
                    })
            ];
            if edited {
                content = content.push(small("(edited)", fonts).color(theme.text.tertiary.0));
            }
            content.into()
        }
        Block::Quote(pieces) => {
            let spans: Vec<Span<'a, Link, Font>> = pieces
                .into_iter()
                .map(|p| piece_span(ctx, p, system, revealed))
                .collect();
            row![
                container(Space::new().width(3).height(Fill)).style(move |_| container::Style {
                    background: Some(theme.text.tertiary.0.into()),
                    ..container::Style::default()
                }),
                rich_text(spans)
                    .size(fonts.size)
                    .on_link_click(move |link| Message::Link(id, link)),
            ]
            .spacing(8)
            .height(Length::Shrink)
            .into()
        }
    }
}

fn piece_span<'a>(
    ctx: &Context<'a>,
    p: Piece,
    system: bool,
    revealed: bool,
) -> Span<'a, Link, Font> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    let mut font = if p.code { fonts.mono } else { fonts.ui };
    if p.bold {
        font.weight = Weight::Bold;
    }
    if p.italic || system {
        font.style = FontStyle::Italic;
    }
    let mut color = if system {
        theme.text.secondary.0
    } else if p.link.as_ref().is_some_and(|l| matches!(l, Link::Url(_))) {
        theme.text.link.0
    } else {
        theme.text.primary.0
    };
    let mut background: Option<Background> = None;
    if p.code {
        background = Some(theme.text.code_bg.0.into());
    }
    if p.mention {
        color = p.color.map_or(theme.text.link.0, super::rgb);
        background = Some(theme.text.link.0.scale_alpha(0.15).into());
    }
    let hidden = p.spoiler && !revealed;
    if hidden {
        color = theme.text.code_bg.0;
        background = Some(theme.text.code_bg.0.into());
    } else if p.spoiler {
        background = Some(theme.text.code_bg.0.into());
    }
    let link = match p.link {
        Some(Link::Spoiler) if !hidden => None,
        other => other,
    };
    let mut span = iced::widget::span(p.text)
        .font(font)
        .color(color)
        .underline(p.underline && !hidden)
        .strikethrough(p.strike && !hidden)
        .link_maybe(link);
    if let Some(bg) = background {
        span = span.background(bg);
    }
    if p.code {
        span = span.size(fonts.size - 1.0);
    }
    span
}

fn attachment_view<'a>(
    ctx: &Context<'a>,
    inst: &InstanceId,
    a: &Attachment,
) -> Element<'a, Message> {
    let fonts = ctx.fonts;
    let theme = ctx.theme;
    if is_image(a) && ctx.config.buffer.image_previews {
        let (w, h) = display_size(a);
        let content: Element<'a, Message> = match ctx.images.get(&(inst.clone(), a.id)) {
            Some(Img::Ready(handle)) => button(image(handle.clone()).width(w).height(h))
                .padding(0)
                .style(|_, _| button::Style::default())
                .on_press(Message::OpenImage(a.clone()))
                .into(),
            Some(Img::Failed) => placeholder("Image not available", w, h, ctx),
            _ => placeholder("Loading…", w, h, ctx),
        };
        return content;
    }
    container(
        row![
            column![
                label(a.filename.clone(), fonts).color(theme.text.link.0),
                small(human_size(a.size), fonts).color(theme.text.tertiary.0),
            ]
            .spacing(2)
            .width(Length::Shrink),
            Space::new().width(16),
            button(small("Download", fonts))
                .padding([3, 10])
                .style(move |_, st| style::secondary(theme, st))
                .on_press(Message::Download(a.clone())),
        ]
        .align_y(Alignment::Center),
    )
    .padding(10)
    .style(move |_| container::Style {
        background: Some(theme.text.code_bg.0.into()),
        border: Border {
            color: theme.general.border.0,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn placeholder<'a>(text: &'a str, w: f32, h: f32, ctx: &Context<'a>) -> Element<'a, Message> {
    let theme = ctx.theme;
    container(small(text, ctx.fonts).color(theme.text.tertiary.0))
        .center_x(w)
        .center_y(h)
        .style(move |_| container::Style {
            background: Some(theme.text.code_bg.0.into()),
            border: Border {
                radius: 6.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

fn reaction_style(theme: &Theme, status: button::Status, mine: bool) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(
            if mine {
                theme.buttons.primary.background.0.scale_alpha(0.25)
            } else if hovered {
                theme.sidebar.hover.0
            } else {
                theme.text.code_bg.0
            }
            .into(),
        ),
        text_color: theme.text.primary.0,
        border: Border {
            color: if mine {
                theme.buttons.primary.background.0
            } else {
                theme.general.border.0
            },
            width: 1.0,
            radius: 10.0.into(),
        },
        ..button::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(w: Option<u32>, h: Option<u32>) -> Attachment {
        Attachment {
            id: Id(1),
            filename: "a.png".into(),
            content_type: "image/png".into(),
            size: 10,
            width: w,
            height: h,
            has_thumb: true,
        }
    }

    #[test]
    fn images_fit_into_400_by_300() {
        assert_eq!(
            display_size(&attachment(Some(800), Some(600))),
            (400.0, 300.0)
        );
        assert_eq!(
            display_size(&attachment(Some(1000), Some(200))),
            (400.0, 80.0)
        );
        assert_eq!(
            display_size(&attachment(Some(100), Some(50))),
            (100.0, 50.0)
        );
        assert_eq!(display_size(&attachment(None, None)), (300.0, 200.0));
    }

    #[test]
    fn sizes_are_human() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(820 * 1024), "820 KB");
        assert_eq!(human_size(1_468_006), "1.4 MB");
    }
}
