//! OpenClaw-Aligned Artifacts System
//!
//! Tracks code snippets, documents, images, and links bound to session lifecycle.
//! Inspired by OpenClaw's `artifacts.ts`.
//!
//! Features:
//! - Artifact creation, retrieval, listing
//! - Session-bound lifecycle (auto-cleanup on session end)
//! - Multiple artifact types
//! - File-backed storage for large artifacts

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, info};

/// Type of artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    /// Code snippet (any language).
    Code,
    /// Document / text content.
    Document,
    /// Image (base64 or URL).
    Image,
    /// External link.
    Link,
    /// Data (JSON, CSV, etc.).
    Data,
    /// Generic file.
    File,
}

impl std::fmt::Display for ArtifactType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArtifactType::Code => write!(f, "code"),
            ArtifactType::Document => write!(f, "document"),
            ArtifactType::Image => write!(f, "image"),
            ArtifactType::Link => write!(f, "link"),
            ArtifactType::Data => write!(f, "data"),
            ArtifactType::File => write!(f, "file"),
        }
    }
}

/// An artifact produced during a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Unique artifact ID.
    pub id: String,
    /// Session ID this artifact belongs to.
    pub session_id: String,
    /// Human-readable title.
    pub title: String,
    /// Artifact type.
    pub artifact_type: ArtifactType,
    /// Content (for small artifacts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// File path (for large artifacts stored on disk).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// For code artifacts: language identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// For link artifacts: URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// MIME type (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// When the artifact was created.
    pub created_at: DateTime<Utc>,
    /// Size in bytes.
    pub size_bytes: usize,
    /// Optional tags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Optional metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl Artifact {
    /// Create a new code artifact.
    pub fn code(
        id: impl Into<String>,
        session_id: impl Into<String>,
        title: impl Into<String>,
        language: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        let content = content.into();
        let size = content.len();
        Self {
            id: id.into(),
            session_id: session_id.into(),
            title: title.into(),
            artifact_type: ArtifactType::Code,
            content: Some(content),
            file_path: None,
            language: Some(language.into()),
            url: None,
            mime_type: Some("text/plain".to_string()),
            created_at: Utc::now(),
            size_bytes: size,
            tags: None,
            metadata: None,
        }
    }

    /// Create a new document artifact.
    pub fn document(
        id: impl Into<String>,
        session_id: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        let content = content.into();
        let size = content.len();
        Self {
            id: id.into(),
            session_id: session_id.into(),
            title: title.into(),
            artifact_type: ArtifactType::Document,
            content: Some(content),
            file_path: None,
            language: None,
            url: None,
            mime_type: Some("text/plain".to_string()),
            created_at: Utc::now(),
            size_bytes: size,
            tags: None,
            metadata: None,
        }
    }

    /// Create a new link artifact.
    pub fn link(
        id: impl Into<String>,
        session_id: impl Into<String>,
        title: impl Into<String>,
        url: impl Into<String>,
    ) -> Self {
        let url_str = url.into();
        Self {
            id: id.into(),
            session_id: session_id.into(),
            title: title.into(),
            artifact_type: ArtifactType::Link,
            content: None,
            file_path: None,
            language: None,
            url: Some(url_str.clone()),
            mime_type: None,
            created_at: Utc::now(),
            size_bytes: url_str.len(),
            tags: None,
            metadata: None,
        }
    }

    /// Create a new data artifact.
    pub fn data(
        id: impl Into<String>,
        session_id: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        let content = content.into();
        let size = content.len();
        Self {
            id: id.into(),
            session_id: session_id.into(),
            title: title.into(),
            artifact_type: ArtifactType::Data,
            content: Some(content),
            file_path: None,
            language: None,
            url: None,
            mime_type: Some("application/json".to_string()),
            created_at: Utc::now(),
            size_bytes: size,
            tags: None,
            metadata: None,
        }
    }

    /// Add a tag.
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.get_or_insert_with(Vec::new).push(tag.into());
        self
    }

    /// Add metadata.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Set file path for large artifacts.
    pub fn with_file_path(mut self, path: impl Into<String>) -> Self {
        self.file_path = Some(path.into());
        self
    }

    /// Get the content (from memory or file).
    pub fn get_content(&self) -> Option<String> {
        if let Some(ref content) = self.content {
            return Some(content.clone());
        }
        if let Some(ref path) = self.file_path {
            return std::fs::read_to_string(path).ok();
        }
        None
    }

    /// Render as markdown.
    pub fn to_markdown(&self) -> String {
        match self.artifact_type {
            ArtifactType::Code => {
                let lang = self.language.as_deref().unwrap_or("");
                let content = self.get_content().unwrap_or_default();
                format!("### {}\n\n```{lang}\n{}\n```\n", self.title, content, lang = lang)
            }
            ArtifactType::Document => {
                let content = self.get_content().unwrap_or_default();
                format!("### {}\n\n{}\n", self.title, content)
            }
            ArtifactType::Link => {
                let url = self.url.as_deref().unwrap_or("#");
                format!("- [{}]({})\n", self.title, url)
            }
            ArtifactType::Data => {
                let content = self.get_content().unwrap_or_default();
                format!("### {} (data)\n\n```json\n{}\n```\n", self.title, content)
            }
            ArtifactType::Image => {
                let url = self.url.as_deref().unwrap_or("");
                format!("### {}\n\n![{}]({})\n", self.title, self.title, url)
            }
            ArtifactType::File => {
                format!(
                    "- **{}** ({}, {} bytes)\n",
                    self.title,
                    self.mime_type.as_deref().unwrap_or("unknown"),
                    self.size_bytes
                )
            }
        }
    }
}

/// In-memory artifact store with session-bound lifecycle.
pub struct ArtifactStore {
    /// session_id -> Vec<Artifact>
    artifacts: std::sync::Mutex<HashMap<String, Vec<Artifact>>>,
    /// Root directory for file-backed artifacts.
    root_dir: PathBuf,
}

impl ArtifactStore {
    /// Create a new artifact store.
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            artifacts: std::sync::Mutex::new(HashMap::new()),
            root_dir: root_dir.into(),
        }
    }

    /// Initialize directories.
    pub fn init(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root_dir)?;
        debug!("Artifact store initialized at {:?}", self.root_dir);
        Ok(())
    }

    /// Add an artifact to a session.
    pub fn add(&self, artifact: Artifact) {
        let mut artifacts = self.artifacts.lock().unwrap();
        let list = artifacts.entry(artifact.session_id.clone()).or_default();
        list.push(artifact);
        debug!(
            "Added artifact '{}' to session {}",
            list.last().unwrap().id,
            list.last().unwrap().session_id
        );
    }

    /// Get all artifacts for a session.
    pub fn get_for_session(&self, session_id: &str) -> Vec<Artifact> {
        let artifacts = self.artifacts.lock().unwrap();
        artifacts.get(session_id).cloned().unwrap_or_default()
    }

    /// Get a specific artifact by ID.
    pub fn get(&self, session_id: &str, artifact_id: &str) -> Option<Artifact> {
        let artifacts = self.artifacts.lock().unwrap();
        artifacts
            .get(session_id)
            .and_then(|list| list.iter().find(|a| a.id == artifact_id).cloned())
    }

    /// List all artifacts across all sessions.
    pub fn list_all(&self) -> Vec<Artifact> {
        let artifacts = self.artifacts.lock().unwrap();
        artifacts.values().flatten().cloned().collect()
    }

    /// List artifact IDs for a session.
    pub fn list_session(&self, session_id: &str) -> Vec<String> {
        let artifacts = self.artifacts.lock().unwrap();
        artifacts
            .get(session_id)
            .map(|list| list.iter().map(|a| a.id.clone()).collect())
            .unwrap_or_default()
    }

    /// Remove a specific artifact.
    pub fn remove(&self, session_id: &str, artifact_id: &str) -> Option<Artifact> {
        let mut artifacts = self.artifacts.lock().unwrap();
        if let Some(list) = artifacts.get_mut(session_id) {
            let pos = list.iter().position(|a| a.id == artifact_id);
            if let Some(pos) = pos {
                return Some(list.remove(pos));
            }
        }
        None
    }

    /// Remove all artifacts for a session (cleanup on session end).
    pub fn clear_session(&self, session_id: &str) -> Vec<Artifact> {
        let mut artifacts = self.artifacts.lock().unwrap();
        let removed = artifacts.remove(session_id).unwrap_or_default();
        info!("Cleared {} artifacts for session {}", removed.len(), session_id);
        removed
    }

    /// Export all artifacts for a session as a single markdown file.
    pub fn export_session(&self, session_id: &str) -> Result<PathBuf, String> {
        let artifacts = self.get_for_session(session_id);
        if artifacts.is_empty() {
            return Err("No artifacts for session".to_string());
        }

        let mut content = format!("# Session Artifacts: {}\n\n", session_id);
        for artifact in &artifacts {
            content.push_str(&artifact.to_markdown());
            content.push('\n');
        }

        let filename = format!("artifacts_{}.md", sanitize(session_id));
        let path = self.root_dir.join(&filename);
        std::fs::write(&path, content)
            .map_err(|e| format!("Failed to write artifact export: {}", e))?;
        info!("Exported {} artifacts to {:?}", artifacts.len(), path);
        Ok(path)
    }

    /// Get store stats.
    pub fn stats(&self) -> ArtifactStoreStats {
        let artifacts = self.artifacts.lock().unwrap();
        let total = artifacts.values().map(|v| v.len()).sum();
        let total_size: usize = artifacts.values().flatten().map(|a| a.size_bytes).sum();
        ArtifactStoreStats {
            session_count: artifacts.len(),
            artifact_count: total,
            total_size_bytes: total_size,
        }
    }
}

/// Artifact store statistics.
#[derive(Debug, Clone)]
pub struct ArtifactStoreStats {
    pub session_count: usize,
    pub artifact_count: usize,
    pub total_size_bytes: usize,
}

fn sanitize(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => c,
            _ => '_',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_code_artifact() {
        let a = Artifact::code("a1", "s1", "main.rs", "rust", "fn main() {}");
        assert_eq!(a.artifact_type, ArtifactType::Code);
        assert_eq!(a.language, Some("rust".to_string()));
        assert_eq!(a.get_content(), Some("fn main() {}".to_string()));
    }

    #[test]
    fn test_link_artifact() {
        let a = Artifact::link("a1", "s1", "Docs", "https://example.com");
        assert_eq!(a.artifact_type, ArtifactType::Link);
        assert_eq!(a.url, Some("https://example.com".to_string()));
    }

    #[test]
    fn test_store_add_and_get() {
        let tmp = TempDir::new().unwrap();
        let store = ArtifactStore::new(tmp.path());
        store.init().unwrap();

        store.add(Artifact::code("a1", "s1", "main.rs", "rust", "fn main() {}"));
        store.add(Artifact::document("a2", "s1", "README", "Hello"));
        store.add(Artifact::link("a3", "s2", "Docs", "https://example.com"));

        let s1 = store.get_for_session("s1");
        assert_eq!(s1.len(), 2);

        let s2 = store.get_for_session("s2");
        assert_eq!(s2.len(), 1);

        let all = store.list_all();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_store_clear_session() {
        let tmp = TempDir::new().unwrap();
        let store = ArtifactStore::new(tmp.path());
        store.init().unwrap();

        store.add(Artifact::code("a1", "s1", "main.rs", "rust", "fn main() {}"));
        store.clear_session("s1");

        let s1 = store.get_for_session("s1");
        assert!(s1.is_empty());
    }

    #[test]
    fn test_markdown_render() {
        let a = Artifact::code("a1", "s1", "main.rs", "rust", "fn main() {}");
        let md = a.to_markdown();
        assert!(md.contains("### main.rs"));
        assert!(md.contains("```rust"));
        assert!(md.contains("fn main() {}"));
    }

    #[test]
    fn test_export_session() {
        let tmp = TempDir::new().unwrap();
        let store = ArtifactStore::new(tmp.path());
        store.init().unwrap();

        store.add(Artifact::code("a1", "s1", "main.rs", "rust", "fn main() {}"));
        store.add(Artifact::document("a2", "s1", "Notes", "Important"));

        let path = store.export_session("s1").unwrap();
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("main.rs"));
        assert!(content.contains("Notes"));
    }
}
