//! H.264 for screen shares, with OpenH264 built from its bundled source (2.7.0: includes the
//! fix for CVE-2025-27091 in the decoder, which parses other people's data).

// The encoder is used by the (Windows-only) capture; it is tested everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use openh264::OpenH264API;
use openh264::decoder::Decoder as H264Decoder;
use openh264::encoder::{
    BitRate, Complexity, Encoder as H264Encoder, EncoderConfig, FrameRate, FrameType,
    RateControlMode, UsageType,
};
use openh264::formats::{BgraSliceU8, RgbaSliceU8, YUVBuffer, YUVSource};

use hermes_client_core::proto::voice::MAX_VIDEO_DIMENSION;

/// Target bitrate of a share (bits per second): about 0.1 bit per pixel per frame, at least
/// 1.5 Mbit/s and at most 8 Mbit/s (which keeps keyframes well under the 512 KiB frame limit).
pub fn bitrate(width: u32, height: u32, fps: f32) -> u32 {
    let bits = f64::from(width) * f64::from(height) * f64::from(fps) * 0.1;
    bits.clamp(1_500_000.0, 8_000_000.0) as u32
}

/// A captured picture: four bytes per pixel, in the order the capture gave them.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Blue, green, red, alpha (Desktop Duplication) rather than red, green, blue, alpha (GDI).
    pub bgra: bool,
    pub pixels: Vec<u8>,
}

/// Encodes captured frames; rebuilt when the size changes.
pub struct Encoder {
    inner: H264Encoder,
    size: (u32, u32),
    fps: f32,
    /// Reused for every frame.
    yuv: YUVBuffer,
}

impl Encoder {
    pub fn new(width: u32, height: u32, fps: f32) -> Result<Encoder, String> {
        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(bitrate(width, height, fps)))
            .max_frame_rate(FrameRate::from_hz(fps))
            .skip_frames(false)
            // The fastest mode: screens are mostly still, and smooth matters more here.
            .complexity(Complexity::Low)
            // Not supported for screen content (OpenH264 turns them off with a warning).
            .adaptive_quantization(false)
            .background_detection(false);
        let inner = H264Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|e| e.to_string())?;
        Ok(Encoder {
            inner,
            size: (width, height),
            fps,
            yuv: YUVBuffer::new(width as usize, height as usize),
        })
    }

    /// Encodes one frame (its width and height must be even). `keyframe` forces an IDR frame;
    /// returns the access unit and whether it is a keyframe, or `None` if nothing came out.
    pub fn encode(
        &mut self,
        frame: &Frame,
        mut keyframe: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String> {
        let size = (frame.width, frame.height);
        if size != self.size {
            *self = Encoder::new(size.0, size.1, self.fps)?;
            keyframe = true;
        }
        if keyframe {
            self.inner.force_intra_frame();
        }
        // The SIMD conversions (AVX2 where available), not the per-pixel `read_rgb`.
        let dimensions = (size.0 as usize, size.1 as usize);
        if frame.bgra {
            self.yuv
                .read_bgra8(BgraSliceU8::new(&frame.pixels, dimensions));
        } else {
            self.yuv
                .read_rgba8(RgbaSliceU8::new(&frame.pixels, dimensions));
        }
        let stream = self.inner.encode(&self.yuv).map_err(|e| e.to_string())?;
        let is_key = matches!(stream.frame_type(), FrameType::IDR | FrameType::I);
        let data = stream.to_vec();
        Ok((!data.is_empty()).then_some((data, is_key)))
    }
}

/// A decoded picture, packed without padding: the full-size Y plane, then either the half-size
/// U and V planes (I420, from OpenH264) or one half-size plane of interleaved U and V (NV12,
/// from Windows' decoder). The GPU turns it into colors ([`super::view`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// NV12 rather than I420.
    pub nv12: bool,
    pub planes: Vec<u8>,
}

impl Picture {
    /// The size of the U and V planes (in samples: an NV12 row holds two bytes per sample).
    pub fn chroma_size(&self) -> (u32, u32) {
        (self.width / 2, self.height / 2)
    }

    pub fn y(&self) -> &[u8] {
        &self.planes[..(self.width * self.height) as usize]
    }

    /// I420: the U plane. NV12: the interleaved U/V plane.
    pub fn u(&self) -> &[u8] {
        let (w, h) = self.chroma_size();
        let start = (self.width * self.height) as usize;
        let len = (w * h) as usize * if self.nv12 { 2 } else { 1 };
        &self.planes[start..start + len]
    }

    /// I420: the V plane. NV12: empty (V is interleaved with U).
    pub fn v(&self) -> &[u8] {
        if self.nv12 {
            return &[];
        }
        let (w, h) = self.chroma_size();
        let start = (self.width * self.height + w * h) as usize;
        &self.planes[start..start + (w * h) as usize]
    }
}

/// Whether Windows' decoder failed to start once: then OpenH264 is used from then on.
#[cfg(windows)]
static MEDIA_FOUNDATION_FAILED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Decodes one person's share.
pub struct Decoder {
    inner: Inner,
}

enum Inner {
    OpenH264(H264Decoder),
    #[cfg(windows)]
    MediaFoundation(super::mf_decode::MfDecoder),
}

impl Decoder {
    /// The best decoder here: on Windows its own (on the GPU where possible), else OpenH264.
    pub fn new() -> Result<Decoder, String> {
        #[cfg(windows)]
        {
            use std::sync::atomic::Ordering;
            if !MEDIA_FOUNDATION_FAILED.load(Ordering::Relaxed) {
                match super::mf_decode::MfDecoder::new() {
                    Ok(decoder) => {
                        tracing::debug!(
                            "screen share decoding with Media Foundation ({})",
                            if decoder.hardware { "GPU" } else { "CPU" }
                        );
                        return Ok(Decoder {
                            inner: Inner::MediaFoundation(decoder),
                        });
                    }
                    Err(err) => {
                        tracing::warn!("screen share decoding: {err}; using OpenH264");
                        MEDIA_FOUNDATION_FAILED.store(true, Ordering::Relaxed);
                    }
                }
            }
        }
        Decoder::software()
    }

    /// OpenH264, on the CPU.
    pub fn software() -> Result<Decoder, String> {
        Ok(Decoder {
            inner: Inner::OpenH264(H264Decoder::new().map_err(|e| e.to_string())?),
        })
    }

    /// What decodes, for the logs.
    pub fn kind(&self) -> &'static str {
        match &self.inner {
            Inner::OpenH264(_) => "OpenH264",
            #[cfg(windows)]
            Inner::MediaFoundation(d) if d.hardware => "Media Foundation, GPU",
            #[cfg(windows)]
            Inner::MediaFoundation(_) => "Media Foundation, CPU",
        }
    }

    /// Decodes one access unit. `Ok(None)` while the decoder waits for more data; an error
    /// means the picture is broken until the next keyframe.
    pub fn decode(&mut self, data: &[u8]) -> Result<Option<Picture>, String> {
        let decoder = match &mut self.inner {
            Inner::OpenH264(decoder) => decoder,
            #[cfg(windows)]
            Inner::MediaFoundation(decoder) => return decoder.decode(data),
        };
        let Some(yuv) = decoder.decode(data).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let (width, height) = yuv.dimensions();
        let max = usize::from(MAX_VIDEO_DIMENSION);
        if width < 2 || height < 2 || width > max || height > max || width % 2 + height % 2 != 0 {
            return Err(format!("unexpected picture size {width}x{height}"));
        }
        let (cw, ch) = (width / 2, height / 2);
        let (y_stride, u_stride, v_stride) = yuv.strides();
        let mut planes = Vec::with_capacity(width * height + 2 * cw * ch);
        let mut copy = |plane: &[u8], stride: usize, w: usize, h: usize| -> Result<(), String> {
            for row in 0..h {
                let line = plane
                    .get(row * stride..row * stride + w)
                    .ok_or("the decoder returned a short plane")?;
                planes.extend_from_slice(line);
            }
            Ok(())
        };
        copy(yuv.y(), y_stride, width, height)?;
        copy(yuv.u(), u_stride, cw, ch)?;
        copy(yuv.v(), v_stride, cw, ch)?;
        Ok(Some(Picture {
            width: width as u32,
            height: height as u32,
            nv12: false,
            planes,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_survive_a_round_trip() {
        let (w, h) = (64u32, 32u32);
        let mut pixels = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                pixels.extend_from_slice(&[(x * 4) as u8, 80, 160, 255]);
            }
        }
        let rgba = Frame {
            width: w,
            height: h,
            bgra: false,
            pixels: pixels.clone(),
        };
        let mut encoder = Encoder::new(w, h, 10.0).unwrap();
        let (data, key) = encoder.encode(&rgba, true).unwrap().unwrap();
        assert!(key, "the first frame is a keyframe");
        let mut decoder = Decoder::software().unwrap();
        let picture = decoder.decode(&data).unwrap().unwrap();
        assert_eq!((picture.width, picture.height), (64, 32));
        assert_eq!(picture.planes.len(), 64 * 32 * 3 / 2);
        assert_eq!(picture.chroma_size(), (32, 16));
        // Lossy, but close: the middle pixel is (128, 80, 160), BT.601 luma ≈ 105.
        let luma = picture.y()[16 * 64 + 32];
        assert!((95..=115).contains(&luma), "{luma}");

        // The same picture as BGRA gives the same luma.
        let mut swapped = pixels;
        for p in swapped.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
        let bgra = Frame {
            bgra: true,
            pixels: swapped,
            ..rgba
        };
        let (data, _) = encoder.encode(&bgra, true).unwrap().unwrap();
        let again = Decoder::software().unwrap().decode(&data).unwrap().unwrap();
        let luma2 = again.y()[16 * 64 + 32];
        assert!(luma.abs_diff(luma2) <= 3, "{luma} vs {luma2}");

        assert_eq!(bitrate(1280, 720, 10.0), 1_500_000);
        assert_eq!(bitrate(1920, 1080, 15.0), 3_110_400);
        assert_eq!(bitrate(3840, 2160, 30.0), 8_000_000);

        // Garbage never panics.
        for decoder in [Decoder::software(), Decoder::new()] {
            let mut decoder = decoder.unwrap();
            let _ = decoder.decode(&[0, 0, 0, 1, 0x65, 0xff, 0x00, 0x12]);
            let _ = decoder.decode(&[0xde, 0xad, 0xbe, 0xef]);
        }
    }

    /// The best decoder here (Windows' own on Windows) decodes OpenH264's pictures to the same
    /// colors as OpenH264.
    #[test]
    fn the_best_decoder_agrees_with_openh264() {
        let (w, h) = (128u32, 96u32);
        let mut pixels = Vec::new();
        for y in 0..h {
            for x in 0..w {
                pixels.extend_from_slice(&[(x * 2) as u8, (y * 2) as u8, 160, 255]);
            }
        }
        let frame = Frame {
            width: w,
            height: h,
            bgra: false,
            pixels,
        };
        let mut encoder = Encoder::new(w, h, 30.0).unwrap();
        let (data, _) = encoder.encode(&frame, true).unwrap().unwrap();
        let reference = Decoder::software().unwrap().decode(&data).unwrap().unwrap();
        let mut best = Decoder::new().unwrap();
        println!("decoding with {}", best.kind());
        // Some decoders want the access unit twice before the first picture comes out.
        let picture = match best.decode(&data).unwrap() {
            Some(picture) => picture,
            None => best.decode(&data).unwrap().expect("a picture"),
        };
        assert_eq!((picture.width, picture.height), (w, h), "{}", best.kind());
        let close = |a: &[u8], b: &[u8]| a.iter().zip(b).all(|(a, b)| a.abs_diff(*b) <= 2);
        assert!(close(picture.y(), reference.y()), "{}: luma", best.kind());
        if picture.nv12 {
            let u: Vec<u8> = picture.u().iter().step_by(2).copied().collect();
            let v: Vec<u8> = picture.u().iter().skip(1).step_by(2).copied().collect();
            assert!(close(&u, reference.u()), "{}: U", best.kind());
            assert!(close(&v, reference.v()), "{}: V", best.kind());
        } else {
            assert!(close(picture.u(), reference.u()));
            assert!(close(picture.v(), reference.v()));
        }
    }
}
