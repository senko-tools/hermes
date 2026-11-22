//! Appearance.

use super::*;
use crate::ui::{ButtonKind, DropWidth, Segment};

impl Shell {
    pub(super) fn slider(&self, page: &SettingsPage, kind: SliderKind) -> AnyElement {
        page.sliders.iter().find(|(k, _)| *k == kind).map_or_else(
            || div().into_any_element(),
            |(_, state)| {
                div()
                    .w_full()
                    .child(Slider::new(state).cursor_pointer())
                    .into_any_element()
            },
        )
    }

    pub(super) fn tab_appearance(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let config = &self.config;
        let cards = self
            .theme_previews
            .iter()
            .enumerate()
            .map(|(n, (name, loaded))| {
                let chosen = *name == config.general.theme;
                let value = name.clone();
                theme_card(n, name, loaded, chosen).on_click(cx.listener(move |this, _, _, cx| {
                    this.set_theme(value.clone(), cx);
                }))
            });
        let theme_group = group_with(
            "Theme",
            div()
                .flex()
                .gap(px(8.))
                .child(crate::ui::button(
                    self,
                    "themes-folder",
                    "Open themes folder",
                    ButtonKind::Secondary,
                    true,
                    |this, _, cx| {
                        if let Some(paths) = this.core.paths() {
                            let dir = paths.dir.join(theme::THEMES_DIR);
                            let _ = std::fs::create_dir_all(&dir);
                            if let Err(err) = crate::open::path(&dir) {
                                this.error(format!("could not open {}: {err}", dir.display()), cx);
                            }
                        }
                    },
                    cx,
                ))
                .child(crate::ui::button(
                    self,
                    "themes-copy",
                    "Copy current theme",
                    ButtonKind::Secondary,
                    true,
                    |this, _, cx| match this.copy_theme(cx) {
                        Ok(name) => this.set_theme(name, cx),
                        Err(err) => this.error(format!("theme not copied: {err}"), cx),
                    },
                    cx,
                ))
                .into_any_element(),
            vec![
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .children(cards)
                    .into_any_element(),
                muted("Your own themes are .toml files in the themes folder."),
            ],
        );
        let shown_px = i32::from(config.font.size) + 3;
        let text_group = group(
            "Text",
            vec![
                setting_fit(
                    "Font",
                    "Any font installed on this computer.",
                    crate::ui::dropdown(&page.font, DropWidth::Medium),
                ),
                setting_fit(
                    "Font size",
                    &format!("{shown_px} px"),
                    sized(SLIDER_W, self.slider(page, SliderKind::FontSize)),
                ),
                self.more(
                    "appearance-text",
                    "More text options",
                    |cx| {
                        vec![
                            setting_fit(
                                "Code font",
                                "For code blocks.",
                                crate::ui::dropdown(&page.mono_font, DropWidth::Medium),
                            ),
                            setting_fit(
                                "Size of titles",
                                "",
                                crate::ui::segmented(
                                    self,
                                    "heading-extra",
                                    TITLE_SIZES
                                        .iter()
                                        .map(|(_, name, size)| {
                                            Segment::with(title_preview(*size), *name)
                                        })
                                        .collect(),
                                    TITLE_SIZES
                                        .iter()
                                        .position(|(e, _, _)| *e == config.font.heading_extra),
                                    true,
                                    |this, n, _, cx| {
                                        let extra = TITLE_SIZES[n].0;
                                        this.set_config(|c| c.font.heading_extra = extra, cx);
                                    },
                                    cx,
                                ),
                            ),
                        ]
                    },
                    cx,
                ),
            ],
        );
        let scale = self.ui_scale_draft.unwrap_or(config.font.ui_scale);
        let presets = crate::ui::segmented(
            self,
            "scale-preset",
            SCALE_PRESETS
                .iter()
                .map(|value| Segment::new(format!("{:.0}%", value * 100.)))
                .collect(),
            SCALE_PRESETS.iter().position(|value| {
                (config.font.ui_scale - value).abs() < 0.001 && self.ui_scale_draft.is_none()
            }),
            true,
            |this, n, window, cx| this.set_ui_scale(SCALE_PRESETS[n], window, cx),
            cx,
        );
        let position = crate::ui::segmented(
            self,
            "sidebar-position",
            SIDEBAR_POSITIONS
                .iter()
                .map(|(value, name)| {
                    Segment::with(
                        layout_preview(*value == config::SidebarPosition::Left),
                        *name,
                    )
                })
                .collect(),
            SIDEBAR_POSITIONS
                .iter()
                .position(|(v, _)| *v == config.sidebar.position),
            true,
            |this, n, _, cx| {
                let value = SIDEBAR_POSITIONS[n].0;
                this.set_config(|c| c.sidebar.position = value, cx);
            },
            cx,
        );
        let shape = crate::ui::segmented(
            self,
            "avatar-shape",
            AVATAR_SHAPES
                .iter()
                .map(|(value, name)| Segment::with(shape_preview(*value), *name))
                .collect(),
            AVATAR_SHAPES
                .iter()
                .position(|(v, _)| *v == config.general.avatar_shape),
            true,
            |this, n, _, cx| {
                let value = AVATAR_SHAPES[n].0;
                this.set_config(|c| c.general.avatar_shape = value, cx);
            },
            cx,
        );
        let layout_group = group(
            "Layout",
            vec![
                setting_below(
                    "UI scale",
                    &if self.ui_scale_draft.is_some() {
                        format!("{:.0} %, applied when you let go", scale * 100.)
                    } else {
                        format!("{:.0} % · Ctrl+= and Ctrl+- zoom", scale * 100.)
                    },
                    div()
                        .flex()
                        .flex_col()
                        .w_full()
                        .gap(px(8.))
                        .items_start()
                        .child(presets)
                        .child(self.slider(page, SliderKind::UiScale)),
                ),
                setting_fit("Sidebar position", "", position),
                setting_fit(
                    "Avatar shape",
                    "How people's pictures are cut, everywhere.",
                    shape,
                ),
                self.more(
                    "appearance-layout",
                    "More layout options",
                    |cx| {
                        vec![setting_fit(
                            "Panel widths",
                            &format!(
                                "Sidebar {} px, member list {} px",
                                config.sidebar.width, config.sidebar.member_list_width
                            ),
                            crate::ui::button(
                                self,
                                "reset-widths",
                                "Reset",
                                ButtonKind::Secondary,
                                true,
                                |this, _, cx| {
                                    this.set_config(
                                        |c| {
                                            c.sidebar.width = 240;
                                            c.sidebar.member_list_width = 232;
                                        },
                                        cx,
                                    );
                                },
                                cx,
                            ),
                        )]
                    },
                    cx,
                ),
            ],
        );
        page_body(
            "Appearance",
            None,
            vec![
                theme_group,
                text_group,
                layout_group,
                self.motion_group(page, cx),
            ],
        )
    }
}

/// The size of titles: extra px over the text, name, and the size of its "Aa".
const TITLE_SIZES: [(i32, &str, f32); 3] =
    [(2, "Small", 11.), (4, "Normal", 13.), (8, "Large", 16.)];
const SIDEBAR_POSITIONS: [(config::SidebarPosition, &str); 2] = [
    (config::SidebarPosition::Left, "Left"),
    (config::SidebarPosition::Right, "Right"),
];
const AVATAR_SHAPES: [(config::AvatarShape, &str); 2] = [
    (config::AvatarShape::Circle, "Circle"),
    (config::AvatarShape::Squircle, "Squircle"),
];
/// A slider on the right of its row.
const SLIDER_W: f32 = 220.;

/// "Aa" at `size`, for the size of titles.
fn title_preview(size: f32) -> crate::ui::Preview {
    std::rc::Rc::new(move |color| {
        div()
            .text_size(px(size))
            .font_weight(FontWeight::MEDIUM)
            .text_color(color)
            .child("Aa")
            .into_any_element()
    })
}

/// A tiny window with its sidebar on the left or the right.
fn layout_preview(left: bool) -> crate::ui::Preview {
    std::rc::Rc::new(move |color| {
        let bar = div().w(px(5.)).h_full().bg(color);
        div()
            .flex()
            .when(!left, |d| d.justify_end())
            .w(px(18.))
            .h(px(12.))
            .rounded(px(2.))
            .border_1()
            .border_color(color)
            .overflow_hidden()
            .child(bar)
            .into_any_element()
    })
}

/// A filled picture in `shape`.
fn shape_preview(shape: config::AvatarShape) -> crate::ui::Preview {
    const SIZE: f32 = 12.;
    let radius = match shape {
        config::AvatarShape::Circle => SIZE / 2.,
        config::AvatarShape::Squircle => SIZE * 0.3,
    };
    std::rc::Rc::new(move |color| {
        div()
            .size(px(SIZE))
            .rounded(px(radius))
            .bg(color)
            .into_any_element()
    })
}
