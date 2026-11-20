//! Searching the settings, and folding groups.

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
            .max_w(px(760.))
            .children(rows)
            .into_any_element();
        page_body("Search", Some(&count), vec![list])
    }

    /// Goes to a setting found by the search: its tab opens on its group (unfolded), and the row
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
        if let Some(key) = found.fold
            && self.config.general.folded_settings.iter().any(|k| k == key)
        {
            self.set_folded(key, false, cx);
        }
        self.settings_open_tab(tab, cx);
        LIT.with(|l| *l.borrow_mut() = Some((found.name, std::time::Instant::now())));
        SCROLL_TO.with(|s| s.set(Some(found.index)));
        cx.notify();
    }

    pub(super) fn set_folded(&mut self, key: &str, folded: bool, cx: &mut Context<Self>) {
        let mut keys = self.config.general.folded_settings.clone();
        keys.retain(|k| k != key);
        if folded {
            keys.push(key.to_owned());
        }
        self.set_config(|c| c.general.folded_settings = keys, cx);
    }

    /// A group of settings that folds into its title (remembered under `key`): for long groups
    /// most people never change.
    pub(super) fn fold_group(
        &self,
        key: &'static str,
        title: &str,
        rows: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let folded = self.config.general.folded_settings.iter().any(|k| k == key);
        let count = rows.len();
        let toggle = div()
            .id(SharedString::from(format!("fold-{key}")))
            .flex()
            .items_center()
            .gap(px(4.))
            .cursor_pointer()
            .text_size(theme::caption_size())
            .text_color(theme::text_tertiary())
            .hover(|d| d.text_color(theme::accent()))
            .child(if folded {
                format!("Show {count}")
            } else {
                "Hide".to_owned()
            })
            .child(crate::ui::chevron(
                icon(assets::CHEVRON_DOWN, 14., theme::text_tertiary()),
                format!("fold-chevron-{key}"),
                !folded,
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_folded(key, !folded, cx);
            }))
            .into_any_element();
        let note_fold = || {
            RECORDING.with(|r| {
                if let Some(r) = r.borrow_mut().as_mut() {
                    r.fold_last(key);
                }
            });
        };
        if folded && !recording() {
            return group_with(title, toggle, Vec::new());
        }
        let header = group_header(title, toggle);
        // Opening grows the card from nothing, so the groups below glide down.
        let card = crate::channel::grow_in(
            &self.grow_heights,
            SharedString::from(format!("fold-open-{key}")),
            group_card(rows).into_any_element(),
        );
        let shown = group_column(header, Some(card));
        note_fold();
        shown
    }
}
