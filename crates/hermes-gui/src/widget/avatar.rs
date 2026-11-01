//! Round avatars: the user's picture when loaded, else their initial on a color picked from
//! their name, so a person keeps the same color everywhere.

use iced::widget::{Space, container, image, stack, text};
use iced::{Border, Color, Element, Length};

use crate::font::Fonts;

/// Initial backgrounds; light enough for dark text in both themes.
const PALETTE: [Color; 6] = [
    Color::from_rgb8(0xe5, 0xa5, 0x50),
    Color::from_rgb8(0x7f, 0xb4, 0xca),
    Color::from_rgb8(0xd0, 0x8c, 0xa4),
    Color::from_rgb8(0x8f, 0xc4, 0x86),
    Color::from_rgb8(0xa8, 0x98, 0xe0),
    Color::from_rgb8(0xe0, 0x8c, 0x70),
];
const INITIAL_TEXT: Color = Color::from_rgb8(0x1d, 0x1c, 0x22);

/// The stable color for `name`.
pub fn color(name: &str) -> Color {
    let hue = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    PALETTE[(hue % PALETTE.len() as u32) as usize]
}

/// A `size` px circle with `picture`, or `name`'s initial.
pub fn view<'a, M: 'a>(
    name: &str,
    picture: Option<&image::Handle>,
    size: f32,
    fonts: &Fonts,
) -> Element<'a, M> {
    if let Some(handle) = picture {
        return image(handle.clone())
            .width(size)
            .height(size)
            .content_fit(iced::ContentFit::Cover)
            .border_radius(size / 2.0)
            .into();
    }
    let initial: String = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into());
    let bg = color(name);
    container(
        text(initial)
            .font(fonts.bold())
            .size((size * 0.45).round())
            .color(INITIAL_TEXT),
    )
    .center_x(size)
    .center_y(size)
    .style(move |_| container::Style {
        background: Some(bg.into()),
        border: Border {
            radius: (size / 2.0).into(),
            ..Border::default()
        },
        ..container::Style::default()
    })
    .into()
}

/// `avatar` with a presence dot in its bottom right corner, ringed in `ring` (the color
/// behind it) so it reads on any picture.
pub fn with_dot<'a, M: 'a>(
    avatar: Element<'a, M>,
    size: f32,
    dot: Color,
    ring: Color,
) -> Element<'a, M> {
    let d = (size * 0.36).round().max(8.0);
    let marker = container(Space::new().width(d).height(d)).style(move |_| container::Style {
        background: Some(dot.into()),
        border: Border {
            color: ring,
            width: 2.0,
            radius: (d / 2.0).into(),
        },
        ..container::Style::default()
    });
    stack![
        avatar,
        container(marker)
            .width(Length::Fixed(size + 2.0))
            .height(Length::Fixed(size + 2.0))
            .align_right(Length::Fixed(size + 2.0))
            .align_bottom(Length::Fixed(size + 2.0)),
    ]
    .into()
}

/// A rounded square with the first letter of a space's name.
pub fn space_icon<'a, M: 'a>(name: &str, size: f32, fonts: &Fonts) -> Element<'a, M> {
    let initial: String = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into());
    let bg = color(name);
    container(
        text(initial)
            .font(fonts.bold())
            .size((size * 0.5).round())
            .color(INITIAL_TEXT),
    )
    .center_x(size)
    .center_y(size)
    .style(move |_| container::Style {
        background: Some(bg.into()),
        border: Border {
            radius: (size * 0.3).into(),
            ..Border::default()
        },
        ..container::Style::default()
    })
    .into()
}
