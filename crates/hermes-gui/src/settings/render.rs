//! The settings page: navigation and the open tab.

use super::*;

impl Shell {
    /// The page: navigation on the left, the tab on the right.
    pub fn render_settings(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let page = self.settings.as_ref()?;
        hint::set_delay(self.config.general.hint_scroll_delay_ms);
        let admin = self.is_instance_admin();
        let item = |(tab, glyph, name): &NavTab, cx: &mut Context<Self>| {
            let tab = *tab;
            crate::ui::nav_item(
                self,
                glyph,
                name,
                page.tab == tab,
                format!("settings-tab-{name}"),
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.settings_open_tab(tab, cx)))
            .into_any_element()
        };
        let mut nav: Vec<AnyElement> = vec![
            div()
                .flex_shrink_0()
                .mb(px(4.))
                .child(
                    Input::new(&page.search)
                        .prefix(icon(assets::SEARCH, 16., theme::text_tertiary()))
                        .cleanable(true),
                )
                .into_any_element(),
        ];
        for (title, tabs) in SECTIONS {
            let tabs: Vec<_> = tabs
                .iter()
                .filter(|(tab, ..)| *tab != Tab::Admin || admin)
                .collect();
            if tabs.is_empty() {
                continue;
            }
            nav.push(nav_section(title));
            nav.extend(tabs.into_iter().map(|t| item(t, cx)));
        }
        // About stays at the bottom, under a line.
        nav.push(div().flex_1().min_h(px(12.)).into_any_element());
        nav.push(
            div()
                .flex_shrink_0()
                .h(px(1.))
                .mx(px(8.))
                .mb(px(6.))
                .bg(theme::border())
                .into_any_element(),
        );
        nav.push(item(&ABOUT, cx));
        let query = page.search.read(cx).value().trim().to_owned();
        let content = if !query.is_empty() {
            self.search_results(&query, cx)
        } else {
            self.render_settings_tab(page, page.tab, cx)
        };
        let key = if query.is_empty() {
            format!("settings-{:?}", page.tab)
        } else {
            "settings-search".to_owned()
        };
        Some(frame(
            "User settings".into(),
            nav,
            theme::enter(SharedString::from(key), content),
            cx.listener(|this, _, _, cx| this.close_settings(cx)),
        ))
    }

    pub(super) fn render_settings_tab(
        &self,
        page: &SettingsPage,
        tab: Tab,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match tab {
            Tab::Account => self.tab_account(page, cx),
            Tab::Privacy => self.tab_privacy(cx),
            Tab::Appearance => self.tab_appearance(page, cx),
            Tab::Chat => self.tab_chat(page, cx),
            Tab::Notifications => self.tab_notifications(cx),
            Tab::Window => self.tab_window(cx),
            Tab::Keybinds => self.tab_keybinds(page, cx),
            Tab::Voice => self.tab_voice(page, cx),
            Tab::ScreenShare => self.tab_screen_share(page, cx),
            Tab::Diagnostics => self.tab_diagnostics(cx),
            Tab::Admin => self.tab_admin(page, cx),
            Tab::About => self.tab_about(cx),
        }
    }
}
