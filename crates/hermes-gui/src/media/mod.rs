//! Sound and video files in the chat, played with FFmpeg: [`ffmpeg`] runs it, [`serve`] hands it
//! the file from memory, [`player`] plays one file and [`waveform`] measures a sound's levels.

pub mod cards;
pub mod ffmpeg;
pub mod player;
pub mod serve;
pub mod waveform;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

const SOUNDS: [&str; 9] = [
    "mp3", "ogg", "oga", "opus", "wav", "flac", "m4a", "aac", "weba",
];
const VIDEOS: [&str; 6] = ["mp4", "m4v", "mkv", "mov", "webm", "avi"];

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

/// Measures a sound or video about to be sent: its waveform (a sound only) and its length.
/// `(None, None)` for anything else, or without FFmpeg. Blocks.
pub fn measure_file(path: &std::path::Path) -> (Option<Vec<u8>>, Option<u64>) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (sound, video) = (is_sound("", &name), is_video("", &name));
    if !sound && !video {
        return (None, None);
    }
    let Ok(bytes) = std::fs::read(path) else {
        return (None, None);
    };
    let Ok(served) = serve::serve(serve::Source::Memory(Arc::new(bytes))) else {
        return (None, None);
    };
    let Ok(probe) = ffmpeg::probe(&served.url) else {
        return (None, None);
    };
    let duration = (probe.duration_ms > 0).then_some(probe.duration_ms);
    if probe.video.is_some() || !probe.audio {
        return (None, duration);
    }
    let wave = std::sync::Mutex::new(waveform::Waveform::default());
    let measured = waveform::measure(
        &served.url,
        probe.duration_ms,
        &wave,
        &AtomicBool::new(false),
    );
    let wave = wave.into_inner().unwrap_or_else(|e| e.into_inner());
    let bars = (measured.is_ok() && wave.done).then(|| wave.to_bytes());
    (bars, duration)
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
}
