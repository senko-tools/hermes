//! "Flipbook" SVGs: pictures whose parts are shown in turn by CSS `@keyframes` that only switch
//! `visibility`, `display` or `opacity` (what tools turning a GIF into an SVG write). resvg draws
//! no CSS animation, so each moment is drawn on its own: a `<style>` added after the file's own
//! pins every animated part to its value then.
//!
//! Anything else that moves (interpolated transforms, SMIL, parts with different durations) is
//! drawn still, as at its start, rather than in the state before any animation applies (which
//! for a flipbook hides every frame).

use std::collections::HashMap;

/// Most moments drawn.
const MAX_FRAMES: usize = 120;

/// The properties a flipbook switches.
const SWITCHED: [&str; 3] = ["visibility", "display", "opacity"];

/// A keyframe: where in the run (0 to 1) and the switched properties it sets.
type Keyframe = (f32, Vec<(String, String)>);

/// What the SVG's CSS animation looks like.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// Nothing animated.
    Still,
    /// The SVG with a pinning `<style>` per moment, and how long each moment shows in ms.
    Frames(Vec<(String, u32)>),
}

/// Reads the CSS animation of `svg`.
pub fn plan(svg: &str) -> Plan {
    let css: String = styles(svg).collect::<Vec<_>>().join("\n");
    if !css.contains("@keyframes") {
        return Plan::Still;
    }
    let (keyframes, rules) = parse_css(&css);
    // Each animated part: the selector pinning it, its keyframes and duration.
    let mut parts: Vec<(String, &Vec<Keyframe>, f32)> = Vec::new();
    let mut supported = true;
    for classes in class_lists(svg) {
        let (mut name, mut duration) = (None, None);
        for (class, decls) in &rules {
            if !classes.iter().any(|c| c == class) {
                continue;
            }
            for (prop, value) in decls {
                match prop.as_str() {
                    "animation" => {
                        for token in value.split_whitespace() {
                            if let Some(seconds) = seconds(token) {
                                duration = Some(seconds);
                            } else if keyframes.contains_key(token) {
                                name = Some(token.to_owned());
                            }
                        }
                    }
                    "animation-name" => name = Some(value.trim().to_owned()),
                    "animation-duration" => duration = seconds(value.trim()),
                    _ => {}
                }
            }
        }
        let Some(name) = name else { continue };
        let Some(frames) = keyframes.get(&name) else {
            continue;
        };
        let Some(frames) = frames.as_ref() else {
            supported = false;
            continue;
        };
        let selector: String = classes.iter().map(|c| format!(".{c}")).collect();
        parts.push((selector, frames, duration.unwrap_or(0.)));
    }
    if parts.is_empty() {
        return Plan::Still;
    }
    let duration = parts[0].2;
    supported &= duration > 0. && parts.iter().all(|p| (p.2 - duration).abs() < 1e-3);
    // Every moment a part switches, from 0 up to the end of the run.
    let mut times: Vec<f32> = vec![0.];
    if supported {
        for (_, frames, _) in &parts {
            times.extend(frames.iter().map(|f| f.0).filter(|&t| t > 0. && t < 1.));
        }
    }
    times.sort_by(f32::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
    times.truncate(MAX_FRAMES);
    let at_style = |t: f32| -> String {
        let mut out = String::from("<style>");
        for (selector, frames, _) in &parts {
            // Step-end: the last keyframe at or before `t` holds.
            let mut values: Vec<(String, String)> = Vec::new();
            for (at, decls) in frames.iter() {
                if *at <= t + 1e-4 {
                    for (prop, value) in decls {
                        values.retain(|(p, _)| p != prop);
                        values.push((prop.clone(), value.clone()));
                    }
                }
            }
            if values.is_empty() {
                continue;
            }
            out.push_str(selector);
            out.push('{');
            for (prop, value) in values {
                out.push_str(&format!("{prop}:{value};"));
            }
            out.push('}');
        }
        out.push_str("</style>");
        out
    };
    let insert_at = svg.find("</style>").map_or_else(
        || svg.find('>').map_or(0, |i| i + 1),
        |i| i + "</style>".len(),
    );
    let mut out = Vec::new();
    for (i, &t) in times.iter().enumerate() {
        let end = times.get(i + 1).copied().unwrap_or(1.);
        let ms = ((end - t) * duration * 1000.).round().max(1.) as u32;
        let mut text = String::with_capacity(svg.len() + 256);
        text.push_str(&svg[..insert_at]);
        text.push_str(&at_style(t));
        text.push_str(&svg[insert_at..]);
        out.push((text, ms));
    }
    Plan::Frames(out)
}

/// The contents of each `<style>` element.
fn styles(svg: &str) -> impl Iterator<Item = &str> {
    let mut rest = svg;
    std::iter::from_fn(move || {
        let start = rest.find("<style")?;
        let open_end = start + rest[start..].find('>')? + 1;
        let close = open_end + rest[open_end..].find("</style>")?;
        let body = &rest[open_end..close];
        rest = &rest[close..];
        Some(
            body.trim()
                .trim_start_matches("<![CDATA[")
                .trim_end_matches("]]>"),
        )
    })
}

/// Every distinct `class` attribute's classes.
fn class_lists(svg: &str) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    for piece in svg.split("class=").skip(1) {
        let Some(quote) = piece.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            continue;
        };
        let Some(end) = piece[1..].find(quote) else {
            continue;
        };
        let classes: Vec<String> = piece[1..1 + end]
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        if !classes.is_empty() && !out.contains(&classes) {
            out.push(classes);
        }
    }
    out
}

/// `0.5s` or `500ms` in seconds.
fn seconds(token: &str) -> Option<f32> {
    if let Some(ms) = token.strip_suffix("ms") {
        return ms.parse::<f32>().ok().map(|ms| ms / 1000.);
    }
    token.strip_suffix('s')?.parse().ok()
}

/// `prop:value;…` as pairs.
fn declarations(body: &str) -> Vec<(String, String)> {
    body.split(';')
        .filter_map(|d| {
            let (prop, value) = d.split_once(':')?;
            let value = value.trim().trim_end_matches("!important").trim();
            Some((prop.trim().to_ascii_lowercase(), value.to_owned()))
        })
        .filter(|(p, _)| !p.is_empty())
        .collect()
}

/// The `@keyframes` (`None` for ones that change more than switched properties) and the rules on
/// single classes, in order.
#[allow(clippy::type_complexity)]
fn parse_css(
    css: &str,
) -> (
    HashMap<String, Option<Vec<Keyframe>>>,
    Vec<(String, Vec<(String, String)>)>,
) {
    let mut keyframes = HashMap::new();
    let mut rules = Vec::new();
    let mut rest = css;
    while let Some(open) = rest.find('{') {
        let head = rest[..open].trim();
        let Some(close) = matching_brace(rest, open) else {
            break;
        };
        let body = &rest[open + 1..close];
        rest = &rest[close + 1..];
        if let Some(name) = head.strip_prefix("@keyframes") {
            keyframes.insert(name.trim().to_owned(), keyframes_of(body));
        } else if !head.starts_with('@') {
            let decls = declarations(body);
            for selector in head.split(',') {
                let selector = selector.trim();
                if let Some(class) = selector.strip_prefix('.')
                    && class
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                {
                    rules.push((class.to_owned(), decls.clone()));
                }
            }
        }
    }
    (keyframes, rules)
}

/// The steps of one `@keyframes` body; `None` if one sets anything but a switched property.
fn keyframes_of(body: &str) -> Option<Vec<Keyframe>> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find('{') {
        let head = rest[..open].trim().to_owned();
        let close = open + rest[open..].find('}')?;
        let decls = declarations(&rest[open + 1..close]);
        rest = &rest[close + 1..];
        if decls.iter().any(|(p, _)| !SWITCHED.contains(&p.as_str())) {
            return None;
        }
        for at in head.split(',') {
            let at = match at.trim() {
                "from" => 0.,
                "to" => 1.,
                pct => pct.strip_suffix('%')?.trim().parse::<f32>().ok()? / 100.,
            };
            out.push((at, decls.clone()));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    Some(out)
}

/// Where the brace opened at `open` closes.
fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLIPBOOK: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"><style>.f{visibility:hidden;animation:0.5s step-end infinite}@keyframes k0{0%{visibility:visible}50%{visibility:hidden}}.f0{animation-name:k0}@keyframes k1{0%{visibility:hidden}50%{visibility:visible}100%{visibility:hidden}}.f1{animation-name:k1}</style><rect class="f f0" width="2" height="2" fill="red"/><rect class="f f1" x="2" width="2" height="2" fill="blue"/></svg>"#;

    #[test]
    fn a_flipbook_gives_a_moment_per_switch() {
        let Plan::Frames(frames) = plan(FLIPBOOK) else {
            panic!("animated");
        };
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].1, 250);
        assert_eq!(frames[1].1, 250);
        assert!(frames[0].0.contains(".f.f0{visibility:visible;}"));
        assert!(frames[0].0.contains(".f.f1{visibility:hidden;}"));
        assert!(frames[1].0.contains(".f.f0{visibility:hidden;}"));
        assert!(frames[1].0.contains(".f.f1{visibility:visible;}"));
    }

    #[test]
    fn other_animation_is_drawn_as_it_starts() {
        let svg = FLIPBOOK.replace(
            "@keyframes k1{0%{visibility:hidden}",
            "@keyframes k1{0%{visibility:hidden;transform:scale(2)}",
        );
        let Plan::Frames(frames) = plan(&svg) else {
            panic!("drawn at its start");
        };
        assert_eq!(frames.len(), 1);
        assert!(frames[0].0.contains(".f.f0{visibility:visible;}"));
    }

    #[test]
    fn a_still_svg_is_still() {
        assert_eq!(
            plan(r#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>"#),
            Plan::Still
        );
    }
}
