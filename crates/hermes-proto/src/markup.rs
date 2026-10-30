//! Message markup (PLAN.md section 7.5). Raw text is stored and sent as-is; both clients render it
//! from the [`Span`] tree produced by [`parse`], so GUI and TUI render identically.
//!
//! Supported: `**bold**`, `*italic*` / `_italic_`, `__underline__`, `~~strike~~`, `` `code` ``,
//! fenced code blocks with an optional language, `> quote` lines, `||spoiler||`, bare `http(s)://`
//! links, `<https://…>`, `[text](https://…)`, mentions `<@USER_ID>`, `<@&ROLE_ID>`, `<#CHANNEL_ID>`,
//! `@everyone`, `@here`, and backslash escapes of ASCII punctuation (`\*`).
//! Everything else (headings, lists, tables, images, HTML) stays plain text.
//!
//! The parser is hand-written (see `docs/DECISIONS.md`) and never fails: anything that does not
//! form a complete construct is kept as text.

use std::collections::HashMap;

use crate::id::Id;
use crate::models::Mentions;

/// One node of parsed markup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Span {
    Text(String),
    LineBreak,
    Bold(Vec<Span>),
    Italic(Vec<Span>),
    Underline(Vec<Span>),
    Strike(Vec<Span>),
    Spoiler(Vec<Span>),
    /// Inline code.
    Code(String),
    /// A fenced code block. Always its own block.
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    /// Consecutive `> ` lines. Always its own block; quotes do not nest.
    Quote(Vec<Span>),
    /// `url` is always `http://` or `https://`.
    Link {
        url: String,
        children: Vec<Span>,
    },
    UserMention(Id),
    RoleMention(Id),
    ChannelMention(Id),
    Everyone,
    Here,
}

/// Parses raw message text into spans.
pub fn parse(input: &str) -> Vec<Span> {
    let normalized;
    let text = if input.contains('\r') {
        normalized = input.replace("\r\n", "\n").replace('\r', "\n");
        &normalized
    } else {
        input
    };

    // Group lines into plain and quote blocks. Lines inside a code fence belong to the block in
    // which the fence was opened, so `> ` inside code is not a quote.
    let fences = fence_ranges(text);
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_quote = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let inside_fence = fences.iter().any(|&(s, e)| s < start && start < e);
        if !inside_fence {
            match (quote_body(line), in_quote) {
                (Some(body), true) => {
                    buf.push_str(body);
                    continue;
                }
                (Some(body), false) => {
                    flush_block(&mut out, &mut buf, false, true);
                    in_quote = true;
                    buf.push_str(body);
                    continue;
                }
                (None, true) => {
                    flush_block(&mut out, &mut buf, true, true);
                    in_quote = false;
                }
                (None, false) => {}
            }
        }
        buf.push_str(line);
    }
    flush_block(&mut out, &mut buf, in_quote, in_quote);
    out
}

/// Collects everything a message mentions (code is ignored). Duplicates are removed.
pub fn extract_mentions(input: &str) -> Mentions {
    fn walk(spans: &[Span], m: &mut Mentions) {
        for span in spans {
            match span {
                Span::UserMention(id) if !m.users.contains(id) => m.users.push(*id),
                Span::RoleMention(id) if !m.roles.contains(id) => m.roles.push(*id),
                Span::Everyone => m.everyone = true,
                Span::Here => m.here = true,
                Span::Bold(c)
                | Span::Italic(c)
                | Span::Underline(c)
                | Span::Strike(c)
                | Span::Spoiler(c)
                | Span::Quote(c)
                | Span::Link { children: c, .. } => walk(c, m),
                _ => {}
            }
        }
    }

    let mut mentions = Mentions::default();
    walk(&parse(input), &mut mentions);
    mentions
}

// ---------------------------------------------------------------------------------------------
// Blocks
// ---------------------------------------------------------------------------------------------

fn quote_body(line: &str) -> Option<&str> {
    if let Some(body) = line.strip_prefix("> ") {
        Some(body)
    } else if line == ">" || line == ">\n" {
        Some(&line[1..])
    } else {
        None
    }
}

/// Byte ranges `[open, close + 3)` of paired ``` fences.
fn fence_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut i = 0;
    while let Some(a) = text[i..].find("```") {
        let open = i + a;
        let Some(b) = text[open + 3..].find("```") else {
            break;
        };
        let end = open + 3 + b + 3;
        ranges.push((open, end));
        i = end;
    }
    ranges
}

/// Parses a finished plain or quote block. The newline separating it from the next block is
/// dropped (`strip_newline`), since blocks are separate lines anyway.
fn flush_block(out: &mut Vec<Span>, buf: &mut String, quote: bool, strip_newline: bool) {
    let mut text = std::mem::take(buf);
    if strip_newline && text.ends_with('\n') {
        text.pop();
    }
    if quote {
        out.push(Span::Quote(parse_fenced(&text)));
    } else if !text.is_empty() {
        out.extend(parse_fenced(&text));
    }
}

/// Splits text into code blocks and inline runs.
fn parse_fenced(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut last = 0;
    for (start, end) in fence_ranges(text) {
        let before = &text[last..start];
        out.extend(parse_inline(before.strip_suffix('\n').unwrap_or(before)));
        out.push(code_block(&text[start + 3..end - 3]));
        last = end;
        if text[last..].starts_with('\n') {
            last += 1;
        }
    }
    out.extend(parse_inline(&text[last..]));
    out
}

fn code_block(content: &str) -> Span {
    let (language, code) = match content.split_once('\n') {
        Some((first, rest)) if is_language(first) => (Some(first.to_owned()), rest),
        Some(("", rest)) => (None, rest),
        _ => (None, content),
    };
    Span::CodeBlock {
        language,
        code: code.strip_suffix('\n').unwrap_or(code).to_owned(),
    }
}

fn is_language(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-#._".contains(c))
}

// ---------------------------------------------------------------------------------------------
// Inline
// ---------------------------------------------------------------------------------------------

/// Maximum nesting of formatting delimiters; deeper markers stay text. Bounds recursion.
const MAX_DEPTH: u8 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Delim {
    Bold,
    Underline,
    Strike,
    Spoiler,
    ItalicStar,
    ItalicUnderscore,
}

impl Delim {
    /// In the order openers are tried: doubles before singles.
    const ALL: [Delim; 6] = [
        Delim::Bold,
        Delim::Underline,
        Delim::Strike,
        Delim::Spoiler,
        Delim::ItalicStar,
        Delim::ItalicUnderscore,
    ];

    const fn marker(self) -> &'static str {
        match self {
            Delim::Bold => "**",
            Delim::Underline => "__",
            Delim::Strike => "~~",
            Delim::Spoiler => "||",
            Delim::ItalicStar => "*",
            Delim::ItalicUnderscore => "_",
        }
    }

    fn wrap(self, children: Vec<Span>) -> Span {
        match self {
            Delim::Bold => Span::Bold(children),
            Delim::Underline => Span::Underline(children),
            Delim::Strike => Span::Strike(children),
            Delim::Spoiler => Span::Spoiler(children),
            Delim::ItalicStar | Delim::ItalicUnderscore => Span::Italic(children),
        }
    }
}

type ParseResult = Option<(usize, Vec<Span>)>;

fn parse_inline(text: &str) -> Vec<Span> {
    Inline::new(text, false).parse_all()
}

struct Inline<'a> {
    s: &'a str,
    /// Inside link text: no nested links.
    in_link: bool,
    /// Per delimiter (in `Delim::ALL` order), the sorted positions where it may close.
    /// An opener with no candidate after it fails without scanning.
    closers: [Vec<usize>; 6],
    /// Results of trying a delimiter at a position and depth. The outcome depends only on these,
    /// so memoizing keeps pathological input (many unclosed markers) from blowing up.
    memo: HashMap<(usize, Delim, u8), ParseResult>,
}

impl<'a> Inline<'a> {
    fn new(s: &'a str, in_link: bool) -> Self {
        let mut inline = Inline {
            s,
            in_link,
            closers: Default::default(),
            memo: HashMap::new(),
        };
        inline.closers = Delim::ALL.map(|d| {
            s.char_indices()
                .map(|(i, _)| i)
                .filter(|&i| inline.may_close(i, d))
                .collect()
        });
        inline
    }

    fn parse_all(&mut self) -> Vec<Span> {
        self.parse_until(0, None, 0)
            .map(|(_, s)| s)
            .unwrap_or_default()
    }

    fn prev_char(&self, i: usize) -> Option<char> {
        self.s[..i].chars().next_back()
    }

    /// Parses from `start` until `closer` (or the end if `None`). Returns the position after the
    /// closer and the children, or `None` if the closer never appears.
    fn parse_until(&mut self, start: usize, closer: Option<Delim>, depth: u8) -> ParseResult {
        let s = self.s;
        let mut out = Vec::new();
        let mut text = String::new();
        let mut i = start;
        while i < s.len() {
            let rest = &s[i..];
            if let Some(d) = closer
                && (!out.is_empty() || !text.is_empty())
                && self.closes_at(i, d, depth)
            {
                flush_text(&mut out, &mut text);
                return Some((i + d.marker().len(), out));
            }

            let c = rest.chars().next().unwrap_or_default();
            if c == '\\'
                && let Some(next) = rest[1..].chars().next()
                && next.is_ascii_punctuation()
            {
                text.push(next);
                i += 2;
                continue;
            }
            if c == '\n' {
                flush_text(&mut out, &mut text);
                out.push(Span::LineBreak);
                i += 1;
                continue;
            }
            if c == '`' {
                match code_span(rest) {
                    Ok((len, span)) => {
                        flush_text(&mut out, &mut text);
                        out.push(span);
                        i += len;
                    }
                    Err(run) => {
                        text.push_str(&rest[..run]);
                        i += run;
                    }
                }
                continue;
            }
            if let Some((len, span)) = self.atom(i) {
                flush_text(&mut out, &mut text);
                out.push(span);
                i += len;
                continue;
            }
            if let Some((end, span)) = self.delimited(i, depth) {
                flush_text(&mut out, &mut text);
                out.push(span);
                i = end;
                continue;
            }
            // A literal `*`/`_` run is skipped whole, so no closer matches in the middle of it.
            let len = match c {
                '*' | '_' => self.run_len(i, c as u8),
                _ => c.len_utf8(),
            };
            text.push_str(&rest[..len]);
            i += len;
        }
        if closer.is_some() {
            return None;
        }
        flush_text(&mut out, &mut text);
        Some((i, out))
    }

    /// Mentions, `@everyone`/`@here` and links.
    fn atom(&self, i: usize) -> Option<(usize, Span)> {
        let rest = &self.s[i..];
        let prev_is_word = self.prev_char(i).is_some_and(is_word_char);
        match rest.as_bytes()[0] {
            b'<' => angle(rest, self.in_link),
            b'@' if !prev_is_word => {
                for (word, span) in [("@everyone", Span::Everyone), ("@here", Span::Here)] {
                    if rest.starts_with(word) && !rest[word.len()..].starts_with(is_word_char) {
                        return Some((word.len(), span));
                    }
                }
                None
            }
            b'h' if !prev_is_word && !self.in_link => bare_url(rest).map(|url| {
                (
                    url.len(),
                    Span::Link {
                        url: url.to_owned(),
                        children: vec![Span::Text(url.to_owned())],
                    },
                )
            }),
            b'[' if !self.in_link && self.prev_char(i) != Some('!') => markdown_link(rest),
            _ => None,
        }
    }

    fn delimited(&mut self, i: usize, depth: u8) -> Option<(usize, Span)> {
        for d in Delim::ALL {
            if self.can_open(i, d)
                && let Some((end, children)) = self.try_delim(i, d, depth)
            {
                return Some((end, d.wrap(children)));
            }
        }
        None
    }

    /// Length of the run of `ch` starting at `i`.
    fn run_len(&self, i: usize, ch: u8) -> usize {
        self.s.as_bytes()[i..]
            .iter()
            .take_while(|&&b| b == ch)
            .count()
    }

    fn can_open(&self, i: usize, d: Delim) -> bool {
        if !self.s[i..].starts_with(d.marker()) || i + d.marker().len() >= self.s.len() {
            return false;
        }
        // For `*`, `**` and `_` the character after the whole marker run must not be whitespace,
        // so `2 * 3` and `** a**` stay text.
        let tight = |ch: u8| {
            let run_end = i + self.run_len(i, ch);
            self.s[run_end..]
                .chars()
                .next()
                .is_some_and(|c| !c.is_whitespace())
        };
        match d {
            Delim::Bold | Delim::ItalicStar => tight(b'*'),
            Delim::ItalicUnderscore => {
                tight(b'_') && !self.prev_char(i).is_some_and(char::is_alphanumeric)
            }
            Delim::Underline | Delim::Strike | Delim::Spoiler => true,
        }
    }

    /// Whether `d` can close at `i`, apart from the `***` case decided in `closes_at`.
    fn may_close(&self, i: usize, d: Delim) -> bool {
        let rest = &self.s[i..];
        if !rest.starts_with(d.marker()) {
            return false;
        }
        let ch = match d {
            Delim::ItalicStar => b'*',
            Delim::ItalicUnderscore => b'_',
            _ => return true,
        };
        if self.prev_char(i).is_none_or(char::is_whitespace) {
            return false;
        }
        if d == Delim::ItalicUnderscore && rest[1..].starts_with(char::is_alphanumeric) {
            return false;
        }
        // A double marker belongs to bold/underline (opening here, or closing an outer one).
        self.run_len(i, ch) != 2
    }

    fn closes_at(&mut self, i: usize, d: Delim, depth: u8) -> bool {
        if !self.may_close(i, d) {
            return false;
        }
        let (ch, double) = match d {
            Delim::ItalicStar => (b'*', Delim::Bold),
            Delim::ItalicUnderscore => (b'_', Delim::Underline),
            _ => return true,
        };
        // `***`: close the italic unless a bold/underline opens here.
        self.run_len(i, ch) == 1
            || !(self.can_open(i, double) && self.try_delim(i, double, depth).is_some())
    }

    fn try_delim(&mut self, i: usize, d: Delim, depth: u8) -> ParseResult {
        if depth >= MAX_DEPTH {
            return None;
        }
        let from = i + d.marker().len();
        let index = Delim::ALL.iter().position(|&x| x == d).unwrap_or_default();
        let closers = &self.closers[index];
        if closers.partition_point(|&c| c < from) == closers.len() {
            return None;
        }
        let key = (i, d, depth);
        if let Some(result) = self.memo.get(&key) {
            return result.clone();
        }
        let result = self.parse_until(from, Some(d), depth + 1);
        self.memo.insert(key, result.clone());
        result
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn flush_text(out: &mut Vec<Span>, text: &mut String) {
    if text.is_empty() {
        return;
    }
    if let Some(Span::Text(prev)) = out.last_mut() {
        prev.push_str(text);
        text.clear();
    } else {
        out.push(Span::Text(std::mem::take(text)));
    }
}

/// A code span starting at a backtick run. `Err(run_len)` if it has no matching closing run.
fn code_span(rest: &str) -> Result<(usize, Span), usize> {
    let run = rest.bytes().take_while(|&b| b == b'`').count();
    let body = &rest[run..];
    let mut search = 0;
    while let Some(pos) = body[search..].find('`') {
        let start = search + pos;
        let len = body[start..].bytes().take_while(|&b| b == b'`').count();
        if len == run {
            let mut code = &body[..start];
            if code.len() >= 2
                && code.starts_with(' ')
                && code.ends_with(' ')
                && !code.trim().is_empty()
            {
                code = &code[1..code.len() - 1];
            }
            if code.is_empty() {
                return Err(run);
            }
            return Ok((run + start + run, Span::Code(code.to_owned())));
        }
        search = start + len;
    }
    Err(run)
}

/// `<@id>`, `<@&id>`, `<#id>` and `<https://…>`.
fn angle(rest: &str, in_link: bool) -> Option<(usize, Span)> {
    let close = rest.find('>')?;
    let inner = &rest[1..close];
    let len = close + 1;
    let span = if let Some(id) = inner.strip_prefix("@&") {
        Span::RoleMention(parse_id(id)?)
    } else if let Some(id) = inner.strip_prefix('@') {
        Span::UserMention(parse_id(id)?)
    } else if let Some(id) = inner.strip_prefix('#') {
        Span::ChannelMention(parse_id(id)?)
    } else if !in_link && is_http_url(inner) && !inner.contains(char::is_whitespace) {
        Span::Link {
            url: inner.to_owned(),
            children: vec![Span::Text(inner.to_owned())],
        }
    } else {
        return None;
    };
    Some((len, span))
}

fn parse_id(digits: &str) -> Option<Id> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().map(Id)
}

fn is_http_url(s: &str) -> bool {
    ["http://", "https://"]
        .iter()
        .any(|scheme| s.len() > scheme.len() && s.starts_with(scheme))
}

/// A bare `http(s)://` URL, without trailing punctuation and unbalanced `)`.
fn bare_url(rest: &str) -> Option<&str> {
    if !rest.starts_with("http://") && !rest.starts_with("https://") {
        return None;
    }
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
        .unwrap_or(rest.len());
    let mut url = &rest[..end];
    loop {
        let trimmed = url.trim_end_matches(|c| ".,:;!?'*_~|".contains(c));
        let trimmed = if trimmed.ends_with(')')
            && trimmed.matches(')').count() > trimmed.matches('(').count()
        {
            &trimmed[..trimmed.len() - 1]
        } else {
            trimmed
        };
        if trimmed.len() == url.len() {
            break;
        }
        url = trimmed;
    }
    is_http_url(url).then_some(url)
}

/// `[text](https://…)`.
fn markdown_link(rest: &str) -> Option<(usize, Span)> {
    let mut depth = 0usize;
    let mut text_end = None;
    for (i, b) in rest.bytes().enumerate().skip(1) {
        match b {
            b'[' => depth += 1,
            b']' if depth == 0 => {
                text_end = Some(i);
                break;
            }
            b']' => depth -= 1,
            _ => {}
        }
    }
    let text_end = text_end?;
    let label = &rest[1..text_end];
    let after = rest[text_end + 1..].strip_prefix('(')?;

    let mut parens = 0usize;
    let mut url_end = None;
    for (i, c) in after.char_indices() {
        match c {
            c if c.is_whitespace() => return None,
            '(' => parens += 1,
            ')' if parens == 0 => {
                url_end = Some(i);
                break;
            }
            ')' => parens -= 1,
            _ => {}
        }
    }
    let url = &after[..url_end?];
    if label.trim().is_empty() || !is_http_url(url) {
        return None;
    }
    let len = text_end + 2 + url.len() + 1;
    let children = Inline::new(label, true).parse_all();
    Some((
        len,
        Span::Link {
            url: url.to_owned(),
            children,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Span::*;

    fn t(s: &str) -> Span {
        Text(s.to_owned())
    }

    fn link(url: &str) -> Span {
        Link {
            url: url.to_owned(),
            children: vec![t(url)],
        }
    }

    #[track_caller]
    fn check(input: &str, expected: Vec<Span>) {
        assert_eq!(parse(input), expected, "input: {input:?}");
    }

    #[test]
    fn plain_text() {
        check("", vec![]);
        check("hello world", vec![t("hello world")]);
        check("a\nb", vec![t("a"), LineBreak, t("b")]);
        check("a\r\nb", vec![t("a"), LineBreak, t("b")]);
    }

    #[test]
    fn basic_formatting() {
        check("**b**", vec![Bold(vec![t("b")])]);
        check("*i*", vec![Italic(vec![t("i")])]);
        check("_i_", vec![Italic(vec![t("i")])]);
        check("__u__", vec![Underline(vec![t("u")])]);
        check("~~s~~", vec![Strike(vec![t("s")])]);
        check("||x||", vec![Spoiler(vec![t("x")])]);
        check("a **b** c", vec![t("a "), Bold(vec![t("b")]), t(" c")]);
    }

    #[test]
    fn nesting() {
        check(
            "**a *b* c**",
            vec![Bold(vec![t("a "), Italic(vec![t("b")]), t(" c")])],
        );
        check(
            "*a **b** c*",
            vec![Italic(vec![t("a "), Bold(vec![t("b")]), t(" c")])],
        );
        check("***x***", vec![Bold(vec![Italic(vec![t("x")])])]);
        check("___x___", vec![Underline(vec![Italic(vec![t("x")])])]);
        check(
            "||**~~x~~**||",
            vec![Spoiler(vec![Bold(vec![Strike(vec![t("x")])])])],
        );
        check(
            "*a**b**c*",
            vec![Italic(vec![t("a"), Bold(vec![t("b")]), t("c")])],
        );
    }

    #[test]
    fn unclosed_markers_stay_text() {
        check("**a", vec![t("**a")]);
        check("a*", vec![t("a*")]);
        check("****", vec![t("****")]);
        check("~a~", vec![t("~a~")]);
        check("|a|", vec![t("|a|")]);
        check("**a *b**", vec![Bold(vec![t("a *b")])]);
    }

    #[test]
    fn emphasis_needs_tight_markers() {
        check("2 * 3 * 4", vec![t("2 * 3 * 4")]);
        check("** a**", vec![t("** a**")]);
        check("*a *", vec![t("*a *")]);
    }

    #[test]
    fn underscores_inside_words_are_text() {
        check("snake_case_name", vec![t("snake_case_name")]);
        check("a _b_ c", vec![t("a "), Italic(vec![t("b")]), t(" c")]);
        check("_a_b", vec![t("_a_b")]);
    }

    #[test]
    fn escapes() {
        check(r"\*not\*", vec![t("*not*")]);
        check(r"\\", vec![t("\\")]);
        check(r"a\b", vec![t(r"a\b")]);
        check(r"\<@1>", vec![t("<@1>")]);
    }

    #[test]
    fn inline_code() {
        check("`a*b*`", vec![Code("a*b*".into())]);
        check("x `y` z", vec![t("x "), Code("y".into()), t(" z")]);
        check("``a ` b``", vec![Code("a ` b".into())]);
        check("`` `x` ``", vec![Code("`x`".into())]);
        check("`open", vec![t("`open")]);
        check("``", vec![t("``")]);
        check("**`**`**", vec![Bold(vec![Code("**".into())])]);
    }

    #[test]
    fn code_blocks() {
        check(
            "```rust\nfn main() {}\n```",
            vec![CodeBlock {
                language: Some("rust".into()),
                code: "fn main() {}".into(),
            }],
        );
        check(
            "```\n**not bold**\n```",
            vec![CodeBlock {
                language: None,
                code: "**not bold**".into(),
            }],
        );
        check(
            "```inline code```",
            vec![CodeBlock {
                language: None,
                code: "inline code".into(),
            }],
        );
        check(
            "before\n```c++\nx\n```\nafter",
            vec![
                t("before"),
                CodeBlock {
                    language: Some("c++".into()),
                    code: "x".into(),
                },
                t("after"),
            ],
        );
        check(
            "```not a lang\nx```",
            vec![CodeBlock {
                language: None,
                code: "not a lang\nx".into(),
            }],
        );
        check("```unclosed", vec![t("```unclosed")]);
    }

    #[test]
    fn quotes() {
        check("> hi", vec![Quote(vec![t("hi")])]);
        check(
            "> a\n> **b**\nafter",
            vec![
                Quote(vec![t("a"), LineBreak, Bold(vec![t("b")])]),
                t("after"),
            ],
        );
        check("before\n> q", vec![t("before"), Quote(vec![t("q")])]);
        check(">\n> x", vec![Quote(vec![LineBreak, t("x")])]);
        // Needs a space, so emoticons stay text; quotes do not nest.
        check(">_<", vec![t(">_<")]);
        check("> > x", vec![Quote(vec![t("> x")])]);
    }

    #[test]
    fn quote_markers_inside_code_blocks_are_code() {
        check(
            "```\n> not a quote\n```",
            vec![CodeBlock {
                language: None,
                code: "> not a quote".into(),
            }],
        );
        check(
            "> ```\n> x\n```",
            vec![Quote(vec![CodeBlock {
                language: None,
                code: "> x".into(),
            }])],
        );
    }

    #[test]
    fn mentions() {
        check(
            "hi <@123> and <@&456> in <#789>",
            vec![
                t("hi "),
                UserMention(Id(123)),
                t(" and "),
                RoleMention(Id(456)),
                t(" in "),
                ChannelMention(Id(789)),
            ],
        );
        check("<@>", vec![t("<@>")]);
        check("<@12a>", vec![t("<@12a>")]);
        check(
            "<@99999999999999999999>",
            vec![t("<@99999999999999999999>")],
        );
        check("**<@1>**", vec![Bold(vec![UserMention(Id(1))])]);
    }

    #[test]
    fn everyone_and_here() {
        check("@everyone hi", vec![Everyone, t(" hi")]);
        check("hey @here!", vec![t("hey "), Here, t("!")]);
        check("@everyones", vec![t("@everyones")]);
        check("mail@here", vec![t("mail@here")]);
        check("`@everyone`", vec![Code("@everyone".into())]);
    }

    #[test]
    fn bare_links() {
        check(
            "see https://example.com/a_b.",
            vec![t("see "), link("https://example.com/a_b"), t(".")],
        );
        check(
            "(http://x.org/wiki/A_(b))",
            vec![t("("), link("http://x.org/wiki/A_(b)"), t(")")],
        );
        check("**https://x.com**", vec![Bold(vec![link("https://x.com")])]);
        check("https://", vec![t("https://")]);
        check("xhttps://a.b", vec![t("xhttps://a.b")]);
        check("ftp://a.b", vec![t("ftp://a.b")]);
        check(
            "<https://a.b/c d>",
            vec![t("<"), link("https://a.b/c"), t(" d>")],
        );
        check("<https://a.b/*x*>", vec![link("https://a.b/*x*")]);
    }

    #[test]
    fn markdown_links() {
        check(
            "[the **site**](https://a.b/(x))",
            vec![Link {
                url: "https://a.b/(x)".into(),
                children: vec![t("the "), Bold(vec![t("site")])],
            }],
        );
        check(
            "[a](javascript:alert(1))",
            vec![t("[a](javascript:alert(1))")],
        );
        check(
            "[a](https://x y)",
            vec![t("[a]("), link("https://x"), t(" y)")],
        );
        check("[](https://x)", vec![t("[]("), link("https://x"), t(")")]);
        // No links inside link text.
        check(
            "[https://a.b](https://c.d)",
            vec![Link {
                url: "https://c.d".into(),
                children: vec![t("https://a.b")],
            }],
        );
    }

    #[test]
    fn unsupported_markdown_is_text() {
        check("# Heading", vec![t("# Heading")]);
        check(
            "- item\n1. item",
            vec![t("- item"), LineBreak, t("1. item")],
        );
        check("| a | b |", vec![t("| a | b |")]);
        check("    indented", vec![t("    indented")]);
        check("<b>html</b>", vec![t("<b>html</b>")]);
        // Images are not rendered; the URL is still a normal link.
        check(
            "![img](https://a.b/c.png)",
            vec![t("![img]("), link("https://a.b/c.png"), t(")")],
        );
    }

    #[test]
    fn extract() {
        let m = extract_mentions("<@1> <@2> <@1> <@&3> @here `<@4>` > x\n> @everyone");
        assert_eq!(m.users, vec![Id(1), Id(2)]);
        assert_eq!(m.roles, vec![Id(3)]);
        assert!(m.here);
        assert!(m.everyone);

        let m = extract_mentions("a\n> @everyone ||<@5>||\n```\n<@6>\n```");
        assert!(m.everyone);
        assert_eq!(m.users, vec![Id(5)]);
        assert!(extract_mentions("plain").is_empty());
    }

    #[test]
    fn pathological_input_is_fast() {
        let inputs = [
            "**__~~||*_".repeat(400),
            "*a ".repeat(1300),
            "**a ".repeat(1000),
            "[".repeat(4000),
            "`".repeat(4000),
            "``a".repeat(1300),
            "~".repeat(4000),
            "_a_".repeat(1300) + "*",
            "> ```\n".repeat(600),
            "*".repeat(4000),
            "<@".repeat(2000),
            "*a*a ".repeat(800),
            "*a **a ~~a ||a __a _a ".repeat(180),
            "a* *a ".repeat(660),
            "[a](".repeat(1000),
            "**a* ".repeat(800),
            "||~~".repeat(1000) + "x",
        ];
        let start = std::time::Instant::now();
        for input in &inputs {
            let _ = parse(input);
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            start.elapsed()
        );
    }
}
