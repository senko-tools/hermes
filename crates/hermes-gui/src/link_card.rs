//! Link previews under messages, when turned on (`[buffer] link_previews`): the first web link
//! of a message, fetched by this device (`hermes_client_core::link_preview`), as a card with the
//! site, the title (opening the link), a few lines of description and the page's picture.

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, SharedString, div, img};
use hermes_client_core::link_preview::{self, LinkPreview};

use crate::images::Img;
use crate::shell::Shell;
use crate::theme::{self, px};

/// A link's preview, by its address.
pub enum LinkCard {
    Loading,
    /// The page has nothing to show, or could not be fetched.
    Missing,
    Ready {
        preview: LinkPreview,
        picture: Img,
    },
}

/// Previews kept while the app runs; the oldest go first.
const MAX_CARDS: usize = 300;
/// The picture's largest side, as decoded.
const PICTURE_SIDE: u32 = 400;
/// A thumbnail's side, beside the text (#10: the picture was as large as the card).
const THUMB_SIDE: f32 = 80.;
/// A large picture's box, for pages that ask for one.
const LARGE: (f32, f32) = (360., 180.);

impl Shell {
    /// Starts fetching the preview of the first link in `text`, if previews are on and it is not
    /// known yet.
    pub fn prepare_link_card(&mut self, text: &str, cx: &mut Context<Self>) {
        if !self.config.buffer.link_previews {
            return;
        }
        let Some(link) = link_preview::first_link(text) else {
            return;
        };
        if self.link_cards.contains_key(&link) {
            return;
        }
        if self.link_cards.len() >= MAX_CARDS {
            self.link_cards.clear();
        }
        self.link_cards.insert(link.clone(), LinkCard::Loading);
        let fetch = link.clone();
        self.run(
            cx,
            async move {
                let preview = link_preview::fetch(&fetch).await?;
                let Some(preview) = preview else {
                    return Ok::<_, String>(None);
                };
                let picture = match &preview.image {
                    Some(src) => match link_preview::fetch_image(src).await {
                        Ok(bytes) => tokio::task::spawn_blocking(move || {
                            crate::images::decode(&bytes, Some(PICTURE_SIDE))
                        })
                        .await
                        .ok()
                        .and_then(Result::ok),
                        Err(_) => None,
                    },
                    None => None,
                };
                Ok(Some((preview, picture)))
            },
            move |this, result, _, cx| {
                let card = match result {
                    Ok(Some((preview, picture))) => LinkCard::Ready {
                        preview,
                        picture: picture.map_or(Img::Failed, Img::Ready),
                    },
                    Ok(None) => LinkCard::Missing,
                    Err(err) => {
                        tracing::debug!("link preview of {link}: {err}");
                        LinkCard::Missing
                    }
                };
                this.link_cards.insert(link.clone(), card);
                cx.notify();
            },
        );
    }

    /// The card of the first link in `text`, once it is fetched.
    pub fn render_link_card(&self, text: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.config.buffer.link_previews {
            return None;
        }
        let link = link_preview::first_link(text)?;
        let LinkCard::Ready { preview, picture } = self.link_cards.get(&link)? else {
            return None;
        };
        let open = preview.url.clone();
        let title = preview.title.clone().map(|title| {
            div()
                .id(SharedString::from(format!("link-title-{link}")))
                .cursor_pointer()
                .text_size(theme::body_size())
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::accent())
                .hover(|d| d.underline())
                .child(title)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Err(err) = crate::open::url(&open) {
                        this.error(format!("could not open the link: {err}"), cx);
                    }
                }))
        });
        let shown = match picture {
            Img::Ready(picture) => Some(picture.shown(self.animating())),
            _ => None,
        };
        let large = preview.large_image;
        let big_picture = shown.clone().filter(|_| large).map(|picture| {
            img(picture)
                .max_w(px(LARGE.0))
                .max_h(px(LARGE.1))
                .rounded(theme::radius_small())
                .object_fit(gpui_kit::ObjectFit::Contain)
        });
        let thumbnail = shown.filter(|_| !large).map(|picture| {
            img(picture)
                .flex_shrink_0()
                .size(px(THUMB_SIDE))
                .rounded(theme::radius_small())
                .object_fit(gpui_kit::ObjectFit::Cover)
        });
        let text = div().flex().flex_col().flex_1().min_w_0().gap(px(4.));
        Some(
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .max_w(px(440.))
                .pl(px(10.))
                .pr(px(12.))
                .py(px(8.))
                .rounded(theme::radius_small())
                .bg(theme::sidebar())
                .border_l_4()
                .border_color(theme::accent())
                .child(
                    text.children(preview.site.clone().map(|site| {
                        div()
                            .text_size(theme::caption_size())
                            .text_color(theme::text_tertiary())
                            .child(site)
                    }))
                    .children(title)
                    .children(preview.description.clone().map(|text| {
                        div()
                            .text_size(theme::secondary_size())
                            .text_color(theme::text_secondary())
                            .line_clamp(3)
                            .child(text)
                    }))
                    .children(big_picture),
                )
                .children(thumbnail)
                .into_any_element(),
        )
    }
}
