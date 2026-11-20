//! Sound and video files in the chat, played with FFmpeg: [`ffmpeg`] runs it, [`serve`] hands it
//! the file from memory, [`player`] plays one file and [`waveform`] measures a sound's levels.

pub mod cards;
pub mod ffmpeg;
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
            poster: poster(&served.url, probe.duration_ms, (width, height)),
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

/// One picture of the video at `url` as a PNG at most [`POSTER_SIDE`] long: a second in (the
/// first frames are often black), or half way through a shorter one.
fn poster(url: &str, duration_ms: u64, (width, height): (u32, u32)) -> Option<Vec<u8>> {
    use std::io::Read as _;
    if width == 0 || height == 0 {
        return None;
    }
    let scale = (POSTER_SIDE as f32 / width.max(height) as f32).min(1.);
    // Even sizes: some encoders' frames refuse odd ones.
    let even = |v: f32| ((v.round() as u32).max(2) / 2) * 2;
    let (w, h) = (even(width as f32 * scale), even(height as f32 * scale));
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
    let image = image::RgbaImage::from_raw(w, h, bgra)?;
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(png.into_inner())
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;

    /// A 2 s clip of 160x90 at 10 fps with a tone, made by FFmpeg; `None` without FFmpeg.
    fn clip() -> Option<Vec<u8>> {
        let path =
            std::env::temp_dir().join(format!("hermes-media-test-{}.mkv", std::process::id()));
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
}
