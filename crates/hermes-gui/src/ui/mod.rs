//! Controls used all over the app, built once so they look and move the same everywhere.
//!
//! Rule: a control that changes colour under the pointer notes its hover with
//! [`Shell::track_hover`] and springs to it. gpui's own text-colour `.hover` keeps a stale
//! "hovered" when the control's hover style comes and goes (a chip while it is chosen), so the
//! text stayed the hover colour once it was unchosen. A plain `.hover`
//! that is always attached is fine; one inside `.when(..)` is not, and a test keeps it out.

use gpui_kit::prelude::*;
use gpui_kit::{AnimationExt as _, Context, SharedString, div};

use crate::assets::icon;
use crate::shell::Shell;
use crate::theme::{self, px};

/// The spring every hover and choice follows.
fn spring(to: f32) -> gpui_kit::SpringAnimation<f32> {
    gpui_kit::SpringAnimation::new(theme::paced(crate::hover::HOVER_SPRING)).to(to)
}

/// Where a control rests: 0 at rest, 1 under the pointer, 2 chosen. One spring between them
/// moves its colours together, so choosing one under the pointer flows on from its hover.
fn state(hovered: bool, chosen: bool) -> f32 {
    if chosen {
        2.
    } else if hovered {
        1.
    } else {
        0.
    }
}

/// A choice among a few, highlighted when chosen. The caller adds what a click does.
pub fn chip(
    shell: &Shell,
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    chosen: bool,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let id = id.into();
    let hover = SharedString::from(format!("{id:?}-hover"));
    let to = state(shell.is_hovered(&hover), chosen);
    let face = div()
        .id("face")
        .px(px(10.))
        .py(px(5.))
        .rounded(theme::radius_small())
        .truncate()
        .child(label.into())
        .with_spring(
            SharedString::from(format!("{id:?}-chosen")),
            spring(to),
            |d, t| {
                let t = t.clamp(0., 2.);
                // Rest to hover: the text turns accent. Hover to chosen: the accent fills in.
                let hover = t.min(1.);
                let fill = (t - 1.).max(0.);
                let text = theme::mix(theme::text_secondary(), theme::accent(), hover);
                d.bg(theme::mix(theme::selected(), theme::accent(), fill))
                    .text_color(theme::mix(text, theme::on_accent(), fill))
            },
        );
    div()
        .id(id)
        .rounded(theme::radius_small())
        .cursor_pointer()
        .max_w_full()
        .overflow_hidden()
        .text_size(theme::secondary_size())
        .on_hover(shell.track_hover(hover, cx))
        .child(face)
}

/// The track a setting's [`segment`]s sit in, side by side: one inset strip, the chosen one
/// filled with the accent. It wraps onto more lines when the row is too narrow.
pub fn segments() -> gpui_kit::Div {
    div()
        .flex()
        .flex_wrap()
        .justify_end()
        .max_w_full()
        .gap(px(2.))
        .p(px(3.))
        .rounded(theme::radius_small())
        .bg(theme::background())
}

/// What a [`segment`] shows before its label, drawn in the segment's text colour of the
/// moment (it turns `on_accent` when chosen).
pub type Preview = std::rc::Rc<dyn Fn(gpui_kit::Rgba) -> gpui_kit::AnyElement>;

/// An icon as a [`segment`]'s picture.
pub fn icon_preview(glyph: &'static str) -> Preview {
    std::rc::Rc::new(move |color| icon(glyph, 14., color).into_any_element())
}

/// One choice of a [`segments`] track: transparent at rest, its text turns accent under the
/// pointer and the accent fills in when chosen. The caller adds what a click does.
pub fn segment(
    shell: &Shell,
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    chosen: bool,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    segment_with(shell, id, None, label, chosen, cx)
}

/// [`segment`] with a small picture before the label (an icon, a shape, a layout).
pub fn segment_with(
    shell: &Shell,
    id: impl Into<gpui_kit::ElementId>,
    preview: Option<Preview>,
    label: impl Into<SharedString>,
    chosen: bool,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let id = id.into();
    let hover = SharedString::from(format!("{id:?}-hover"));
    let to = state(shell.is_hovered(&hover), chosen);
    let label: SharedString = label.into();
    let face = div()
        .id("face")
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(4.))
        .rounded(theme::radius_small())
        .with_spring(
            SharedString::from(format!("{id:?}-chosen")),
            spring(to),
            move |d, t| {
                let t = t.clamp(0., 2.);
                let hover = t.min(1.);
                let fill = (t - 1.).max(0.);
                let text = theme::mix(theme::text_secondary(), theme::accent(), hover);
                let text = theme::mix(text, theme::on_accent(), fill);
                d.bg(theme::alpha(theme::accent(), fill))
                    .text_color(text)
                    .children(preview.as_ref().map(|p| p(text)))
                    .child(div().truncate().child(label.clone()))
            },
        );
    div()
        .id(id)
        .rounded(theme::radius_small())
        .cursor_pointer()
        .max_w_full()
        .overflow_hidden()
        .text_size(theme::secondary_size())
        .on_hover(shell.track_hover(hover, cx))
        .child(face)
}

/// An entry of a page's navigation column (settings tabs, server settings tabs). The caller
/// adds what a click does.
pub fn nav_item(
    shell: &Shell,
    glyph: &str,
    name: &str,
    selected: bool,
    id: String,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let hover = SharedString::from(format!("{id}-hover"));
    let hovered = shell.is_hovered(&hover);
    // The chosen tab's background flows out of the last one and into the new one.
    let pill = div()
        .absolute()
        .inset_0()
        .rounded(theme::row_radius())
        .with_spring(
            SharedString::from(format!("{id}-pill")),
            spring(if selected { 1. } else { 0. }),
            |d, t| {
                let t = t.clamp(0., 1.);
                // It grows from the left edge as it fills in.
                d.bg(theme::alpha(theme::selected(), t))
                    .right(gpui_kit::relative(0.3 * (1. - t)))
            },
        );
    // Under the pointer the text and icon turn accent; the chosen one keeps its own colours.
    let glyph = glyph.to_owned();
    let name = SharedString::from(name.to_owned());
    let label = div().flex().items_center().gap(px(8.)).with_spring(
        SharedString::from(format!("{id}-text")),
        spring(state(hovered, selected)),
        move |d, t| {
            let t = t.clamp(0., 2.);
            let hover = t.min(1.);
            let chosen = (t - 1.).max(0.);
            let text = theme::mix(theme::text_secondary(), theme::accent(), hover);
            let glyph_color = theme::mix(theme::text_tertiary(), theme::accent(), hover);
            d.text_color(theme::mix(text, theme::text(), chosen))
                .child(icon(
                    &glyph,
                    18.,
                    theme::mix(glyph_color, theme::text_tertiary(), chosen),
                ))
                .child(name.clone())
        },
    );
    div()
        .id(SharedString::from(id))
        .relative()
        .child(pill)
        .flex()
        .flex_shrink_0()
        .items_center()
        .h(px(34.))
        .px(px(8.))
        .rounded(theme::row_radius())
        .cursor_pointer()
        .text_size(theme::body_size())
        .on_hover(shell.track_hover(hover, cx))
        .child(label)
}

/// A row of a list (DMs, requests, server settings' roles, channels and members): its background
/// fades in under the pointer and fills in when selected, on one spring. The caller adds the
/// contents and what a click does; it may change the corner radius and text size.
pub fn list_row(
    shell: &Shell,
    id: impl Into<SharedString>,
    selected: bool,
    height: f32,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let id = id.into();
    let hover = SharedString::from(format!("{id}-hover"));
    let to = state(shell.is_hovered(&hover), selected);
    // Under the contents; the row's own corners clip it.
    let fill = div().absolute().inset_0().with_spring(
        SharedString::from(format!("{id}-fill")),
        spring(to),
        |d, t| {
            let t = t.clamp(0., 2.);
            let hover = t.min(1.);
            let chosen = (t - 1.).max(0.);
            d.bg(theme::mix(
                theme::alpha(theme::hover(), hover),
                theme::selected(),
                chosen,
            ))
        },
    );
    div()
        .id(id)
        .relative()
        .overflow_hidden()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(8.))
        .h(px(height))
        .px(px(8.))
        .rounded(theme::row_radius())
        .text_size(theme::body_size())
        .cursor_pointer()
        .text_color(if selected {
            theme::text()
        } else {
            theme::text_secondary()
        })
        .on_hover(shell.track_hover(hover, cx))
        .child(fill)
}

/// An error's text, selectable like any text, with a copy button that shows a check mark for
/// a moment once used. The caller sets the colour and size around it.
pub fn error_text(
    shell: &Shell,
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    cx: &Context<Shell>,
) -> gpui_kit::Div {
    let id: SharedString = id.into();
    let text: SharedString = text.into();
    let button = format!("{id}-copy");
    let copied = shell.copied.as_deref() == Some(button.as_str());
    let glyph = if copied {
        crate::assets::CHECK
    } else {
        crate::assets::COPY
    };
    let tip = if copied { "Copied" } else { "Copy" };
    let copy_text = text.to_string();
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .min_w_0()
        .child(
            div().flex_1().min_w_0().child(
                gpui_kit::component::text::TextView::markdown(
                    SharedString::from(format!("{id}-text")),
                    text,
                )
                .selectable(true),
            ),
        )
        .child(
            tip_button(shell, SharedString::from(button.clone()), glyph, tip, cx).on_click(
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.copy_quietly(button.clone(), copy_text.clone(), cx);
                }),
            ),
        )
}

/// A fold's chevron (`glyph`, a down-pointing icon the caller styled): it points down while
/// `open` and turns to point right as it folds, on a spring. Server categories and settings
/// groups share it.
pub fn chevron(glyph: gpui_kit::Svg, id: String, open: bool) -> impl IntoElement + use<> {
    glyph.with_spring(
        SharedString::from(id),
        gpui_kit::SpringAnimation::new(theme::paced(gpui_kit::SpringConfig::new(420., 32., 1.)))
            .to(!open),
        |d, phase| {
            d.with_transformation(gpui_kit::Transformation::rotate(gpui_kit::radians(
                -std::f32::consts::FRAC_PI_2 * phase.0,
            )))
        },
    )
}

/// A small square button holding an icon. Under the pointer a soft background fades in behind
/// the icon and the icon turns accent. The caller adds what a click does.
pub fn icon_button(
    shell: &Shell,
    id: impl Into<gpui_kit::ElementId>,
    glyph: &'static str,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let id = id.into();
    let hover = SharedString::from(format!("{id:?}-hover"));
    let to = if shell.is_hovered(&hover) { 1. } else { 0. };
    // Its own layer, so a background the caller sets (a scrim over video) stays.
    let face = div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::radius_small())
        .with_spring(
            SharedString::from(format!("{id:?}-face")),
            spring(to),
            move |d, t| {
                let t = t.clamp(0., 1.);
                d.bg(theme::alpha(theme::hover(), t)).child(icon(
                    glyph,
                    18.,
                    theme::mix(theme::text_secondary(), theme::accent(), t),
                ))
            },
        );
    div()
        .id(id)
        .relative()
        .flex_shrink_0()
        .size(px(28.))
        .rounded(theme::radius_small())
        .cursor_pointer()
        .on_hover(shell.track_hover(hover, cx))
        .child(face)
}

/// [`icon_button`] that says what it does when hovered.
pub fn tip_button(
    shell: &Shell,
    id: impl Into<gpui_kit::ElementId>,
    glyph: &'static str,
    tip: &'static str,
    cx: &Context<Shell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    icon_button(shell, id, glyph, cx).tooltip(move |window, cx| {
        gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
    })
}

#[cfg(test)]
mod tests {
    /// gpui's `.hover` keeps a stale state for a text colour whose hover style comes and goes
    /// (see the module docs): no text-colour `.hover` may sit inside a `.when(..)`.
    #[test]
    fn no_text_colour_hovers_that_come_and_go() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let lines: Vec<&str> = text.lines().collect();
                    let indent = |l: &str| l.len() - l.trim_start().len();
                    for (n, line) in lines.iter().enumerate() {
                        if !(line.contains(".hover(") && line.contains("text_color")) {
                            continue;
                        }
                        // On the same line as a `.when(`, or under one opened just above it.
                        let inside_when = line.contains(".when(")
                            || lines[n.saturating_sub(4)..n].iter().any(|above| {
                                above.contains(".when(")
                                    && above.trim_end().ends_with('{')
                                    && indent(above) < indent(line)
                            });
                        if inside_when {
                            found.push(format!("{}:{}", path.display(), n + 1));
                        }
                    }
                }
            }
        }
        assert!(
            found.is_empty(),
            "use Shell::track_hover instead: {found:#?}"
        );
    }
}
