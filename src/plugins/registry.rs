//! Plugin Registry Client
//!
//! Provides a client for fetching plugin indexes from remote registries,
//! searching for plugins, downloading plugin archives, and verifying
//! SHA-256 checksums.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::info;

/// The index returned by a plugin registry (index.json).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryIndex {
    /// Base URL of this registry
    pub registry_url: String,
    /// All plugins available in this registry
    pub plugins: Vec<RegistryPluginEntry>,
}

/// An entry in a plugin registry index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryPluginEntry {
    /// Unique plugin identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Latest version
    pub version: String,
    /// Short description
    pub description: String,
    /// Author (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Download URL (absolute or relative to registry base)
    pub download_url: String,
    /// SHA-256 checksum of the archive
    pub checksum_sha256: String,
    /// Raw manifest JSON for additional metadata
    pub manifest: serde_json::Value,
}

/// Client for communicating with a remote plugin registry.
pub struct RegistryClient {
    http_client: reqwest::Client,
    registry_url: String,
}

impl RegistryClient {
    /// Create a new RegistryClient pointing at `registry_url`.
    pub fn new(registry_url: &str) -> Self {
        Self {
            http_client: reqwest::Client::new(),
            registry_url: registry_url.trim_end_matches('/').to_string(),
        }
    }

    /// Fetch the full registry index.
    pub async fn fetch_index(&self) -> crate::Result<RegistryIndex> {
        let url = format!("{}/index.json", self.registry_url);
        let resp = self.http_client.get(&url).send().await?;
        let index: RegistryIndex = resp.json().await?;
        Ok(index)
    }

    /// Search the registry for plugins matching `query`.
    ///
    /// Matches against plugin id, name, and description (case-insensitive).
    pub async fn search(&self, query: &str) -> crate::Result<Vec<RegistryPluginEntry>> {
        let index = self.fetch_index().await?;
        let q = query.to_lowercase();
        Ok(index
            .plugins
            .into_iter()
            .filter(|p| {
                p.id.to_lowercase().contains(&q)
                    || p.name.to_lowercase().contains(&q)
                    || p.description.to_lowercase().contains(&q)
            })
            .collect())
    }

    /// Download a plugin archive and verify its SHA-256 checksum.
    pub async fn download(&self, entry: &RegistryPluginEntry) -> crate::Result<Vec<u8>> {
        let url = if entry.download_url.starts_with("http") {
            entry.download_url.clone()
        } else {
            format!("{}/{}", self.registry_url, entry.download_url)
        };
        info!("Downloading plugin from {}", url);
        let resp = self.http_client.get(&url).send().await?;
        let bytes = resp.bytes().await?;

        // Verify SHA-256 checksum
        if !entry.checksum_sha256.is_empty() {
            let actual = hex::encode(Sha256::digest(&bytes));
            if actual != entry.checksum_sha256 {
                return Err(crate::error::SyscityError::Internal(format!(
                    "Checksum mismatch for {}: expected {}, got {}",
                    entry.id, entry.checksum_sha256, actual
                )));
            }
        }

        Ok(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_index_deserialize() {
        let json = serde_json::json!({
            "registry_url": "https://plugins.syscity.dev",
            "plugins": [{
                "id": "com.example.test",
                "name": "Test Plugin",
                "version": "1.0.0",
                "description": "A test plugin",
                "download_url": "/plugins/test.tar.gz",
                "checksum_sha256": "abc123",
                "manifest": {}
            }]
        });
        let index: RegistryIndex = serde_json::from_value(json).unwrap();
        assert_eq!(index.plugins.len(), 1);
        assert_eq!(index.plugins[0].id, "com.example.test");
    }
}
