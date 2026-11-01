//! The update check (PLAN.md section 15.7): on start, ask the GitHub releases API whether a newer
//! version exists. Nothing is downloaded or installed; the UIs show a link.

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
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
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
    /// compare numerically when both are numbers.
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
                                    _ => x.cmp(y),
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

/// The newest release newer than `current`, from a releases API answer. Pre-releases count
/// only when `current` is one itself.
fn newest(current: &str, releases: Vec<Release>) -> Option<Update> {
    let current = Version::parse(current)?;
    releases
        .into_iter()
        .filter(|r| !r.draft && (!r.prerelease || current.pre.is_some()))
        .filter_map(|r| Some((Version::parse(&r.tag_name)?, r)))
        .filter(|(v, _)| *v > current)
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, r)| Update {
            version: r.tag_name.trim_start_matches('v').to_owned(),
            url: r.html_url,
        })
}

/// Asks GitHub; `Ok(None)` when this is the newest version.
pub async fn check(current: &str) -> Result<Option<Update>, String> {
    let config =
        crate::tls::client_config(crate::tls::Trust::Ca, true).map_err(|e| e.to_string())?;
    let http = reqwest::Client::builder()
        .use_preconfigured_tls((*config).clone())
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
        }
    }

    #[test]
    fn versions_order_like_semver() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("1.0.0") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-alpha.10") > v("1.0.0-alpha.9"));
        assert!(v("v0.10.0") > v("0.9.9"));
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
