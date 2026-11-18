//! What to share, picked from pictures: a panel over most of the window with the screens on
//! top, a divider, and the windows below in a scrolling grid. All cards are the same size, and
//! rows are centred. Picking a card only selects it; Share starts. The pictures are taken on a
//! background thread every few seconds, only while the picker is open.
//!
//! The choice is `voice.share_screen`, saved when sharing starts: the last screen comes back
//! selected, and the last window too while it is still open (its id does not outlive it).

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, ObjectFit, RenderImage, SharedString, div, img};
use image::Frame;

use crate::assets::{self, icon};
use crate::shell::Shell;
use crate::theme::{self, px};
use crate::voice::chip;

/// How often the pictures are taken again while the picker is open.
const REFRESH: Duration = Duration::from_millis(2500);
/// A card: its picture's size, and the two lines under it.
const CARD_W: f32 = 184.;
const PICTURE_H: f32 = 104.;
/// The gap between cards.
const GAP: f32 = 12.;
/// The space kept around the picker, and the widest it gets.
const MARGIN: f32 = 40.;
const MAX_W: f32 = 1080.;
/// The id `close_later` knows it by.
const ID: &str = "share-picker";

/// A screen or window in the picker.
pub struct ShareSource {
    pub value: String,
    pub title: String,
    pub detail: String,
    pub window: bool,
    pub minimized: bool,
    pub picture: Option<Arc<RenderImage>>,
}

/// The open picker.
pub struct SharePicker {
    /// The selected `voice.share_screen` value.
    pub selected: String,
    /// `None` until the first listing arrived.
    pub sources: Option<Vec<ShareSource>>,
    /// The pictures came at least once (the first listing comes quickly without them).
    pictured: bool,
    /// Which refresh loop is the current one: one that finds another number stops.
    generation: u64,
}

/// Lists everything that can be shared, with pictures (call it off the UI thread).
fn list(pictures: bool) -> Vec<ShareSource> {
    crate::screen::capture::sources(pictures)
        .into_iter()
        .map(|s| ShareSource {
            value: s.value,
            title: s.title,
            detail: s.detail,
            window: s.window,
            minimized: s.minimized,
            picture: s
                .picture
                .map(|p| Arc::new(RenderImage::new(vec![Frame::new(p)]))),
        })
        .collect()
}

impl Shell {
    /// Opens the picker with the last choice selected, and starts taking pictures.
    pub fn open_share_picker(&mut self, cx: &mut Context<Self>) {
        self.cancel_closing(ID);
        self.share_generation += 1;
        self.share_menu = Some(SharePicker {
            selected: self.config.voice.share_screen.clone(),
            sources: None,
            pictured: false,
            generation: self.share_generation,
        });
        self.refresh_share_sources(self.share_generation, false, cx);
        cx.notify();
    }

    /// Takes the pictures again, and again after [`REFRESH`] while the picker stays open.
    fn refresh_share_sources(&mut self, generation: u64, pictures: bool, cx: &mut Context<Self>) {
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(move || list(pictures))
                    .await
                    .unwrap_or_default()
            },
            move |this, sources, _, cx| {
                let Some(picker) = this
                    .share_menu
                    .as_mut()
                    .filter(|p| p.generation == generation && !crate::modal::closing(ID))
                else {
                    this.images
                        .retired
                        .extend(sources.into_iter().filter_map(|s| s.picture));
                    return;
                };
                // A window closed since: the primary screen instead.
                if !sources.iter().any(|s| s.value == picker.selected) {
                    picker.selected = String::new();
                }
                picker.pictured |= pictures;
                let old = picker.sources.replace(sources).unwrap_or_default();
                this.images
                    .retired
                    .extend(old.into_iter().filter_map(|s| s.picture));
                cx.notify();
                if !pictures {
                    this.refresh_share_sources(generation, true, cx);
                    return;
                }
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(REFRESH).await;
                    let _ = this.update(cx, |this, cx| {
                        if this
                            .share_menu
                            .as_ref()
                            .is_some_and(|p| p.generation == generation)
                        {
                            this.refresh_share_sources(generation, true, cx);
                        }
                    });
                })
                .detach();
            },
        );
    }

    /// Closes the picker after it played its way out.
    pub fn close_share_picker(&mut self, cx: &mut Context<Self>) {
        self.close_later(
            ID,
            |s| {
                s.take_share_picker();
            },
            cx,
        );
        cx.notify();
    }

    /// Closes the picker at once (Escape) and frees its pictures. `true` if it was open.
    pub fn take_share_picker(&mut self) -> bool {
        let Some(picker) = self.share_menu.take() else {
            return false;
        };
        self.images.retired.extend(
            picker
                .sources
                .unwrap_or_default()
                .into_iter()
                .filter_map(|s| s.picture),
        );
        true
    }

    /// Saves the choice and starts sharing it.
    fn share_picked(&mut self, cx: &mut Context<Self>) {
        let Some(picked) = self.share_menu.as_ref().map(|p| p.selected.clone()) else {
            return;
        };
        self.set_share_setting(
            "voice.share_screen",
            &crate::settings::toml_string(picked),
            cx,
        );
        self.close_share_picker(cx);
        self.start_share(cx);
    }

    /// One card: its picture (or a plain face for a minimized window), its name and what it is.
    fn share_card(
        &self,
        n: usize,
        source: &ShareSource,
        selected: bool,
        pictured: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let value = source.value.clone();
        let picture: AnyElement = match &source.picture {
            Some(picture) => img(picture.clone())
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            // The pictures are on their way.
            None if !pictured && !source.minimized => div().into_any_element(),
            None => {
                let (glyph, note) = if source.minimized {
                    (None, "Minimized")
                } else if source.window {
                    (None, "No preview")
                } else {
                    (Some("monitor"), "No preview")
                };
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .size_full()
                    .child(match glyph {
                        Some(glyph) => icon(glyph, 28., theme::text_tertiary()).into_any_element(),
                        None => window_glyph().into_any_element(),
                    })
                    .child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(note),
                    )
                    .into_any_element()
            }
        };
        div()
            .id(SharedString::from(format!(
                "share-card-{n}-{}",
                source.value
            )))
            .relative()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(CARD_W))
            .p(px(6.))
            .gap(px(6.))
            .rounded(theme::row_radius())
            .cursor_pointer()
            .when(selected, |d| d.bg(theme::alpha(theme::accent(), 0.12)))
            .when(!selected, |d| d.hover(|d| d.bg(theme::hover())))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w_full()
                    .h(px(PICTURE_H))
                    .rounded(theme::radius_small())
                    .overflow_hidden()
                    .bg(theme::code_background())
                    .child(picture),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .px(px(2.))
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::secondary_size())
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if selected {
                                theme::accent()
                            } else {
                                theme::text()
                            })
                            .child(source.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .h(px(16.))
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(source.detail.clone()),
                    ),
            )
            // The ring is drawn over the card so nothing inside moves.
            .when(selected, |d| {
                d.child(
                    theme::border_width(
                        div().absolute().inset_0().rounded(theme::row_radius()),
                        px(2.),
                    )
                    .border_color(theme::accent()),
                )
            })
            .on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
                    cx.stop_propagation();
                    if let Some(picker) = &mut this.share_menu {
                        picker.selected = value.clone();
                    }
                    // A double click shares it right away.
                    if event.click_count() >= 2 {
                        this.share_picked(cx);
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    pub fn render_share_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::screen::capture::{FRAME_RATES, RESOLUTIONS};
        use gpui_kit::{Animation, AnimationExt as _, ease_out_quint};
        let picker = self.share_menu.as_ref()?;
        let voice = &self.config.voice;
        let grid = |cards: Vec<AnyElement>| {
            div()
                .flex()
                .flex_wrap()
                .justify_center()
                .gap(px(GAP))
                .w_full()
                .children(cards)
        };
        let (screens, windows): (Vec<_>, Vec<_>) = match &picker.sources {
            Some(sources) => sources
                .iter()
                .enumerate()
                .map(|(n, s)| {
                    (
                        s.window,
                        self.share_card(n, s, s.value == picker.selected, picker.pictured, cx),
                    )
                })
                .partition(|(window, _)| !window),
            None => (Vec::new(), Vec::new()),
        };
        let screens: Vec<AnyElement> = screens.into_iter().map(|(_, c)| c).collect();
        let windows: Vec<AnyElement> = windows.into_iter().map(|(_, c)| c).collect();
        let caption = |text: &'static str| {
            div()
                .text_size(theme::caption_size())
                .text_color(theme::text_tertiary())
                .child(text)
        };
        // A rule with its name in the middle, between the screens and the windows.
        let divider = |name: &'static str| {
            let line = || div().flex_1().h(px(1.)).bg(theme::border());
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .w_full()
                .child(line())
                .child(
                    div()
                        .text_size(theme::caption_size())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text_secondary())
                        .child(name),
                )
                .child(line())
        };
        let body = if picker.sources.is_none() {
            div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(theme::text_tertiary())
                .child("Looking for screens and windows…")
                .into_any_element()
        } else {
            div()
                .id("share-picker-body")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .gap(px(16.))
                .px(px(20.))
                .py(px(16.))
                .child(divider("SCREENS"))
                .child(grid(screens))
                .child(divider("WINDOWS"))
                .child(if windows.is_empty() {
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .text_size(theme::secondary_size())
                        .text_color(theme::text_tertiary())
                        .child("No windows to share")
                        .into_any_element()
                } else {
                    grid(windows).into_any_element()
                })
                .into_any_element()
        };
        let resolutions = RESOLUTIONS.into_iter().enumerate().map(|(n, name)| {
            let label = if name == "native" { "Full size" } else { name };
            chip(("share-res", n), label, voice.share_resolution == name).on_click(cx.listener(
                move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.set_share_setting("voice.share_resolution", &format!("{name:?}"), cx);
                },
            ))
        });
        let rates = FRAME_RATES.into_iter().enumerate().map(|(n, fps)| {
            chip(
                ("share-fps", n),
                format!("{fps} fps"),
                voice.share_fps == fps,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.set_share_setting("voice.share_fps", &fps.to_string(), cx);
            }))
        });
        let option = |name: &'static str, choices: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(caption(name))
                .child(choices)
        };
        // The server says whether it passes a share's sound on; known once in a room.
        let no_sound = self.core.voice_channel().is_some() && !self.core.share_audio_supported();
        let sound = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                Checkbox::new("share-sound")
                    .cursor_pointer()
                    .label("Share sound")
                    .checked(voice.share_sound)
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.set_share_setting("voice.share_sound", &on.to_string(), cx);
                    })),
            )
            .when(no_sound, |d| {
                d.child(caption("This server does not pass sound on"))
            });
        let footer = div()
            .flex()
            .flex_wrap()
            .items_end()
            .gap(px(20.))
            .px(px(20.))
            .py(px(14.))
            .border_t_1()
            .border_color(theme::border())
            .child(option(
                "RESOLUTION",
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .children(resolutions)
                    .into_any_element(),
            ))
            .child(option(
                "FRAME RATE",
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .children(rates)
                    .into_any_element(),
            ))
            .child(option(
                "WHEN IT FALLS BEHIND, KEEP",
                self.render_preference_picker(cx),
            ))
            .child(sound)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        Button::new("share-cancel")
                            .cursor_pointer()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_share_picker(cx);
                            })),
                    )
                    .child(
                        Button::new("share-start")
                            .cursor_pointer()
                            .primary()
                            .label("Share")
                            .disabled(picker.sources.is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.share_picked(cx);
                            })),
                    ),
            );
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(20.))
            .py(px(12.))
            .border_b_1()
            .border_color(theme::border())
            .child(icon(assets::MONITOR_SHARE, 18., theme::accent()))
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(theme::body_size())
                    .child("Share your screen"),
            )
            .child(
                crate::channel::tip_button("share-close", assets::CLOSE, "Close").on_click(
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_share_picker(cx);
                    }),
                ),
            );
        let closing = crate::modal::closing(ID);
        let way = if closing { "out" } else { "in" };
        let shown = move |t: f32| if closing { 1. - t } else { t };
        let scrim = theme::scrim();
        let width = (self.viewport.width.as_f32() / theme::scale() - 2. * MARGIN).min(MAX_W);
        let card = div()
            .id("share-picker-card")
            .occlude()
            .relative()
            .flex()
            .flex_col()
            .w(px(width.max(320.)))
            .h_full()
            .rounded(theme::radius_large())
            .bg(theme::sidebar())
            .border_1()
            .border_color(theme::border())
            .shadow_lg()
            .overflow_hidden()
            .font_family(theme::font())
            .text_color(theme::text())
            .text_size(theme::secondary_size())
            // Clicks inside never reach the backdrop, which closes it.
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(header)
            .child(body)
            .child(footer)
            .with_animation(
                SharedString::from(format!("{ID}-card-{way}")),
                Animation::new(crate::modal::pop()).with_easing(ease_out_quint()),
                move |d, t| {
                    let t = shown(t);
                    d.opacity(t).top(px(8. * (1. - t)))
                },
            );
        Some(
            gpui_kit::deferred(
                div()
                    .id(ID)
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .p(px(MARGIN))
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| this.close_share_picker(cx)))
                    .child(card)
                    .with_animation(
                        SharedString::from(format!("{ID}-scrim-{way}")),
                        Animation::new(crate::modal::pop()).with_easing(ease_out_quint()),
                        move |d, t| d.bg(theme::alpha(scrim, shown(t))),
                    ),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }
}

/// A window drawn in lines: a frame with a title bar, for a window without a picture.
fn window_glyph() -> gpui_kit::Div {
    let color = theme::text_tertiary();
    div()
        .flex()
        .flex_col()
        .w(px(34.))
        .h(px(26.))
        .rounded(px(3.))
        .border(px(2.))
        .border_color(color)
        .child(div().w_full().h(px(5.)).bg(color))
}
