//! H.264 for screen shares, with OpenH264 built from its bundled source (2.7.0: includes the
//! fix for CVE-2025-27091 in the decoder, which parses other people's data).

// The encoder is used by the (Windows-only) capture; it is tested everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use image::RgbaImage;
use openh264::OpenH264API;
use openh264::decoder::Decoder as H264Decoder;
use openh264::encoder::{
    BitRate, Encoder as H264Encoder, EncoderConfig, FrameRate, FrameType, RateControlMode,
    UsageType,
};
use openh264::formats::{RgbaSliceU8, YUVBuffer, YUVSource};

use hermes_client_core::proto::voice::MAX_VIDEO_DIMENSION;

/// Target bitrate of a share (bits per second).
const BITRATE: u32 = 1_500_000;

/// Encodes captured frames; rebuilt when the size changes.
pub struct Encoder {
    inner: H264Encoder,
    size: (u32, u32),
    fps: f32,
}

impl Encoder {
    pub fn new(width: u32, height: u32, fps: f32) -> Result<Encoder, String> {
        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(BITRATE))
            .max_frame_rate(FrameRate::from_hz(fps))
            .skip_frames(false);
        let inner = H264Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|e| e.to_string())?;
        Ok(Encoder {
            inner,
            size: (width, height),
            fps,
        })
    }

    /// Encodes one frame (its width and height must be even). `keyframe` forces an IDR frame;
    /// returns the access unit and whether it is a keyframe, or `None` if nothing came out.
    pub fn encode(
        &mut self,
        frame: &RgbaImage,
        mut keyframe: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String> {
        let size = frame.dimensions();
        if size != self.size {
            *self = Encoder::new(size.0, size.1, self.fps)?;
            keyframe = true;
        }
        if keyframe {
            self.inner.force_intra_frame();
        }
        let rgba = RgbaSliceU8::new(frame.as_raw(), (size.0 as usize, size.1 as usize));
        let yuv = YUVBuffer::from_rgb_source(rgba);
        let stream = self.inner.encode(&yuv).map_err(|e| e.to_string())?;
        let is_key = matches!(stream.frame_type(), FrameType::IDR | FrameType::I);
        let data = stream.to_vec();
        Ok((!data.is_empty()).then_some((data, is_key)))
    }
}

/// Decodes one person's share.
pub struct Decoder {
    inner: H264Decoder,
}

/// A decoded picture: width, height and RGBA bytes.
pub type Picture = (u32, u32, Vec<u8>);

impl Decoder {
    pub fn new() -> Result<Decoder, String> {
        Ok(Decoder {
            inner: H264Decoder::new().map_err(|e| e.to_string())?,
        })
    }

    /// Decodes one access unit. `Ok(None)` while the decoder waits for more data; an error
    /// means the picture is broken until the next keyframe.
    pub fn decode(&mut self, data: &[u8]) -> Result<Option<Picture>, String> {
        let Some(yuv) = self.inner.decode(data).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let (width, height) = yuv.dimensions();
        let max = usize::from(MAX_VIDEO_DIMENSION);
        if width == 0 || height == 0 || width > max || height > max {
            return Err(format!("unexpected picture size {width}x{height}"));
        }
        let mut rgba = vec![0u8; width * height * 4];
        yuv.write_rgba8(&mut rgba);
        Ok(Some((width as u32, height as u32, rgba)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_survive_a_round_trip() {
        let mut frame = RgbaImage::new(64, 32);
        for (x, _, p) in frame.enumerate_pixels_mut() {
            *p = image::Rgba([(x * 4) as u8, 80, 160, 255]);
        }
        let mut encoder = Encoder::new(64, 32, 10.0).unwrap();
        let (data, key) = encoder.encode(&frame, true).unwrap().unwrap();
        assert!(key, "the first frame is a keyframe");
        let mut decoder = Decoder::new().unwrap();
        let (w, h, rgba) = decoder.decode(&data).unwrap().unwrap();
        assert_eq!((w, h), (64, 32));
        assert_eq!(rgba.len(), 64 * 32 * 4);
        // Lossy, but close: the middle pixel's blue channel.
        let blue = rgba[(16 * 64 + 32) * 4 + 2];
        assert!((140..=180).contains(&blue), "{blue}");

        // Garbage never panics.
        let _ = Decoder::new()
            .unwrap()
            .decode(&[0, 0, 0, 1, 0x65, 0xff, 0x00, 0x12]);
        let _ = Decoder::new().unwrap().decode(&[0xde, 0xad, 0xbe, 0xef]);
    }
}
