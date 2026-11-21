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
                    wrap().children(
                        [(2, "Small"), (4, "Normal"), (8, "Large")]
                            .into_iter()
                            .enumerate()
                            .map(|(n, (extra, name))| {
                                crate::ui::chip(
                                    self,
                                    ("heading-extra", n),
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
            crate::ui::chip(
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
            crate::ui::chip(
                self,
                ("sidebar-position", n),
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
            crate::ui::chip(
                self,
                ("avatar-shape", n),
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
                        .child(wrap().children(presets))
                        .child(self.slider(page, SliderKind::UiScale)),
                ),
                setting("Sidebar position", "", wrap().children(position_chips)),
                setting(
                    "Avatar shape",
                    "How people's pictures are cut, everywhere.",
                    wrap().children(shape_chips),
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
