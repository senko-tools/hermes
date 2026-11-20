//! Link previews, when turned on (`[buffer] link_previews`, off by default): this device fetches
//! a linked page itself (the Hermes server never does), reads its title, description and picture
//! from the page's `<head>` (Open Graph or Twitter tags, else `<title>`) and shows them under the
//! message. The site sees this device's address, as when the link is opened; the setting says so.
//!
//! Only `http` and `https` pages on the internet are fetched: never this computer or the local
//! network (by address literal; a name that resolves there is not caught), also after a redirect.
//! At most [`MAX_PAGE`] bytes of a page and [`MAX_IMAGE`] of a picture are read.

use std::net::IpAddr;
use std::time::Duration;

use reqwest::Url;

/// What a page says about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkPreview {
    pub url: String,
    pub site: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    /// The picture's address, made absolute.
    pub image: Option<String>,
    /// The page asks for its picture large (`twitter:card` `summary_large_image`); otherwise
    /// it is a small thumbnail beside the text.
    pub large_image: bool,
}

impl LinkPreview {
    /// Nothing worth showing.
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.description.is_none() && self.image.is_none()
    }
}

/// The most of a page read (its head is near the start).
const MAX_PAGE: usize = 512 << 10;
/// The largest picture fetched.
pub const MAX_IMAGE: usize = 4 << 20;
const MAX_TITLE: usize = 200;
const MAX_DESCRIPTION: usize = 300;
const MAX_REDIRECTS: usize = 5;

/// Whether `url` may be fetched: `http(s)`, and not an address of this computer or the local
/// network.
pub fn previewable(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    // An IPv6 address comes in brackets.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return public(ip);
    }
    let name = host.trim_end_matches('.').to_ascii_lowercase();
    !(name == "localhost" || name.ends_with(".localhost") || name.ends_with(".local"))
}

fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_documentation()
                // 100.64.0.0/10, shared address space.
                || (ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64))
        }
        IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            !(ip.is_loopback()
                || ip.is_unspecified()
                // fc00::/7 unique local, fe80::/10 link local.
                || first & 0xfe00 == 0xfc00
                || first & 0xffc0 == 0xfe80
                || ip.to_ipv4_mapped().is_some_and(|v4| !public(IpAddr::V4(v4))))
        }
    }
}

/// The first web link in a message's text, without the punctuation around it.
pub fn first_link(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || c == '<' || c == '>')
        .map(|word| word.trim_start_matches(['(', '[', '"', '\'']))
        .find(|word| word.starts_with("https://") || word.starts_with("http://"))
        .map(|word| {
            word.trim_end_matches([
                '.', ',', '!', '?', ')', ']', '"', '\'', ';', ':', '*', '_', '|',
            ])
            .to_owned()
        })
        .filter(|link| Url::parse(link).is_ok_and(|url| previewable(&url)))
}

fn client(timeout: Duration) -> Result<reqwest::Client, String> {
    let config =
        crate::tls::client_config(crate::tls::Trust::Ca, true).map_err(|e| e.to_string())?;
    reqwest::Client::builder()
        .use_preconfigured_tls((*config).clone())
        .timeout(timeout)
        .user_agent(concat!(
            "hermes-client/",
            env!("CARGO_PKG_VERSION"),
            " (link preview)"
        ))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS || !previewable(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| e.to_string())
}

/// Reads at most `max` bytes of the answer.
async fn read_capped(mut res: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
        let room = max.saturating_sub(out.len());
        out.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if out.len() >= max {
            break;
        }
    }
    Ok(out)
}

/// Fetches the page at `url` and reads its preview; `None` for anything but a web page, or one
/// with nothing to show.
pub async fn fetch(url: &str) -> Result<Option<LinkPreview>, String> {
    let parsed = Url::parse(url).map_err(|e| e.to_string())?;
    if !previewable(&parsed) {
        return Ok(None);
    }
    let res = client(Duration::from_secs(8))?
        .get(parsed)
        .header("Accept", "text/html,application/xhtml+xml")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Ok(None);
    }
    let html = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("html"));
    if !html {
        return Ok(None);
    }
    let base = res.url().clone();
    let bytes = read_capped(res, MAX_PAGE).await?;
    let text = String::from_utf8_lossy(&bytes);
    let preview = parse(&text, &base, url);
    Ok((!preview.is_empty()).then_some(preview))
}

/// Fetches a preview's picture.
pub async fn fetch_image(url: &str) -> Result<Vec<u8>, String> {
    let parsed = Url::parse(url).map_err(|e| e.to_string())?;
    if !previewable(&parsed) {
        return Err("not fetched".into());
    }
    let res = client(Duration::from_secs(10))?
        .get(parsed)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("the site answered {}", res.status()));
    }
    if res.content_length().is_some_and(|n| n > MAX_IMAGE as u64) {
        return Err("the picture is too large".into());
    }
    let bytes = read_capped(res, MAX_IMAGE + 1).await?;
    if bytes.len() > MAX_IMAGE {
        return Err("the picture is too large".into());
    }
    Ok(bytes)
}

/// The preview of the page `html`, found at `base` (for relative picture addresses); `url` is
/// the link as written.
pub fn parse(html: &str, base: &Url, url: &str) -> LinkPreview {
    // Only the head: the body's tags are not about the page.
    let lower = html.to_ascii_lowercase();
    let end = lower.find("</head").unwrap_or(html.len());
    let (head, lower_head) = (&html[..end], &lower[..end]);
    let mut meta = std::collections::HashMap::<String, String>::new();
    let mut from = 0;
    while let Some(at) = lower_head[from..].find("<meta") {
        let start = from + at;
        let Some(close) = head[start..].find('>') else {
            break;
        };
        let tag = &head[start + 5..start + close];
        from = start + close;
        let attrs = attributes(tag);
        let key = attrs
            .iter()
            .find(|(k, _)| k == "property" || k == "name")
            .map(|(_, v)| v.to_ascii_lowercase());
        let content = attrs.iter().find(|(k, _)| k == "content").map(|(_, v)| v);
        if let (Some(key), Some(content)) = (key, content) {
            meta.entry(key).or_insert_with(|| content.clone());
        }
    }
    let title_tag = lower_head.find("<title").and_then(|at| {
        let open = at + head[at..].find('>')? + 1;
        let close = open + lower_head[open..].find("</title")?;
        Some(head[open..close].to_owned())
    });
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| meta.get(*k))
            .map(|v| clean(v))
            .filter(|v| !v.is_empty())
    };
    let title = pick(&["og:title", "twitter:title"])
        .or_else(|| title_tag.map(|t| clean(&t)).filter(|t| !t.is_empty()))
        .map(|t| cut(&t, MAX_TITLE));
    let description = pick(&["og:description", "twitter:description", "description"])
        .map(|d| cut(&d, MAX_DESCRIPTION));
    let image = pick(&[
        "og:image",
        "og:image:url",
        "twitter:image",
        "twitter:image:src",
    ])
    .and_then(|src| base.join(&src).ok())
    .filter(previewable)
    .map(String::from);
    let site = pick(&["og:site_name"]).or_else(|| base.host_str().map(str::to_owned));
    let large_image = pick(&["twitter:card"]).is_some_and(|c| c == "summary_large_image");
    LinkPreview {
        url: url.to_owned(),
        site,
        title,
        description,
        image,
        large_image,
    }
}

/// The attributes of a tag's inside: `name="value"`, `name='value'` or `name=value`.
fn attributes(tag: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = tag.trim_start_matches('/');
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        if rest.is_empty() {
            break;
        }
        let name_end = rest
            .find(|c: char| c == '=' || c.is_whitespace() || c == '/')
            .unwrap_or(rest.len());
        let name = rest[..name_end].to_ascii_lowercase();
        rest = rest[name_end..].trim_start();
        if !rest.starts_with('=') {
            out.push((name, String::new()));
            continue;
        }
        rest = rest[1..].trim_start();
        let value;
        if let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
            let body = &rest[1..];
            let end = body.find(quote).unwrap_or(body.len());
            value = body[..end].to_owned();
            rest = body.get(end + 1..).unwrap_or("");
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            value = rest[..end].to_owned();
            rest = &rest[end..];
        }
        if name.is_empty() {
            break;
        }
        out.push((name, value));
    }
    out
}

/// Text from a page: common entities decoded, spaces collapsed.
fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|n| n.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pages_on_the_internet_are_fetched() {
        let ok = |u: &str| previewable(&Url::parse(u).unwrap());
        assert!(ok("https://example.org/a"));
        assert!(ok("http://93.184.216.34/"));
        assert!(!ok("ftp://example.org/"));
        assert!(!ok("http://localhost:8080/"));
        assert!(!ok("http://printer.local/"));
        assert!(!ok("http://127.0.0.1/"));
        assert!(!ok("http://192.168.1.1/"));
        assert!(!ok("http://10.0.0.5/"));
        assert!(!ok("http://169.254.169.254/latest/meta-data"));
        assert!(!ok("http://100.100.1.1/"));
        assert!(!ok("http://[::1]/"));
        assert!(!ok("http://[fd00::1]/"));
        assert!(!ok("http://[::ffff:192.168.0.1]/"));
    }

    #[test]
    fn the_first_link_is_found_without_its_punctuation() {
        assert_eq!(
            first_link("see (https://example.org/x?a=1), it's good."),
            Some("https://example.org/x?a=1".into())
        );
        assert_eq!(
            first_link("<https://a.example/> and https://b.example"),
            Some("https://a.example/".into())
        );
        assert_eq!(first_link("http://localhost/ only"), None);
        assert_eq!(first_link("no links here"), None);
    }

    #[test]
    fn pages_tell_their_title_description_and_picture() {
        let html = r#"<html><head>
            <title>Fallback &amp; title</title>
            <meta property="og:title" content="Fox &quot;news&quot;">
            <META name='description' content='Small foxes &#8212; big tails'>
            <meta property=og:image content=/pics/fox.png />
            <meta property="og:site_name" content="Den">
            </head><body><meta property="og:title" content="not this"></body></html>"#;
        let base = Url::parse("https://example.org/story/1").unwrap();
        let p = parse(html, &base, "https://example.org/story/1");
        assert_eq!(p.title.as_deref(), Some("Fox \"news\""));
        assert_eq!(
            p.description.as_deref(),
            Some("Small foxes \u{2014} big tails")
        );
        assert_eq!(p.image.as_deref(), Some("https://example.org/pics/fox.png"));
        assert_eq!(p.site.as_deref(), Some("Den"));
        assert!(!p.large_image, "a thumbnail unless the page asks");
        let large = parse(
            r#"<head><meta name="twitter:card" content="summary_large_image"></head>"#,
            &base,
            "https://example.org/story/1",
        );
        assert!(large.large_image);

        let p = parse("<title>Just   a\n title</title>", &base, "x");
        assert_eq!(p.title.as_deref(), Some("Just a title"));
        assert_eq!(p.site.as_deref(), Some("example.org"));
        assert!(parse("<p>nothing</p>", &base, "x").is_empty());
    }
}
