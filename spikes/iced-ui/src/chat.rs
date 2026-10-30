//! 10,000 fake chat rows and a virtualized list that only builds the visible rows.

use iced::widget::{Column, container, row, scrollable, space, text};
use iced::{Color, Element, Fill, Font, font};

/// Every row has the same height so the visible range follows directly from the scroll offset.
pub const ROW_HEIGHT: f32 = 28.0;
/// Rows built above and below the viewport so fast scrolling does not show gaps.
const OVERSCAN: usize = 10;

pub struct ChatRow {
    pub time: String,
    pub author: &'static str,
    pub color: Color,
    pub body: String,
}

const AUTHORS: [(&str, Color); 6] = [
    ("alice", Color::from_rgb(0.95, 0.55, 0.55)),
    ("bob", Color::from_rgb(0.55, 0.75, 0.95)),
    ("carol", Color::from_rgb(0.65, 0.90, 0.60)),
    ("dave", Color::from_rgb(0.95, 0.80, 0.45)),
    ("erin", Color::from_rgb(0.80, 0.65, 0.95)),
    ("frank", Color::from_rgb(0.55, 0.90, 0.90)),
];

/// Mixed content: plain text, single emoji, skin tones, ZWJ sequences, flags, VS16, CJK, RTL.
const BODIES: [&str; 16] = [
    "hey, anyone up for voice later?",
    "sure 😀",
    "👍🏽 sounds good",
    "the family is here 👨‍👩‍👧‍👦 and the dog 🐕",
    "greetings from 🇫🇮 and 🇯🇵",
    "❤️ ❤ ♥️ ✔️ ☑️ ⚠️",
    "spooky season 🎃👻🦇🕸️",
    "こんにちは、元気ですか？ 🍣",
    "مرحبا بالجميع 👋",
    "lol 😂😂😂 that was great",
    "brb, making coffee ☕",
    "🧑🏻‍💻 👩🏿‍🚀 🏳️‍🌈 🏴‍☠️",
    "a much longer message that goes on and on to see how a long line is clipped when it does not fit in the pane width at all",
    "ok",
    "has anyone tried the new build? it crashed for me 🐛",
    "GG 🎉🎉",
];

/// Deterministic fake history (a simple LCG, so every run looks the same).
pub fn fake_rows(count: usize) -> Vec<ChatRow> {
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as usize
    };
    let mut minutes = 8 * 60;
    (0..count)
        .map(|i| {
            let (author, color) = AUTHORS[next() % AUTHORS.len()];
            minutes += next() % 3;
            ChatRow {
                time: format!("{:02}:{:02}", (minutes / 60) % 24, minutes % 60),
                author,
                color,
                body: format!("#{} {}", i + 1, BODIES[next() % BODIES.len()]),
            }
        })
        .collect()
}

/// Index range of rows to build for a viewport at `offset_y` with height `height`.
pub fn visible_range(total: usize, offset_y: f32, height: f32) -> std::ops::Range<usize> {
    let first = (offset_y.max(0.0) / ROW_HEIGHT) as usize;
    let shown = (height.max(0.0) / ROW_HEIGHT).ceil() as usize + 1;
    let start = first.saturating_sub(OVERSCAN).min(total);
    let end = (first + shown + OVERSCAN).min(total);
    start..end
}

fn view_row<'a, M: 'a>(r: &'a ChatRow) -> Element<'a, M> {
    let bold = Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    };
    container(
        row![
            text(&r.time)
                .size(12)
                .color(Color::from_rgb(0.5, 0.5, 0.55)),
            text(r.author).font(bold).color(r.color),
            text(&r.body).wrapping(text::Wrapping::None),
        ]
        .spacing(8),
    )
    .height(ROW_HEIGHT)
    .width(Fill)
    .clip(true)
    .padding([4, 8])
    .into()
}

/// The list: a spacer for the rows above, the visible rows, a spacer for the rows below.
pub fn view_list<'a, M: 'a + Clone>(
    rows: &'a [ChatRow],
    range: std::ops::Range<usize>,
    id: iced::widget::Id,
    on_scroll: impl Fn(scrollable::Viewport) -> M + 'a,
) -> Element<'a, M> {
    let above = range.start as f32 * ROW_HEIGHT;
    let below = (rows.len() - range.end) as f32 * ROW_HEIGHT;
    let content = Column::new()
        .push(space().height(above))
        .extend(rows[range].iter().map(view_row))
        .push(space().height(below))
        .width(Fill);
    scrollable(content)
        .id(id)
        .on_scroll(on_scroll)
        .height(Fill)
        .width(Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_rows_are_deterministic() {
        let a = fake_rows(100);
        let b = fake_rows(100);
        assert_eq!(a.len(), 100);
        assert!(a.iter().zip(&b).all(|(x, y)| x.body == y.body));
        assert!(a.iter().any(|r| r.body.contains('🇫')));
    }

    #[test]
    fn visible_range_covers_viewport_plus_overscan() {
        assert_eq!(visible_range(10_000, 0.0, 280.0), 0..21);
        let r = visible_range(10_000, 28.0 * 5_000.0, 280.0);
        assert_eq!(r, 4_990..5_021);
        assert_eq!(visible_range(10_000, 28.0 * 10_000.0, 280.0).end, 10_000);
        assert_eq!(visible_range(5, 0.0, 800.0), 0..5);
    }
}
