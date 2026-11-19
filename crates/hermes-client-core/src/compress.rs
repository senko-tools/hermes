//! Shrinking files losslessly before they are encrypted. Only the sender can do this: what reaches
//! the server is ciphertext, which does not compress. A file is only changed when that saves at
//! least [`MIN_SAVING_PERCENT`]; most pictures, sound and video are compressed already.

use std::io::Read as _;

use hermes_proto::e2ee::Compression;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ImageEncoder as _, ImageFormat};

/// A file has to get this much smaller to be stored in another form.
const MIN_SAVING_PERCENT: usize = 5;
/// The compression level: good ratios at a speed that does not hold up sending.
const LEVEL: i32 = 9;
/// How much of the start and the middle of a file are tried first, to skip what will not shrink.
const SAMPLE: usize = 128 * 1024;
/// A compressed file is never expanded past this, whatever it claims.
const MAX_PLAIN_BYTES: u64 = 2 * 1024 * 1024 * 1024;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// A preview fits in this square, like the thumbnails the server makes of plain pictures.
const PREVIEW_SIDE: u32 = 400;
/// Pictures smaller than this are shown as they are.
const MIN_PREVIEWED_BYTES: usize = 64 * 1024;
const PREVIEW_JPEG_QUALITY: u8 = 80;

/// A file ready to be encrypted.
#[derive(Debug, PartialEq, Eq)]
pub struct Shrunk {
    pub bytes: Vec<u8>,
    /// How large the file is once restored: what the people it is for see.
    pub plain_size: u64,
    pub compression: Option<Compression>,
}

fn worth_it(before: usize, after: usize) -> bool {
    after.saturating_mul(100) <= before.saturating_mul(100 - MIN_SAVING_PERCENT)
}

/// Whether a PNG holds nothing but its pixels: re-encoding drops colour profiles, text and
/// animation, so anything else stays as it is.
fn only_pixels(bytes: &[u8]) -> bool {
    let Some(mut rest) = bytes.strip_prefix(PNG_SIGNATURE) else {
        return false;
    };
    loop {
        let Some((head, after)) = rest.split_first_chunk::<8>() else {
            return false;
        };
        let length = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let kind = &head[4..];
        if !matches!(kind, b"IHDR" | b"PLTE" | b"tRNS" | b"IDAT" | b"IEND") {
            return false;
        }
        if kind == b"IEND" {
            return true;
        }
        let Some(next) = after.get(length.saturating_add(4)..) else {
            return false;
        };
        rest = next;
    }
}

/// The same pixels at the best PNG compression.
fn repack_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory_with_format(bytes, ImageFormat::Png).ok()?;
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive)
        .write_image(
            image.as_bytes(),
            image.width(),
            image.height(),
            image.color().into(),
        )
        .ok()?;
    Some(out)
}

/// The start and the middle of a file, or all of a small one.
fn sample(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() <= 2 * SAMPLE {
        return bytes.to_vec();
    }
    let middle = bytes.len() / 2;
    [&bytes[..SAMPLE], &bytes[middle..middle + SAMPLE]].concat()
}

fn zstd_smaller(bytes: &[u8]) -> Option<Vec<u8>> {
    let tried = zstd::bulk::compress(&sample(bytes), LEVEL).ok()?;
    if !worth_it(sample_len(bytes), tried.len()) {
        return None;
    }
    let packed = zstd::bulk::compress(bytes, LEVEL).ok()?;
    worth_it(bytes.len(), packed.len()).then_some(packed)
}

fn sample_len(bytes: &[u8]) -> usize {
    bytes.len().min(2 * SAMPLE)
}

/// Makes a file smaller without losing anything of it, if that is worth it. Blocks for a while
/// on a large file: call it off the async threads.
pub fn shrink(original: Vec<u8>) -> Shrunk {
    let mut bytes = original;
    if only_pixels(&bytes)
        && let Some(repacked) = repack_png(&bytes)
        && worth_it(bytes.len(), repacked.len())
    {
        bytes = repacked;
    }
    let plain_size = bytes.len() as u64;
    match zstd_smaller(&bytes) {
        Some(packed) => Shrunk {
            bytes: packed,
            plain_size,
            compression: Some(Compression::Zstd),
        },
        None => Shrunk {
            bytes,
            plain_size,
            compression: None,
        },
    }
}

/// A small preview of a picture, so people need not download all of it to see it in the chat.
/// `None` if it is no picture, moves, or is small already. Blocks: call it off the async threads.
pub fn preview(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < MIN_PREVIEWED_BYTES {
        return None;
    }
    let format = image::guess_format(bytes).ok()?;
    if !matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::WebP
            | ImageFormat::Bmp
            | ImageFormat::Tiff
    ) {
        return None;
    }
    let image = image::load_from_memory_with_format(bytes, format).ok()?;
    let mut out = Vec::new();
    if image.color().has_alpha() {
        let rgba = crate::picture::thumbnail(&image.to_rgba8(), PREVIEW_SIDE, PREVIEW_SIDE);
        PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive)
            .write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )
            .ok()?;
    } else {
        let image = image.thumbnail(PREVIEW_SIDE, PREVIEW_SIDE);
        JpegEncoder::new_with_quality(&mut out, PREVIEW_JPEG_QUALITY)
            .encode_image(&image.to_rgb8())
            .ok()?;
    }
    (out.len() < bytes.len() / 2).then_some(out)
}

/// Undoes [`shrink`]. `plain_size` comes with the file, and a file that expands to anything else
/// is refused, so a hostile sender cannot fill the memory of those who open it.
pub fn restore(
    bytes: Vec<u8>,
    compression: Option<Compression>,
    plain_size: u64,
) -> Result<Vec<u8>, String> {
    let Some(Compression::Zstd) = compression else {
        return Ok(bytes);
    };
    if plain_size > MAX_PLAIN_BYTES {
        return Err("the file claims to be too large".to_owned());
    }
    let decoder = zstd::stream::read::Decoder::new(bytes.as_slice()).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    decoder
        .take(plain_size.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    if out.len() as u64 != plain_size {
        return Err("the file is not the size it claims".to_owned());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes that do not compress.
    fn noise(len: usize) -> Vec<u8> {
        let mut x = 0x2545_f491_4f6c_dd1d_u64;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        // Stored without compression, so there is something to gain.
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 7) as u8, (y % 5) as u8, 9])
        });
        let mut out = Vec::new();
        PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::NoFilter)
            .write_image(
                image.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        out
    }

    #[test]
    fn text_shrinks_and_comes_back_the_same() {
        let text = "the same line again and again\n".repeat(5_000).into_bytes();
        let shrunk = shrink(text.clone());
        assert_eq!(shrunk.compression, Some(Compression::Zstd));
        assert!(shrunk.bytes.len() < text.len() / 10);
        assert_eq!(shrunk.plain_size, text.len() as u64);
        assert_eq!(
            restore(shrunk.bytes, shrunk.compression, shrunk.plain_size).unwrap(),
            text
        );
    }

    #[test]
    fn what_will_not_shrink_stays_as_it_is() {
        let random = noise(600 * 1024);
        let shrunk = shrink(random.clone());
        assert_eq!(shrunk.compression, None);
        assert_eq!(shrunk.bytes, random);
    }

    #[test]
    fn a_png_keeps_every_pixel() {
        let original = png(300, 200);
        let shrunk = shrink(original.clone());
        let restored = restore(shrunk.bytes, shrunk.compression, shrunk.plain_size).unwrap();
        assert!(restored.len() < original.len());
        assert_eq!(
            image::load_from_memory(&restored).unwrap().to_rgb8(),
            image::load_from_memory(&original).unwrap().to_rgb8()
        );
    }

    #[test]
    fn a_png_with_more_than_pixels_is_left_alone() {
        let plain = png(10, 10);
        assert!(only_pixels(&plain));
        // A text chunk right after the header: 8 signature bytes, then 25 of IHDR.
        let mut with_text = plain[..33].to_vec();
        with_text.extend_from_slice(&[0, 0, 0, 1]);
        with_text.extend_from_slice(b"tEXt");
        with_text.extend_from_slice(&[b'a', 1, 2, 3, 4]);
        with_text.extend_from_slice(&plain[33..]);
        assert!(!only_pixels(&with_text));
        assert!(!only_pixels(b"not a png at all"));
        assert!(!only_pixels(&plain[..20]), "cut off");
    }

    #[test]
    fn a_file_that_expands_past_its_claim_is_refused() {
        let packed = zstd::bulk::compress(&vec![0u8; 1_000_000], 3).unwrap();
        assert!(restore(packed.clone(), Some(Compression::Zstd), 10).is_err());
        assert!(restore(packed.clone(), Some(Compression::Zstd), 1_000_001).is_err());
        assert!(restore(packed, Some(Compression::Zstd), u64::MAX).is_err());
    }

    #[test]
    fn a_large_picture_gets_a_small_preview() {
        let original = png(1600, 1200);
        assert!(original.len() >= MIN_PREVIEWED_BYTES);
        let small = preview(&original).expect("a preview");
        assert!(small.len() < original.len() / 2);
        let image = image::load_from_memory(&small).unwrap();
        assert_eq!((image.width(), image.height()), (400, 300));
    }

    #[test]
    fn small_pictures_and_other_files_get_none() {
        assert_eq!(preview(&png(10, 10)), None);
        assert_eq!(preview(&noise(200 * 1024)), None);
    }

    #[test]
    fn an_uncompressed_file_passes_through() {
        assert_eq!(restore(vec![1, 2], None, 99).unwrap(), vec![1, 2]);
    }
}
