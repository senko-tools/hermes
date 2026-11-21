//! Animated WebP, put together here instead of by `image-webp` (0.2.4), which keeps what a frame
//! should clear away ("dispose to background") unless a background colour is set, and then
//! clears opaque frames with the wrong stride. Each frame is cut out of the file as a still WebP,
//! decoded by `image`, and laid onto the canvas the way the WebP container spec says.

use image::{Delay, Frame, Frames, RgbaImage};

/// One frame as the file describes it.
struct Part<'a> {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    duration_ms: u32,
    /// Laid over what is there (else it replaces it).
    blend: bool,
    /// Its area is cleared before the next frame.
    dispose: bool,
    alpha: Option<&'a [u8]>,
    /// The `VP8 ` or `VP8L` chunk: its name and data.
    image: ([u8; 4], &'a [u8]),
}

/// The canvas size and the frames of an animated WebP; `None` for any other file.
fn parse(bytes: &[u8]) -> Option<((u32, u32), Vec<Part<'_>>)> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    let mut canvas = None;
    let mut parts = Vec::new();
    for (name, data) in chunks(&bytes[12..]) {
        match &name {
            b"VP8X" if data.len() >= 10 => {
                canvas = Some((u24(&data[4..]) + 1, u24(&data[7..]) + 1));
            }
            b"ANMF" if data.len() >= 16 => {
                let flags = data[15];
                let mut alpha = None;
                let mut image = None;
                for (inner, body) in chunks(&data[16..]) {
                    match &inner {
                        b"ALPH" => alpha = Some(body),
                        b"VP8 " | b"VP8L" => image = Some((inner, body)),
                        _ => {}
                    }
                }
                parts.push(Part {
                    x: u24(&data[0..]) * 2,
                    y: u24(&data[3..]) * 2,
                    width: u24(&data[6..]) + 1,
                    height: u24(&data[9..]) + 1,
                    duration_ms: u24(&data[12..]),
                    blend: flags & 0b10 == 0,
                    dispose: flags & 0b01 != 0,
                    alpha,
                    image: image?,
                });
            }
            _ => {}
        }
    }
    let canvas = canvas?;
    (!parts.is_empty()).then_some((canvas, parts))
}

fn u24(b: &[u8]) -> u32 {
    u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16
}

/// The RIFF chunks in `bytes`: name and data (each padded to an even length in the file).
fn chunks(mut bytes: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        if bytes.len() < 8 {
            return None;
        }
        let name: [u8; 4] = bytes[..4].try_into().ok()?;
        let len = u32::from_le_bytes(bytes[4..8].try_into().ok()?) as usize;
        let data = bytes.get(8..8 + len)?;
        bytes = bytes.get(8 + len + (len & 1)..).unwrap_or(&[]);
        Some((name, data))
    })
}

/// A chunk as bytes: name, length, data, padding.
fn chunk(out: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(name);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() & 1 == 1 {
        out.push(0);
    }
}

/// A frame on its own, as a still WebP file.
fn still(part: &Part) -> Vec<u8> {
    let mut body = Vec::new();
    if let Some(alpha) = part.alpha.filter(|_| &part.image.0 == b"VP8 ") {
        let mut header = [0u8; 10];
        header[0] = 0x10; // has alpha
        header[4..7].copy_from_slice(&(part.width - 1).to_le_bytes()[..3]);
        header[7..10].copy_from_slice(&(part.height - 1).to_le_bytes()[..3]);
        chunk(&mut body, b"VP8X", &header);
        chunk(&mut body, b"ALPH", alpha);
    }
    chunk(&mut body, &part.image.0, part.image.1);
    let mut out = Vec::with_capacity(body.len() + 12);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32 + 4).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    out
}

/// `top` laid over `under` (straight alpha).
fn over(under: [u8; 4], top: [u8; 4]) -> [u8; 4] {
    let ta = u32::from(top[3]);
    if ta == 255 {
        return top;
    }
    if ta == 0 {
        return under;
    }
    let ua = u32::from(under[3]) * (255 - ta) / 255;
    let a = ta + ua;
    let mut out = [0, 0, 0, a as u8];
    for i in 0..3 {
        out[i] = ((u32::from(top[i]) * ta + u32::from(under[i]) * ua + a / 2) / a) as u8;
    }
    out
}

/// The frames of an animated WebP, each the whole canvas; `None` if it is not one. Frames are
/// decoded as they are asked for.
pub fn frames(bytes: &[u8]) -> Option<Frames<'_>> {
    let ((width, height), parts) = parse(bytes)?;
    let mut canvas = RgbaImage::new(width, height);
    // The area the frame before asked to be cleared.
    let mut clear: Option<(u32, u32, u32, u32)> = None;
    let mut parts = parts.into_iter();
    Some(Frames::new(Box::new(std::iter::from_fn(move || {
        let part = parts.next()?;
        if let Some((x, y, w, h)) = clear.take() {
            for py in y..(y + h).min(height) {
                for px in x..(x + w).min(width) {
                    canvas.put_pixel(px, py, image::Rgba([0, 0, 0, 0]));
                }
            }
        }
        let pixels =
            match image::load_from_memory_with_format(&still(&part), image::ImageFormat::WebP) {
                Ok(image) => image.to_rgba8(),
                Err(err) => return Some(Err(err)),
            };
        for (fx, fy, pixel) in pixels.enumerate_pixels() {
            let (cx, cy) = (part.x + fx, part.y + fy);
            if cx >= width || cy >= height {
                continue;
            }
            let value = if part.blend {
                over(canvas.get_pixel(cx, cy).0, pixel.0)
            } else {
                pixel.0
            };
            canvas.put_pixel(cx, cy, image::Rgba(value));
        }
        if part.dispose {
            clear = Some((part.x, part.y, part.width, part.height));
        }
        let delay = Delay::from_numer_denom_ms(part.duration_ms, 1);
        Some(Ok(Frame::from_parts(canvas.clone(), 0, 0, delay)))
    }))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A still picture as a lossless WebP's `VP8L` chunk data.
    fn vp8l(pixels: &RgbaImage) -> Vec<u8> {
        let mut file = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut file)
            .encode(
                pixels.as_raw(),
                pixels.width(),
                pixels.height(),
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        let (name, data) = chunks(&file[12..])
            .find(|(n, _)| n == b"VP8L")
            .expect("a VP8L chunk");
        assert_eq!(&name, b"VP8L");
        data.to_vec()
    }

    /// An animated WebP of `frames`: (x, y, picture, blend, dispose).
    fn animation(size: (u32, u32), frames: &[(u32, u32, RgbaImage, bool, bool)]) -> Vec<u8> {
        let mut body = Vec::new();
        let mut header = [0u8; 10];
        header[0] = 0x12; // animation, alpha
        header[4..7].copy_from_slice(&(size.0 - 1).to_le_bytes()[..3]);
        header[7..10].copy_from_slice(&(size.1 - 1).to_le_bytes()[..3]);
        chunk(&mut body, b"VP8X", &header);
        chunk(&mut body, b"ANIM", &[0, 0, 0, 0, 0, 0]);
        for (x, y, pixels, blend, dispose) in frames {
            let mut anmf = Vec::new();
            anmf.extend_from_slice(&(x / 2).to_le_bytes()[..3]);
            anmf.extend_from_slice(&(y / 2).to_le_bytes()[..3]);
            anmf.extend_from_slice(&(pixels.width() - 1).to_le_bytes()[..3]);
            anmf.extend_from_slice(&(pixels.height() - 1).to_le_bytes()[..3]);
            anmf.extend_from_slice(&100u32.to_le_bytes()[..3]);
            anmf.push(u8::from(!blend) << 1 | u8::from(*dispose));
            chunk(&mut anmf, b"VP8L", &vp8l(pixels));
            chunk(&mut body, b"ANMF", &anmf);
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32 + 4).to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend_from_slice(&body);
        out
    }

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba(rgba))
    }

    #[test]
    fn a_disposed_frame_leaves_nothing_behind() {
        let file = animation(
            (4, 4),
            &[
                (0, 0, solid(4, 4, [255, 0, 0, 255]), true, true),
                (0, 0, solid(2, 2, [0, 255, 0, 255]), true, false),
            ],
        );
        let frames: Vec<_> = frames(&file).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(frames.len(), 2);
        let second = frames[1].buffer();
        assert_eq!(second.get_pixel(0, 0).0, [0, 255, 0, 255]);
        // The red of the first frame was cleared away, not kept.
        assert_eq!(second.get_pixel(3, 3).0[3], 0);
        assert_eq!(frames[1].delay().numer_denom_ms(), (100, 1));
    }

    #[test]
    fn a_kept_frame_shows_under_the_next() {
        let file = animation(
            (4, 4),
            &[
                (0, 0, solid(4, 4, [255, 0, 0, 255]), true, false),
                (2, 2, solid(2, 2, [0, 0, 255, 128]), true, false),
            ],
        );
        let frames: Vec<_> = frames(&file).unwrap().collect::<Result<_, _>>().unwrap();
        let second = frames[1].buffer();
        assert_eq!(second.get_pixel(0, 0).0, [255, 0, 0, 255]);
        // Half blue over red.
        let mixed = second.get_pixel(3, 3).0;
        assert_eq!(mixed[3], 255);
        assert!(mixed[0] > 100 && mixed[2] > 100, "{mixed:?}");
    }

    #[test]
    fn without_blending_a_frame_replaces_what_is_there() {
        let file = animation(
            (2, 2),
            &[
                (0, 0, solid(2, 2, [255, 0, 0, 255]), true, false),
                (0, 0, solid(2, 2, [0, 0, 255, 0]), false, false),
            ],
        );
        let frames: Vec<_> = frames(&file).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(frames[1].buffer().get_pixel(1, 1).0[3], 0);
    }

    #[test]
    fn other_files_are_not_animations() {
        assert!(frames(b"GIF89a").is_none());
        let mut still = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut still)
            .encode(&[0; 16], 2, 2, image::ExtendedColorType::Rgba8)
            .unwrap();
        assert!(frames(&still).is_none());
    }
}
