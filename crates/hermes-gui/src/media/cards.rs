//! The cards sound and video files get in the chat. Nothing loads until play is clicked; then
//! the file is fetched (and decrypted, in a DM), handed to a [`Player`] and played inline. A
//! sound's waveform comes with the file when its sender measured it, else it grows in while the
//! file plays. A paused file is let go once it is off screen for a while, or when the paused
//! ones together hold more than [`CACHE_BYTES`]; it plays on from where it was when clicked.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use hermes_audio::media::MediaOutput;

use gpui_kit::prelude::*;
use gpui_kit::{
    AnimationExt as _, AnyElement, Context, Div, ObjectFit, SharedString, Window, div, img,
};
use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Attachment;

use super::player::Player;
use super::waveform::{self, BARS, Waveform};
use crate::assets::{self, icon, media_icons};
use crate::channel::icon_button;
use crate::shell::Shell;
use crate::theme::{self, px};

/// How many steps the video's seek bar has.
const SEEK_STEPS: usize = 100;
/// The inline video's box, like a picture's.
const VIDEO_W: f32 = 400.;
const VIDEO_H: f32 = 300.;

/// How many bytes of paused files stay in memory, so playing them again needs no download.
const CACHE_BYTES: usize = 64 << 20;
/// A paused file off screen this long is let go.
const OFF_SCREEN_FOR: Duration = Duration::from_secs(15);
/// How often paused files are looked over.
const SWEEP_EVERY: Duration = Duration::from_secs(5);

pub type MediaKey = (InstanceId, Id);

pub enum Load {
    Loading,
    Ready(Box<Player>),
    Failed(String),
}

/// A sound or video someone clicked.
pub struct MediaItem {
    pub load: Load,
    /// Measured while it plays, when it came without one.
    pub waveform: Option<Arc<Mutex<Waveform>>>,
    wave_stop: Arc<AtomicBool>,
    pub muted: bool,
    /// Shown over the whole window.
    pub full: bool,
    /// The frame it was last drawn in (see [`Media::frame`]).
    drawn: Cell<u64>,
    /// Since when it has not been drawn.
    hidden_since: Option<Instant>,
    /// When it last played or was paused.
    used: Instant,
}

impl MediaItem {
    fn new() -> MediaItem {
        MediaItem {
            load: Load::Loading,
            waveform: None,
            wave_stop: Arc::new(AtomicBool::new(false)),
            muted: false,
            full: false,
            drawn: Cell::new(0),
            hidden_since: None,
            used: Instant::now(),
        }
    }

    /// The bytes of a loaded file that is not playing: what letting it go frees.
    fn idle_bytes(&self) -> Option<usize> {
        match &self.load {
            Load::Ready(player) if !player.playing() && !self.full => Some(player.file_size()),
            _ => None,
        }
    }
}

/// Every clicked file, and what outlives letting one go.
#[derive(Default)]
pub struct Media {
    items: HashMap<MediaKey, MediaItem>,
    /// Where files that were let go had got to, and how long they last.
    resume: HashMap<MediaKey, (u64, u64)>,
    /// Waveforms measured here, kept when their file is let go.
    measured: HashMap<MediaKey, Vec<u8>>,
    /// The sound output every file shares, and the device it was opened on.
    output: Option<(String, Weak<MediaOutput>)>,
    /// Counts draws, so a card knows whether it was on screen in the last one.
    frame: u64,
}

impl Drop for MediaItem {
    fn drop(&mut self) {
        self.wave_stop.store(true, Ordering::Relaxed);
    }
}

/// `1:05`, or `1:01:42` past an hour.
fn clock(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The video's size inside `max`, keeping its shape.
fn fit((w, h): (u32, u32), (max_w, max_h): (f32, f32)) -> (f32, f32) {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let scale = (max_w / w).min(max_h / h).min(1.);
    (w * scale, h * scale)
}

/// A round play or pause button.
fn play_button(
    id: SharedString,
    playing: bool,
    loading: bool,
    size: f32,
) -> gpui_kit::Stateful<Div> {
    let glyph = if playing {
        media_icons::PAUSE
    } else {
        media_icons::PLAY
    };
    let inner = icon(glyph, size * 0.55, theme::on_accent());
    let inner = if loading {
        inner
            .with_animation(
                "media-loading",
                gpui_kit::Animation::new(std::time::Duration::from_millis(900)).repeat(),
                |d, t| d.opacity(0.4 + 0.6 * (t * std::f32::consts::PI).sin()),
            )
            .into_any_element()
    } else {
        inner.into_any_element()
    };
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(size / 2.))
        .bg(theme::accent())
        .cursor_pointer()
        .hover(|d| d.bg(theme::accent_highlight()))
        .child(inner)
}

impl Shell {
    /// Play or pause; the first click loads the file.
    fn media_toggle(&mut self, inst: InstanceId, attachment: Attachment, cx: &mut Context<Self>) {
        let key = (inst.clone(), attachment.id);
        match self.media.items.get(&key).map(|i| &i.load) {
            Some(Load::Ready(player)) => {
                if player.playing() {
                    self.media_pause(&key);
                } else {
                    self.media_play(&key);
                }
                cx.notify();
            }
            Some(Load::Loading) => {}
            Some(Load::Failed(_)) => {
                self.media.items.remove(&key);
                self.media_load(inst, attachment, cx);
            }
            None => self.media_load(inst, attachment, cx),
        }
    }

    /// Fetches a file and plays it once it is ready, from where it was let go.
    fn media_load(&mut self, inst: InstanceId, attachment: Attachment, cx: &mut Context<Self>) {
        let key = (inst.clone(), attachment.id);
        self.media.items.insert(key.clone(), MediaItem::new());
        let core = self.core.clone();
        let device = self.config.voice.output_device.clone();
        let shared = self
            .media
            .output
            .as_ref()
            .filter(|(on, _)| *on == device)
            .and_then(|(_, output)| output.upgrade());
        let volume = self.config.voice.output_volume as f32;
        let at = self.media.resume.remove(&key).map_or(0, |(at, _)| at);
        let measure = attachment.waveform.is_none()
            && !self.media.measured.contains_key(&key)
            && super::is_sound(&attachment.content_type, &attachment.filename);
        self.run(
            cx,
            async move {
                let bytes = core
                    .download(&inst, &attachment)
                    .await
                    .map_err(|e| e.to_string())?;
                tokio::task::spawn_blocking(move || {
                    Player::open(Arc::new(bytes), shared, Some(&device), volume, at)
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |this, result, _, cx| {
                let Some(item) = this.media.items.get_mut(&key) else {
                    return;
                };
                match result {
                    Ok(player) => {
                        if measure {
                            item.waveform = Some(waveform::start(
                                player.url().to_owned(),
                                player.probe.duration_ms,
                                item.wave_stop.clone(),
                            ));
                        }
                        if let Some(output) = player.output() {
                            let device = this.config.voice.output_device.clone();
                            this.media.output = Some((device, Arc::downgrade(output)));
                        }
                        item.load = Load::Ready(Box::new(player));
                        this.media_play(&key);
                    }
                    Err(err) => {
                        tracing::warn!("media {}: {err}", key.1);
                        item.load = Load::Failed(err);
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Plays a loaded file; whatever else plays pauses first (they share the output).
    fn media_play(&mut self, key: &MediaKey) {
        let others: Vec<MediaKey> = self
            .media
            .items
            .iter()
            .filter(|(k, i)| *k != key && matches!(&i.load, Load::Ready(p) if p.playing()))
            .map(|(k, _)| k.clone())
            .collect();
        for other in &others {
            self.media_pause(other);
        }
        if let Some(item) = self.media.items.get_mut(key)
            && let Load::Ready(player) = &mut item.load
        {
            player.play();
            item.used = Instant::now();
        }
    }

    fn media_pause(&mut self, key: &MediaKey) {
        if let Some(item) = self.media.items.get_mut(key)
            && let Load::Ready(player) = &mut item.load
        {
            player.pause();
            item.used = Instant::now();
        }
    }

    fn media_seek(&mut self, key: &MediaKey, fraction: f32, cx: &mut Context<Self>) {
        let Some(item) = self.media.items.get_mut(key) else {
            return;
        };
        let Load::Ready(player) = &mut item.load else {
            return;
        };
        let ms = (player.probe.duration_ms as f32 * fraction.clamp(0., 1.)) as u64;
        player.seek(ms);
        if !player.playing() {
            self.media_play(key);
        }
        cx.notify();
    }

    fn media_mute(&mut self, key: &MediaKey, cx: &mut Context<Self>) {
        let volume = self.config.voice.output_volume as f32;
        if let Some(item) = self.media.items.get_mut(key) {
            item.muted = !item.muted;
            if let Load::Ready(player) = &mut item.load {
                player.set_volume(if item.muted { 0. } else { volume });
            }
            cx.notify();
        }
    }

    fn media_full(&mut self, key: &MediaKey, full: bool, cx: &mut Context<Self>) {
        if let Some(item) = self.media.items.get_mut(key) {
            item.full = full;
            cx.notify();
        }
    }

    /// Closes the full view, if one is open. `false` if none was.
    pub fn close_media_full(&mut self) -> bool {
        let mut closed = false;
        for item in self.media.items.values_mut() {
            closed |= std::mem::take(&mut item.full);
        }
        closed
    }

    /// Looks over the paused files every few seconds and lets go of what is not needed.
    pub fn start_media_sweep(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SWEEP_EVERY).await;
                let alive = this.update(cx, |this, cx| {
                    if this.sweep_media() {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Lets go of paused files off screen for [`OFF_SCREEN_FOR`], then of the least recently
    /// used paused ones while together they hold more than [`CACHE_BYTES`]. Whether any went.
    fn sweep_media(&mut self) -> bool {
        let frame = self.media.frame;
        let now = Instant::now();
        let mut free = Vec::new();
        for (key, item) in &mut self.media.items {
            if item.drawn.get() >= frame {
                item.hidden_since = None;
                continue;
            }
            let since = *item.hidden_since.get_or_insert(now);
            let idle = item.idle_bytes().is_some() || matches!(item.load, Load::Failed(_));
            if idle && now.duration_since(since) >= OFF_SCREEN_FOR {
                free.push(key.clone());
            }
        }
        let mut kept: Vec<(Instant, usize, MediaKey)> = self
            .media
            .items
            .iter()
            .filter(|(key, _)| !free.contains(key))
            .filter_map(|(key, item)| Some((item.used, item.idle_bytes()?, key.clone())))
            .collect();
        kept.sort_by_key(|(used, ..)| *used);
        let mut total: usize = kept.iter().map(|(_, bytes, _)| bytes).sum();
        for (_, bytes, key) in kept {
            if total <= CACHE_BYTES {
                break;
            }
            total -= bytes;
            free.push(key);
        }
        for key in &free {
            self.free_media(key);
        }
        !free.is_empty()
    }

    /// Forgets a file, keeping where it was and its measured waveform.
    fn free_media(&mut self, key: &MediaKey) {
        let Some(item) = self.media.items.remove(key) else {
            return;
        };
        if let Load::Ready(player) = &item.load {
            let at = player.position_ms();
            if at > 0 && at + 250 < player.probe.duration_ms {
                let length = player.probe.duration_ms;
                self.media.resume.insert(key.clone(), (at, length));
            }
        }
        if let Some(wave) = &item.waveform {
            let wave = wave.lock().unwrap_or_else(|e| e.into_inner());
            if wave.done {
                self.media.measured.insert(key.clone(), wave.to_bytes());
            }
        }
    }

    /// Before each draw: each playing video takes its picture due now, decoded at the size it is
    /// shown at; what ended stops. Keeps drawing while something plays or a waveform grows.
    pub fn advance_media(&mut self, window: &mut Window) {
        self.media.frame += 1;
        let scale = window.scale_factor();
        let viewport = self.viewport;
        let full_box = (
            viewport.width.as_f32() * 0.9 / theme::scale(),
            (viewport.height.as_f32() - 120.) / theme::scale(),
        );
        let mut busy = false;
        for item in self.media.items.values_mut() {
            busy |= item
                .waveform
                .as_ref()
                .is_some_and(|w| !w.lock().unwrap_or_else(|e| e.into_inner()).done);
            let Load::Ready(player) = &mut item.load else {
                busy |= matches!(item.load, Load::Loading);
                continue;
            };
            if let Some((w, h, _)) = player.probe.video {
                let shown = fit(
                    (w, h),
                    if item.full {
                        full_box
                    } else {
                        (VIDEO_W, VIDEO_H)
                    },
                );
                let factor = theme::scale() * scale;
                player.set_size(
                    ((shown.0 * factor) as u32).min(w),
                    ((shown.1 * factor) as u32).min(h),
                );
                for old in player.advance() {
                    let _ = window.drop_image(old);
                }
            }
            if player.playing() {
                if player.ended() {
                    player.pause();
                } else {
                    busy = true;
                }
            }
        }
        if busy {
            window.request_animation_frame();
        }
    }

    /// The position, length and playing state of a file.
    fn media_progress(&self, key: &MediaKey, a: &Attachment) -> (u64, u64, bool) {
        match self.media.items.get(key).map(|i| &i.load) {
            Some(Load::Ready(player)) => (
                player.position_ms(),
                player.probe.duration_ms.max(1),
                player.playing(),
            ),
            _ => match self.media.resume.get(key) {
                Some(&(at, length)) => (at, length.max(1), false),
                None => (0, a.duration_ms.unwrap_or(0).max(1), false),
            },
        }
    }

    /// Notes that a card is on screen in this draw.
    fn media_drawn(&self, key: &MediaKey) {
        if let Some(item) = self.media.items.get(key) {
            item.drawn.set(self.media.frame);
        }
    }

    /// A sound file: play, its name and time, and its waveform as the progress bar.
    pub fn render_audio_card(
        &self,
        inst: &InstanceId,
        a: &Attachment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key: MediaKey = (inst.clone(), a.id);
        self.media_drawn(&key);
        let item = self.media.items.get(&key);
        let loading = matches!(item.map(|i| &i.load), Some(Load::Loading));
        let failed = match item.map(|i| &i.load) {
            Some(Load::Failed(err)) => Some(err.clone()),
            _ => None,
        };
        let (position, duration, playing) = self.media_progress(&key, a);
        let known = a
            .waveform
            .as_deref()
            .or_else(|| self.media.measured.get(&key).map(Vec::as_slice))
            .map(Waveform::known);
        let live = item
            .and_then(|i| i.waveform.as_ref())
            .map(|w| w.lock().unwrap_or_else(|e| e.into_inner()).clone());
        let (bars, grows) = match (known, live) {
            (Some(wave), _) => (wave.bars, false),
            (None, Some(wave)) => (wave.bars, true),
            (None, None) => (Vec::new(), false),
        };
        let played = position as f32 / duration as f32;
        let started = item.is_some_and(|i| matches!(i.load, Load::Ready(_)))
            || self.media.resume.contains_key(&key);
        let time = if started {
            format!("{} / {}", clock(position), clock(duration))
        } else if let Some(ms) = a.duration_ms {
            clock(ms)
        } else {
            human_size(a.size)
        };
        let (toggle_inst, toggle_a) = (inst.clone(), a.clone());
        let (down_inst, down_a) = (inst.clone(), a.clone());
        let bar_row = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .h(px(32.))
            .children((0..BARS).map(|i| {
                let level = bars.get(i).copied();
                let done = (i as f32 + 0.5) / BARS as f32 <= played;
                let color = if done {
                    theme::accent()
                } else if level.is_some() {
                    theme::alpha(theme::text_secondary(), 0.55)
                } else {
                    theme::alpha(theme::text_tertiary(), 0.35)
                };
                let height = 4. + level.unwrap_or(0.) * 26.;
                let seek_key = key.clone();
                let bar = div()
                    .id(SharedString::from(format!("wave-{}-{i}", a.id)))
                    .flex()
                    .items_center()
                    .h_full()
                    .px(px(0.5))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.media_seek(&seek_key, (i as f32 + 0.5) / BARS as f32, cx);
                    }));
                let pill = div().w(px(4.)).rounded(px(2.)).bg(color);
                // A level measured just now grows in.
                let pill = match level {
                    Some(_) if grows => pill
                        .with_animation(
                            SharedString::from(format!("wave-in-{}-{i}", a.id)),
                            gpui_kit::Animation::new(std::time::Duration::from_millis(320))
                                .with_easing(gpui_kit::ease_out_quint()),
                            move |d, t| d.h(px(4. + (height - 4.) * t)),
                        )
                        .into_any_element(),
                    _ => pill.h(px(height)).into_any_element(),
                };
                bar.child(pill)
            }));
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .mt(px(4.))
            .w(px(420.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(8.))
            .bg(theme::sidebar())
            .border_1()
            .border_color(theme::border())
            .child(
                play_button(
                    SharedString::from(format!("play-{}", a.id)),
                    playing,
                    loading,
                    38.,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.media_toggle(toggle_inst.clone(), toggle_a.clone(), cx);
                })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(theme::secondary_size())
                            .child(icon(media_icons::MUSIC, 14., theme::accent()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(a.filename.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(theme::caption_size())
                                    .text_color(theme::text_tertiary())
                                    .child(time),
                            ),
                    )
                    .child(match failed {
                        Some(err) => div()
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .text_size(theme::caption_size())
                            .text_color(theme::danger())
                            .child(err)
                            .into_any_element(),
                        None => bar_row.into_any_element(),
                    }),
            )
            .child(
                icon_button(
                    SharedString::from(format!("download-{}", a.id)),
                    assets::DOWNLOAD,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.download(down_inst.clone(), down_a.clone(), cx);
                })),
            )
            .into_any_element()
    }

    /// The controls under a playing video: play, time, the seek bar, sound and full view.
    fn video_controls(
        &self,
        key: &MediaKey,
        a: &Attachment,
        inst: &InstanceId,
        full: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let (position, duration, playing) = self.media_progress(key, a);
        let muted = self.media.items.get(key).is_some_and(|i| i.muted);
        let played = position as f32 / duration as f32;
        let (toggle_inst, toggle_a) = (inst.clone(), a.clone());
        let (mute_key, full_key) = (key.clone(), key.clone());
        let seek = div()
            .flex()
            .flex_1()
            .h(px(16.))
            .items_center()
            .children((0..SEEK_STEPS).map(|i| {
                let seek_key = key.clone();
                let done = (i as f32 + 0.5) / SEEK_STEPS as f32 <= played;
                div()
                    .id(SharedString::from(format!("seek-{}-{i}", a.id)))
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .child(div().w_full().h(px(4.)).bg(if done {
                        theme::accent()
                    } else {
                        theme::alpha(theme::text_tertiary(), 0.4)
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.media_seek(&seek_key, (i as f32 + 0.5) / SEEK_STEPS as f32, cx);
                    }))
            }));
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .py(px(6.))
            .bg(theme::sidebar())
            .text_size(theme::caption_size())
            .text_color(theme::text_secondary())
            .child(
                div()
                    .id(SharedString::from(format!("vplay-{}-{full}", a.id)))
                    .cursor_pointer()
                    .child(icon(
                        if playing {
                            media_icons::PAUSE
                        } else {
                            media_icons::PLAY
                        },
                        20.,
                        theme::text(),
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.media_toggle(toggle_inst.clone(), toggle_a.clone(), cx);
                    })),
            )
            .child(div().flex_shrink_0().child(format!(
                "{} / {}",
                clock(position),
                clock(duration)
            )))
            .child(seek)
            .child(
                icon_button(
                    SharedString::from(format!("vmute-{}-{full}", a.id)),
                    if muted {
                        media_icons::VOLUME_OFF
                    } else {
                        assets::VOLUME
                    },
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.media_mute(&mute_key, cx);
                })),
            )
            .child(
                icon_button(
                    SharedString::from(format!("vfull-{}-{full}", a.id)),
                    if full {
                        media_icons::FULLSCREEN_EXIT
                    } else {
                        media_icons::FULLSCREEN
                    },
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.media_full(&full_key, !full, cx);
                })),
            )
    }

    /// A video: a card with play until clicked, then the player inline.
    pub fn render_video_card(
        &self,
        inst: &InstanceId,
        a: &Attachment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key: MediaKey = (inst.clone(), a.id);
        self.media_drawn(&key);
        let item = self.media.items.get(&key);
        let (toggle_inst, toggle_a) = (inst.clone(), a.clone());
        let (down_inst, down_a) = (inst.clone(), a.clone());
        let caption = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(6.))
            .text_size(theme::caption_size())
            .text_color(theme::text_secondary())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(a.filename.clone()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(theme::text_tertiary())
                    .child(human_size(a.size)),
            )
            .child(
                icon_button(
                    SharedString::from(format!("download-{}", a.id)),
                    assets::DOWNLOAD,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.download(down_inst.clone(), down_a.clone(), cx);
                })),
            );
        let frame = |w: f32, h: f32| {
            div()
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .w(px(w))
                .h(px(h))
                .bg(gpui_kit::rgb(0x000000))
        };
        let body = match item.map(|i| (&i.load, i.full)) {
            Some((Load::Ready(player), full)) => {
                let (w, h) = player
                    .probe
                    .video
                    .map_or((VIDEO_W, VIDEO_H * 0.75), |(w, h, _)| {
                        fit((w, h), (VIDEO_W, VIDEO_H))
                    });
                let picture = if full {
                    div()
                        .text_size(theme::caption_size())
                        .text_color(theme::text_tertiary())
                        .child("Playing in full view")
                        .into_any_element()
                } else {
                    match &player.current {
                        Some(picture) => img(picture.clone())
                            .size_full()
                            .object_fit(ObjectFit::Contain)
                            .into_any_element(),
                        None if player.probe.video.is_none() => {
                            icon(media_icons::MUSIC, 40., theme::accent()).into_any_element()
                        }
                        None => div().into_any_element(),
                    }
                };
                let click_inst = inst.clone();
                let click_a = a.clone();
                div()
                    .flex()
                    .flex_col()
                    .child(
                        frame(w, h)
                            .id(SharedString::from(format!("video-{}", a.id)))
                            .cursor_pointer()
                            .child(picture)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.media_toggle(click_inst.clone(), click_a.clone(), cx);
                            })),
                    )
                    .child(self.video_controls(&key, a, inst, false, cx))
                    .into_any_element()
            }
            Some((Load::Failed(err), _)) => frame(VIDEO_W, VIDEO_H * 0.6)
                .text_size(theme::caption_size())
                .text_color(theme::danger())
                .child(err.clone())
                .into_any_element(),
            other => {
                let loading = matches!(other, Some((Load::Loading, _)));
                frame(VIDEO_W, VIDEO_H * 0.6)
                    .child(
                        play_button(
                            SharedString::from(format!("play-{}", a.id)),
                            false,
                            loading,
                            56.,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.media_toggle(toggle_inst.clone(), toggle_a.clone(), cx);
                        })),
                    )
                    .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .mt(px(4.))
            .w(px(VIDEO_W))
            .rounded(px(8.))
            .overflow_hidden()
            .bg(theme::sidebar())
            .border_1()
            .border_color(theme::border())
            .child(body)
            .child(caption)
            .into_any_element()
    }

    /// A video in full view, over the whole window.
    pub fn render_video_full(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (key, item) = self.media.items.iter().find(|(_, i)| i.full)?;
        self.media_drawn(key);
        let Load::Ready(player) = &item.load else {
            return None;
        };
        let attachment = self.find_attachment(key)?;
        let picture = player.current.clone().map(|p| {
            img(p)
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        });
        let (click_inst, click_a) = (key.0.clone(), attachment.clone());
        Some(
            gpui_kit::deferred(
                div()
                    .id("video-full")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .flex_col()
                    .bg(gpui_kit::rgb(0x000000))
                    .occlude()
                    .font_family(theme::font())
                    .child(
                        div()
                            .id("video-full-picture")
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .cursor_pointer()
                            .children(picture)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.media_toggle(click_inst.clone(), click_a.clone(), cx);
                            })),
                    )
                    .child(self.video_controls(key, &attachment, &key.0, true, cx)),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }

    /// The attachment a clicked file came from, from the loaded messages.
    fn find_attachment(&self, (inst, id): &MediaKey) -> Option<Attachment> {
        let store = self.core.store();
        let s = store.instance(inst)?;
        s.windows.values().find_map(|w| {
            w.messages
                .iter()
                .flat_map(|m| m.attachments.iter())
                .find(|a| a.id == *id)
                .cloned()
        })
    }
}

fn human_size(bytes: u64) -> String {
    crate::channel::human_size(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks() {
        assert_eq!(clock(65_000), "1:05");
        assert_eq!(clock(3_702_000), "1:01:42");
        assert_eq!(clock(0), "0:00");
    }

    #[test]
    fn videos_fit_their_box_without_growing() {
        assert_eq!(fit((1920, 1080), (400., 300.)), (400., 225.));
        assert_eq!(fit((200, 100), (400., 300.)), (200., 100.));
    }
}
