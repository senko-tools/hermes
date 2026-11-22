//! Animated WebP, decoded by libwebp's animation decoder: it lays each frame onto the canvas the
//! way the WebP container spec says (dispose, blend) and is several times faster than `image-webp`
//! (15 against 54 ms for a full-HD lossy frame on one thread). `image-webp` 0.2.4 also kept what a
//! frame should clear away ("dispose to background").

use std::marker::PhantomData;
use std::ptr::NonNull;

use image::{Delay, Frame, Frames, RgbaImage};
use libwebp_sys as webp;

/// The canvas size and frame count of an animated WebP, read from its chunks only; `None` for any
/// other file (a still WebP too).
pub fn size_and_count(bytes: &[u8]) -> Option<((u32, u32), usize)> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    let mut canvas = None;
    let mut count = 0;
    for (name, data) in chunks(&bytes[12..]) {
        match &name {
            b"VP8X" if data.len() >= 10 => {
                canvas = Some((u24(&data[4..]) + 1, u24(&data[7..]) + 1));
            }
            b"ANMF" if data.len() >= 16 => count += 1,
            _ => {}
        }
    }
    canvas.filter(|_| count > 0).map(|canvas| (canvas, count))
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

/// libwebp's animation decoder over `bytes`, which it reads in place.
pub struct Decoder<'a> {
    dec: NonNull<webp::WebPAnimDecoder>,
    width: u32,
    height: u32,
    /// Where the frame before ended, in ms from the start.
    ended: i32,
    _bytes: PhantomData<&'a [u8]>,
}

// SAFETY: the decoder is only used through `&mut self` (one thread at a time) and holds no
// thread-local state; the bytes it reads are borrowed for `'a` and never written.
#[allow(unsafe_code)]
unsafe impl Send for Decoder<'_> {}

impl<'a> Decoder<'a> {
    /// A decoder of whole frames in BGRA (what gpui draws) or, without `bgra`, RGBA; `None` when
    /// `bytes` are not an animated WebP.
    #[allow(unsafe_code)]
    pub fn new(bytes: &'a [u8], bgra: bool) -> Option<Self> {
        size_and_count(bytes)?;
        // SAFETY: the options are initialised by libwebp before use; `data` points into `bytes`,
        // which outlive the decoder (`'a`); a null decoder is turned into `None`.
        unsafe {
            let mut options: webp::WebPAnimDecoderOptions = std::mem::zeroed();
            if webp::WebPAnimDecoderOptionsInit(&mut options) == 0 {
                return None;
            }
            options.color_mode = if bgra {
                webp::WEBP_CSP_MODE::MODE_BGRA
            } else {
                webp::WEBP_CSP_MODE::MODE_RGBA
            };
            options.use_threads = 0;
            let data = webp::WebPData {
                bytes: bytes.as_ptr(),
                size: bytes.len(),
            };
            let dec = NonNull::new(webp::WebPAnimDecoderNew(&data, &options))?;
            let mut info: webp::WebPAnimInfo = std::mem::zeroed();
            if webp::WebPAnimDecoderGetInfo(dec.as_ptr(), &mut info) == 0 {
                webp::WebPAnimDecoderDelete(dec.as_ptr());
                return None;
            }
            Some(Decoder {
                dec,
                width: info.canvas_width,
                height: info.canvas_height,
                ended: 0,
                _bytes: PhantomData,
            })
        }
    }

    /// The next whole frame and how long it shows; `None` after the last one.
    #[allow(unsafe_code)]
    pub fn next_frame(&mut self) -> Option<Result<(RgbaImage, Delay), String>> {
        // SAFETY: `dec` is a live decoder; on success `buf` points at its canvas of
        // width × height × 4 bytes, valid until the next call, and is copied out at once.
        unsafe {
            if webp::WebPAnimDecoderHasMoreFrames(self.dec.as_ptr()) == 0 {
                return None;
            }
            let mut buf = std::ptr::null_mut();
            let mut ends = 0;
            if webp::WebPAnimDecoderGetNext(self.dec.as_ptr(), &mut buf, &mut ends) == 0
                || buf.is_null()
            {
                return Some(Err("a WebP frame could not be decoded".into()));
            }
            let len = self.width as usize * self.height as usize * 4;
            let pixels = std::slice::from_raw_parts(buf, len).to_vec();
            let ms = (ends - self.ended).max(0) as u32;
            self.ended = ends;
            let image = RgbaImage::from_raw(self.width, self.height, pixels)?;
            Some(Ok((image, Delay::from_numer_denom_ms(ms, 1))))
        }
    }
}

impl Drop for Decoder<'_> {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: `dec` came from `WebPAnimDecoderNew` and is freed only here.
        unsafe { webp::WebPAnimDecoderDelete(self.dec.as_ptr()) };
    }
}

/// The frames of an animated WebP in RGBA, each the whole canvas; `None` if it is not one.
pub fn frames(bytes: &[u8]) -> Option<Frames<'_>> {
    let mut decoder = Decoder::new(bytes, false)?;
    Some(Frames::new(Box::new(std::iter::from_fn(move || {
        Some(match decoder.next_frame()? {
            Ok((pixels, delay)) => Ok(Frame::from_parts(pixels, 0, 0, delay)),
            Err(err) => Err(image::ImageError::Decoding(
                image::error::DecodingError::new(
                    image::error::ImageFormatHint::Exact(image::ImageFormat::WebP),
                    err,
                ),
            )),
        })
    }))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chunk as bytes: name, length, data, padding.
    fn chunk(out: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(name);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        if data.len() & 1 == 1 {
            out.push(0);
        }
    }

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
