//! Colours in code blocks: Sublime Text grammars (through `syntect`) say what each piece of the
//! code is, and the app's theme says which colour that gets, so code follows the theme.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;

use gpui_kit::{HighlightStyle, Rgba};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

use crate::theme;

/// Longer code is shown plain: colouring it would cost more than it gives.
const MAX_CHARS: usize = 20_000;
/// Coloured blocks remembered; emptied when full.
const CACHE_SIZE: usize = 512;

/// What a piece of code is, as far as its colour goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Comment,
    String,
    Number,
    Keyword,
    Function,
    Type,
}

impl Kind {
    fn color(self) -> Rgba {
        match self {
            Kind::Comment => theme::text_tertiary(),
            Kind::String => theme::success(),
            Kind::Number => theme::warning(),
            Kind::Keyword => theme::accent(),
            Kind::Function => theme::link(),
            Kind::Type => theme::accent_highlight(),
        }
    }

    /// The kind of the innermost scope that has one (`keyword.control.rust` is a keyword).
    fn of(stack: &ScopeStack) -> Option<Kind> {
        stack.as_slice().iter().rev().find_map(|scope| {
            let name = scope.build_string();
            let starts = |p: &str| name.starts_with(p);
            if starts("comment") {
                Some(Kind::Comment)
            } else if starts("string") || starts("constant.character") {
                Some(Kind::String)
            } else if starts("constant.numeric") || starts("constant.language") {
                Some(Kind::Number)
            } else if starts("keyword") || starts("storage") {
                Some(Kind::Keyword)
            } else if starts("entity.name.function") || starts("support.function") {
                Some(Kind::Function)
            } else if starts("entity.name") || starts("support.type") || starts("support.class") {
                Some(Kind::Type)
            } else {
                None
            }
        })
    }
}

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// The grammar for a code block's language (`rust`, `rs`, `py`, `json`, …).
fn syntax(language: &str) -> Option<&'static SyntaxReference> {
    let set = syntaxes();
    let language = language.trim().to_lowercase();
    let alias = match language.as_str() {
        "js" | "javascript" | "ts" | "typescript" | "jsx" | "tsx" => "js",
        "py" | "python" => "py",
        "sh" | "bash" | "shell" | "zsh" => "sh",
        "c++" | "cpp" | "cc" => "cpp",
        "cs" | "csharp" | "c#" => "cs",
        "yml" | "yaml" => "yaml",
        "md" | "markdown" => "md",
        other => other,
    };
    set.find_syntax_by_token(alias)
        .or_else(|| set.find_syntax_by_extension(alias))
}

/// The coloured ranges (byte offsets) of `code` in `language`; empty if the language is
/// unknown, or the code too long.
pub fn kinds(code: &str, language: &str) -> Vec<(Range<usize>, Kind)> {
    if code.len() > MAX_CHARS {
        return Vec::new();
    }
    let Some(syntax) = syntax(language) else {
        return Vec::new();
    };
    let set = syntaxes();
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out: Vec<(Range<usize>, Kind)> = Vec::new();
    let mut line_start = 0;
    for line in code.split_inclusive('\n') {
        let Ok(ops) = state.parse_line(line, set) else {
            return out;
        };
        let mut at = 0;
        for (offset, op) in ops {
            if offset > at
                && let Some(kind) = Kind::of(&stack)
            {
                push(&mut out, line_start + at..line_start + offset, kind);
            }
            at = offset;
            if stack.apply(&op).is_err() {
                return out;
            }
        }
        if line.len() > at
            && let Some(kind) = Kind::of(&stack)
        {
            push(&mut out, line_start + at..line_start + line.len(), kind);
        }
        line_start += line.len();
    }
    out
}

/// Adds a range, joined to the one before when they touch and are the same kind.
fn push(out: &mut Vec<(Range<usize>, Kind)>, range: Range<usize>, kind: Kind) {
    if let Some((last, last_kind)) = out.last_mut()
        && *last_kind == kind
        && last.end == range.start
    {
        last.end = range.end;
        return;
    }
    out.push((range, kind));
}

/// What each coloured range of a code block is.
type Kinds = Vec<(Range<usize>, Kind)>;

/// Coloured ranges of code blocks, by their language and text.
#[derive(Default)]
pub struct Cache(HashMap<(String, String), Kinds>);

impl Cache {
    /// The highlights for `code`, in the theme's colours now.
    pub fn highlights(
        &mut self,
        code: &str,
        language: Option<&str>,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let Some(language) = language.filter(|l| !l.trim().is_empty()) else {
            return Vec::new();
        };
        if self.0.len() >= CACHE_SIZE {
            self.0.clear();
        }
        self.0
            .entry((language.to_owned(), code.to_owned()))
            .or_insert_with(|| kinds(code, language))
            .iter()
            .map(|(range, kind)| {
                (
                    range.clone(),
                    HighlightStyle {
                        color: Some(kind.color().into()),
                        ..HighlightStyle::default()
                    },
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(code: &str, kinds: &[(Range<usize>, Kind)], kind: Kind) -> Vec<String> {
        kinds
            .iter()
            .filter(|(_, k)| *k == kind)
            .map(|(r, _)| code[r.clone()].trim().to_owned())
            .collect()
    }

    #[test]
    fn rust_gets_its_parts() {
        let code = "// hi\nfn main() {\n    let x = \"text\";\n    let n = 42;\n}\n";
        let kinds = kinds(code, "rust");
        assert_eq!(text_of(code, &kinds, Kind::Comment), ["// hi"]);
        assert!(text_of(code, &kinds, Kind::String).contains(&"\"text\"".to_owned()));
        assert!(text_of(code, &kinds, Kind::Number).contains(&"42".to_owned()));
        assert!(
            text_of(code, &kinds, Kind::Keyword)
                .iter()
                .any(|k| k == "fn")
        );
        assert!(text_of(code, &kinds, Kind::Function).contains(&"main".to_owned()));
        // Ranges never overlap and stay inside the text.
        for pair in kinds.windows(2) {
            assert!(pair[0].0.end <= pair[1].0.start);
        }
        assert!(kinds.last().unwrap().0.end <= code.len());
    }

    #[test]
    fn aliases_and_unknown_languages() {
        assert!(!kinds("x = 1  # one", "py").is_empty());
        assert!(!kinds("{\"a\": 1}", "json").is_empty());
        assert!(kinds("anything", "klingon").is_empty());
        assert!(kinds(&"a".repeat(MAX_CHARS + 1), "rust").is_empty());
    }
}
