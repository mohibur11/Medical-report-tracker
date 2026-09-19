//! Finding out whether a newer installer has been published.
//!
//! Releases live on GitHub, one per tag, each carrying the Windows installer as
//! an asset. This asks GitHub for the newest one and compares versions; it does
//! not download anything, and it runs only when the user asks. The app's whole
//! stance is that nothing leaves the machine unbidden, and a version check that
//! phoned home on every launch would be the first thing to break that.

use serde::{Deserialize, Serialize};

pub const REPO: &str = "mohibur11/Medical-report-tracker";

/// Where a person goes to download by hand — also the fallback when the
/// release carries no installer this code recognises.
pub fn releases_page() -> String {
    format!("https://github.com/{REPO}/releases/latest")
}

fn latest_api() -> String {
    format!("https://api.github.com/repos/{REPO}/releases/latest")
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    pub current: String,
    pub latest: String,
    pub newer: bool,
    /// The installer itself, when the release has one; the release page otherwise.
    pub download_url: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// `1.2.3` or `v1.2.3`, with anything after a `-` or `+` ignored.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let core = version
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let major = parts.next()??;
    let minor = parts.next().unwrap_or(Some(0))?;
    let patch = parts.next().unwrap_or(Some(0))?;
    Some((major, minor, patch))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(c), Some(now)) => c > now,
        // A tag this cannot read is not offered as an upgrade.
        _ => false,
    }
}

/// Compare a release description against the running version.
fn evaluate(current: &str, release: Release) -> UpdateCheck {
    let latest = release.tag_name.trim_start_matches('v').to_string();
    let installer = release
        .assets
        .iter()
        .find(|a| a.name.ends_with("-setup.exe"))
        .map(|a| a.browser_download_url.clone());
    UpdateCheck {
        newer: is_newer(&latest, current),
        download_url: installer
            .or(release.html_url)
            .unwrap_or_else(releases_page),
        current: current.to_string(),
        latest,
    }
}

/// Ask GitHub for the newest release. Blocking; run it off the UI thread.
pub fn check(current: &str) -> Result<UpdateCheck, String> {
    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?
        .get(latest_api())
        // GitHub refuses requests with no User-Agent.
        .header(reqwest::header::USER_AGENT, format!("medicine-report-tracker/{current}"))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .map_err(|e| format!("cannot reach GitHub: {e}"))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err("No release has been published yet.".into());
    }
    if !response.status().is_success() {
        return Err(format!("GitHub answered {}.", response.status()));
    }

    let release: Release = response
        .json()
        .map_err(|e| format!("GitHub's reply could not be read: {e}"))?;
    Ok(evaluate(current, release))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically_and_ignore_the_v() {
        assert!(is_newer("v0.3.0", "0.2.0"));
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.0"), "not a string compare");
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("0.3.0-beta", "0.3.0"), "a pre-release tag is not offered over its release");
        assert!(is_newer("0.3.0-beta", "0.2.0"));
        assert!(!is_newer("nightly", "0.2.0"), "unreadable tags are never an upgrade");
    }

    #[test]
    fn a_newer_release_points_at_its_installer() {
        let release: Release = serde_json::from_str(
            r#"{
                "tag_name": "v0.3.0",
                "html_url": "https://github.com/x/y/releases/tag/v0.3.0",
                "assets": [
                    { "name": "Medicine Report Tracker_0.3.0_x64-setup.exe.sig", "browser_download_url": "https://dl/sig" },
                    { "name": "Medicine Report Tracker_0.3.0_x64-setup.exe", "browser_download_url": "https://dl/setup.exe" }
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(
            evaluate("0.2.0", release),
            UpdateCheck {
                current: "0.2.0".into(),
                latest: "0.3.0".into(),
                newer: true,
                download_url: "https://dl/setup.exe".into(),
            }
        );
    }

    #[test]
    fn a_release_without_an_installer_points_at_its_page() {
        let release: Release = serde_json::from_str(
            r#"{ "tag_name": "v0.2.0", "html_url": "https://github.com/x/y/releases/tag/v0.2.0", "assets": [] }"#,
        )
        .unwrap();
        let check = evaluate("0.2.0", release);
        assert!(!check.newer);
        assert_eq!(check.download_url, "https://github.com/x/y/releases/tag/v0.2.0");
    }
}
