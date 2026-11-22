//! The update check (PLAN.md section 15.7): on start, ask the GitHub releases API whether a newer
//! version exists, and what changed on the way to it (each release's notes, written in
//! CHANGELOG.md; see `.github/release-notes.sh`). Nothing is downloaded or installed here.

use std::cmp::Ordering;
use std::time::Duration;

use serde::Deserialize;

/// Where releases are published.
pub const RELEASES_API: &str =
    "https://api.github.com/repos/senko-tools/hermes/releases?per_page=20";

/// A newer release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    /// Without the `v`.
    pub version: String,
    /// The release page.
    pub url: String,
    /// What changed: every release after this version up to and including `version`, newest
    /// first.
    pub notes: Vec<ReleaseNotes>,
}

/// One release's notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseNotes {
    /// Without the `v`.
    pub version: String,
    /// `YYYY-MM-DD`, when GitHub says.
    pub date: Option<String>,
    pub lines: Vec<NoteLine>,
}

/// A line of release notes: the little markdown CHANGELOG.md uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteLine {
    /// `#`, `##` or `###`.
    Heading(String),
    /// `- ` or `* `; `depth` 0 is the outermost list, each two spaces or tab of indent one more.
    Item {
        depth: usize,
        text: String,
    },
    Text(String),
}

/// Reads release notes: headings, `-`/`*` lists (nested by indent) and plain lines. Blank lines
/// go, and so do `**` and backticks; anything else is kept as written.
pub fn parse_notes(body: &str) -> Vec<NoteLine> {
    let clean = |text: &str| text.replace("**", "").replace('`', "").trim().to_owned();
    body.lines()
        .filter_map(|line| {
            let line = line.trim_end();
            let trimmed = line.trim_start();
            if trimmed.is_empty() {
                return None;
            }
            let heading = trimmed.trim_start_matches('#');
            if heading.len() < trimmed.len() && (heading.is_empty() || heading.starts_with(' ')) {
                return Some(NoteLine::Heading(clean(heading)));
            }
            if let Some(text) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
            {
                let indent: usize = line[..line.len() - trimmed.len()]
                    .chars()
                    .map(|c| if c == '\t' { 2 } else { 1 })
                    .sum();
                return Some(NoteLine::Item {
                    depth: indent / 2,
                    text: clean(text),
                });
            }
            Some(NoteLine::Text(clean(trimmed)))
        })
        .collect()
}

/// The lines under `## <version>` in a changelog, up to the next `## ` heading, blank lines
/// around them dropped; `None` when there are none. The same rule as `.github/release-notes.sh`,
/// which makes each release's text.
pub fn changelog_section(changelog: &str, version: &str) -> Option<String> {
    let want = format!("## {version}");
    let mut inside = false;
    let mut lines = Vec::new();
    for line in changelog.lines() {
        if line.starts_with("## ") {
            inside = line.trim_end() == want;
            continue;
        }
        if inside {
            lines.push(line.trim_end_matches('\r'));
        }
    }
    let text = lines.join("\n");
    let text = text.trim_start_matches('\n').trim_end();
    (!text.is_empty()).then(|| text.to_owned())
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
}

/// `major.minor.patch` and an optional pre-release part (`1.0.0-alpha.6`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub numbers: [u64; 3],
    pub pre: Option<String>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim().trim_start_matches('v');
        // Build metadata (`+…`) plays no part in the order.
        let text = text.split_once('+').map_or(text, |(version, _)| version);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre.to_owned())),
            None => (text, None),
        };
        let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
        let numbers = [parts.next()??, parts.next()??, parts.next()??];
        if parts.next().is_some() {
            return None;
        }
        Some(Version { numbers, pre })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    /// Semver order: numbers first; a pre-release is older than its release; pre-release parts
    /// compare numerically when both are numbers, and a number comes before a word.
    fn cmp(&self, other: &Self) -> Ordering {
        self.numbers
            .cmp(&other.numbers)
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => {
                    let (mut a, mut b) = (a.split('.'), b.split('.'));
                    loop {
                        match (a.next(), b.next()) {
                            (None, None) => return Ordering::Equal,
                            (None, Some(_)) => return Ordering::Less,
                            (Some(_), None) => return Ordering::Greater,
                            (Some(x), Some(y)) => {
                                let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                                    (Ok(x), Ok(y)) => x.cmp(&y),
                                    (Ok(_), Err(_)) => Ordering::Less,
                                    (Err(_), Ok(_)) => Ordering::Greater,
                                    (Err(_), Err(_)) => x.cmp(y),
                                };
                                if o != Ordering::Equal {
                                    return o;
                                }
                            }
                        }
                    }
                }
            })
    }
}

/// The newest release newer than `current`, from a releases API answer, with the notes of every
/// release on the way to it. Pre-releases count only when `current` is one itself.
fn newest(current: &str, releases: Vec<Release>) -> Option<Update> {
    let current = Version::parse(current)?;
    let mut newer: Vec<(Version, Release)> = releases
        .into_iter()
        .filter(|r| !r.draft && (!r.prerelease || current.pre.is_some()))
        .filter_map(|r| Some((Version::parse(&r.tag_name)?, r)))
        .filter(|(v, _)| *v > current)
        .collect();
    newer.sort_by(|(a, _), (b, _)| b.cmp(a));
    let url = newer.first()?.1.html_url.clone();
    let notes: Vec<ReleaseNotes> = newer
        .into_iter()
        .map(|(_, r)| ReleaseNotes {
            version: r.tag_name.trim_start_matches('v').to_owned(),
            date: r
                .published_at
                .as_deref()
                .and_then(|d| d.get(..10))
                .map(str::to_owned),
            lines: parse_notes(r.body.as_deref().unwrap_or_default()),
        })
        .collect();
    Some(Update {
        version: notes[0].version.clone(),
        url,
        notes,
    })
}

/// Asks GitHub (or the update test server, see `updater::TESTING`); `Ok(None)` when this is the
/// newest version.
pub async fn check(current: &str) -> Result<Option<Update>, String> {
    if let Some(result) = crate::updater::test_check(current).await {
        return result;
    }
    let config =
        crate::tls::client_config(crate::tls::Trust::Ca, true).map_err(|e| e.to_string())?;
    let http = reqwest::Client::builder()
        .use_preconfigured_tls((*config).clone())
        .dns_resolver(crate::net::Resolver)
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("hermes-client/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let res = http
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("GitHub answered {}", res.status()));
    }
    let bytes = res.bytes().await.map_err(|e| e.to_string())?;
    let releases: Vec<Release> = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok(newest(current, releases))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool) -> Release {
        Release {
            tag_name: tag.into(),
            html_url: format!("https://example.org/{tag}"),
            draft: false,
            prerelease,
            body: Some(format!("### Fixed\n- something in {tag}")),
            published_at: Some("2026-11-22T10:00:00Z".into()),
        }
    }

    #[test]
    fn notes_cover_every_skipped_release_newest_first() {
        let list = vec![
            release("v1.0.1", false),
            release("v1.0.3", false),
            release("v1.0.2", false),
            release("v1.0.0", false),
        ];
        let update = newest("1.0.0", list).unwrap();
        assert_eq!(update.version, "1.0.3");
        assert_eq!(update.url, "https://example.org/v1.0.3");
        let versions: Vec<&str> = update.notes.iter().map(|n| n.version.as_str()).collect();
        assert_eq!(versions, ["1.0.3", "1.0.2", "1.0.1"]);
        assert_eq!(update.notes[0].date.as_deref(), Some("2026-11-22"));
        assert_eq!(
            update.notes[2].lines,
            [
                NoteLine::Heading("Fixed".into()),
                NoteLine::Item {
                    depth: 0,
                    text: "something in v1.0.1".into()
                },
            ]
        );
    }

    #[test]
    fn notes_read_headings_nested_lists_and_text() {
        let body = "## New\r\n\n- **Bold** thing\n  - nested `code`\n    * deeper\n\t- tab\nplain line\n#hashtag";
        let item = |depth, text: &str| NoteLine::Item {
            depth,
            text: text.into(),
        };
        assert_eq!(
            parse_notes(body),
            [
                NoteLine::Heading("New".into()),
                item(0, "Bold thing"),
                item(1, "nested code"),
                item(2, "deeper"),
                item(1, "tab"),
                NoteLine::Text("plain line".into()),
                NoteLine::Text("#hashtag".into()),
            ]
        );
    }

    #[test]
    fn changelog_sections_are_found_like_the_release_script() {
        let changelog = "# Changelog\n\nintro\n\n## Unreleased\n\n### New\n- a\n\n## 1.4.2\n\n- b\n  - c\n\n## 1.4.1\n";
        assert_eq!(
            changelog_section(changelog, "Unreleased").as_deref(),
            Some("### New\n- a")
        );
        assert_eq!(
            changelog_section(changelog, "1.4.2").as_deref(),
            Some("- b\n  - c")
        );
        assert_eq!(changelog_section(changelog, "1.4.1"), None);
        assert_eq!(changelog_section(changelog, "1.4"), None);
    }

    /// The release workflow stops without notes: this catches it before then.
    #[test]
    fn the_changelog_has_notes_for_the_next_release() {
        let changelog = include_str!("../../../CHANGELOG.md");
        assert!(
            changelog_section(changelog, "Unreleased").is_some()
                || changelog_section(changelog, env!("CARGO_PKG_VERSION")).is_some(),
            "CHANGELOG.md needs notes under ## Unreleased or ## {}",
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn versions_order_like_semver() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("1.0.0") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-alpha.10") > v("1.0.0-alpha.9"));
        assert!(v("v0.10.0") > v("0.9.9"));
        // A number before a word, whatever their characters: `2` < `10a`.
        assert!(v("1.0.0-rc.2") < v("1.0.0-rc.10a"));
        assert!(v("1.0.0-alpha.1") < v("1.0.0-alpha.beta"));
        assert_eq!(v("1.2.3+build.7"), v("1.2.3"));
        assert!(v("1.2.3-rc.1+abc") < v("1.2.3"));
        assert_eq!(Version::parse("1.0"), None);
        assert_eq!(Version::parse("x.y.z"), None);
    }

    #[test]
    fn only_newer_releases_are_offered() {
        let list = || {
            vec![
                release("v1.1.0-beta.1", true),
                release("v1.0.1", false),
                release("v0.9.0", false),
            ]
        };
        assert_eq!(newest("1.0.0", list()).unwrap().version, "1.0.1");
        assert_eq!(newest("1.0.1", list()), None);
        assert_eq!(
            newest("1.0.0-rc.1", list()).unwrap().version,
            "1.1.0-beta.1",
            "pre-release users get pre-releases"
        );
    }
}
