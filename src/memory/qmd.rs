//! QMD Query System with Scope-based Access Control
//!
//! QMD (Query Markdown/Document) provides semantic and structured querying
//! over memory documents. This module wraps external `qmd` CLI execution
//! and enforces QmdScope-based access control.

use serde::{Deserialize, Serialize};

/// A single QMD query result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QmdQueryResult {
    /// Document unique ID.
    pub docid: Option<String>,
    /// Match score.
    pub score: Option<f64>,
    /// Collection name.
    pub collection: Option<String>,
    /// Source file path.
    pub file: Option<String>,
    /// Match snippet summary.
    pub snippet: Option<String>,
    /// Full body content.
    pub body: Option<String>,
    /// Start line number.
    pub start_line: Option<u32>,
    /// End line number.
    pub end_line: Option<u32>,
}

/// Scope-based access control for QMD queries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QmdScope {
    /// Channel restriction (e.g. "telegram", "discord").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Chat type restriction ("direct" | "group").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_type: Option<String>,
    /// Session key prefix filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_prefix: Option<String>,
    /// Explicitly allowed identifiers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    /// Explicitly denied identifiers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
}

impl QmdScope {
    /// Check if a session key is allowed by this scope.
    pub fn is_allowed(&self, session_key: Option<&str>) -> bool {
        // Deny list takes precedence
        if let Some(key) = session_key {
            if self.deny.iter().any(|d| key.contains(d)) {
                return false;
            }
            if !self.allow.is_empty() && !self.allow.iter().any(|a| key.contains(a)) {
                return false;
            }
            if let Some(ref prefix) = self.key_prefix {
                if !key.starts_with(prefix) {
                    return false;
                }
            }
        }
        true
    }

    /// Builder: set channel.
    pub fn with_channel(mut self, channel: impl Into<String>) -> Self {
        self.channel = Some(channel.into());
        self
    }

    /// Builder: set chat type.
    pub fn with_chat_type(mut self, chat_type: impl Into<String>) -> Self {
        self.chat_type = Some(chat_type.into());
        self
    }

    /// Builder: set key prefix.
    pub fn with_key_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.key_prefix = Some(prefix.into());
        self
    }

    /// Builder: add to allow list.
    pub fn allow(mut self, id: impl Into<String>) -> Self {
        self.allow.push(id.into());
        self
    }

    /// Builder: add to deny list.
    pub fn deny(mut self, id: impl Into<String>) -> Self {
        self.deny.push(id.into());
        self
    }
}

/// QMD executor wrapping the external `qmd` CLI.
#[derive(Debug, Clone)]
pub struct QmdExecutor {
    /// Working directory for qmd execution.
    cwd: std::path::PathBuf,
    /// Default timeout in seconds.
    timeout_secs: u64,
}

impl QmdExecutor {
    /// Create a new QMD executor.
    pub fn new(cwd: impl Into<std::path::PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            timeout_secs: 30,
        }
    }

    /// Builder: set timeout.
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Check if `qmd` is available in PATH.
    pub async fn is_available(&self) -> bool {
        match tokio::process::Command::new("qmd")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
        {
            Ok(status) => status.success(),
            Err(_) => false,
        }
    }

    /// Run a QMD query with optional scope filtering.
    ///
    /// If `qmd` is not available, falls back to an empty result set.
    pub async fn query(
        &self,
        query: impl AsRef<str>,
        scope: Option<&QmdScope>,
    ) -> crate::Result<Vec<QmdQueryResult>> {
        if !self.is_available().await {
            tracing::warn!("qmd CLI not available in PATH; returning empty results");
            return Ok(Vec::new());
        }

        let mut cmd = tokio::process::Command::new("qmd");
        cmd.arg("query")
            .arg(query.as_ref())
            .current_dir(&self.cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Add scope filters as CLI flags if provided
        if let Some(scope) = scope {
            if let Some(ref channel) = scope.channel {
                cmd.arg("--channel").arg(channel);
            }
            if let Some(ref chat_type) = scope.chat_type {
                cmd.arg("--chat-type").arg(chat_type);
            }
            if let Some(ref prefix) = scope.key_prefix {
                cmd.arg("--prefix").arg(prefix);
            }
            for allowed in &scope.allow {
                cmd.arg("--allow").arg(allowed);
            }
            for denied in &scope.deny {
                cmd.arg("--deny").arg(denied);
            }
        }

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            cmd.output(),
        )
        .await
        .map_err(|_| crate::error::MantaError::ExternalService {
            source: "qmd query timed out".to_string(),
            cause: None,
        })?
        .map_err(|e| crate::error::MantaError::ExternalService {
            source: format!("Failed to run qmd: {}", e),
            cause: None,
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::error::MantaError::ExternalService {
                source: format!("qmd query failed: {}", stderr),
                cause: None,
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let results: Vec<QmdQueryResult> = serde_json::from_str(&stdout).map_err(|e| {
            crate::error::MantaError::ExternalService {
                source: format!("Failed to parse qmd output: {}", e),
                cause: Some(stdout.to_string().into()),
            }
        })?;

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qmd_scope_allow() {
        let scope = QmdScope::default()
            .allow("user:alice")
            .deny("user:bob");

        assert!(scope.is_allowed(Some("user:alice:session1")));
        assert!(!scope.is_allowed(Some("user:bob:session1")));
        assert!(!scope.is_allowed(Some("user:charlie:session1")));
    }

    #[test]
    fn test_qmd_scope_prefix() {
        let scope = QmdScope::default().with_key_prefix("telegram:");
        assert!(scope.is_allowed(Some("telegram:12345")));
        assert!(!scope.is_allowed(Some("discord:12345")));
    }

    #[test]
    fn test_qmd_scope_empty() {
        let scope = QmdScope::default();
        assert!(scope.is_allowed(Some("anything")));
        assert!(scope.is_allowed(None));
    }
}
