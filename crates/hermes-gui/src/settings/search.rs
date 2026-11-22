//! Searching the settings, and the folded "More" part of a group.

use super::*;

impl Shell {
    /// Every setting of every tab (name, hint, group), in the order shown: the tabs are built
    /// with their rows noted instead of drawn.
    pub(super) fn settings_index(&self, cx: &mut Context<Self>) -> Vec<(Tab, Found)> {
        let Some(page) = self.settings.as_ref() else {
            return Vec::new();
        };
        let admin = self.is_instance_admin();
        let mut out = Vec::new();
        for (tab, _) in all_tabs() {
            if (tab == Tab::Admin && !admin) || tab == Tab::Diagnostics {
                continue;
            }
            RECORDING.with(|r| *r.borrow_mut() = Some(Recording::default()));
            let _ = self.render_settings_tab(page, tab, cx);
            let found = RECORDING
                .with(|r| r.borrow_mut().take())
                .unwrap_or_default();
            out.extend(found.found.into_iter().map(|f| (tab, f)));
        }
        out
    }

    /// The settings matching what is typed in the search: every word somewhere in the tab,
    /// group, name or hint; those with all words in the name first.
    pub(super) fn settings_found(&self, cx: &mut Context<Self>) -> Vec<(Tab, Found)> {
        let Some(page) = self.settings.as_ref() else {
            return Vec::new();
        };
        let query = page.search.read(cx).value().trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let words: Vec<&str> = query.split_whitespace().collect();
        let mut found: Vec<(bool, Tab, Found)> = self
            .settings_index(cx)
            .into_iter()
            .filter_map(|(tab, f)| {
                let name = f.name.to_lowercase();
                let all = format!(
                    "{} {} {name} {}",
                    tab_name(tab).to_lowercase(),
                    f.group.to_lowercase(),
                    f.hint.to_lowercase()
                );
                words
                    .iter()
                    .all(|w| all.contains(w))
                    .then(|| (!words.iter().all(|w| name.contains(w)), tab, f))
            })
            .collect();
        found.sort_by_key(|(weaker, ..)| *weaker);
        found.into_iter().map(|(_, tab, f)| (tab, f)).collect()
    }

    pub(super) fn search_results(&self, query: &str, cx: &mut Context<Self>) -> AnyElement {
        let found = self.settings_found(cx);
        let count = match found.len() {
            0 => format!("Nothing found for \u{201c}{query}\u{201d}."),
            1 => "1 setting".to_owned(),
            n => format!("{n} settings"),
        };
        let rows = found
            .into_iter()
            .enumerate()
            .map(|(n, (tab, f))| {
                let place = format!("{} \u{203a} {}", tab_name(tab), f.group);
                let (name, hint) = (f.name.clone(), f.hint.clone());
                div()
                    .id(("settings-found", n))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(theme::row_radius())
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::hover()))
                    .child(div().text_size(theme::body_size()).child(name))
                    .child(
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::accent())
                            .child(place),
                    )
                    .when(!hint.is_empty(), |d| d.child(muted(hint)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_found(tab, f.clone(), window, cx);
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let list = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(2.))
            .w_full()
            .children(rows)
            .into_any_element();
        page_body("Search", Some(&count), vec![list])
    }

    /// Goes to a setting found by the search: its tab opens on its group (its "More"
    /// part opened), and the row
    /// lights up for a moment.
    pub(super) fn open_found(
        &mut self,
        tab: Tab,
        found: Found,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(page) = &self.settings {
            page.search
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        if let Some(key) = found.more
            && !self.more_open(key)
        {
            self.set_more_open(key, true, cx);
        }
        self.settings_open_tab(tab, cx);
        let lit = found.lit.unwrap_or(found.name);
        LIT.with(|l| *l.borrow_mut() = Some((lit, std::time::Instant::now())));
        SCROLL_TO.with(|s| s.set(Some(found.index)));
        cx.notify();
    }

    /// Whether a group's "More" part is open. It holds what is rarely changed, so it is closed
    /// unless opened. Opened ones are kept in `general.folded_settings` (which held folded
    /// groups before #64) as `more:<key>`.
    pub(super) fn more_open(&self, key: &str) -> bool {
        let open = format!("more:{key}");
        self.config.general.folded_settings.contains(&open)
    }

    pub(super) fn set_more_open(&mut self, key: &str, open: bool, cx: &mut Context<Self>) {
        let entry = format!("more:{key}");
        let mut keys = self.config.general.folded_settings.clone();
        keys.retain(|k| *k != entry);
        if open {
            keys.push(entry);
        }
        self.set_config(|c| c.general.folded_settings = keys, cx);
    }

    /// A group's rarely changed rows, folded under a line saying `label` (#64). It goes last
    /// among the group's rows. `rows` builds them; the search finds them and opens the fold.
    pub(super) fn more(
        &self,
        key: &'static str,
        label: &str,
        rows: impl FnOnce(&mut Context<Self>) -> Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.more_open(key);
        IN_MORE.with(|m| m.set(Some(key)));
        let rows = rows(cx);
        IN_MORE.with(|m| m.set(None));
        let hover = SharedString::from(format!("more-{key}-hover"));
        let color = if self.is_hovered(&hover) {
            theme::accent()
        } else {
            theme::text_secondary()
        };
        let toggle = div()
            .id(SharedString::from(format!("more-{key}")))
            .flex()
            .items_center()
            .gap(px(6.))
            .py(px(10.))
            .cursor_pointer()
            .text_size(theme::secondary_size())
            .text_color(color)
            .on_hover(self.track_hover(hover, cx))
            .child(crate::ui::chevron(
                icon(assets::CHEVRON_DOWN, 14., color),
                format!("more-chevron-{key}"),
                open,
            ))
            .child(SharedString::from(label.to_owned()))
            .when(!open, |d| {
                d.child(
                    div()
                        .text_color(theme::text_tertiary())
                        .child(format!("({})", rows.len())),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_more_open(key, !open, cx);
            }));
        let mut part = div().flex().flex_col().child(toggle);
        if open {
            let mut children: Vec<AnyElement> = Vec::with_capacity(rows.len() * 2);
            for row in rows {
                children.push(
                    div()
                        .h(px(1.))
                        .flex_shrink_0()
                        .bg(theme::border())
                        .into_any_element(),
                );
                children.push(row);
            }
            // Opening grows the rows from nothing, so what is below glides down.
            part = part.child(crate::channel::grow_in(
                &self.grow_heights,
                SharedString::from(format!("more-open-{key}")),
                div()
                    .flex()
                    .flex_col()
                    .children(children)
                    .into_any_element(),
            ));
        }
        part.into_any_element()
    }
}
