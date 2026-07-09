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
            http_client: Self::build_http_client(),
            registry_url: registry_url.trim_end_matches('/').to_string(),
        }
    }

    /// Build an HTTP client with a 30-second timeout.
    fn build_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

    #[tokio::test]
    async fn test_fetch_index() {
        let mock_server = MockServer::start().await;
        let index_json = serde_json::json!({
            "registry_url": mock_server.uri(),
            "plugins": [{
                "id": "com.test.mock",
                "name": "Mock Plugin",
                "version": "0.1.0",
                "description": "A mock plugin for testing",
                "download_url": "/plugins/mock.tar.gz",
                "checksum_sha256": "deadbeef",
                "manifest": {}
            }]
        });

        Mock::given(method("GET"))
            .and(path("/index.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&index_json))
            .mount(&mock_server)
            .await;

        let client = RegistryClient::new(&mock_server.uri());
        let index = client.fetch_index().await.unwrap();
        assert_eq!(index.plugins.len(), 1);
        assert_eq!(index.plugins[0].id, "com.test.mock");
    }

    #[tokio::test]
    async fn test_search_matches_id() {
        let mock_server = MockServer::start().await;
        let index_json = serde_json::json!({
            "registry_url": mock_server.uri(),
            "plugins": [{
                "id": "com.test.search",
                "name": "Search Target",
                "version": "1.0.0",
                "description": "A searchable plugin",
                "download_url": "/plugins/search.tar.gz",
                "checksum_sha256": "cafe01",
                "manifest": {}
            }]
        });

        Mock::given(method("GET"))
            .and(path("/index.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&index_json))
            .mount(&mock_server)
            .await;

        let client = RegistryClient::new(&mock_server.uri());
        let results = client.search("search").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "com.test.search");
    }

    #[tokio::test]
    async fn test_search_no_match() {
        let mock_server = MockServer::start().await;
        let index_json = serde_json::json!({
            "registry_url": mock_server.uri(),
            "plugins": [{
                "id": "com.test.alpha",
                "name": "Alpha",
                "version": "1.0.0",
                "description": "First plugin",
                "download_url": "/plugins/alpha.tar.gz",
                "checksum_sha256": "aaaa",
                "manifest": {}
            }]
        });

        Mock::given(method("GET"))
            .and(path("/index.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&index_json))
            .mount(&mock_server)
            .await;

        let client = RegistryClient::new(&mock_server.uri());
        let results = client.search("nonexistent").await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_download_checksum_ok() {
        let mock_server = MockServer::start().await;
        let content = b"plugin archive content";
        let checksum = hex::encode(sha2::Sha256::digest(content));

        Mock::given(method("GET"))
            .and(path("/plugins/valid.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(content))
            .mount(&mock_server)
            .await;

        let entry = RegistryPluginEntry {
            id: "com.test.dl".to_string(),
            name: "Download Test".to_string(),
            version: "1.0.0".to_string(),
            description: "Testing download".to_string(),
            author: None,
            download_url: format!("{}/plugins/valid.tar.gz", mock_server.uri()),
            checksum_sha256: checksum,
            manifest: serde_json::json!({}),
        };

        let client = RegistryClient::new(&mock_server.uri());
        let bytes = client.download(&entry).await.unwrap();
        assert_eq!(bytes, content);
    }

    #[tokio::test]
    async fn test_download_checksum_mismatch() {
        let mock_server = MockServer::start().await;
        let content = b"plugin archive content";
        // Intentionally wrong checksum
        let wrong_checksum = hex::encode(sha2::Sha256::digest(b"different content"));

        Mock::given(method("GET"))
            .and(path("/plugins/badsum.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(content))
            .mount(&mock_server)
            .await;

        let entry = RegistryPluginEntry {
            id: "com.test.badsum".to_string(),
            name: "Bad Checksum".to_string(),
            version: "1.0.0".to_string(),
            description: "Testing checksum failure".to_string(),
            author: None,
            download_url: format!("{}/plugins/badsum.tar.gz", mock_server.uri()),
            checksum_sha256: wrong_checksum,
            manifest: serde_json::json!({}),
        };

        let client = RegistryClient::new(&mock_server.uri());
        let result = client.download(&entry).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Checksum mismatch"));
    }

    #[tokio::test]
    async fn test_fetch_index_404() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/index.json"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let client = RegistryClient::new(&mock_server.uri());
        let result = client.fetch_index().await;
        assert!(result.is_err());
    }
}
