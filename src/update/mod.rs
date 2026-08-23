//! Online self-update via GitHub Releases.
//!
//! Shared core used by the CLI (`syscity update`), the web/daemon update
//! endpoints, and the desktop updater. Provides release discovery, download
//! with SHA-256 verification, and atomic binary replacement.
//!
//! Security model: every downloaded tarball is verified against a SHA-256
//! checksum published alongside the release artifact before it touches the
//! running binary. Nothing is ever applied unverified.
// INVARIANTS-NONE: self-update verifies downloads before swapping; failure paths leave the current binary untouched.

pub mod apply;
pub mod download;
pub mod github;
pub mod platform;

use std::fmt;

/// GitHub repository that hosts syscity release artifacts.
pub const REPO: &str = "lightconsen/syscity";

/// Result of a release check against the current installed version.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UpdateInfo {
    /// Current installed version (typically `crate::VERSION`).
    pub current: String,
    /// Latest published version (tag with a leading `v` stripped).
    pub latest: String,
    /// Whether an update is available (`latest` > `current`).
    pub update_available: bool,
}

impl UpdateInfo {
    /// Build an "up to date" result (no update available).
    pub fn up_to_date(current: &str) -> Self {
        Self {
            current: current.to_string(),
            latest: current.to_string(),
            update_available: false,
        }
    }
}

impl fmt::Display for UpdateInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.update_available {
            write!(f, "v{} → v{}", self.current, self.latest)
        } else {
            write!(f, "v{} (up to date)", self.current)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_info_up_to_date() {
        let info = UpdateInfo::up_to_date("1.2.3");
        assert!(!info.update_available);
        assert_eq!(info.current, "1.2.3");
        assert_eq!(info.latest, "1.2.3");
    }

    #[test]
    fn update_info_display() {
        let info = UpdateInfo {
            current: "1.2.3".into(),
            latest: "2.0.0".into(),
            update_available: true,
        };
        assert_eq!(info.to_string(), "v1.2.3 → v2.0.0");
    }
}
