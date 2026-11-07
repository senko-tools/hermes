//! Round avatars: the user's picture when loaded, else their initial on a color picked from
//! their name, so a person keeps the same color everywhere.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use iced::widget::{Space, container, image, stack, text};
use iced::{Border, Color, Element, Length};

use hermes_client_core::InstanceId;
use hermes_client_core::proto::models::User;

use crate::buffer::channel::message::{Avatar, Images};
use crate::font::Fonts;

/// The sizes (in physical pixels) every avatar is resized to. The renderer samples images
/// bilinearly without mipmaps, which turns a big picture drawn small into jagged pixels; drawing
/// the copy closest to the size on screen keeps it smooth.
pub const SIZES: [u32; 4] = [32, 64, 128, 256];

/// An avatar at each of [`SIZES`], cropped to a square.
#[derive(Debug, Clone)]
pub struct Picture(Arc<[image::Handle]>);

impl Picture {
    /// The smallest copy at least `pixels` physical pixels wide (the largest if none is).
    fn for_pixels(&self, pixels: f32) -> &image::Handle {
        SIZES
            .iter()
            .position(|&s| s as f32 >= pixels)
            .and_then(|i| self.0.get(i))
            .or_else(|| self.0.last())
            .expect("a picture has every size")
    }
}

/// Decodes an avatar thumbnail, crops it to its center square and resizes it to every size of
/// [`SIZES`] with a Lanczos filter, as RGBA. Slow: run it off the UI thread.
pub fn resize(bytes: &[u8]) -> Result<Vec<(u32, Vec<u8>)>, String> {
    use ::image::imageops::{FilterType, crop_imm, resize};
    let decoded = ::image::load_from_memory(bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let (w, h) = decoded.dimensions();
    let side = w.min(h);
    if side == 0 {
        return Err("empty image".into());
    }
    let square = crop_imm(&decoded, (w - side) / 2, (h - side) / 2, side, side).to_image();
    Ok(SIZES
        .iter()
        .map(|&size| {
            let scaled = resize(&square, size, size, FilterType::Lanczos3);
            (size, scaled.into_raw())
        })
        .collect())
}

/// Builds the picture from [`resize`]'s output (on the UI thread: handles are cheap).
pub fn picture_from(sizes: Vec<(u32, Vec<u8>)>) -> Picture {
    Picture(
        sizes
            .into_iter()
            .map(|(size, rgba)| image::Handle::from_rgba(size, size, rgba))
            .collect(),
    )
}

/// The window's scale factor (from the OS), as `f32` bits. Avatars pick their copy by
/// physical size, and the views don't otherwise know the display's scale.
static WINDOW_SCALE: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0

pub fn set_window_scale(scale: f32) {
    if scale.is_finite() && scale > 0.0 {
        WINDOW_SCALE.store(scale.to_bits(), Ordering::Relaxed);
    }
}

fn window_scale() -> f32 {
    f32::from_bits(WINDOW_SCALE.load(Ordering::Relaxed))
}

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

/// `user`'s picture, once its thumbnail is loaded (`App::load_avatars` fetches them).
pub fn picture(images: &Images, inst: &InstanceId, user: Option<&User>) -> Option<Picture> {
    let avatar = user?.avatar.as_ref()?;
    match images.avatars.get(&(inst.clone(), avatar.id)) {
        Some(Avatar::Ready(picture)) => Some(picture.clone()),
        _ => None,
    }
}

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
    picture: Option<&Picture>,
    size: f32,
    fonts: &Fonts,
) -> Element<'a, M> {
    if let Some(picture) = picture {
        let pixels = size * fonts.scale * window_scale();
        return image(picture.for_pixels(pixels).clone())
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
            .color(INITIAL_TEXT)
            // A tight line box, so the letter is centered on its glyph, not on the font's
            // ascender and descender space.
            .line_height(1.0),
    )
    // Capitals sit above the middle of their line; this moves the letter down by half of it.
    .padding(iced::Padding::ZERO.top(size * 0.06))
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
            .color(INITIAL_TEXT)
            // A tight line box, so the letter is centered on its glyph, not on the font's
            // ascender and descender space.
            .line_height(1.0),
    )
    // Capitals sit above the middle of their line; this moves the letter down by half of it.
    .padding(iced::Padding::ZERO.top(size * 0.06))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A 300×200 PNG: red on the left 50 px, green in the middle, blue on the right 50 px.
    fn wide_png() -> Vec<u8> {
        let img = ::image::RgbaImage::from_fn(300, 200, |x, _| match x {
            0..50 => ::image::Rgba([255, 0, 0, 255]),
            250.. => ::image::Rgba([0, 0, 255, 255]),
            _ => ::image::Rgba([0, 255, 0, 255]),
        });
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, ::image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn resize_crops_the_center_square_to_every_size() {
        let sizes = resize(&wide_png()).unwrap();
        assert_eq!(
            sizes.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            SIZES.to_vec()
        );
        for (size, rgba) in &sizes {
            assert_eq!(rgba.len(), (size * size * 4) as usize);
            // The center square is the 200 px from x = 50: all green, no red or blue edge.
            for px in rgba.as_chunks::<4>().0 {
                assert!(px[1] > 200 && px[0] < 40 && px[2] < 40, "{px:?} at {size}");
            }
        }
    }

    #[test]
    fn picks_the_smallest_copy_that_is_big_enough() {
        let picture = picture_from(resize(&wide_png()).unwrap());
        let index = |pixels: f32| {
            let chosen = picture.for_pixels(pixels).id();
            picture.0.iter().position(|h| h.id() == chosen).unwrap()
        };
        assert_eq!(index(16.0), 0);
        assert_eq!(index(32.0), 0);
        assert_eq!(index(40.0), 1);
        assert_eq!(index(144.0), 3);
        assert_eq!(index(1000.0), 3);
    }

    #[test]
    fn rejects_garbage() {
        assert!(resize(b"not an image").is_err());
    }
}
