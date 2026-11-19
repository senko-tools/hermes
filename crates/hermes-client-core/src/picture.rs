//! Scaling pictures that have see-through parts. The `image` crate's scalers mix the colour of
//! neighbouring pixels as they are, which counts the (usually black) colour of fully clear
//! pixels in: soft edges get a dark rim. Here the colour is weighted by alpha first
//! (premultiplied) and divided back after, so clear pixels add nothing.

use image::RgbaImage;
use image::imageops::{self, FilterType};

/// Whether any pixel is partly or fully see-through.
fn has_alpha(pixels: &RgbaImage) -> bool {
    pixels.as_raw().as_chunks::<4>().0.iter().any(|p| p[3] != u8::MAX)
}

/// Weights each pixel's colour by its alpha.
pub fn premultiply(pixels: &mut RgbaImage) {
    for p in pixels.as_mut().as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        for c in &mut p[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
    }
}

/// Undoes [`premultiply`].
pub fn unpremultiply(pixels: &mut RgbaImage) {
    for p in pixels.as_mut().as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        if a == 0 {
            p[..3].fill(0);
            continue;
        }
        if a == 255 {
            continue;
        }
        for c in &mut p[..3] {
            *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
        }
    }
}

/// `pixels` scaled to exactly `width` × `height` with `filter`, see-through parts kept clean.
pub fn resize(pixels: &RgbaImage, width: u32, height: u32, filter: FilterType) -> RgbaImage {
    if !has_alpha(pixels) {
        return imageops::resize(pixels, width, height, filter);
    }
    let mut weighted = pixels.clone();
    premultiply(&mut weighted);
    let mut out = imageops::resize(&weighted, width, height, filter);
    unpremultiply(&mut out);
    out
}

/// `pixels` shrunk to at most `side` on its long side, keeping its shape; as it is when smaller.
pub fn fit(pixels: RgbaImage, side: u32, filter: FilterType) -> RgbaImage {
    let (w, h) = pixels.dimensions();
    if w <= side && h <= side {
        return pixels;
    }
    let scale = f64::from(side) / f64::from(w.max(h));
    let (nw, nh) = (
        ((f64::from(w) * scale).round() as u32).max(1),
        ((f64::from(h) * scale).round() as u32).max(1),
    );
    resize(&pixels, nw, nh, filter)
}

/// Whether some pixel is partly see-through (not just fully clear or fully solid): a GIF,
/// which has only clear and solid, would lose it.
pub fn has_soft_alpha(pixels: &RgbaImage) -> bool {
    pixels
        .as_raw()
        .as_chunks::<4>().0.iter()
        .any(|p| p[3] != 0 && p[3] != u8::MAX)
}

/// `pixels` shrunk quickly to fit `max_width` × `max_height`, keeping its shape (like
/// `DynamicImage::thumbnail`), see-through parts kept clean.
pub fn thumbnail(pixels: &RgbaImage, max_width: u32, max_height: u32) -> RgbaImage {
    let (w, h) = pixels.dimensions();
    let scale = (f64::from(max_width) / f64::from(w)).min(f64::from(max_height) / f64::from(h));
    let (nw, nh) = (
        ((f64::from(w) * scale).round() as u32).max(1),
        ((f64::from(h) * scale).round() as u32).max(1),
    );
    if !has_alpha(pixels) {
        return imageops::thumbnail(pixels, nw, nh);
    }
    let mut weighted = pixels.clone();
    premultiply(&mut weighted);
    let mut out = imageops::thumbnail(&weighted, nw, nh);
    unpremultiply(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// Half white and opaque, half fully clear (stored as black, as editors often do).
    fn edge() -> RgbaImage {
        RgbaImage::from_fn(8, 8, |x, _| {
            if x < 4 {
                Rgba([255, 255, 255, 255])
            } else {
                Rgba([0, 0, 0, 0])
            }
        })
    }

    #[test]
    fn soft_edges_get_no_dark_rim() {
        for out in [
            resize(&edge(), 4, 4, FilterType::Triangle),
            resize(&edge(), 3, 3, FilterType::Lanczos3),
            thumbnail(&edge(), 3, 3),
        ] {
            for p in out.pixels().filter(|p| p[3] > 0) {
                assert!(p[0] >= 250 && p[1] >= 250 && p[2] >= 250, "{p:?}");
            }
        }
    }

    #[test]
    fn opaque_pictures_are_unchanged_by_the_weighting() {
        let mut p = RgbaImage::from_pixel(2, 2, Rgba([10, 200, 30, 255]));
        let before = p.clone();
        premultiply(&mut p);
        unpremultiply(&mut p);
        assert_eq!(p, before);
    }

    #[test]
    fn thumbnails_keep_the_shape() {
        let out = thumbnail(&RgbaImage::new(400, 100), 100, 100);
        assert_eq!(out.dimensions(), (100, 25));
    }
}
