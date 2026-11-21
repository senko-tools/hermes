//! Drawing a message's text: blocks, pieces and links.

use super::*;

impl Shell {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_block(
        &self,
        block: Block,
        inst: &InstanceId,
        channel: Id,
        message: Id,
        index: usize,
        system: bool,
        revealed: bool,
        edited: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let edited_mark = || {
            div()
                .text_size(theme::caption_size())
                .text_color(theme::text_tertiary())
                .child(EDITED)
        };
        match block {
            Block::Text(pieces) => {
                let mut pieces = pieces;
                if edited {
                    pieces.push(Piece {
                        text: format!(" {EDITED}"),
                        muted: true,
                        ..Piece::default()
                    });
                }
                self.render_pieces(pieces, inst, channel, message, index, system, revealed, cx)
            }
            Block::Code { code, language } => {
                let part = Part {
                    message,
                    index: selection::BLOCKS + index as u16,
                };
                let colors = self
                    .syntax_cache
                    .borrow_mut()
                    .highlights(&code, language.as_deref());
                let (text, record) = self.selectable(inst, channel, part, code, colors);
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .relative()
                            .p(px(8.))
                            .rounded(theme::radius_small())
                            .bg(theme::code_background())
                            .border_1()
                            .border_color(theme::border())
                            .font_family(theme::mono_font())
                            .text_size(theme::secondary_size())
                            .whitespace_normal()
                            .child(text)
                            .child(record),
                    )
                    .when(edited, |d| d.child(edited_mark()))
                    .into_any_element()
            }
            Block::Quote(pieces) => div()
                .flex()
                .gap(px(8.))
                .child(
                    div()
                        .flex_shrink_0()
                        .w(px(3.))
                        .rounded(px(1.))
                        .bg(theme::text_tertiary()),
                )
                .child(div().flex_1().min_w_0().child(
                    self.render_pieces(pieces, inst, channel, message, index, system, revealed, cx),
                ))
                .into_any_element(),
        }
    }

    /// The message being written as it will look: blocks drawn without selection or links.
    pub(super) fn composer_preview(&self, key: &ChannelKey, text: &str) -> AnyElement {
        let blocks = {
            let store = self.core.store();
            match store.instance(&key.0) {
                Some(s) => {
                    let space = s.space_of(key.1);
                    markup::blocks(text, s, space)
                }
                None => markup::plain(text),
            }
        };
        let styled = |pieces: Vec<Piece>| {
            let mut text = String::new();
            let mut highlights = Vec::new();
            for piece in pieces {
                let start = text.len();
                text.push_str(&piece.text);
                if start < text.len() {
                    highlights.push((start..text.len(), piece_style(&piece, false, true)));
                }
            }
            gpui_kit::StyledText::new(text).with_highlights(highlights)
        };
        let rows = blocks.into_iter().map(|block| match block {
            Block::Text(pieces) => div().child(styled(pieces)).into_any_element(),
            Block::Code { code, language } => {
                let colors = self
                    .syntax_cache
                    .borrow_mut()
                    .highlights(&code, language.as_deref());
                div()
                    .p(px(8.))
                    .rounded(theme::radius_small())
                    .bg(theme::code_background())
                    .border_1()
                    .border_color(theme::border())
                    .font_family(theme::mono_font())
                    .text_size(theme::secondary_size())
                    .child(gpui_kit::StyledText::new(code).with_highlights(colors))
                    .into_any_element()
            }
            Block::Quote(pieces) => div()
                .flex()
                .gap(px(8.))
                .child(
                    div()
                        .flex_shrink_0()
                        .w(px(3.))
                        .rounded(px(1.))
                        .bg(theme::text_tertiary()),
                )
                .child(div().flex_1().min_w_0().child(styled(pieces)))
                .into_any_element(),
        });
        div()
            .id("composer-preview-text")
            .flex()
            .flex_col()
            .gap(px(4.))
            .max_h(px(220.))
            .overflow_y_scroll()
            .mb(px(6.))
            .px(px(10.))
            .py(px(8.))
            .rounded(theme::radius_small())
            .bg(theme::sidebar())
            .border_1()
            .border_color(theme::border())
            .text_size(theme::body_size())
            .children(rows)
            .into_any_element()
    }

    /// Styled, clickable text. Links and spoilers act on release, as everything does.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_pieces(
        &self,
        pieces: Vec<Piece>,
        inst: &InstanceId,
        channel: Id,
        message: Id,
        index: usize,
        system: bool,
        revealed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut text = String::new();
        let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
        let mut links: Vec<(Range<usize>, Link)> = Vec::new();
        for piece in pieces {
            let start = text.len();
            text.push_str(&piece.text);
            let range = start..text.len();
            if range.is_empty() {
                continue;
            }
            highlights.push((range.clone(), piece_style(&piece, system, revealed)));
            let hidden = piece.spoiler && !revealed;
            match piece.link {
                Some(Link::Spoiler) if !hidden => {}
                Some(link) => links.push((range, link)),
                None => {}
            }
        }
        if text.is_empty() {
            return div().into_any_element();
        }
        let part = Part {
            message,
            index: selection::BLOCKS + index as u16,
        };
        let (styled, record) = self.selectable(inst, channel, part, text, highlights);
        let body = div()
            .relative()
            .text_size(theme::body_size())
            .when(system, |d| d.text_color(theme::text_secondary()));
        if links.is_empty() {
            return body.child(styled).child(record).into_any_element();
        }
        let ranges: Vec<Range<usize>> = links.iter().map(|(range, _)| range.clone()).collect();
        let shell = cx.entity().downgrade();
        let inst = inst.clone();
        let element_id = SharedString::from(format!("text-{message}-{index}"));
        body.child(InteractiveText::new(element_id, styled).on_click(
            ranges,
            move |clicked, window, cx| {
                cx.stop_propagation();
                let Some((_, link)) = links.get(clicked) else {
                    return;
                };
                let link = link.clone();
                let inst = inst.clone();
                let _ = shell.update(cx, |this, cx| {
                    if !this.selection.dragged {
                        this.follow_link(inst, channel, message, link, window, cx);
                    }
                });
            },
        ))
        .child(record)
        .into_any_element()
    }

    pub(super) fn follow_link(
        &mut self,
        inst: InstanceId,
        channel: Id,
        message: Id,
        link: Link,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match link {
            Link::Url(url) => {
                if let Err(err) = crate::open::url(&url) {
                    self.error(format!("could not open the link: {err}"), cx);
                }
            }
            Link::Channel(target) => self.select(inst, target, window, cx),
            Link::User(user) => {
                let space = self
                    .core
                    .store()
                    .instance(&inst)
                    .and_then(|s| s.space_of(channel));
                self.run_action(
                    Target::Member(inst, space, user),
                    crate::menu_entries::Action::Profile,
                    window,
                    cx,
                );
            }
            Link::Spoiler | Link::Reveal => {
                if let Some(view) = self.chats.get_mut(&(inst, channel)) {
                    view.revealed.insert(message);
                }
                cx.notify();
            }
        }
    }
}
