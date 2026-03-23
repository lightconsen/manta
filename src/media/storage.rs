//! Local filesystem storage backend for media files.
//!
//! Stores media in a configurable base directory with content-addressed filenames.

use super::{MediaStorage, StorageReference};
use crate::error::{MantaError, Result};
use async_trait::async_trait;
use bytes::Bytes;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, error, info, warn};

/// Local filesystem storage backend.
pub struct LocalMediaStorage {
    base_path: PathBuf,
    public_url_base: Option<String>,
}

impl LocalMediaStorage {
    /// Create a new local storage backend.
    ///
    /// # Arguments
    /// * `base_path` - Directory to store files (created if not exists)
    /// * `public_url_base` - Optional URL prefix for generating public URLs
    pub async fn new(base_path: impl AsRef<Path>, public_url_base: Option<String>) -> Result<Self> {
        let base_path = base_path.as_ref().to_path_buf();

        // Create base directory if needed
        if !base_path.exists() {
            fs::create_dir_all(&base_path).await.map_err(|e| {
                MantaError::Config(crate::error::ConfigError::InvalidValue {
                    key: "media.storage.base_path".to_string(),
                    message: format!("Failed to create media directory: {}", e),
                })
            })?;
            info!("Created media storage directory: {:?}", base_path);
        }

        Ok(Self {
            base_path,
            public_url_base,
        })
    }

    /// Compute content hash for deduplication and verification.
    fn compute_hash(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hex::encode(hasher.finalize())
    }

    /// Generate storage path from hash.
    fn hash_to_path(&self, hash: &str) -> PathBuf {
        // Use first 4 chars as subdirectory for better filesystem performance
        let prefix = &hash[..4];
        let suffix = &hash[4..];
        self.base_path.join(prefix).join(suffix)
    }

    /// Generate relative key from hash.
    fn hash_to_key(&self, hash: &str) -> String {
        format!("{}/{}", &hash[..4], &hash[4..])
    }
}

#[async_trait]
impl MediaStorage for LocalMediaStorage {
    async fn store(&self, _key: &str, data: Bytes, content_type: &str) -> Result<StorageReference> {
        let hash = Self::compute_hash(&data);
        let path = self.hash_to_path(&hash);
        let key = self.hash_to_key(&hash);

        // Create subdirectory if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).await.map_err(|e| {
                    MantaError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to create storage subdirectory: {}", e),
                    ))
                })?;
            }
        }

        // Skip if file already exists (content-addressed)
        if !path.exists() {
            fs::write(&path, &data).await.map_err(|e| {
                MantaError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to write media file: {}", e),
                ))
            })?;
            debug!("Stored media file: {:?} ({} bytes)", path, data.len());
        } else {
            debug!("Media file already exists: {:?}", path);
        }

        // Generate public URL if base URL configured
        let url = self.public_url_base.as_ref().map(|base| {
            format!("{}/{}", base.trim_end_matches('/'), key)
        });

        let mut reference = StorageReference::new(
            "local",
            self.base_path.to_string_lossy(),
            key,
            hash,
            data.len(),
        );

        if let Some(url) = url {
            reference = reference.with_url(url);
        }

        Ok(reference)
    }

    async fn retrieve(&self, reference: &StorageReference) -> Result<Bytes> {
        let path = self.base_path.join(&reference.key);

        if !path.exists() {
            return Err(MantaError::NotFound(format!(
                "Media file not found: {:?}",
                path
            )));
        }

        let data = fs::read(&path).await.map_err(|e| {
            MantaError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to read media file: {}", e),
            ))
        })?;

        // Verify hash
        let actual_hash = Self::compute_hash(&data);
        if actual_hash != reference.hash {
            error!(
                "Hash mismatch for {:?}: expected {}, got {}",
                path, reference.hash, actual_hash
            );
            return Err(MantaError::Validation(
                "Media file integrity check failed".into(),
            ));
        }

        debug!("Retrieved media file: {:?} ({} bytes)", path, data.len());
        Ok(Bytes::from(data))
    }

    async fn delete(&self, reference: &StorageReference) -> Result<()> {
        let path = self.base_path.join(&reference.key);

        if path.exists() {
            fs::remove_file(&path).await.map_err(|e| {
                MantaError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to delete media file: {}", e),
                ))
            })?;
            debug!("Deleted media file: {:?}", path);

            // Try to remove empty parent directory
            if let Some(parent) = path.parent() {
                if parent != self.base_path {
                    if let Ok(entries) = fs::read_dir(parent).await {
                        // Note: read_dir returns an async stream, we'd need to check if empty
                        // For simplicity, we skip this cleanup step
                    }
                }
            }
        } else {
            warn!("Media file not found for deletion: {:?}", path);
        }

        Ok(())
    }

    async fn presigned_url(&self, reference: &StorageReference, expiry_secs: u64) -> Result<String> {
        // For local storage, we just return the public URL if configured
        // In a real implementation, this might generate a JWT-signed URL with expiry

        if let Some(url) = &reference.url {
            Ok(url.clone())
        } else {
            Err(MantaError::Validation(
                "Local storage has no public URL base configured".into(),
            ))
        }
    }

    fn name(&self) -> &str {
        "local"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_storage_roundtrip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = LocalMediaStorage::new(temp_dir.path(), None).await.unwrap();

        let data = Bytes::from("Hello, world!");
        let content_type = "text/plain";

        // Store
        let reference = storage.store("test.txt", data.clone(), content_type).await.unwrap();
        assert_eq!(reference.size, data.len());
        assert_eq!(reference.backend, "local");

        // Retrieve
        let retrieved = storage.retrieve(&reference).await.unwrap();
        assert_eq!(retrieved, data);

        // Delete
        storage.delete(&reference).await.unwrap();

        // Verify deletion
        assert!(storage.retrieve(&reference).await.is_err());
    }

    #[tokio::test]
    async fn test_content_addressed_dedup() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = LocalMediaStorage::new(temp_dir.path(), None).await.unwrap();

        let data = Bytes::from("Same content");

        // Store same content twice with different keys
        let ref1 = storage.store("file1.txt", data.clone(), "text/plain").await.unwrap();
        let ref2 = storage.store("file2.txt", data.clone(), "text/plain").await.unwrap();

        // Same hash = same storage location
        assert_eq!(ref1.hash, ref2.hash);
        assert_eq!(ref1.key, ref2.key);
    }

    #[tokio::test]
    async fn test_hash_verification() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = LocalMediaStorage::new(temp_dir.path(), None).await.unwrap();

        let data = Bytes::from("Test data");
        let reference = storage.store("test.txt", data, "text/plain").await.unwrap();

        // Tamper with stored file
        let path = temp_dir.path().join(&reference.key);
        fs::write(&path, b"tampered data").await.unwrap();

        // Retrieval should fail hash check
        let result = storage.retrieve(&reference).await;
        assert!(result.is_err());
    }
}
