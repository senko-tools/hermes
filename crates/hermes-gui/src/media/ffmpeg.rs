//! Running FFmpeg: `ffmpeg` and `ffprobe` next to `hermes.exe` (the installer bundles them), else
//! on the PATH (Linux). They read a file from [`super::serve`] and write raw frames or samples.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// Where a program is: beside the executable first, then the PATH.
fn program(name: &str) -> PathBuf {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&file)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(file))
}

/// A command that opens no console window.
fn command(name: &str) -> Command {
    let mut command = Command::new(program(name));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.stdin(Stdio::null()).stderr(Stdio::null());
    command
}

/// The first line of `ffmpeg -version` (diagnostics), or why FFmpeg could not run.
pub fn version() -> Result<String, String> {
    let out = command("ffmpeg")
        .arg("-version")
        .stdout(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("ffmpeg -version failed ({})", out.status));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.lines().next().unwrap_or("ffmpeg").trim().to_owned())
}

/// What a file holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    pub duration_ms: u64,
    /// Width, height and frames per second of the first video stream.
    pub video: Option<(u32, u32, f32)>,
    pub audio: bool,
}

/// Asks `ffprobe` about `url`.
pub fn probe(url: &str) -> Result<Probe, String> {
    let out = command("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,width,height,avg_frame_rate:stream_disposition=attached_pic",
            "-of",
            "default=noprint_wrappers=1",
            url,
        ])
        .output()
        .map_err(missing)?;
    if !out.status.success() {
        return Err("this file cannot be played".into());
    }
    Ok(parse_probe(&String::from_utf8_lossy(&out.stdout)))
}

/// Reads `ffprobe`'s `key=value` lines. A stream's keys come after its `codec_type`.
fn parse_probe(text: &str) -> Probe {
    let mut probe = Probe::default();
    let mut kind = "";
    let (mut width, mut height, mut fps, mut cover) = (0u32, 0u32, 0f32, false);
    let finish = |kind: &str, width: u32, height: u32, fps: f32, cover: bool, probe: &mut Probe| {
        // An album cover in a sound file is a still "video" stream: not a video.
        if kind == "video" && !cover && width > 0 && height > 0 && probe.video.is_none() {
            probe.video = Some((width, height, if fps > 0. { fps } else { 30. }));
        }
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "codec_type" => {
                finish(kind, width, height, fps, cover, &mut probe);
                (width, height, fps, cover) = (0, 0, 0., false);
                kind = match value {
                    "video" => "video",
                    "audio" => {
                        probe.audio = true;
                        "audio"
                    }
                    _ => "",
                };
            }
            "width" => width = value.parse().unwrap_or(0),
            "height" => height = value.parse().unwrap_or(0),
            "avg_frame_rate" => fps = rate(value),
            "DISPOSITION:attached_pic" => cover = value == "1",
            "duration" => {
                if let Ok(seconds) = value.parse::<f64>() {
                    probe.duration_ms = (seconds * 1000.).round() as u64;
                }
            }
            _ => {}
        }
    }
    finish(kind, width, height, fps, cover, &mut probe);
    probe
}

/// `30000/1001` as 29.97.
fn rate(value: &str) -> f32 {
    match value.split_once('/') {
        Some((a, b)) => {
            let (a, b) = (
                a.parse::<f32>().unwrap_or(0.),
                b.parse::<f32>().unwrap_or(0.),
            );
            if b > 0. { a / b } else { 0. }
        }
        None => value.parse().unwrap_or(0.),
    }
}

fn missing(err: std::io::Error) -> String {
    if err.kind() == std::io::ErrorKind::NotFound {
        "FFmpeg is missing: install it to play media".into()
    } else {
        format!("cannot run FFmpeg: {err}")
    }
}

fn seek_args(from_ms: u64) -> Vec<String> {
    if from_ms == 0 {
        Vec::new()
    } else {
        vec!["-ss".into(), format!("{:.3}", from_ms as f64 / 1000.)]
    }
}

/// Decoder threads for a video. Each holds frames of its own, and by default FFmpeg starts one
/// per core: a 1080p decoder then needs well over 100 MB.
const VIDEO_THREADS: &str = "4";

/// Decodes the video from `from_ms` as BGRA frames of `width` by `height` at `fps`, one after
/// another on stdout.
pub fn video(url: &str, from_ms: u64, width: u32, height: u32, fps: f32) -> Result<Child, String> {
    command("ffmpeg")
        .args(["-nostdin", "-v", "error"])
        .args(["-threads", VIDEO_THREADS, "-filter_threads", "1"])
        .args(seek_args(from_ms))
        .args(["-i", url, "-an", "-sn", "-dn", "-vf"])
        .arg(format!(
            "scale={width}:{height}:flags=bilinear,fps={fps:.3}"
        ))
        .args(["-f", "rawvideo", "-pix_fmt", "bgra", "pipe:1"])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(missing)
}

/// Decodes the one picture at `at_ms`, as a single BGRA frame of `width` by `height` on stdout.
pub fn still(url: &str, at_ms: u64, width: u32, height: u32) -> Result<Child, String> {
    command("ffmpeg")
        .args(["-nostdin", "-v", "error"])
        .args(["-threads", VIDEO_THREADS, "-filter_threads", "1"])
        .args(seek_args(at_ms))
        .args(["-i", url, "-an", "-sn", "-dn", "-frames:v", "1", "-vf"])
        .arg(format!("scale={width}:{height}:flags=bilinear"))
        .args(["-f", "rawvideo", "-pix_fmt", "bgra", "pipe:1"])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(missing)
}

/// The first picture of the file at `path` as a PNG: for picture formats the app cannot read
/// itself (AVIF, HEIC, JPEG XL), when FFmpeg can.
pub fn picture_as_png(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let out = command("ffmpeg")
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-c:v",
            "png",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .output()
        .map_err(missing)?;
    if !out.status.success() || out.stdout.is_empty() {
        return Err("FFmpeg could not read this picture".into());
    }
    Ok(out.stdout)
}

/// Decodes the sound from `from_ms` as interleaved stereo `f32` at `rate`.
pub fn audio(url: &str, from_ms: u64, rate: u32) -> Result<Child, String> {
    command("ffmpeg")
        .args(["-nostdin", "-v", "error"])
        .args(seek_args(from_ms))
        .args(["-i", url, "-vn", "-sn", "-dn", "-ac", "2", "-ar"])
        .arg(rate.to_string())
        .args(["-f", "f32le", "pipe:1"])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(missing)
}

/// The sample rate of [`levels`].
pub const LEVEL_RATE: u32 = 4000;

/// The whole sound as mono `f32` at [`LEVEL_RATE`], for a waveform: quick to decode.
pub fn levels(url: &str) -> Result<Child, String> {
    command("ffmpeg")
        .args([
            "-nostdin", "-v", "error", "-i", url, "-vn", "-sn", "-dn", "-ac", "1", "-ar",
        ])
        .arg(LEVEL_RATE.to_string())
        .args(["-f", "f32le", "pipe:1"])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_video_with_sound() {
        let text = "codec_type=video\nwidth=1920\nheight=1080\navg_frame_rate=30000/1001\n\
                    DISPOSITION:attached_pic=0\ncodec_type=audio\navg_frame_rate=0/0\n\
                    DISPOSITION:attached_pic=0\nduration=12.345\n";
        let probe = parse_probe(text);
        assert_eq!(probe.duration_ms, 12345);
        assert!(probe.audio);
        let (w, h, fps) = probe.video.unwrap();
        assert_eq!((w, h), (1920, 1080));
        assert!((fps - 29.97).abs() < 0.01);
    }

    #[test]
    fn an_album_cover_is_no_video() {
        let text = "codec_type=audio\nDISPOSITION:attached_pic=0\ncodec_type=video\nwidth=500\n\
                    height=500\navg_frame_rate=0/0\nDISPOSITION:attached_pic=1\nduration=200.0\n";
        let probe = parse_probe(text);
        assert!(probe.audio);
        assert_eq!(probe.video, None);
        assert_eq!(probe.duration_ms, 200_000);
    }
}
