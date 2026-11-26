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
    /// Width, height and frames per second of the first video stream, as it is shown: turned
    /// as its rotation says (a phone video held upright) and with square pixels. FFmpeg turns
    /// the frames it decodes the same way before scaling them.
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
            "format=duration:stream=codec_type,width,height,avg_frame_rate,sample_aspect_ratio:\
             stream_disposition=attached_pic:stream_tags=rotate:stream_side_data=rotation",
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
    let mut stream = Stream::default();
    let finish = |kind: &str, s: &Stream, probe: &mut Probe| {
        // An album cover in a sound file is a still "video" stream: not a video.
        if kind == "video" && !s.cover && s.width > 0 && s.height > 0 && probe.video.is_none() {
            let (w, h) = s.shown();
            probe.video = Some((w, h, if s.fps > 0. { s.fps } else { 30. }));
        }
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "codec_type" => {
                finish(kind, &stream, &mut probe);
                stream = Stream::default();
                kind = match value {
                    "video" => "video",
                    "audio" => {
                        probe.audio = true;
                        "audio"
                    }
                    _ => "",
                };
            }
            "width" => stream.width = value.parse().unwrap_or(0),
            "height" => stream.height = value.parse().unwrap_or(0),
            "avg_frame_rate" => stream.fps = rate(value),
            "sample_aspect_ratio" => stream.pixel = pixel_shape(value),
            // The display matrix (newer files) or the older tag; either turns it.
            "rotation" | "TAG:rotate" => {
                if let Ok(degrees) = value.trim().parse::<f64>() {
                    stream.quarter_turned = (degrees.round() as i64).rem_euclid(180) == 90;
                }
            }
            "DISPOSITION:attached_pic" => stream.cover = value == "1",
            "duration" => {
                if let Ok(seconds) = value.parse::<f64>() {
                    probe.duration_ms = (seconds * 1000.).round() as u64;
                }
            }
            _ => {}
        }
    }
    finish(kind, &stream, &mut probe);
    probe
}

/// What `ffprobe` says about one stream.
#[derive(Default)]
struct Stream {
    width: u32,
    height: u32,
    fps: f32,
    cover: bool,
    /// A pixel's width over its height, when not square.
    pixel: Option<f64>,
    /// Turned a quarter (90 or 270 degrees): shown as tall as it is stored wide.
    quarter_turned: bool,
}

impl Stream {
    /// Its size as it is shown: pixels made square (wider, as players do), then turned.
    fn shown(&self) -> (u32, u32) {
        let width = match self.pixel {
            Some(ratio) => ((f64::from(self.width) * ratio).round() as u32).max(1),
            None => self.width,
        };
        if self.quarter_turned {
            (self.height, width)
        } else {
            (width, self.height)
        }
    }
}

/// `sample_aspect_ratio=4:3` as 4/3; `None` for square pixels or an unknown shape (`0:1`).
fn pixel_shape(value: &str) -> Option<f64> {
    let (a, b) = value.split_once(':')?;
    let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
    let ratio = a / b;
    (a > 0. && b > 0. && (ratio - 1.).abs() > 1e-3 && (0.1..=10.).contains(&ratio)).then_some(ratio)
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
    fn a_rotated_video_is_shown_upright() {
        // What ffprobe 9 prints for a 320x180 file with a 90 degree display matrix.
        let text = "codec_type=video\nwidth=320\nheight=180\nsample_aspect_ratio=1:1\n\
                    avg_frame_rate=10/1\nDISPOSITION:attached_pic=0\nrotation=90\n\
                    duration=2.000000\n";
        let (w, h, _) = parse_probe(text).video.unwrap();
        assert_eq!((w, h), (180, 320));
        let old_tag = "codec_type=video\nwidth=1920\nheight=1080\nTAG:rotate=-270\n";
        let (w, h, _) = parse_probe(old_tag).video.unwrap();
        assert_eq!((w, h), (1080, 1920));
        let upside_down = "codec_type=video\nwidth=1920\nheight=1080\nrotation=-180\n";
        let (w, h, _) = parse_probe(upside_down).video.unwrap();
        assert_eq!((w, h), (1920, 1080));
    }

    #[test]
    fn wide_pixels_widen_the_picture() {
        let text = "codec_type=video\nwidth=320\nheight=180\nsample_aspect_ratio=2:1\n";
        let (w, h, _) = parse_probe(text).video.unwrap();
        assert_eq!((w, h), (640, 180));
        let unknown = "codec_type=video\nwidth=320\nheight=180\nsample_aspect_ratio=0:1\n";
        assert_eq!(parse_probe(unknown).video.unwrap().0, 320);
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
