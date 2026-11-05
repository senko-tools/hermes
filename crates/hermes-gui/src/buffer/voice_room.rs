//! The VoiceRoom buffer (PLAN.md section 15.1): the people in a voice room as a grid of tiles
//! that always uses the room best (the column count that makes the tiles largest), with
//! speaking rings and mute/deafen state. Someone sharing their screen shows it in their own
//! tile, and only to people in the call (docs/DECISIONS.md 2026-11-02), once you chose to watch
//! it (docs/DECISIONS.md 2026-11-05). Right-click a tile for that person's volume menu, screen
//! share or not.

use std::collections::{HashMap, HashSet};

use hermes_client_core::proto::id::Id;
use hermes_client_core::{Core, InstanceId};
use iced::widget::{Space, column, container, mouse_area, responsive, row, stack, text};
use iced::{Alignment, Border, Color, Element, Fill, Length, Size};

use crate::font::Fonts;
use crate::message::Message;
use crate::theme::{Theme, style};
use crate::voice::VoiceMsg;
use crate::widget::icon::{self, icon};
use crate::widget::{ButtonKind, caption, icon_button, label, text_button, tip};

/// Tiles keep this aspect ratio (width / height), the shape of most screens.
const ASPECT: f32 = 16.0 / 9.0;
const AVATAR_MAX: f32 = 72.0;

/// One participant card.
#[derive(Debug, Clone, PartialEq)]
pub struct Participant {
    pub user: Id,
    pub name: String,
    pub speaking: bool,
    pub muted: bool,
    pub deafened: bool,
    /// Your volume for them (1.0 = 100 %), or `None` for yourself.
    pub volume: Option<f32>,
    pub locally_muted: bool,
}

/// The people in the room of `channel`, in join order.
pub fn participants(core: &Core, inst: &InstanceId, channel: Id) -> Vec<Participant> {
    let store = core.store();
    let Some(s) = store.instance(inst) else {
        return Vec::new();
    };
    let me = s.me_id();
    let space = s.space_of(channel);
    let mut states: Vec<_> = s
        .voice_states
        .values()
        .filter(|v| v.channel_id == channel)
        .copied()
        .collect();
    states.sort_by_key(|v| (v.joined_at, v.user_id));
    states
        .into_iter()
        .map(|v| {
            let own = Some(v.user_id) == me;
            Participant {
                user: v.user_id,
                name: s.display_name(space, v.user_id),
                speaking: core.is_speaking(inst, v.user_id),
                muted: v.self_mute || v.server_mute,
                deafened: v.self_deaf || v.server_deaf,
                volume: (!own).then(|| core.user_volume(inst, v.user_id)),
                locally_muted: !own && core.user_muted(inst, v.user_id),
            }
        })
        .collect()
}

/// What a tile shows.
#[derive(Debug, Clone)]
struct Tile {
    person: Participant,
    /// They share their screen (known only in the call).
    sharing: bool,
    /// I chose to watch their share.
    watched: bool,
    /// For my own tile while I share: how many watch (`None`: the server does not say).
    watchers: Option<u16>,
    /// For my own tile while I share: what the share keeps when it cannot keep up.
    preference: Option<crate::screen::capture::Preference>,
    /// The latest picture of their share.
    picture: Option<crate::screen::view::Share>,
}

/// The column count and tile size that make `n` tiles of [`ASPECT`] largest in `area` with
/// `gap` between them; on a tie, more columns (side by side reads better than stacked).
pub fn layout(n: usize, area: Size, gap: f32) -> (usize, Size) {
    let mut best = (1, Size::ZERO);
    for cols in 1..=n.max(1) {
        let rows = n.max(1).div_ceil(cols);
        let width = (area.width - gap * (cols - 1) as f32) / cols as f32;
        let height = (area.height - gap * (rows - 1) as f32) / rows as f32;
        let tile_width = width.min(height * ASPECT).max(0.0);
        if tile_width >= best.1.width {
            best = (cols, Size::new(tile_width, tile_width / ASPECT));
        }
    }
    best
}

/// The screen shares a voice room shows.
pub struct Shares<'s> {
    /// The latest picture of each share I watch.
    pub pictures: &'s HashMap<(InstanceId, Id), crate::screen::view::Share>,
    /// The shares I chose to watch.
    pub watching: &'s HashSet<(InstanceId, Id)>,
    /// The share shown alone.
    pub enlarged: Option<Id>,
    /// What my own share keeps when it cannot keep up (changeable on my tile while I share).
    pub preference: crate::screen::capture::Preference,
}

pub fn view<'a>(
    core: &Core,
    inst: &InstanceId,
    channel: Id,
    shares: &Shares<'_>,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    // Joining, leaving and sharing are in the pane header.
    let people = participants(core, inst, channel);
    let in_call = core.voice_channel() == Some((inst.clone(), channel));
    let sharers: HashSet<Id> = if in_call {
        core.screen_sharers()
            .into_iter()
            .filter(|(i, _)| i == inst)
            .map(|(_, user)| user)
            .collect()
    } else {
        HashSet::new()
    };
    let me_sharing = in_call && core.sharing_screen();
    let my_watchers = core.share_watchers();
    let summary = match people.len() {
        0 => "Nobody is here yet".to_owned(),
        1 => "1 person".to_owned(),
        n => format!("{n} people"),
    };
    let mut tiles: Vec<Tile> = people
        .into_iter()
        .map(|person| {
            let own = person.volume.is_none();
            Tile {
                sharing: sharers.contains(&person.user) || (own && me_sharing),
                watched: shares.watching.contains(&(inst.clone(), person.user)),
                watchers: if own { my_watchers } else { None },
                preference: (own && me_sharing).then_some(shares.preference),
                picture: shares.pictures.get(&(inst.clone(), person.user)).cloned(),
                person,
            }
        })
        .collect();
    // An enlarged share is the only tile, as long as it has a picture.
    let enlarged = shares.enlarged.and_then(|user| {
        tiles
            .iter()
            .position(|t| t.person.user == user && t.picture.is_some())
    });
    let summary = match enlarged {
        Some(i) => format!(
            "{summary} · {}'s screen · click it or press Esc to see everyone",
            tiles[i].person.name
        ),
        None => summary,
    };
    if let Some(i) = enlarged {
        tiles = vec![tiles.swap_remove(i)];
    }
    let top = crate::widget::section_title(&summary, fonts).color(theme.text.tertiary.0);
    let inst = inst.clone();
    let fonts = *fonts;
    let enlarged = enlarged.is_some();
    let grid = responsive(move |size| grid(&inst, &tiles, size, enlarged, &fonts, theme));
    column![top, grid]
        .spacing(style::GAP_L)
        .padding(style::GAP_L)
        .into()
}

fn grid<'a>(
    inst: &InstanceId,
    tiles: &[Tile],
    area: Size,
    enlarged: bool,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    if tiles.is_empty() {
        return Space::new().into();
    }
    // An enlarged share takes the whole room, whatever its shape.
    if enlarged && let [t] = tiles {
        return tile(inst, t, area, true, fonts, theme);
    }
    let gap = style::GAP;
    let (cols, size) = layout(tiles.len(), area, gap);
    let mut rows = column![].spacing(gap).align_x(Alignment::Center);
    for chunk in tiles.chunks(cols) {
        let mut line = row![].spacing(gap);
        for t in chunk {
            line = line.push(tile(inst, t, size, false, fonts, theme));
        }
        rows = rows.push(line);
    }
    container(rows).center(Fill).into()
}

fn tile<'a>(
    inst: &InstanceId,
    t: &Tile,
    size: Size,
    enlarged: bool,
    fonts: &Fonts,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let p = &t.person;
    let ring = if p.speaking {
        theme.voice.speaking_ring.0
    } else {
        theme.general.border.0
    };
    let state_icon = if p.deafened {
        Some(icon::HEADPHONES_OFF)
    } else if p.muted || p.locally_muted {
        Some(icon::MIC_OFF)
    } else {
        None
    };
    let mut name_row = row![label(p.name.clone(), fonts).wrapping(text::Wrapping::None)]
        .spacing(style::GAP_XS)
        .align_y(Alignment::Center);
    if let Some(glyph) = state_icon {
        name_row = name_row.push(icon(glyph, 14.0).color(theme.voice.muted.0));
    }
    if t.sharing {
        name_row = name_row.push(icon(icon::MONITOR_SHARE, 14.0).color(theme.accent.base.0));
    }

    let content: Element<'a, Message> =
        match &t.picture {
            // Their screen fills the tile; the name stays readable on a dark badge.
            Some(picture) => {
                let badge = container(name_row)
                    .padding([style::GAP_XS, style::GAP_S])
                    .style(move |_| container::Style {
                        text_color: Some(Color::WHITE),
                        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.6).into()),
                        border: Border {
                            radius: style::radius(theme).into(),
                            ..Border::default()
                        },
                        ..container::Style::default()
                    });
                stack![
                    container(
                        iced::widget::shader(picture.clone())
                            .width(Fill)
                            .height(Fill)
                    )
                    .center(Fill)
                    .style(|_| container::Style {
                        background: Some(Color::BLACK.into()),
                        ..container::Style::default()
                    }),
                    container(badge)
                        .align_left(Fill)
                        .align_bottom(Fill)
                        .padding(style::GAP_S),
                    container(tip(
                        container(
                            icon_button(icon::CLOSE, false, theme).on_press(Message::Voice(
                                VoiceMsg::WatchShare(inst.clone(), p.user, false)
                            ))
                        )
                        .style(|_| container::Style {
                            text_color: Some(Color::WHITE),
                            background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.6).into()),
                            border: Border {
                                radius: style::radius(theme).into(),
                                ..Border::default()
                            },
                            ..container::Style::default()
                        }),
                        "Stop watching",
                        fonts,
                        theme,
                    ))
                    .align_right(Fill)
                    .align_top(Fill)
                    .padding(style::GAP_S),
                ]
                .into()
            }
            None => {
                let side = (size.height * 0.4).clamp(24.0, AVATAR_MAX);
                let initial: String = p
                    .name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().collect())
                    .unwrap_or_default();
                let avatar = container(text(initial).font(fonts.bold()).size(fonts.heading()))
                    .center(Length::Fixed(side))
                    .style(move |_| container::Style {
                        text_color: Some(theme.text.primary.0),
                        background: Some(theme.surface.hover.0.into()),
                        border: Border {
                            radius: (side / 2.0).into(),
                            ..Border::default()
                        },
                        ..container::Style::default()
                    });
                let mut info = column![avatar, name_row]
                    .spacing(style::GAP_S)
                    .align_x(Alignment::Center);
                let note = if t.sharing && p.volume.is_none() {
                    Some(match t.watchers {
                        Some(0) => "You are sharing your screen · nobody is watching".to_owned(),
                        Some(1) => "You are sharing your screen · 1 person is watching".to_owned(),
                        Some(n) => format!("You are sharing your screen · {n} people are watching"),
                        None => "You are sharing your screen".to_owned(),
                    })
                } else if t.sharing && t.watched {
                    Some("Loading their screen…".to_owned())
                } else if t.sharing {
                    Some("Sharing their screen".to_owned())
                } else {
                    p.volume
                        .filter(|v| (*v - 1.0).abs() > 0.001)
                        .map(|v| format!("volume {:.0} %", v * 100.0))
                };
                if let Some(note) = note {
                    info = info.push(caption(note, fonts).color(theme.text.tertiary.0));
                }
                // My own share's quick setting.
                if let Some(preference) = t.preference {
                    info = info.push(crate::voice::share_preference_picker(
                        preference, fonts, theme,
                    ));
                }
                // Someone else's share is received only once I choose to watch it.
                if t.sharing && p.volume.is_some() {
                    let (label, kind) = if t.watched {
                        ("Stop watching", ButtonKind::Secondary)
                    } else {
                        ("Watch", ButtonKind::Primary)
                    };
                    info = info.push(text_button(label, kind, fonts, theme).on_press(
                        Message::Voice(VoiceMsg::WatchShare(inst.clone(), p.user, !t.watched)),
                    ));
                }
                container(info).center(Fill).into()
            }
        };
    let ring_width = if p.speaking { 2.0 } else { 1.0 };
    let card = container(content)
        .width(Length::Fixed(size.width))
        .height(Length::Fixed(size.height))
        .clip(true)
        .style(move |_| container::Style {
            background: Some(theme.surface.raised.0.into()),
            border: Border {
                color: ring,
                width: ring_width,
                radius: style::radius_large(theme).into(),
            },
            ..container::Style::default()
        });
    // The menu is the person's, also over their screen share.
    let mut area = mouse_area(card);
    if p.volume.is_some() {
        area = area.on_right_press(Message::Voice(VoiceMsg::UserMenu(inst.clone(), p.user)));
    }
    // A click on a share I do not watch starts watching it; on a share enlarges it, and on the
    // enlarged one goes back to everyone.
    if t.sharing && !t.watched && p.volume.is_some() {
        area = area
            .on_press(Message::Voice(VoiceMsg::WatchShare(
                inst.clone(),
                p.user,
                true,
            )))
            .interaction(iced::mouse::Interaction::Pointer);
    } else if t.picture.is_some() {
        let target = (!enlarged).then(|| (inst.clone(), p.user));
        area = area
            .on_press(Message::Voice(VoiceMsg::EnlargeShare(target)))
            .interaction(iced::mouse::Interaction::Pointer);
    }
    area.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_makes_tiles_as_large_as_it_can() {
        let wide = Size::new(1600.0, 900.0);
        // One tile fills the room.
        let (cols, size) = layout(1, wide, 0.0);
        assert_eq!(cols, 1);
        assert_eq!(size, Size::new(1600.0, 900.0));
        // Two side by side beat two stacked in a wide room...
        assert_eq!(layout(2, wide, 0.0).0, 2);
        // ...and stack in a tall one.
        assert_eq!(layout(2, Size::new(600.0, 900.0), 0.0).0, 1);
        // Four make a 2×2 grid; five a 3-column one.
        assert_eq!(layout(4, wide, 12.0).0, 2);
        assert_eq!(layout(5, wide, 12.0).0, 3);
        // Tiles never overflow the room, gaps included.
        for n in 1..=25 {
            let (cols, size) = layout(n, wide, 12.0);
            let rows = n.div_ceil(cols) as f32;
            assert!(size.width * cols as f32 + 12.0 * (cols as f32 - 1.0) <= wide.width + 0.01);
            assert!(size.height * rows + 12.0 * (rows - 1.0) <= wide.height + 0.01);
            assert!((size.width / size.height - ASPECT).abs() < 0.01);
        }
        // A room too small for anything gives empty tiles instead of panicking.
        assert_eq!(layout(3, Size::new(0.0, 0.0), 12.0).1, Size::ZERO);
    }
}
