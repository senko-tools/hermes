//! The cards sound and video files get in the chat. Nothing loads until play is clicked; then
//! the file is fetched (and decrypted, in a DM), handed to a [`Player`] and played inline. A
//! sound's waveform comes with the file when its sender measured it, else it grows in while the
//! file plays. A paused file is let go once it is off screen for a while, or when the paused
//! ones together hold more than [`CACHE_BYTES`]; it plays on from where it was when clicked.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use hermes_audio::media::MediaOutput;

use gpui_kit::prelude::*;
use gpui_kit::{
    AnimationExt as _, AnyElement, Bounds, Context, DispatchPhase, Div, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels, Point, ScrollDelta,
    ScrollEvent, SharedString, Window, canvas, div, img,
};
use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::Attachment;

use super::player::Player;
use super::waveform::{self, BARS, Waveform};
use crate::assets::{self, icon, media_icons};
use crate::shell::Shell;
use crate::theme::{self, px};

/// The inline video's box, like a picture's.
const VIDEO_W: f32 = 400.;
const VIDEO_H: f32 = 300.;

/// How many bytes of paused files stay in memory, so playing them again needs no download.
const CACHE_BYTES: usize = 64 << 20;
/// A paused file off screen this long is let go.
const OFF_SCREEN_FOR: Duration = Duration::from_secs(15);
/// How often paused files are looked over.
const SWEEP_EVERY: Duration = Duration::from_secs(5);
/// How much the volume changes per pixel scrolled: 5 % per wheel notch (75 px, smooth-scrolled).
const VOLUME_PER_PIXEL: f64 = 0.05 / 75.;
/// How long the volume shows after it changed; it is saved then.
const VOLUME_SHOWN_FOR: Duration = Duration::from_millis(1100);
/// The width of a waveform's bars.
const PILL_W: f32 = 4.;
/// The hover group of a card's download button.
const DOWNLOAD_GROUP: &str = "media-download";
/// The hover group of a sound card's speaker button.
const SPEAKER_GROUP: &str = "media-speaker";
/// The hover group of a video's seek bar: it grows under the mouse.
const SEEK_GROUP: &str = "media-seek";
/// A playing video's controls hide after the mouse rests this many seconds, fading this long.
const CONTROLS_IDLE: f32 = 2.;
const CONTROLS_FADE: f32 = 0.25;

pub type MediaKey = (InstanceId, Id);
/// A seek bar: a file's, and whether it is the one in the full view.
type BarKey = (MediaKey, bool);

/// A press on a seek bar, until it is let go.
struct Scrub {
    bar: BarKey,
    inst: InstanceId,
    attachment: Attachment,
    /// Where on the bar the mouse is, from 0 to 1.
    fraction: f32,
}

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
    /// The card the volume was just changed on: it shows the volume for a moment.
    volume_shown: Option<MediaKey>,
    /// Counts volume changes; it is saved once one stays the last for a moment.
    volume_changes: u64,
    /// A seek bar being held.
    scrub: Option<Scrub>,
    /// Where each seek bar was drawn last.
    bars: Rc<RefCell<HashMap<BarKey, Bounds<Pixels>>>>,
    /// The video the mouse is over (inline or in the full view), and when it last moved.
    pointer: Option<(BarKey, Instant)>,
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
        let volume = self.media_gain();
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

    /// The media volume as a slider position: nothing while muted.
    fn media_gain(&self) -> f32 {
        if self.config.voice.media_muted {
            0.
        } else {
            self.config.voice.media_volume as f32
        }
    }

    /// Mutes or unmutes every sound and video file, and remembers it.
    fn media_mute(&mut self, cx: &mut Context<Self>) {
        let muted = !self.config.voice.media_muted;
        self.config.voice.media_muted = muted;
        self.persist("voice.media_muted", &muted.to_string(), cx);
    }

    /// The wheel over a speaker button or a full view changes the volume of every file, and
    /// shows it on that card.
    fn media_wheel(&mut self, key: &MediaKey, event: &ScrollEvent, cx: &mut Context<Self>) {
        let pixels = match event.delta {
            ScrollDelta::Pixels(p) => f64::from(p.y.as_f32()),
            ScrollDelta::Lines(l) => f64::from(l.y) * 75.,
        };
        if pixels == 0. {
            return;
        }
        let voice = &mut self.config.voice;
        voice.media_volume = (voice.media_volume + pixels * VOLUME_PER_PIXEL).clamp(0., 1.);
        if pixels > 0. {
            voice.media_muted = false;
        }
        self.media.volume_shown = Some(key.clone());
        self.media.volume_changes += 1;
        let change = self.media.volume_changes;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(VOLUME_SHOWN_FOR).await;
            let _ = this.update(cx, |this, cx| {
                if this.media.volume_changes != change {
                    return;
                }
                this.media.volume_shown = None;
                let voice = &this.config.voice;
                let (volume, muted) = (voice.media_volume, voice.media_muted);
                this.persist("voice.media_volume", &format!("{volume:.3}"), cx);
                this.persist("voice.media_muted", &muted.to_string(), cx);
            });
        })
        .detach();
        cx.notify();
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
        let gain = self.media_gain();
        for item in self.media.items.values_mut() {
            busy |= item
                .waveform
                .as_ref()
                .is_some_and(|w| !w.lock().unwrap_or_else(|e| e.into_inner()).done);
            let Load::Ready(player) = &mut item.load else {
                busy |= matches!(item.load, Load::Loading);
                continue;
            };
            player.set_volume(gain);
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
        let (position, duration, playing) = self.media_progress_now(key, a);
        match &self.media.scrub {
            // Held: where it would seek to.
            Some(scrub) if scrub.bar.0 == *key => {
                ((duration as f32 * scrub.fraction) as u64, duration, playing)
            }
            _ => (position, duration, playing),
        }
    }

    fn media_progress_now(&self, key: &MediaKey, a: &Attachment) -> (u64, u64, bool) {
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

    /// A seek bar's area. It notes where it is drawn; a press in it starts scrubbing, which
    /// follows the mouse anywhere in the window and seeks when the button is let go.
    fn seek_area(
        &self,
        bar: BarKey,
        inst: &InstanceId,
        a: &Attachment,
        cx: &mut Context<Self>,
    ) -> Div {
        let bars = self.media.bars.clone();
        let drawn = bar.clone();
        let (inst, a) = (inst.clone(), a.clone());
        div()
            .relative()
            .cursor_pointer()
            .child(
                canvas(
                    move |bounds, _, _| {
                        bars.borrow_mut().insert(drawn.clone(), bounds);
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let Some(fraction) = this.bar_fraction(&bar, event.position) else {
                        return;
                    };
                    this.media.scrub = Some(Scrub {
                        bar: bar.clone(),
                        inst: inst.clone(),
                        attachment: a.clone(),
                        fraction,
                    });
                    cx.notify();
                }),
            )
    }

    /// Where on a seek bar `at` is, from 0 (its left end) to 1, past its ends included.
    fn bar_fraction(&self, bar: &BarKey, at: Point<Pixels>) -> Option<f32> {
        let bounds = *self.media.bars.borrow().get(bar)?;
        let width = bounds.size.width.as_f32();
        (width > 0.).then(|| ((at.x - bounds.origin.x).as_f32() / width).clamp(0., 1.))
    }

    /// While a seek bar is held: listens to the mouse in the whole window, also over the full
    /// view, which hides everything under it from the mouse. It goes in the root.
    pub fn media_scrub_catcher(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.media.scrub.as_ref()?;
        let this = cx.entity().downgrade();
        Some(
            canvas(
                |_, _, _| (),
                move |_, (), window, _| {
                    let moved = this.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            let _ = moved.update(cx, |this, cx| {
                                this.media_scrub_move(event.position, cx);
                            });
                        }
                    });
                    let released = this.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                            cx.stop_propagation();
                            let _ = released.update(cx, |this, cx| this.media_scrub_end(cx));
                        }
                    });
                },
            )
            .absolute()
            .size_0()
            .into_any_element(),
        )
    }

    fn media_scrub_move(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(bar) = self.media.scrub.as_ref().map(|s| s.bar.clone()) else {
            return;
        };
        if let Some(fraction) = self.bar_fraction(&bar, at)
            && let Some(scrub) = &mut self.media.scrub
        {
            scrub.fraction = fraction;
            cx.notify();
        }
    }

    fn media_scrub_end(&mut self, cx: &mut Context<Self>) {
        if let Some(scrub) = self.media.scrub.take() {
            self.media_seek_to(scrub.inst, scrub.attachment, scrub.fraction, cx);
        }
    }

    /// Seeks to `fraction` of a file; one not loaded yet loads and plays from there.
    fn media_seek_to(
        &mut self,
        inst: InstanceId,
        attachment: Attachment,
        fraction: f32,
        cx: &mut Context<Self>,
    ) {
        let key = (inst.clone(), attachment.id);
        match self.media.items.get(&key).map(|i| &i.load) {
            Some(Load::Ready(_)) => self.media_seek(&key, fraction, cx),
            Some(Load::Loading) => {}
            _ => {
                let length = attachment
                    .duration_ms
                    .or_else(|| self.media.resume.get(&key).map(|(_, length)| *length));
                if let Some(length) = length {
                    let at = (length as f32 * fraction) as u64;
                    self.media.resume.insert(key.clone(), (at, length));
                }
                self.media.items.remove(&key);
                self.media_load(inst, attachment, cx);
            }
        }
    }

    /// A small download button, for a card's title row or a video's top bar.
    fn download_button(
        &self,
        inst: &InstanceId,
        a: &Attachment,
        color: gpui_kit::Rgba,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let (inst, a) = (inst.clone(), a.clone());
        div()
            .id(SharedString::from(format!("download-{}", a.id)))
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(px(22.))
            .rounded(px(4.))
            .cursor_pointer()
            .group(DOWNLOAD_GROUP)
            .hover(|d| d.bg(theme::alpha(theme::text_tertiary(), 0.18)))
            .child(
                icon(assets::DOWNLOAD, 15., color)
                    .group_hover(DOWNLOAD_GROUP, |s| s.text_color(theme::accent())),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.download(inst.clone(), a.clone(), cx);
            }))
    }

    /// A sound card's speaker, once the file has played: a click mutes every file, and the wheel
    /// over it sets their volume. Over the rest of a card the wheel scrolls the chat.
    fn speaker_button(
        &self,
        key: &MediaKey,
        a: &Attachment,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let glyph = if self.media_gain() <= 0. {
            media_icons::VOLUME_OFF
        } else {
            assets::VOLUME
        };
        let wheel_key = key.clone();
        div()
            .id(SharedString::from(format!("speaker-{}", a.id)))
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(px(22.))
            .rounded(px(4.))
            .cursor_pointer()
            .group(SPEAKER_GROUP)
            .hover(|d| d.bg(theme::alpha(theme::text_tertiary(), 0.18)))
            .child(
                icon(glyph, 15., theme::text_tertiary())
                    .group_hover(SPEAKER_GROUP, |s| s.text_color(theme::accent())),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.media_mute(cx);
            }))
            .on_scroll(cx.listener(move |this, event, _, cx| {
                cx.stop_propagation();
                this.media_wheel(&wheel_key, event, cx);
            }))
    }

    /// What a card says about a file: where it is and how long it lasts, and its size.
    fn media_meta(&self, key: &MediaKey, a: &Attachment) -> String {
        let (position, duration, _) = self.media_progress(key, a);
        let started = self.media.scrub.as_ref().is_some_and(|s| s.bar.0 == *key)
            || self
                .media
                .items
                .get(key)
                .is_some_and(|i| matches!(i.load, Load::Ready(_)))
            || self.media.resume.contains_key(key);
        let size = human_size(a.size);
        if started {
            format!("{} / {}  ·  {size}", clock(position), clock(duration))
        } else if let Some(ms) = a.duration_ms {
            format!("{}  ·  {size}", clock(ms))
        } else {
            size
        }
    }

    /// The volume in place of a card's time, for a moment after it changed there.
    fn volume_meta(&self, key: &MediaKey) -> Option<AnyElement> {
        if self.media.volume_shown.as_ref() != Some(key) {
            return None;
        }
        let volume = self.media_gain();
        let glyph = if volume <= 0. {
            media_icons::VOLUME_OFF
        } else {
            assets::VOLUME
        };
        Some(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(4.))
                .text_size(theme::caption_size())
                .text_color(theme::accent())
                .child(icon(glyph, 13., theme::accent()))
                .child(format!("{:.0} %", volume * 100.))
                .into_any_element(),
        )
    }

    /// A sound file: play, its name, time and size, and its waveform as the progress bar.
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
        // How many bars are played, the last one partly.
        let filled = position as f32 / duration as f32 * BARS as f32;
        let wave = self
            .seek_area((key.clone(), false), inst, a, cx)
            .flex()
            .items_center()
            .h(px(32.))
            .children((0..BARS).map(|i| {
                let level = bars.get(i).copied();
                let part = (filled - i as f32).clamp(0., 1.);
                let color = if level.is_some() {
                    theme::alpha(theme::text_secondary(), 0.55)
                } else {
                    theme::alpha(theme::text_tertiary(), 0.35)
                };
                let height = 4. + level.unwrap_or(0.) * 26.;
                let pill = div()
                    .relative()
                    .overflow_hidden()
                    .w(px(PILL_W))
                    .rounded(px(PILL_W / 2.))
                    .bg(color)
                    .when(part > 0., |pill| {
                        pill.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .h_full()
                                .w(px(PILL_W * part))
                                .rounded_l(px(PILL_W / 2.))
                                .when(part >= 1., |d| d.rounded(px(PILL_W / 2.)))
                                .bg(theme::accent()),
                        )
                    });
                // A level measured just now grows in.
                let pill = match level {
                    Some(_) if grows => pill
                        .with_animation(
                            SharedString::from(format!("wave-in-{}-{i}", a.id)),
                            gpui_kit::Animation::new(Duration::from_millis(320))
                                .with_easing(gpui_kit::ease_out_quint()),
                            move |d, t| d.h(px(4. + (height - 4.) * t)),
                        )
                        .into_any_element(),
                    _ => pill.h(px(height)).into_any_element(),
                };
                div()
                    .flex()
                    .flex_1()
                    .justify_center()
                    .items_center()
                    .h_full()
                    .child(pill)
            }));
        let meta = self.volume_meta(&key).unwrap_or_else(|| {
            div()
                .flex_shrink_0()
                .text_size(theme::caption_size())
                .text_color(theme::text_tertiary())
                .child(self.media_meta(&key, a))
                .into_any_element()
        });
        let (toggle_inst, toggle_a) = (inst.clone(), a.clone());
        let speaker = item.is_some().then(|| self.speaker_button(&key, a, cx));
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .mt(px(4.))
            .w(px(420.))
            .pl(px(12.))
            .pr(px(8.))
            .py(px(8.))
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
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(22.))
                            .text_size(theme::secondary_size())
                            .child(icon(media_icons::MUSIC, 14., theme::accent()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(a.filename.clone()),
                            )
                            .child(meta)
                            .children(speaker)
                            .child(self.download_button(inst, a, theme::text_tertiary(), cx)),
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
                        // Up to the edge of the download button above it.
                        None => wave.mr(px(4.)).into_any_element(),
                    }),
            )
            .into_any_element()
    }

    /// How visible a video's controls are, from 0 to 1: fully while it is paused, its seek bar
    /// is held, its volume just changed or the mouse moves over it; then they fade out.
    fn controls_shown(&self, bar: &BarKey, playing: bool) -> f32 {
        let held = self.media.scrub.as_ref().is_some_and(|s| s.bar == *bar);
        let volume = self.media.volume_shown.as_ref() == Some(&bar.0);
        if !playing || held || volume {
            return 1.;
        }
        match &self.media.pointer {
            Some((over, moved)) if over == bar => {
                let idle = moved.elapsed().as_secs_f32();
                (1. - (idle - CONTROLS_IDLE) / CONTROLS_FADE).clamp(0., 1.)
            }
            _ => 0.,
        }
    }

    /// A round button on a video, in white.
    fn overlay_button(id: SharedString, glyph: &'static str) -> gpui_kit::Stateful<Div> {
        div()
            .id(id)
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(px(28.))
            .rounded(px(14.))
            .cursor_pointer()
            .hover(|d| d.bg(gpui_kit::hsla(0., 0., 1., 0.16)))
            .child(icon(glyph, 18., gpui_kit::rgb(0xffffff)))
    }

    /// A video's top bar: its name, length and size, and download.
    fn video_top(&self, inst: &InstanceId, a: &Attachment, cx: &mut Context<Self>) -> Div {
        let meta = match a.duration_ms {
            Some(ms) => format!("{}  ·  {}", clock(ms), human_size(a.size)),
            None => human_size(a.size),
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(12.))
            .pr(px(6.))
            .pt(px(6.))
            .pb(px(18.))
            .bg(gpui_kit::linear_gradient(
                180.,
                gpui_kit::linear_color_stop(gpui_kit::hsla(0., 0., 0., 0.7), 0.),
                gpui_kit::linear_color_stop(gpui_kit::hsla(0., 0., 0., 0.), 1.),
            ))
            .text_size(theme::caption_size())
            .text_color(gpui_kit::rgb(0xffffff))
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
                    .text_color(gpui_kit::hsla(0., 0., 1., 0.75))
                    .child(meta),
            )
            .child(self.download_button(inst, a, gpui_kit::rgb(0xffffff), cx))
    }

    /// A playing video's bottom bar: the seek bar, then play, time, volume and full view.
    fn video_bottom(
        &self,
        key: &MediaKey,
        a: &Attachment,
        inst: &InstanceId,
        full: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let (position, duration, playing) = self.media_progress(key, a);
        let played = (position as f32 / duration as f32).clamp(0., 1.);
        let muted = self.media_gain() <= 0.;
        let (toggle_inst, toggle_a) = (inst.clone(), a.clone());
        let (full_key, wheel_key) = (key.clone(), key.clone());
        let id = |what: &str| SharedString::from(format!("{what}-{}-{full}", a.id));
        let seek = self
            .seek_area((key.clone(), full), inst, a, cx)
            .flex()
            .items_center()
            .w_full()
            .h(px(14.))
            .group(SEEK_GROUP)
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(3.))
                    .rounded(px(2.))
                    .bg(gpui_kit::hsla(0., 0., 1., 0.3))
                    .group_hover(SEEK_GROUP, |s| s.h(px(5.)))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_full()
                            .w(gpui_kit::relative(played))
                            .rounded(px(2.))
                            .bg(theme::accent()),
                    ),
            );
        let volume = Self::overlay_button(
            id("vmute"),
            if muted {
                media_icons::VOLUME_OFF
            } else {
                assets::VOLUME
            },
        )
        .on_click(cx.listener(|this, _, _, cx| {
            cx.stop_propagation();
            this.media_mute(cx);
        }))
        .on_scroll(cx.listener(move |this, event, _, cx| {
            cx.stop_propagation();
            this.media_wheel(&wheel_key, event, cx);
        }));
        div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .flex()
            .flex_col()
            .px(px(10.))
            .pt(px(22.))
            .pb(px(4.))
            .bg(gpui_kit::linear_gradient(
                0.,
                gpui_kit::linear_color_stop(gpui_kit::hsla(0., 0., 0., 0.75), 0.),
                gpui_kit::linear_color_stop(gpui_kit::hsla(0., 0., 0., 0.), 1.),
            ))
            .text_size(theme::caption_size())
            .text_color(gpui_kit::rgb(0xffffff))
            .child(seek)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        Self::overlay_button(
                            id("vplay"),
                            if playing {
                                media_icons::PAUSE
                            } else {
                                media_icons::PLAY
                            },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.media_toggle(toggle_inst.clone(), toggle_a.clone(), cx);
                        })),
                    )
                    .child(div().flex_shrink_0().ml(px(2.)).child(format!(
                        "{} / {}",
                        clock(position),
                        clock(duration)
                    )))
                    .child(div().flex_1())
                    .children(self.volume_meta(key))
                    .child(volume)
                    .child(
                        Self::overlay_button(
                            id("vfull"),
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
                    ),
            )
    }

    /// What a video shows over its picture: its bars while they show, and it notes the mouse
    /// moving over it. Clicking the picture plays or pauses. In the full view the wheel sets the
    /// volume; in the chat it scrolls, and only the speaker button sets it.
    fn video_surface(
        &self,
        surface: gpui_kit::Stateful<Div>,
        key: &MediaKey,
        a: &Attachment,
        inst: &InstanceId,
        full: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let bar: BarKey = (key.clone(), full);
        let playing = self.media_progress(key, a).2;
        let shown = self.controls_shown(&bar, playing);
        let (moved, left) = (bar.clone(), bar);
        let (click_inst, click_a) = (inst.clone(), a.clone());
        let wheel_key = key.clone();
        surface
            .cursor_pointer()
            .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                this.media.pointer = Some((moved.clone(), Instant::now()));
                cx.notify();
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if !hovered && this.media.pointer.as_ref().is_some_and(|(b, _)| *b == left) {
                    this.media.pointer = None;
                    cx.notify();
                }
            }))
            .when(full, |d| {
                d.on_scroll(cx.listener(move |this, event, _, cx| {
                    cx.stop_propagation();
                    this.media_wheel(&wheel_key, event, cx);
                }))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.media_toggle(click_inst.clone(), click_a.clone(), cx);
            }))
            .when(shown > 0., |d| {
                d.child(self.video_top(inst, a, cx).opacity(shown))
                    .child(self.video_bottom(key, a, inst, full, cx).opacity(shown))
            })
    }

    /// A video: the picture with play until clicked, then the player inline, its controls over
    /// it. Its name and size show at the top, so it is clear what clicking loads.
    pub fn render_video_card(
        &self,
        inst: &InstanceId,
        a: &Attachment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key: MediaKey = (inst.clone(), a.id);
        self.media_drawn(&key);
        let item = self.media.items.get(&key);
        let shape = match (item.map(|i| &i.load), a.width, a.height) {
            (Some(Load::Ready(player)), ..) => player.probe.video,
            (_, Some(w), Some(h)) => Some((w, h, 0.)),
            _ => None,
        };
        let (w, h) = shape.map_or((VIDEO_W, VIDEO_W * 9. / 16.), |(w, h, _)| {
            let (w, h) = fit((w, h), (VIDEO_W, VIDEO_H));
            (w.max(240.), h.max(135.))
        });
        let card = div()
            .id(SharedString::from(format!("video-{}", a.id)))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .mt(px(4.))
            .w(px(w))
            .h(px(h))
            .rounded(px(8.))
            .overflow_hidden()
            .bg(gpui_kit::rgb(0x000000))
            .border_1()
            .border_color(theme::border());
        match item.map(|i| (&i.load, i.full)) {
            Some((Load::Ready(player), full)) => {
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
                self.video_surface(card.child(picture), &key, a, inst, false, cx)
                    .into_any_element()
            }
            Some((Load::Failed(err), _)) => card
                .text_size(theme::caption_size())
                .text_color(theme::danger())
                .child(err.clone())
                .child(self.video_top(inst, a, cx))
                .into_any_element(),
            other => {
                let loading = matches!(other, Some((Load::Loading, _)));
                let (toggle_inst, toggle_a) = (inst.clone(), a.clone());
                card.child(
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
                .child(self.video_top(inst, a, cx))
                .into_any_element()
            }
        }
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
        let surface = div()
            .id("video-full")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui_kit::rgb(0x000000))
            .occlude()
            .font_family(theme::font())
            .children(picture);
        Some(
            gpui_kit::deferred(self.video_surface(surface, key, &attachment, &key.0, true, cx))
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
