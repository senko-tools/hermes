//! Playing one file: FFmpeg decodes its sound into a [`MediaOutput`] and its pictures into a
//! short queue; the sound played so far is the clock the pictures follow. Seeking starts both
//! decoders again at the new time.

use std::collections::VecDeque;
use std::io::Read as _;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui_kit::RenderImage;
use hermes_audio::media::MediaOutput;
use image::Frame;

use super::ffmpeg::{self, Probe};
use super::serve::Served;

/// How many decoded pictures wait at most.
const FRAMES_AHEAD: usize = 8;

/// A decoder process and the thread that reads it. Dropping it stops both.
struct Decoder {
    child: Arc<Mutex<Child>>,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        let _ = child.kill();
        let _ = child.wait();
    }
}

type Pictures = Arc<Mutex<VecDeque<(u64, Arc<RenderImage>)>>>;

pub struct Player {
    served: Served,
    pub probe: Probe,
    output: Option<MediaOutput>,
    /// Where the decoders started, and the output's played frames then.
    start_ms: u64,
    base_frames: u64,
    /// The clock without sound: when it last started, and the position then.
    wall: Option<Instant>,
    wall_ms: u64,
    playing: bool,
    /// The size pictures are decoded at.
    size: (u32, u32),
    pictures: Pictures,
    audio: Option<Decoder>,
    video: Option<Decoder>,
    /// The picture shown now.
    pub current: Option<Arc<RenderImage>>,
}

impl Player {
    /// Opens a file in memory: probes it and opens the sound output, paused at the start.
    /// Blocks: call it off the UI thread.
    pub fn open(bytes: Arc<Vec<u8>>, device: Option<&str>, volume: f32) -> Result<Player, String> {
        let served = super::serve::serve(bytes)?;
        let probe = ffmpeg::probe(&served.url)?;
        if !probe.audio && probe.video.is_none() {
            return Err("this file has no sound or picture".into());
        }
        let output = if probe.audio {
            let output = MediaOutput::open(device)?;
            output.set_volume(volume);
            Some(output)
        } else {
            None
        };
        Ok(Player {
            served,
            probe,
            output,
            start_ms: 0,
            base_frames: 0,
            wall: None,
            wall_ms: 0,
            playing: false,
            size: (0, 0),
            pictures: Arc::default(),
            audio: None,
            video: None,
            current: None,
        })
    }

    pub fn url(&self) -> &str {
        &self.served.url
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// Where playback is, in milliseconds.
    pub fn position_ms(&self) -> u64 {
        let at = match &self.output {
            Some(output) => {
                let played = output.played_frames().saturating_sub(self.base_frames);
                self.start_ms + played * 1000 / u64::from(output.rate().max(1))
            }
            None => {
                self.wall_ms
                    + self
                        .wall
                        .map_or(0, |since| since.elapsed().as_millis() as u64)
            }
        };
        at.min(self.probe.duration_ms.max(1))
    }

    /// Whether it played to the end.
    pub fn ended(&self) -> bool {
        let decoded =
            |d: &Option<Decoder>| d.as_ref().is_none_or(|d| d.done.load(Ordering::Relaxed));
        let sound_left = self.output.as_ref().is_some_and(|o| o.queued_frames() > 0);
        let pictures_left = !self
            .pictures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty();
        self.started()
            && decoded(&self.audio)
            && decoded(&self.video)
            && !sound_left
            && !pictures_left
            && (self.output.is_some() || self.position_ms() + 100 >= self.probe.duration_ms)
    }

    fn started(&self) -> bool {
        self.audio.is_some() || self.video.is_some()
    }

    pub fn play(&mut self) {
        if self.ended() {
            self.seek(0);
        }
        if !self.started() {
            self.restart(self.start_ms);
        }
        self.playing = true;
        if let Some(output) = &self.output {
            output.set_paused(false);
        } else if self.wall.is_none() {
            self.wall = Some(Instant::now());
        }
    }

    pub fn pause(&mut self) {
        self.playing = false;
        if let Some(output) = &self.output {
            output.set_paused(true);
        } else if let Some(since) = self.wall.take() {
            self.wall_ms += since.elapsed().as_millis() as u64;
        }
    }

    pub fn set_volume(&self, volume: f32) {
        if let Some(output) = &self.output {
            output.set_volume(volume);
        }
    }

    /// Jumps to `ms`; keeps playing or paused as it was.
    pub fn seek(&mut self, ms: u64) {
        let ms = ms.min(self.probe.duration_ms.saturating_sub(1));
        self.wall_ms = ms;
        if self.wall.is_some() {
            self.wall = Some(Instant::now());
        }
        self.restart(ms);
    }

    /// The size pictures are decoded at; a change starts the video again where it is.
    pub fn set_size(&mut self, width: u32, height: u32) {
        let size = (width.max(2) & !1, height.max(2) & !1);
        if size != self.size {
            self.size = size;
            if self.playing || self.started() {
                let at = self.position_ms();
                self.restart_video(at);
            }
        }
    }

    /// Starts both decoders at `ms`, dropping what was queued.
    fn restart(&mut self, ms: u64) {
        self.audio = None;
        if let Some(output) = &self.output {
            output.clear();
            self.base_frames = output.played_frames();
            match ffmpeg::audio(&self.served.url, ms, output.rate()) {
                Ok(child) => self.audio = Some(audio_decoder(child, output)),
                Err(err) => tracing::warn!("media sound: {err}"),
            }
        }
        self.start_ms = ms;
        self.restart_video(ms);
    }

    fn restart_video(&mut self, ms: u64) {
        self.video = None;
        self.pictures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let Some((_, _, fps)) = self.probe.video else {
            return;
        };
        if self.size.0 == 0 {
            return;
        }
        let fps = fps.clamp(1., 60.);
        match ffmpeg::video(&self.served.url, ms, self.size.0, self.size.1, fps) {
            Ok(child) => {
                self.video = Some(video_decoder(
                    child,
                    self.size,
                    fps,
                    ms,
                    self.pictures.clone(),
                ));
            }
            Err(err) => tracing::warn!("media video: {err}"),
        }
    }

    /// Takes the picture due now. Returns the pictures it replaces, to be freed from the GPU.
    pub fn advance(&mut self) -> Vec<Arc<RenderImage>> {
        let now = self.position_ms();
        let mut dropped = Vec::new();
        let mut pictures = self.pictures.lock().unwrap_or_else(|e| e.into_inner());
        while pictures.front().is_some_and(|(at, _)| *at <= now) {
            if let Some((_, picture)) = pictures.pop_front()
                && let Some(old) = self.current.replace(picture)
            {
                dropped.push(old);
            }
        }
        dropped
    }

    /// Everything still held on the GPU, when the player goes away.
    pub fn take_pictures(&mut self) -> Vec<Arc<RenderImage>> {
        let mut out: Vec<_> = self.current.take().into_iter().collect();
        out.extend(
            self.pictures
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .drain(..)
                .map(|(_, p)| p),
        );
        out
    }
}

/// Reads stereo samples from `child` into the output while it has room.
fn audio_decoder(mut child: Child, output: &MediaOutput) -> Decoder {
    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let stdout = child.stdout.take();
    let child = Arc::new(Mutex::new(child));
    let sink = output.sink();
    let (thread_stop, thread_done) = (stop.clone(), done.clone());
    let spawned = std::thread::Builder::new()
        .name("media-sound".into())
        .spawn(move || {
            if let Some(mut stdout) = stdout {
                let mut bytes = vec![0u8; 16 * 1024];
                let mut carry = Vec::new();
                loop {
                    while !sink.wants_more() {
                        if thread_stop.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    let read = match stdout.read(&mut bytes) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if thread_stop.load(Ordering::Relaxed) {
                        return;
                    }
                    carry.extend_from_slice(&bytes[..read]);
                    let whole = carry.len() - carry.len() % 8;
                    let samples: Vec<f32> = carry[..whole]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| f32::from_le_bytes(*b))
                        .collect();
                    carry.drain(..whole);
                    sink.push(&samples);
                }
            }
            thread_done.store(true, Ordering::Relaxed);
        });
    if let Err(err) = spawned {
        tracing::warn!("media sound thread: {err}");
        done.store(true, Ordering::Relaxed);
    }
    Decoder { child, stop, done }
}

/// Reads BGRA pictures from `child`, each due `1/fps` after the one before, from `from_ms`.
fn video_decoder(
    mut child: Child,
    (width, height): (u32, u32),
    fps: f32,
    from_ms: u64,
    pictures: Pictures,
) -> Decoder {
    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let stdout = child.stdout.take();
    let child = Arc::new(Mutex::new(child));
    let (thread_stop, thread_done) = (stop.clone(), done.clone());
    let spawned = std::thread::Builder::new()
        .name("media-video".into())
        .spawn(move || {
            if let Some(mut stdout) = stdout {
                let size = width as usize * height as usize * 4;
                let mut n: u64 = 0;
                loop {
                    let mut pixels = vec![0u8; size];
                    if stdout.read_exact(&mut pixels).is_err() {
                        break;
                    }
                    let Some(buffer) = image::RgbaImage::from_raw(width, height, pixels) else {
                        break;
                    };
                    let picture = Arc::new(RenderImage::new(vec![Frame::new(buffer)]));
                    let due = from_ms + (n as f64 * 1000. / f64::from(fps)) as u64;
                    n += 1;
                    loop {
                        if thread_stop.load(Ordering::Relaxed) {
                            return;
                        }
                        let mut queue = pictures.lock().unwrap_or_else(|e| e.into_inner());
                        if queue.len() < FRAMES_AHEAD {
                            queue.push_back((due, picture));
                            break;
                        }
                        drop(queue);
                        std::thread::sleep(std::time::Duration::from_millis(4));
                    }
                }
            }
            thread_done.store(true, Ordering::Relaxed);
        });
    if let Err(err) = spawned {
        tracing::warn!("media video thread: {err}");
        done.store(true, Ordering::Relaxed);
    }
    Decoder { child, stop, done }
}
