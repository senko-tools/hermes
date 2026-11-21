//! Appearance.

use super::*;

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
                .child(
                    Button::new("themes-folder")
                        .cursor_pointer()
                        .small()
                        .label("Open themes folder")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(paths) = this.core.paths() {
                                let dir = paths.dir.join(theme::THEMES_DIR);
                                let _ = std::fs::create_dir_all(&dir);
                                if let Err(err) = crate::open::path(&dir) {
                                    this.error(
                                        format!("could not open {}: {err}", dir.display()),
                                        cx,
                                    );
                                }
                            }
                        })),
                )
                .child(
                    Button::new("themes-copy")
                        .cursor_pointer()
                        .small()
                        .label("Copy current theme")
                        .on_click(cx.listener(|this, _, _, cx| match this.copy_theme(cx) {
                            Ok(name) => this.set_theme(name, cx),
                            Err(err) => this.error(format!("theme not copied: {err}"), cx),
                        })),
                )
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
                setting(
                    "Font",
                    "Any font installed on this computer.",
                    Select::new(&page.font).cursor_pointer(),
                ),
                setting(
                    "Code font",
                    "For code blocks.",
                    Select::new(&page.mono_font).cursor_pointer(),
                ),
                setting(
                    "Size of titles",
                    "",
                    crate::ui::segments().children(
                        [(2, "Small", 11.), (4, "Normal", 13.), (8, "Large", 16.)]
                            .into_iter()
                            .enumerate()
                            .map(|(n, (extra, name, size))| {
                                crate::ui::segment_with(
                                    self,
                                    ("heading-extra", n),
                                    Some(title_preview(size)),
                                    name,
                                    config.font.heading_extra == extra,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.set_config(|c| c.font.heading_extra = extra, cx);
                                    },
                                ))
                            }),
                    ),
                ),
                setting(
                    "Font size",
                    &format!("{shown_px} px"),
                    self.slider(page, SliderKind::FontSize),
                ),
            ],
        );
        let scale = self.ui_scale_draft.unwrap_or(config.font.ui_scale);
        let presets = SCALE_PRESETS.iter().enumerate().map(|(n, value)| {
            let value = *value;
            let chosen =
                (config.font.ui_scale - value).abs() < 0.001 && self.ui_scale_draft.is_none();
            crate::ui::segment(
                self,
                ("scale-preset", n),
                format!("{:.0}%", value * 100.),
                chosen,
                cx,
            )
            .on_click(cx.listener(move |this, _, window, cx| this.set_ui_scale(value, window, cx)))
        });
        let position_chips = [
            (config::SidebarPosition::Left, "Left"),
            (config::SidebarPosition::Right, "Right"),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (value, name))| {
            crate::ui::segment_with(
                self,
                ("sidebar-position", n),
                Some(layout_preview(value == config::SidebarPosition::Left)),
                name,
                config.sidebar.position == value,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_config(|c| c.sidebar.position = value, cx);
            }))
        });
        let shape_chips = [
            (config::AvatarShape::Circle, "Circle"),
            (config::AvatarShape::Squircle, "Squircle"),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (value, name))| {
            crate::ui::segment_with(
                self,
                ("avatar-shape", n),
                Some(shape_preview(value)),
                name,
                config.general.avatar_shape == value,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_config(|c| c.general.avatar_shape = value, cx);
            }))
        });
        let layout_group = group(
            "Layout",
            vec![
                setting(
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
                        .items_end()
                        .child(crate::ui::segments().children(presets))
                        .child(self.slider(page, SliderKind::UiScale)),
                ),
                setting(
                    "Sidebar position",
                    "",
                    crate::ui::segments().children(position_chips),
                ),
                setting(
                    "Avatar shape",
                    "How people's pictures are cut, everywhere.",
                    crate::ui::segments().children(shape_chips),
                ),
                setting(
                    "Panel widths",
                    &format!(
                        "Sidebar {} px, member list {} px",
                        config.sidebar.width, config.sidebar.member_list_width
                    ),
                    Button::new("reset-widths")
                        .cursor_pointer()
                        .label("Reset")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_config(
                                |c| {
                                    c.sidebar.width = 240;
                                    c.sidebar.member_list_width = 232;
                                },
                                cx,
                            );
                        })),
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
