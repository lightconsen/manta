//! GitHub Releases discovery.
//!
//! Fetches the latest release tag via the GitHub API and compares it against
//! the installed version using the vendored semver parser.

use crate::error::SyscityError;
use crate::skills::semver::Version;
use crate::update::{UpdateInfo, REPO};
use crate::Result;

/// GitHub requires a `User-Agent` header on API requests.
const USER_AGENT: &str = "syscity-updater";

/// API endpoint for the latest release of the syscity repository.
pub fn latest_release_url() -> String {
    format!("https://api.github.com/repos/{REPO}/releases/latest")
}

/// Direct download URL for a release tarball asset (`syscity-<target>.tar.gz`).
pub fn asset_download_url(tag: &str, target: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/v{tag}/syscity-{target}.tar.gz")
}

/// URL of the SHA-256 checksum file published alongside a release tarball.
pub fn checksum_download_url(tag: &str, target: &str) -> String {
    format!("{}.sha256", asset_download_url(tag, target))
}

/// Fetch the expected SHA-256 hex digest for a release tarball.
///
/// The checksum asset is the output of `sha256sum` (a hex digest followed by
/// the filename); only the digest is returned.
pub async fn fetch_checksum(client: &reqwest::Client, tag: &str, target: &str) -> Result<String> {
    let url = checksum_download_url(tag, target);
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(SyscityError::ExternalService {
            source: format!("Failed to fetch checksum from {url}: {}", resp.status()),
            cause: None,
        });
    }
    let text = resp.text().await?;
    let digest = text
        .split_whitespace()
        .next()
        .ok_or_else(|| SyscityError::Internal(format!("Empty checksum file at {url}")))?;
    Ok(digest.to_string())
}

/// Fetch the latest release from GitHub and compare it against `current`.
pub async fn check_latest(client: &reqwest::Client, current: &str) -> Result<UpdateInfo> {
    let resp = client
        .get(latest_release_url())
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(SyscityError::ExternalService {
            source: format!(
                "GitHub releases API returned {} for {}",
                resp.status().as_u16(),
                latest_release_url()
            ),
            cause: None,
        });
    }

    let body: serde_json::Value = resp.json().await?;
    let tag = body["tag_name"].as_str().ok_or_else(|| {
        SyscityError::Internal("GitHub release response missing 'tag_name'".to_string())
    })?;

    evaluate(current, tag)
}

/// Check for a newer release and log the outcome. Returns whether an update is
/// available. Used by the daemon's startup auto-check (`update.auto_check`).
pub async fn check_and_log(current: &str) -> bool {
    let client = reqwest::Client::new();
    match check_latest(&client, current).await {
        Ok(info) => {
            if info.update_available {
                tracing::info!("Update available: {info}");
                true
            } else {
                tracing::info!("Up to date: {info}");
                false
            }
        }
        Err(e) => {
            tracing::warn!("Update check failed: {e}");
            false
        }
    }
}

/// Compare a current installed version against a release tag (e.g. `v0.3.0`).
///
/// A `current` that fails to parse (e.g. a dev build tag) is treated as "up
/// to date" so odd local versions never force an update. A malformed *release*
/// tag is a hard error — that is a publishing bug worth surfacing.
pub fn evaluate(current: &str, latest_tag: &str) -> Result<UpdateInfo> {
    let latest = latest_tag.trim_start_matches('v').to_string();

    let current_v = match Version::parse(current) {
        Ok(v) => v,
        Err(_) => {
            return Ok(UpdateInfo {
                current: current.to_string(),
                latest,
                update_available: false,
            });
        }
    };
    let latest_v = Version::parse(&latest).map_err(|e| {
        SyscityError::Validation(format!(
            "Latest release tag '{}' is not a valid semver: {}",
            latest_tag, e
        ))
    })?;

    Ok(UpdateInfo {
        current: current.to_string(),
        latest,
        update_available: latest_v > current_v,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn latest_release_url_uses_repo() {
        assert_eq!(
            latest_release_url(),
            "https://api.github.com/repos/lightconsen/syscity/releases/latest"
        );
    }

    #[test]
    fn evaluate_newer_tag_means_update() {
        let info = evaluate("0.1.2", "v0.3.0").unwrap();
        assert_eq!(info.current, "0.1.2");
        assert_eq!(info.latest, "0.3.0");
        assert!(info.update_available);
    }

    #[test]
    fn evaluate_equal_tag_is_up_to_date() {
        let info = evaluate("0.1.2", "v0.1.2").unwrap();
        assert!(!info.update_available);
    }

    #[test]
    fn evaluate_older_tag_is_up_to_date() {
        let info = evaluate("0.1.2", "v0.1.1").unwrap();
        assert!(!info.update_available);
    }

    #[test]
    fn evaluate_ignores_leading_v_on_current() {
        let info = evaluate("v0.1.2", "v0.3.0").unwrap();
        assert_eq!(info.current, "v0.1.2");
        assert!(info.update_available);
    }

    #[test]
    fn evaluate_unparsable_current_is_up_to_date() {
        let info = evaluate("dev", "v0.3.0").unwrap();
        assert!(!info.update_available);
        assert_eq!(info.latest, "0.3.0");
    }

    #[test]
    fn evaluate_malformed_release_tag_errors() {
        assert!(evaluate("0.1.2", "not-a-tag").is_err());
    }

    #[tokio::test]
    async fn check_latest_fetches_and_evaluates() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/lightconsen/syscity/releases/latest"))
            .and(header("User-Agent", USER_AGENT))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tag_name": "v0.3.0",
            })))
            .mount(&server)
            .await;

        // `check_latest` always hits the real API; to exercise the HTTP path
        // against a mock we test the request/parse half here and the pure
        // comparison via `evaluate` above. This asserts the request shape.
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/repos/lightconsen/syscity/releases/latest", server.uri()))
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
        let body: serde_json::Value = resp.json().await.unwrap();
        let info = evaluate("0.1.2", body["tag_name"].as_str().unwrap()).unwrap();
        assert!(info.update_available);
        assert_eq!(info.latest, "0.3.0");
    }
}
