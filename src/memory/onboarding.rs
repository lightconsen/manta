//! First-launch identity onboarding
//!
//! After the LLM model is configured, the user completes a short identity
//! wizard that writes the agent's SOUL.md, IDENTITY.md and USER.md into the
//! workspace, then marks workspace setup as completed so the app skips the
//! wizard on subsequent launches.

use std::path::Path;

use serde::Deserialize;
use tracing::info;

use crate::memory::soul::{SoulConfig, SoulFile};
use crate::memory::workspace_state::WorkspaceManager;

/// Identity form payload. Every field is optional so the user may skip any of
/// them; the files are still written (with whatever was provided) and setup is
/// marked completed.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OnboardingPayload {
    /// Agent name / call sign (SOUL.md `name`).
    #[serde(default)]
    pub name: Option<String>,
    /// Short persona / vibe description (SOUL.md `persona`).
    #[serde(default)]
    pub vibe: Option<String>,
    /// Signature emoji (SOUL.md `emoji`).
    #[serde(default)]
    pub emoji: Option<String>,
    /// How the user wants to be addressed (USER.md).
    #[serde(default)]
    pub user_name: Option<String>,
    /// The user's city (USER.md).
    #[serde(default)]
    pub city: Option<String>,
    /// Free-form context about the user (USER.md).
    #[serde(default)]
    pub user_context: Option<String>,
}

/// Onboarding completion status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingStatus {
    /// The identity wizard has not been completed yet.
    Pending,
    /// Identity files exist and setup is marked completed.
    Done,
}

/// Read the workspace setup state and report whether onboarding is done.
pub async fn status(dir: &Path) -> crate::Result<OnboardingStatus> {
    let manager = WorkspaceManager::new(dir.to_path_buf());
    let state = manager.load_state().await?;
    Ok(if state.is_setup_completed() {
        OnboardingStatus::Done
    } else {
        OnboardingStatus::Pending
    })
}

/// Write SOUL.md / IDENTITY.md / USER.md from the payload and mark setup done.
///
/// `dir` is the workspace directory (production passes
/// `crate::dirs::workspace_data_dir()`; tests pass a temporary directory so
/// they never touch `~/.syscity`). An existing SOUL.md is overwritten.
pub async fn apply(dir: &Path, payload: &OnboardingPayload) -> crate::Result<()> {
    let soul = build_soul_file(payload);
    write_file(&dir.join("SOUL.md"), &soul.to_markdown()?).await?;
    write_file(&dir.join("IDENTITY.md"), &build_identity_md(payload)).await?;
    write_file(&dir.join("USER.md"), &build_user_md(payload)).await?;

    WorkspaceManager::new(dir.to_path_buf())
        .mark_setup_completed()
        .await?;

    info!("Onboarding identity files written and setup marked completed");
    Ok(())
}

/// Build the SOUL.md content from the identity fields.
fn build_soul_file(payload: &OnboardingPayload) -> SoulFile {
    let config = SoulConfig {
        name: payload.name.clone(),
        persona: payload.vibe.clone(),
        voice: None,
        emoji: payload.emoji.clone(),
        ..Default::default()
    };

    // Keep a stable default body when the user skipped every identity field so
    // SOUL.md is never an empty frontmatter block (which would not round-trip).
    let has_frontmatter = !config.is_empty();
    let body = if has_frontmatter {
        String::new()
    } else {
        "# Core Truths\n\nBe genuinely helpful.\n".to_string()
    };

    SoulFile { config, body, has_frontmatter }
}

/// Build the assistant identity file (IDENTITY.md).
fn build_identity_md(payload: &OnboardingPayload) -> String {
    let mut out = String::from(
        "# Identity\n\n<!-- Assistant identity established during first-launch onboarding. -->\n\n",
    );
    push_bullet(&mut out, "Name", payload.name.as_deref());
    push_bullet(&mut out, "Vibe", payload.vibe.as_deref());
    push_bullet(&mut out, "Emoji", payload.emoji.as_deref());
    out
}

/// Build the user profile file (USER.md).
fn build_user_md(payload: &OnboardingPayload) -> String {
    let mut out = String::from(
        "# User\n\n<!-- User profile established during first-launch onboarding. -->\n\n",
    );
    push_bullet(&mut out, "Name", payload.user_name.as_deref());
    push_bullet(&mut out, "City", payload.city.as_deref());

    if let Some(context) = payload.user_context.as_deref().map(str::trim) {
        if !context.is_empty() {
            out.push_str("\n## Context\n\n");
            out.push_str(context);
            out.push('\n');
        }
    }

    out
}

/// Append a markdown bullet when `value` is non-empty.
fn push_bullet(out: &mut String, label: &str, value: Option<&str>) {
    let value = value.map(str::trim).unwrap_or_default();
    if !value.is_empty() {
        out.push_str(&format!("- **{}**: {}\n", label, value));
    }
}

/// Write `content` to `path`, creating the parent directory if needed.
async fn write_file(path: &Path, content: &str) -> crate::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            crate::error::SyscityError::Storage {
                context: format!("Failed to create directory: {:?}", parent),
                details: e.to_string(),
            }
        })?;
    }
    tokio::fs::write(path, content)
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: format!("Failed to write file: {:?}", path),
            details: e.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn sample_payload() -> OnboardingPayload {
        OnboardingPayload {
            name: Some("Syscity".to_string()),
            vibe: Some("curious and helpful".to_string()),
            emoji: Some("🦑".to_string()),
            user_name: Some("Alice".to_string()),
            city: Some("Shanghai".to_string()),
            user_context: Some("likes Rust and coffee".to_string()),
        }
    }

    #[tokio::test]
    async fn test_status_pending_without_state() {
        let dir = TempDir::new().unwrap();
        assert_eq!(status(dir.path()).await.unwrap(), OnboardingStatus::Pending);
    }

    #[tokio::test]
    async fn test_apply_writes_three_files_and_marks_done() {
        let dir = TempDir::new().unwrap();
        let payload = sample_payload();

        assert_eq!(status(dir.path()).await.unwrap(), OnboardingStatus::Pending);

        apply(dir.path(), &payload).await.unwrap();

        assert!(dir.path().join("SOUL.md").exists());
        assert!(dir.path().join("IDENTITY.md").exists());
        assert!(dir.path().join("USER.md").exists());

        let soul = tokio::fs::read_to_string(dir.path().join("SOUL.md"))
            .await
            .unwrap();
        assert!(soul.contains("name: Syscity"));
        assert!(soul.contains("persona: curious and helpful"));
        assert!(soul.contains("🦑"));
        // No empty/null frontmatter fields should leak into the output.
        assert!(!soul.contains("null"));

        let identity = tokio::fs::read_to_string(dir.path().join("IDENTITY.md"))
            .await
            .unwrap();
        assert!(identity.contains("Name"));
        assert!(identity.contains("Syscity"));

        let user = tokio::fs::read_to_string(dir.path().join("USER.md"))
            .await
            .unwrap();
        assert!(user.contains("Alice"));
        assert!(user.contains("Shanghai"));
        assert!(user.contains("likes Rust and coffee"));

        // Round-trips back through the parser.
        let parsed = SoulFile::parse(&soul).unwrap();
        assert_eq!(parsed.config.name, Some("Syscity".to_string()));
        assert_eq!(parsed.config.emoji, Some("🦑".to_string()));

        assert_eq!(status(dir.path()).await.unwrap(), OnboardingStatus::Done);
    }

    #[tokio::test]
    async fn test_apply_overwrites_existing_soul() {
        let dir = TempDir::new().unwrap();
        let soul_path = dir.path().join("SOUL.md");
        tokio::fs::write(&soul_path, "stale content").await.unwrap();

        let payload = OnboardingPayload {
            name: Some("New".to_string()),
            ..Default::default()
        };
        apply(dir.path(), &payload).await.unwrap();

        let soul = tokio::fs::read_to_string(&soul_path).await.unwrap();
        assert!(soul.contains("name: New"));
        assert!(!soul.contains("stale content"));
    }

    #[tokio::test]
    async fn test_apply_all_none_still_writes_files() {
        let dir = TempDir::new().unwrap();
        apply(dir.path(), &OnboardingPayload::default())
            .await
            .unwrap();

        let soul = tokio::fs::read_to_string(dir.path().join("SOUL.md"))
            .await
            .unwrap();
        assert!(soul.contains("# Core Truths"));

        assert!(dir.path().join("IDENTITY.md").exists());
        assert!(dir.path().join("USER.md").exists());
        assert_eq!(status(dir.path()).await.unwrap(), OnboardingStatus::Done);
    }
}
