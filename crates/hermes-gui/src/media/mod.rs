//! Sound and video files in the chat, played with FFmpeg: [`ffmpeg`] runs it, [`serve`] hands it
//! the file from memory, [`player`] plays one file and [`waveform`] measures a sound's levels.

pub mod cards;
pub mod ffmpeg;
pub mod fill;
pub mod player;
pub mod serve;
pub mod waveform;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

// What FFmpeg plays; the type sent along may be generic (`application/octet-stream`).
const SOUNDS: [&str; 12] = [
    "mp3", "ogg", "oga", "opus", "wav", "flac", "m4a", "aac", "weba", "wma", "aiff", "alac",
];
const VIDEOS: [&str; 12] = [
    "mp4", "m4v", "mkv", "mov", "webm", "avi", "ogv", "wmv", "flv", "ts", "m2ts", "3gp",
];

fn extension(filename: &str) -> String {
    filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Whether an attachment is a sound file: it gets an audio card.
pub fn is_sound(content_type: &str, filename: &str) -> bool {
    content_type.starts_with("audio/") || SOUNDS.contains(&extension(filename).as_str())
}

/// Whether an attachment is a video: it gets a player.
pub fn is_video(content_type: &str, filename: &str) -> bool {
    content_type.starts_with("video/") || VIDEOS.contains(&extension(filename).as_str())
}

/// What was measured of a sound or video about to be sent.
#[derive(Debug, Default)]
pub struct Measured {
    /// A sound's loudness, a byte per bar.
    pub waveform: Option<Vec<u8>>,
    pub duration_ms: Option<u64>,
    /// A video's picture size.
    pub size: Option<(u32, u32)>,
    /// A video's poster as a PNG (#8).
    pub poster: Option<Vec<u8>>,
}

/// The poster's longest side.
const POSTER_SIDE: u32 = 480;

/// Measures a sound or video about to be sent: a sound's waveform, the length, and a video's
/// size and poster. Nothing for anything else, or without FFmpeg. Blocks.
pub fn measure_file(path: &std::path::Path) -> Measured {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (sound, video) = (is_sound("", &name), is_video("", &name));
    if !sound && !video {
        return Measured::default();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Measured::default();
    };
    let Ok(served) = serve::serve(serve::Source::Memory(Arc::new(bytes))) else {
        return Measured::default();
    };
    let Ok(probe) = ffmpeg::probe(&served.url) else {
        return Measured::default();
    };
    let duration_ms = (probe.duration_ms > 0).then_some(probe.duration_ms);
    if let Some((width, height, _)) = probe.video {
        return Measured {
            waveform: None,
            duration_ms,
            size: Some((width, height)),
            poster: still_picture(
                &served.url,
                probe.duration_ms,
                (width, height),
                (POSTER_SIDE, POSTER_SIDE),
            )
            .and_then(|image| encode(&image, image::ImageFormat::Png)),
        };
    }
    if !probe.audio {
        return Measured {
            duration_ms,
            ..Measured::default()
        };
    }
    let wave = std::sync::Mutex::new(waveform::Waveform::default());
    let measured = waveform::measure(
        &served.url,
        probe.duration_ms,
        &wave,
        &AtomicBool::new(false),
    );
    let wave = wave.into_inner().unwrap_or_else(|e| e.into_inner());
    Measured {
        waveform: (measured.is_ok() && wave.done).then(|| wave.to_bytes()),
        duration_ms,
        ..Measured::default()
    }
}

/// What a receiver learnt of a video that came without its size or a sharp enough poster.
#[derive(Debug, Clone)]
pub struct Inspected {
    /// Its size as shown.
    pub size: (u32, u32),
    pub duration_ms: Option<u64>,
    /// A picture from it as a JPEG, sized for the card it is shown on.
    pub poster: Option<Vec<u8>>,
}

/// Looks at a video someone sent (#76): its size as shown, its length, and a picture fitting
/// within `bound` device pixels (never larger than the video). A file read in parts is only
/// read where FFmpeg looks: its start, its index and around the picture. Blocks.
pub fn inspect(source: serve::Source, bound: (u32, u32)) -> Result<Inspected, String> {
    let served = serve::serve(source).map_err(|e| e.to_string())?;
    let probe = ffmpeg::probe(&served.url)?;
    let Some((width, height, _)) = probe.video else {
        return Err("no picture in it".into());
    };
    let poster = still_picture(&served.url, probe.duration_ms, (width, height), bound)
        .and_then(|image| encode(&image, image::ImageFormat::Jpeg));
    Ok(Inspected {
        size: (width, height),
        duration_ms: (probe.duration_ms > 0).then_some(probe.duration_ms),
        poster,
    })
}

/// The size of a picture of a `width` by `height` video fitting within `bound`, never larger
/// than the video, in even numbers (some encoders' frames refuse odd ones).
fn poster_size((width, height): (u32, u32), (max_w, max_h): (u32, u32)) -> (u32, u32) {
    let scale = (max_w as f32 / width.max(1) as f32)
        .min(max_h as f32 / height.max(1) as f32)
        .min(1.);
    let even = |v: f32| ((v.round() as u32).max(2) / 2) * 2;
    (even(width as f32 * scale), even(height as f32 * scale))
}

/// `image` as a PNG or JPEG file.
fn encode(image: &image::RgbaImage, format: image::ImageFormat) -> Option<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    if format == image::ImageFormat::Jpeg {
        // JPEG has no transparency; a video frame has none to lose.
        let rgb = image::DynamicImage::ImageRgba8(image.clone()).into_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88)
            .encode_image(&rgb)
            .ok()?;
    } else {
        image.write_to(&mut out, format).ok()?;
    }
    Some(out.into_inner())
}

/// One picture of the video at `url`, fitting within `bound` and never larger than the video:
/// a second in (the first frames are often black), or half way through a shorter one.
fn still_picture(
    url: &str,
    duration_ms: u64,
    (width, height): (u32, u32),
    (max_w, max_h): (u32, u32),
) -> Option<image::RgbaImage> {
    use std::io::Read as _;
    if width == 0 || height == 0 || max_w == 0 || max_h == 0 {
        return None;
    }
    let (w, h) = poster_size((width, height), (max_w, max_h));
    let at = if duration_ms > 2_000 {
        1_000
    } else {
        duration_ms / 2
    };
    let mut child = ffmpeg::still(url, at, w, h).ok()?;
    let mut bgra = Vec::new();
    child.stdout.take()?.read_to_end(&mut bgra).ok()?;
    let _ = child.wait();
    if bgra.len() != (w * h * 4) as usize {
        return None;
    }
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    image::RgbaImage::from_raw(w, h, bgra)
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;

    /// A 2 s clip of 160x90 at 10 fps with a tone, made by FFmpeg; `None` without FFmpeg.
    fn clip() -> Option<Vec<u8>> {
        // Its own file per call: tests run side by side.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("hermes-media-test-{}-{n}.mkv", std::process::id()));
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=160x90:rate=10",
            ])
            .args(["-f", "lavfi", "-i", "sine=frequency=440", "-t", "2"])
            .args(["-c:v", "mjpeg", "-c:a", "pcm_s16le"])
            .arg(&path)
            .status()
            .ok()?;
        let bytes = made.success().then(|| std::fs::read(&path).ok()).flatten();
        let _ = std::fs::remove_file(&path);
        bytes
    }

    #[test]
    fn decodes_pictures_and_measures_a_waveform_with_ffmpeg() {
        let Some(bytes) = clip() else {
            eprintln!("FFmpeg is not installed: skipped");
            return;
        };
        let served = serve::serve(serve::Source::Memory(Arc::new(bytes))).unwrap();
        let probe = ffmpeg::probe(&served.url).unwrap();
        assert!(probe.audio);
        assert_eq!(probe.video.map(|(w, h, _)| (w, h)), Some((160, 90)));
        assert!(
            (1900..=2100).contains(&probe.duration_ms),
            "{}",
            probe.duration_ms
        );

        // From the middle: about 1 s of pictures at 10 fps.
        let mut child = ffmpeg::video(&served.url, 1000, 80, 46, 10.).unwrap();
        let mut out = Vec::new();
        child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
        child.wait().unwrap();
        let frames = out.len() / (80 * 46 * 4);
        assert!((8..=12).contains(&frames), "{frames} frames");

        let wave = std::sync::Mutex::new(waveform::Waveform::default());
        waveform::measure(
            &served.url,
            probe.duration_ms,
            &wave,
            &AtomicBool::new(false),
        )
        .unwrap();
        let wave = wave.into_inner().unwrap();
        assert!(wave.done);
        assert_eq!(wave.bars.len(), waveform::BARS);
        assert!(
            wave.bars.iter().all(|l| *l > 0.5),
            "a steady tone: {:?}",
            wave.bars
        );
    }

    #[test]
    fn a_video_gets_its_size_and_a_poster() {
        let Some(bytes) = clip() else {
            eprintln!("FFmpeg is not installed: skipped");
            return;
        };
        let path =
            std::env::temp_dir().join(format!("hermes-poster-test-{}.mkv", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let measured = measure_file(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(measured.size, Some((160, 90)));
        assert!(measured.waveform.is_none(), "a video has no waveform");
        let poster = image::load_from_memory(&measured.poster.expect("a poster")).unwrap();
        assert_eq!(
            (poster.width(), poster.height()),
            (160, 90),
            "small videos keep their size"
        );
    }

    #[test]
    fn a_turned_video_is_inspected_upright() {
        let Some(bytes) = clip() else {
            eprintln!("FFmpeg is not installed: skipped");
            return;
        };
        let dir = std::env::temp_dir();
        let (plain, turned) = (
            dir.join(format!("hermes-turn-in-{}.mkv", std::process::id())),
            dir.join(format!("hermes-turn-out-{}.mp4", std::process::id())),
        );
        std::fs::write(&plain, bytes).unwrap();
        // `-display_rotation` needs FFmpeg 6.1 or later.
        let made = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-display_rotation", "90", "-i"])
            .arg(&plain)
            .args(["-c", "copy"])
            .arg(&turned)
            .status()
            .is_ok_and(|s| s.success());
        let bytes = std::fs::read(&turned).ok().filter(|_| made);
        let _ = (std::fs::remove_file(&plain), std::fs::remove_file(&turned));
        let Some(bytes) = bytes else {
            eprintln!("this FFmpeg cannot turn a video: skipped");
            return;
        };
        let seen = inspect(serve::Source::Memory(Arc::new(bytes)), (1000, 1000)).unwrap();
        assert_eq!(seen.size, (90, 160));
        let poster = image::load_from_memory(seen.poster.as_ref().expect("a poster")).unwrap();
        assert_eq!((poster.width(), poster.height()), (90, 160));
        // Made smaller to fit the card, keeping its shape.
        assert_eq!(poster_size(seen.size, (45, 1000)), (44, 80));
    }
}
