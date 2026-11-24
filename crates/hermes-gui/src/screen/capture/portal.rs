//! Screen capture on Wayland: the `org.freedesktop.portal.ScreenCast` portal asks the user what
//! to share in the desktop's own dialog (Wayland does not let apps see other windows), and the
//! picture arrives over PipeWire.
//!
//! - [`pick`] runs the dialog. With the restore token of the last share, the desktop shares the
//!   same thing again without asking (portal version 4 and later).
//! - [`start`] runs a PipeWire stream on a thread of its own and hands frames to the same
//!   encoder thread as on Windows and X11, at most `quality.fps` times a second. Frames arrive
//!   only when something changed; the last one is sent again when a keyframe is due.
//!
//! Frames come in shared memory (BGRx and friends), with the cursor drawn in where the desktop
//! can. DMA-BUF is not asked for: every compositor falls back to shared memory.

use std::cell::RefCell;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
    StartCastOptions,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode, ResponseError, Session};
use hermes_client_core::Core;
use pipewire as pw;
use pw::spa;

use super::{
    Frame, KEYFRAME_EVERY, Quality, STATS_EVERY, Stats, linux::spawn_encoder, log_stats, micros,
    scale,
};

/// What the user chose in the desktop's dialog, ready to stream.
pub struct Picked {
    fd: OwnedFd,
    node: u32,
    /// Hand it to the next [`pick`] to share the same thing without the dialog.
    pub token: Option<String>,
    session: Session<Screencast>,
}

impl std::fmt::Debug for Picked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picked(node {})", self.node)
    }
}

#[derive(Debug)]
pub enum PickError {
    /// The user closed the dialog.
    Cancelled,
    Failed(String),
}

impl From<ashpd::Error> for PickError {
    fn from(err: ashpd::Error) -> PickError {
        match err {
            ashpd::Error::Response(ResponseError::Cancelled) => PickError::Cancelled,
            ashpd::Error::Portal(ashpd::PortalError::Cancelled(_)) => PickError::Cancelled,
            other => PickError::Failed(other.to_string()),
        }
    }
}

/// Asks the desktop what to share (a screen or a window).
pub async fn pick(restore_token: Option<String>) -> Result<Picked, PickError> {
    let portal = Screencast::new().await?;
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await?;
    // The cursor drawn into the picture, as on Windows; else none rather than metadata.
    let cursor = match portal.available_cursor_modes().await {
        Ok(modes) if modes.contains(CursorMode::EMBEDDED) => CursorMode::EMBEDDED,
        _ => CursorMode::HIDDEN,
    };
    let types = portal
        .available_source_types()
        .await
        .map(|t| t & (SourceType::MONITOR | SourceType::WINDOW))
        .ok()
        .filter(|t| !t.is_empty())
        .unwrap_or(SourceType::MONITOR);
    let mut options = SelectSourcesOptions::default()
        .set_cursor_mode(cursor)
        .set_sources(types)
        .set_multiple(false);
    // Remembering what was chosen came with version 4.
    if portal.version() >= 4 {
        options = options
            .set_persist_mode(PersistMode::ExplicitlyRevoked)
            .set_restore_token(restore_token.as_deref().filter(|t| !t.is_empty()));
    }
    portal.select_sources(&session, options).await?.response()?;
    let streams = portal
        .start(&session, None, StartCastOptions::default())
        .await?
        .response()?;
    let Some(stream) = streams.streams().first() else {
        let _ = session.close().await;
        return Err(PickError::Failed("nothing was chosen".into()));
    };
    let node = stream.pipe_wire_node_id();
    let token = streams.restore_token().map(str::to_owned);
    let fd = portal
        .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
        .await?;
    tracing::info!(
        node,
        kind = ?stream.source_type(),
        size = ?stream.size(),
        "screen share: the desktop's dialog chose"
    );
    Ok(Picked {
        fd,
        node,
        token,
        session,
    })
}

/// Streams what was picked until `stop` (or the share ends, or the user stops it from the
/// desktop's own controls).
pub fn start(
    picked: Picked,
    core: Core,
    quality: Quality,
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
) -> Result<(), String> {
    let runtime = tokio::runtime::Handle::try_current().map_err(|e| e.to_string())?;
    let Picked {
        fd, node, session, ..
    } = picked;
    std::thread::Builder::new()
        .name("hermes-screen-capture".into())
        .spawn(move || {
            if let Err(err) = stream(fd, node, &core, quality, &stop, &keyframe) {
                tracing::warn!("screen share: PipeWire: {err}");
            }
            // Ended from here (the desktop stopped it, or PipeWire failed): the share ends too.
            if !stop.load(Ordering::Relaxed) {
                core.stop_screen_share();
            }
            // The desktop's "you are sharing" indicator goes away with the session.
            runtime.spawn(async move {
                let _ = session.close().await;
            });
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// The newest frame from PipeWire, as it came (before scaling).
struct Raw {
    width: u32,
    height: u32,
    bgra: bool,
    pixels: Vec<u8>,
}

/// Shared by PipeWire's callbacks and the pacing timer (all on the stream's thread).
struct State {
    format: spa::param::video::VideoInfoRaw,
    /// Arrived since the last tick.
    fresh: Option<Raw>,
    /// A spare buffer for the next frame (no allocation per frame).
    spare: Vec<u8>,
    last: Option<Arc<Frame>>,
    last_sent: Instant,
    idle: bool,
    stats_since: Instant,
    error: Option<String>,
}

fn stream(
    fd: OwnedFd,
    node: u32,
    core: &Core,
    quality: Quality,
    stop: &Arc<AtomicBool>,
    keyframe: &Arc<AtomicBool>,
) -> Result<(), String> {
    use pw::properties::properties;

    pw::init();
    let stats = Arc::new(Stats::default());
    let frames = spawn_encoder(core, quality, keyframe, &stats).ok_or("no encoder thread")?;
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let pw_core = context.connect_fd_rc(fd, None).map_err(|e| e.to_string())?;
    let stream = pw::stream::StreamBox::new(
        &pw_core,
        "hermes-screen-share",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| e.to_string())?;
    let state = Rc::new(RefCell::new(State {
        format: spa::param::video::VideoInfoRaw::default(),
        fresh: None,
        spare: Vec::new(),
        last: None,
        last_sent: Instant::now(),
        idle: false,
        stats_since: Instant::now(),
        error: None,
    }));

    let quit = mainloop.clone();
    let _listener = stream
        .add_local_listener_with_user_data(state.clone())
        .state_changed(move |_, state, old, new| {
            tracing::debug!("screen share stream: {old:?} -> {new:?}");
            match new {
                pw::stream::StreamState::Error(err) => {
                    state.borrow_mut().error = Some(err);
                    quit.quit();
                }
                // The desktop ended the cast (its own "stop sharing" button).
                pw::stream::StreamState::Unconnected => quit.quit(),
                _ => {}
            }
        })
        .param_changed(|_, state, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut state = state.borrow_mut();
            if state.format.parse(param).is_ok() {
                let size = state.format.size();
                tracing::info!(
                    "screen share stream: {:?} {}x{}",
                    state.format.format(),
                    size.width,
                    size.height
                );
            }
        })
        .process(|stream, state| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let mut state = state.borrow_mut();
            let format = state.format.format();
            let bgra = match format {
                spa::param::video::VideoFormat::BGRx | spa::param::video::VideoFormat::BGRA => true,
                spa::param::video::VideoFormat::RGBx | spa::param::video::VideoFormat::RGBA => {
                    false
                }
                _ => return,
            };
            let size = state.format.size();
            let (width, height) = (size.width, size.height);
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else {
                return;
            };
            let chunk = data.chunk();
            // A cursor-only update, or a damaged buffer: nothing new to show.
            if chunk.size() == 0 || chunk.flags().contains(spa::buffer::ChunkFlags::CORRUPTED) {
                return;
            }
            let row = width as usize * 4;
            let stride = usize::try_from(chunk.stride()).ok().filter(|s| *s >= row);
            let stride = stride.unwrap_or(row);
            let offset = chunk.offset() as usize;
            let Some(bytes) = data.data() else {
                return;
            };
            if bytes.len() < offset + stride * (height as usize).saturating_sub(1) + row {
                return;
            }
            let mut pixels = std::mem::take(&mut state.spare);
            pixels.clear();
            pixels.reserve(row * height as usize);
            for y in 0..height as usize {
                let start = offset + y * stride;
                pixels.extend_from_slice(&bytes[start..start + row]);
            }
            if let Some(old) = state.fresh.replace(Raw {
                width,
                height,
                bgra,
                pixels,
            }) {
                state.spare = old.pixels;
            }
        })
        .register()
        .map_err(|e| e.to_string())?;

    let format = format_pod(quality.fps)?;
    let mut params = [spa::pod::Pod::from_bytes(&format).ok_or("bad format")?];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| e.to_string())?;

    // Paces what goes to the encoder: at most the share's frame rate.
    let interval = Duration::from_millis(1000 / u64::from(quality.fps.max(1)));
    let tick = {
        let state = state.clone();
        let quit = mainloop.clone();
        let keyframe = keyframe.clone();
        let core = core.clone();
        let stats = stats.clone();
        let stop = stop.clone();
        move |_| {
            if stop.load(Ordering::Relaxed) || !core.sharing_screen() {
                quit.quit();
                return;
            }
            let mut state = state.borrow_mut();
            if core.share_watchers() == Some(0) {
                if !state.idle {
                    tracing::info!("screen share: nobody is watching, frames are not sent");
                    state.idle = true;
                }
                if let Some(raw) = state.fresh.take() {
                    state.spare = raw.pixels;
                }
                return;
            }
            if state.idle {
                tracing::info!("screen share: someone is watching, frames are sent again");
                state.idle = false;
                keyframe.store(true, Ordering::Relaxed);
                state.stats_since = Instant::now();
            }
            let begin = Instant::now();
            let frame = match state.fresh.take() {
                Some(raw) => {
                    let frame = Arc::new(scale(
                        Frame {
                            width: raw.width,
                            height: raw.height,
                            bgra: raw.bgra,
                            pixels: raw.pixels,
                        },
                        quality,
                    ));
                    stats.captured.fetch_add(1, Ordering::Relaxed);
                    stats
                        .capture_us
                        .fetch_add(micros(begin.elapsed()), Ordering::Relaxed);
                    state.last = Some(frame.clone());
                    Some(frame)
                }
                None if keyframe.load(Ordering::Relaxed)
                    || state.last_sent.elapsed() >= KEYFRAME_EVERY =>
                {
                    state.last.clone()
                }
                None => None,
            };
            if let Some(frame) = frame {
                match frames.try_send(frame) {
                    Ok(()) => state.last_sent = Instant::now(),
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {
                        stats.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => quit.quit(),
                }
            }
            if state.stats_since.elapsed() >= STATS_EVERY {
                let size = state
                    .last
                    .as_deref()
                    .map_or((0, 0), |f| (f.width, f.height));
                let over = state.stats_since.elapsed();
                log_stats(&stats, over, size, quality.fps, "PipeWire, OpenH264");
                state.stats_since = Instant::now();
            }
        }
    };
    let timer = mainloop.loop_().add_timer(tick);
    let _ = timer.update_timer(Some(interval), Some(interval));
    mainloop.run();
    let error = state.borrow_mut().error.take();
    match error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// The formats Hermes takes: 4 bytes per pixel in shared memory, any size, at most `fps`.
fn format_pod(fps: u32) -> Result<Vec<u8>, String> {
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use spa::param::video::VideoFormat;
    use spa::pod::{self, serialize::PodSerializer};
    use spa::utils::{Fraction, Rectangle, SpaTypes};

    let object = pod::object!(
        SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA
        ),
        pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: 1920,
                height: 1080
            },
            Rectangle {
                width: 1,
                height: 1
            },
            Rectangle {
                width: 8192,
                height: 8192
            }
        ),
        pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: fps, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: fps, denom: 1 }
        ),
    );
    PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pod::Value::Object(object),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(|e| format!("{e:?}"))
}
