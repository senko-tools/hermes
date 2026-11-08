//! The cards sound and video files get in the chat. Nothing loads until play is clicked; then
//! the file is fetched (and decrypted, in a DM), handed to a [`Player`] and played inline. A
//! sound's waveform comes with the file when its sender measured it, else it grows in while the
//! file plays.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
        if let Some(item) = self.media.get_mut(&key) {
            if let Load::Ready(player) = &mut item.load {
                if player.playing() {
                    player.pause();
                } else {
                    player.play();
                    self.pause_other_media(&key);
                }
            } else if matches!(item.load, Load::Failed(_)) {
                self.media.remove(&key);
                self.media_toggle(inst, attachment, cx);
                return;
            }
            cx.notify();
            return;
        }
        self.media.insert(
            key.clone(),
            MediaItem {
                load: Load::Loading,
                waveform: None,
                wave_stop: Arc::new(AtomicBool::new(false)),
                muted: false,
                full: false,
            },
        );
        let core = self.core.clone();
        let device = self.config.voice.output_device.clone();
        let volume = self.config.voice.output_volume as f32;
        let measure = attachment.waveform.is_none()
            && super::is_sound(&attachment.content_type, &attachment.filename);
        self.run(
            cx,
            async move {
                let bytes = core
                    .download(&inst, &attachment)
                    .await
                    .map_err(|e| e.to_string())?;
                tokio::task::spawn_blocking(move || {
                    Player::open(Arc::new(bytes), Some(&device), volume)
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |this, result, _, cx| {
                let Some(item) = this.media.get_mut(&key) else {
                    return;
                };
                match result {
                    Ok(mut player) => {
                        if measure {
                            item.waveform = Some(waveform::start(
                                player.url().to_owned(),
                                player.probe.duration_ms,
                                item.wave_stop.clone(),
                            ));
                        }
                        player.play();
                        item.load = Load::Ready(Box::new(player));
                        this.pause_other_media(&key);
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

    /// One plays at a time.
    fn pause_other_media(&mut self, playing: &MediaKey) {
        for (key, item) in &mut self.media {
            if key != playing
                && let Load::Ready(player) = &mut item.load
                && player.playing()
            {
                player.pause();
            }
        }
    }

    fn media_seek(&mut self, key: &MediaKey, fraction: f32, cx: &mut Context<Self>) {
        if let Some(MediaItem {
            load: Load::Ready(player),
            ..
        }) = self.media.get_mut(key)
        {
            let ms = (player.probe.duration_ms as f32 * fraction.clamp(0., 1.)) as u64;
            player.seek(ms);
            if !player.playing() {
                player.play();
                self.pause_other_media(key);
            }
            cx.notify();
        }
    }

    fn media_mute(&mut self, key: &MediaKey, cx: &mut Context<Self>) {
        let volume = self.config.voice.output_volume as f32;
        if let Some(item) = self.media.get_mut(key) {
            item.muted = !item.muted;
            if let Load::Ready(player) = &item.load {
                player.set_volume(if item.muted { 0. } else { volume });
            }
            cx.notify();
        }
    }

    fn media_full(&mut self, key: &MediaKey, full: bool, cx: &mut Context<Self>) {
        if let Some(item) = self.media.get_mut(key) {
            item.full = full;
            cx.notify();
        }
    }

    /// Closes the full view, if one is open. `false` if none was.
    pub fn close_media_full(&mut self) -> bool {
        let mut closed = false;
        for item in self.media.values_mut() {
            closed |= std::mem::take(&mut item.full);
        }
        closed
    }

    /// Before each draw: each playing video takes its picture due now, decoded at the size it is
    /// shown at; what ended stops. Keeps drawing while something plays or a waveform grows.
    pub fn advance_media(&mut self, window: &mut Window) {
        let scale = window.scale_factor();
        let viewport = self.viewport;
        let full_box = (
            viewport.width.as_f32() * 0.9 / theme::scale(),
            (viewport.height.as_f32() - 120.) / theme::scale(),
        );
        let mut busy = false;
        for item in self.media.values_mut() {
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

    /// The position, length and playing state of a clicked file.
    fn media_progress(&self, key: &MediaKey, a: &Attachment) -> (u64, u64, bool) {
        match self.media.get(key).map(|i| &i.load) {
            Some(Load::Ready(player)) => (
                player.position_ms(),
                player.probe.duration_ms.max(1),
                player.playing(),
            ),
            _ => (0, a.duration_ms.unwrap_or(0).max(1), false),
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
        let item = self.media.get(&key);
        let loading = matches!(item.map(|i| &i.load), Some(Load::Loading));
        let failed = match item.map(|i| &i.load) {
            Some(Load::Failed(err)) => Some(err.clone()),
            _ => None,
        };
        let (position, duration, playing) = self.media_progress(&key, a);
        let known = a.waveform.as_deref().map(Waveform::known);
        let live = item
            .and_then(|i| i.waveform.as_ref())
            .map(|w| w.lock().unwrap_or_else(|e| e.into_inner()).clone());
        let (bars, grows) = match (known, live) {
            (Some(wave), _) => (wave.bars, false),
            (None, Some(wave)) => (wave.bars, true),
            (None, None) => (Vec::new(), false),
        };
        let played = position as f32 / duration as f32;
        let started = item.is_some_and(|i| matches!(i.load, Load::Ready(_)));
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
        let muted = self.media.get(key).is_some_and(|i| i.muted);
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
        let item = self.media.get(&key);
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
        let (key, item) = self.media.iter().find(|(_, i)| i.full)?;
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

/// Every clicked file, by instance and attachment.
pub type Media = HashMap<MediaKey, MediaItem>;

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
