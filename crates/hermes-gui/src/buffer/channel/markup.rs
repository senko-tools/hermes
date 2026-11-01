//! Message markup (PLAN.md section 7.5) to iced rich text. Parsing is `hermes_proto::markup`;
//! this flattens the span tree into styled pieces grouped in blocks (paragraphs, code blocks,
//! quotes).

use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::markup::{self, Span};
use hermes_client_core::store::InstanceStore;

/// What a clickable piece does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Url(String),
    Channel(Id),
    /// Reveals the message's spoilers.
    Spoiler,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Piece {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    /// Hidden until revealed.
    pub spoiler: bool,
    /// A mention (of me or anyone): drawn with a background.
    pub mention: bool,
    /// A role mention's color.
    pub color: Option<u32>,
    pub link: Option<Link>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Text(Vec<Piece>),
    Code {
        language: Option<String>,
        code: String,
    },
    Quote(Vec<Piece>),
}

#[derive(Clone, Copy, Default)]
struct Style {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    spoiler: bool,
}

/// Parses and flattens `content`. Mentions are resolved to names through the store.
pub fn blocks(content: &str, store: &InstanceStore, space: Option<Id>) -> Vec<Block> {
    let mut out = Vec::new();
    let mut current: Vec<Piece> = Vec::new();
    for span in markup::parse(content) {
        match span {
            Span::CodeBlock { language, code } => {
                flush(&mut out, &mut current);
                out.push(Block::Code { language, code });
            }
            Span::Quote(children) => {
                flush(&mut out, &mut current);
                let mut pieces = Vec::new();
                flatten(&children, Style::default(), store, space, &mut pieces);
                out.push(Block::Quote(pieces));
            }
            other => flatten(
                std::slice::from_ref(&other),
                Style::default(),
                store,
                space,
                &mut current,
            ),
        }
    }
    flush(&mut out, &mut current);
    out
}

fn flush(out: &mut Vec<Block>, current: &mut Vec<Piece>) {
    // Trailing line breaks before a block would leave an empty line.
    while current.last().is_some_and(|p| p.text == "\n") {
        current.pop();
    }
    if !current.is_empty() {
        out.push(Block::Text(std::mem::take(current)));
    }
}

fn flatten(
    spans: &[Span],
    style: Style,
    store: &InstanceStore,
    space: Option<Id>,
    out: &mut Vec<Piece>,
) {
    let piece = |text: String, style: Style| Piece {
        text,
        bold: style.bold,
        italic: style.italic,
        underline: style.underline,
        strike: style.strike,
        spoiler: style.spoiler,
        link: style.spoiler.then_some(Link::Spoiler),
        ..Piece::default()
    };
    for span in spans {
        match span {
            Span::Text(text) => out.push(piece(text.clone(), style)),
            Span::LineBreak => out.push(piece("\n".into(), style)),
            Span::Bold(c) => flatten(
                c,
                Style {
                    bold: true,
                    ..style
                },
                store,
                space,
                out,
            ),
            Span::Italic(c) => flatten(
                c,
                Style {
                    italic: true,
                    ..style
                },
                store,
                space,
                out,
            ),
            Span::Underline(c) => flatten(
                c,
                Style {
                    underline: true,
                    ..style
                },
                store,
                space,
                out,
            ),
            Span::Strike(c) => flatten(
                c,
                Style {
                    strike: true,
                    ..style
                },
                store,
                space,
                out,
            ),
            Span::Spoiler(c) => flatten(
                c,
                Style {
                    spoiler: true,
                    ..style
                },
                store,
                space,
                out,
            ),
            Span::Code(code) => out.push(Piece {
                code: true,
                ..piece(code.clone(), style)
            }),
            // Blocks inside inline content (not produced by the parser) are shown as text.
            Span::CodeBlock { code, .. } => out.push(Piece {
                code: true,
                ..piece(code.clone(), style)
            }),
            Span::Quote(c) => flatten(c, style, store, space, out),
            Span::Link { url, children } => {
                let start = out.len();
                flatten(
                    children,
                    Style {
                        underline: true,
                        ..style
                    },
                    store,
                    space,
                    out,
                );
                for p in &mut out[start..] {
                    if !p.spoiler {
                        p.link = Some(Link::Url(url.clone()));
                    }
                }
            }
            Span::UserMention(id) => out.push(Piece {
                mention: true,
                ..piece(format!("@{}", store.display_name(space, *id)), style)
            }),
            Span::RoleMention(id) => {
                let role = space
                    .and_then(|s| store.spaces.get(&s))
                    .and_then(|s| s.roles.iter().find(|r| r.id == *id));
                out.push(Piece {
                    mention: true,
                    color: role.and_then(|r| r.color),
                    ..piece(
                        format!("@{}", role.map_or("deleted-role", |r| r.name.as_str())),
                        style,
                    )
                });
            }
            Span::ChannelMention(id) => {
                let name = store
                    .channel(*id)
                    .and_then(|c| c.name.clone())
                    .unwrap_or_else(|| "unknown".into());
                out.push(Piece {
                    mention: true,
                    link: (!style.spoiler).then_some(Link::Channel(*id)),
                    ..piece(format!("#{name}"), style)
                });
            }
            Span::Everyone => out.push(Piece {
                mention: true,
                ..piece("@everyone".into(), style)
            }),
            Span::Here => out.push(Piece {
                mention: true,
                ..piece("@here".into(), style)
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    fn texts(block: &Block) -> Vec<String> {
        match block {
            Block::Text(p) | Block::Quote(p) => p.iter().map(|p| p.text.clone()).collect(),
            Block::Code { code, .. } => vec![code.clone()],
        }
    }

    #[test]
    fn styles_mentions_links_and_blocks() {
        let store = fixtures::store();
        let blocks = blocks(
            "hi **<@3>** see <#13> and [docs](https://example.com) ||secret||\n```rust\nfn x() {}\n```\n> quoted *text*",
            &store,
            Some(Id(10)),
        );
        assert_eq!(blocks.len(), 3, "{blocks:#?}");
        let Block::Text(pieces) = &blocks[0] else {
            panic!("{blocks:?}")
        };
        let bob = pieces.iter().find(|p| p.text == "@Bobby").unwrap();
        assert!(bob.bold && bob.mention);
        let chess = pieces.iter().find(|p| p.text == "#chess").unwrap();
        assert_eq!(chess.link, Some(Link::Channel(Id(13))));
        let docs = pieces.iter().find(|p| p.text == "docs").unwrap();
        assert_eq!(docs.link, Some(Link::Url("https://example.com".into())));
        assert!(docs.underline);
        let secret = pieces.iter().find(|p| p.text == "secret").unwrap();
        assert!(secret.spoiler);
        assert_eq!(secret.link, Some(Link::Spoiler));
        assert_eq!(
            blocks[1],
            Block::Code {
                language: Some("rust".into()),
                code: "fn x() {}".into()
            }
        );
        let Block::Quote(quote) = &blocks[2] else {
            panic!()
        };
        assert!(quote.iter().any(|p| p.text == "text" && p.italic));
        assert_eq!(texts(&blocks[2]).concat(), "quoted text");
    }

    #[test]
    fn unknown_mentions_still_render() {
        let store = fixtures::store();
        let blocks = blocks("<@999> <#999> <@&999> @everyone", &store, Some(Id(10)));
        let all = texts(&blocks[0]).concat();
        assert!(all.contains("#unknown"), "{all}");
        assert!(all.contains("@deleted-role"), "{all}");
        assert!(all.contains("@everyone"), "{all}");
    }
}
