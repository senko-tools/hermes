//! In-app notices (#61): one card, one place. Every notice (the status line, the connection,
//! an incoming call, an update) is drawn by [`stack`] over the top of the content area, the
//! same gap from its top, left and right edges. It never moves the page under it; popups,
//! menus and pickers are deferred, so they stay above it.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::{AnimationExt as _, AnyElement, Rgba, SharedString, canvas, div};

use crate::assets::{self, icon};
use crate::theme::{self, px};

/// The gap between a notice and the content area's edges, and between notices.
pub const GAP: f32 = 8.;
/// At most this many show at once; the rest wait their turn.
pub const MAX_SHOWN: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
    /// Something is broken until it is acted on: the card itself is tinted.
    Critical,
}

impl Severity {
    pub fn color(self) -> Rgba {
        match self {
            Severity::Info => theme::accent(),
            Severity::Success => theme::success(),
            Severity::Warning => theme::warning(),
            Severity::Error | Severity::Critical => theme::danger(),
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Severity::Info => assets::INFO,
            Severity::Success => assets::CHECK,
            Severity::Warning | Severity::Critical => assets::ALERT,
            Severity::Error => assets::BAN,
        }
    }
}

/// One notice: what [`stack`] draws, all in the same card.
pub struct Notice {
    /// Its animations play under this key: a new key for a new notice plays them again.
    key: SharedString,
    severity: Severity,
    icon: Option<AnyElement>,
    text: AnyElement,
    buttons: Vec<AnyElement>,
    dismiss: Option<AnyElement>,
    /// A temporary notice: a thin bar runs out over this long, then it goes.
    countdown: Option<Duration>,
    /// On its way out: it plays its coming in backwards.
    leaving: bool,
}

impl Notice {
    pub fn new(key: impl Into<SharedString>, severity: Severity, text: impl IntoElement) -> Self {
        Notice {
            key: key.into(),
            severity,
            icon: None,
            text: text.into_any_element(),
            buttons: Vec::new(),
            dismiss: None,
            countdown: None,
            leaving: false,
        }
    }

    /// In place of the severity's own icon.
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    pub fn button(mut self, button: impl IntoElement) -> Self {
        self.buttons.push(button.into_any_element());
        self
    }

    /// The close button at the right end.
    pub fn dismiss(mut self, button: impl IntoElement) -> Self {
        self.dismiss = Some(button.into_any_element());
        self
    }

    pub fn countdown(mut self, length: Duration) -> Self {
        self.countdown = Some(length);
        self
    }

    pub fn leaving(mut self, leaving: bool) -> Self {
        self.leaving = leaving;
        self
    }

    fn render(self) -> AnyElement {
        let Notice {
            key,
            severity,
            icon: own_icon,
            text,
            buttons,
            dismiss,
            countdown,
            leaving,
        } = self;
        let color = severity.color();
        let critical = severity == Severity::Critical;
        let countdown = countdown.map(|length| {
            div()
                .absolute()
                .left_0()
                .bottom_0()
                .h(px(2.))
                .bg(theme::alpha(color, 0.5))
                .with_animation(
                    SharedString::from(format!("{key}-countdown")),
                    gpui_kit::Animation::new(length),
                    |d, t| d.w(gpui_kit::relative(1. - t)),
                )
        });
        div()
            .id(key.clone())
            .occlude()
            .relative()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_h(px(36.))
            .pl(px(14.))
            .pr(px(6.))
            .py(px(5.))
            .rounded(theme::radius_large())
            .overflow_hidden()
            .bg(if critical {
                theme::mix(theme::sidebar(), theme::danger(), 0.12)
            } else {
                theme::sidebar()
            })
            .border_1()
            .border_color(if critical {
                theme::alpha(theme::danger(), 0.6)
            } else {
                theme::border()
            })
            .shadow_lg()
            .cursor_default()
            .text_size(theme::secondary_size())
            .text_color(theme::text())
            // The severity's accent along the left edge.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(3.))
                    .bg(color),
            )
            .child(
                own_icon.unwrap_or_else(|| icon(severity.glyph(), 16., color).into_any_element()),
            )
            .child(div().flex_1().min_w_0().child(text))
            .children(buttons)
            .children(dismiss)
            .children(countdown)
            // It drops in from above as it fades in, and goes back the same way.
            .with_animation(
                SharedString::from(format!("{key}-{}", if leaving { "out" } else { "in" })),
                gpui_kit::Animation::new(if leaving {
                    crate::modal::pop()
                } else {
                    theme::fade() * 2
                })
                .with_easing(gpui_kit::ease_out_quint()),
                move |d, t| {
                    let shown = if leaving { 1. - t } else { t };
                    d.opacity(shown).top(px(-6. * (1. - shown)))
                },
            )
            .into_any_element()
    }
}

/// The notices over the top of the content area, the first on top. `height` learns how tall
/// they are (in unscaled pixels, the gap below them included; 0 with none), for what has to
/// stay out from under them.
pub fn stack(notices: Vec<Notice>, height: Rc<Cell<f32>>) -> Option<AnyElement> {
    if notices.is_empty() {
        height.set(0.);
        return None;
    }
    Some(
        div()
            .absolute()
            .top(px(GAP))
            .left(px(GAP))
            .right(px(GAP))
            .flex()
            .flex_col()
            .gap(px(GAP))
            .children(notices.into_iter().take(MAX_SHOWN).map(Notice::render))
            .child(
                canvas(
                    move |bounds, window, _| {
                        let tall = bounds.size.height.as_f32() / theme::scale() + GAP;
                        if (height.get() - tall).abs() > 0.5 {
                            height.set(tall);
                            window.refresh();
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .into_any_element(),
    )
}
