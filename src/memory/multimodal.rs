//! Multimodal File Storage for Syscity Memory
//!
//! Stores image and audio files in workspace subdirectories:
//! - `memory/images/` — image files
//! - `memory/audio/` — audio files
//!
//! Features:
//! - File classification by extension and MIME type
//! - Size limits per file
//! - SQLite metadata tracking
//! - Glob-based file scanning

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::info;

/// Supported image extensions.
pub const IMAGE_EXTENSIONS: &[&str] = &[".jpg", ".jpeg", ".png", ".webp", ".gif", ".heic", ".heif"];
/// Supported audio extensions.
pub const AUDIO_EXTENSIONS: &[&str] = &[".mp3", ".wav", ".ogg", ".opus", ".m4a", ".aac", ".flac"];

/// Default max file size (10 MB).
pub const DEFAULT_MEMORY_MULTIMODAL_MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// A single multimodal modality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryMultimodalModality {
    Image,
    Audio,
}

impl std::fmt::Display for MemoryMultimodalModality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryMultimodalModality::Image => write!(f, "image"),
            MemoryMultimodalModality::Audio => write!(f, "audio"),
        }
    }
}

/// Configuration for multimodal storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMultimodalConfig {
    /// Whether multimodal storage is enabled.
    pub enabled: bool,
    /// Which modalities to enable.
    pub modalities: Vec<MemoryMultimodalModality>,
    /// Max file size in bytes.
    pub max_file_bytes: u64,
}

impl Default for MemoryMultimodalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            modalities: vec![
                MemoryMultimodalModality::Image,
                MemoryMultimodalModality::Audio,
            ],
            max_file_bytes: DEFAULT_MEMORY_MULTIMODAL_MAX_FILE_BYTES,
        }
    }
}

/// Metadata for a stored multimodal file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultimodalFileEntry {
    /// Unique ID for the file.
    pub id: String,
    /// Original filename.
    pub filename: String,
    /// Stored path relative to workspace.
    pub relative_path: String,
    /// File size in bytes.
    pub size: u64,
    /// Modality (image or audio).
    pub modality: MemoryMultimodalModality,
    /// MIME type.
    pub content_type: String,
    /// When the file was stored.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Optional human-readable label for LLM reference.
    pub label: Option<String>,
}

/// Guess MIME type from file extension.
fn guess_mime_from_extension(ext: &str) -> String {
    match ext.to_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "opus" => "audio/opus",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Classification result for a file path.
#[derive(Debug, Clone)]
pub struct FileClassification {
    pub modality: MemoryMultimodalModality,
    pub extension: String,
}

/// Classify a file by its path.
pub fn classify_multimodal_file(
    file_path: impl AsRef<Path>,
    config: &MemoryMultimodalConfig,
) -> Option<FileClassification> {
    let path = file_path.as_ref();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext.is_empty() {
        return None;
    }

    let ext_with_dot = format!(".{}", ext);

    if IMAGE_EXTENSIONS.contains(&ext_with_dot.as_str())
        && config.modalities.contains(&MemoryMultimodalModality::Image)
    {
        return Some(FileClassification {
            modality: MemoryMultimodalModality::Image,
            extension: ext,
        });
    }

    if AUDIO_EXTENSIONS.contains(&ext_with_dot.as_str())
        && config.modalities.contains(&MemoryMultimodalModality::Audio)
    {
        return Some(FileClassification {
            modality: MemoryMultimodalModality::Audio,
            extension: ext,
        });
    }

    None
}

/// Build a glob pattern for scanning a modality directory.
pub fn build_multimodal_glob(modality: MemoryMultimodalModality) -> String {
    let exts = match modality {
        MemoryMultimodalModality::Image => IMAGE_EXTENSIONS,
        MemoryMultimodalModality::Audio => AUDIO_EXTENSIONS,
    };
    let patterns: Vec<String> = exts
        .iter()
        .map(|e| format!("*{}", e.to_lowercase()))
        .collect();
    format!("{{{}}}", patterns.join(","))
}

/// Multimodal file storage service.
#[derive(Debug, Clone)]
pub struct MultimodalStore {
    config: MemoryMultimodalConfig,
    workspace_dir: PathBuf,
}

impl MultimodalStore {
    /// Create a new multimodal store.
    pub fn new(workspace_dir: impl Into<PathBuf>, config: MemoryMultimodalConfig) -> Self {
        Self {
            config,
            workspace_dir: workspace_dir.into(),
        }
    }

    /// Store a file from bytes.
    pub async fn store_file(
        &self,
        filename: impl AsRef<str>,
        data: &[u8],
        content_type: impl AsRef<str>,
    ) -> crate::Result<MultimodalFileEntry> {
        if !self.config.enabled {
            return Err(crate::error::SyscityError::Config(
                crate::error::ConfigError::InvalidValue {
                    key: "memory.multimodal.enabled".to_string(),
                    message: "Multimodal storage is disabled".to_string(),
                },
            ));
        }

        let filename = filename.as_ref();
        let classification = classify_multimodal_file(filename, &self.config).ok_or_else(|| {
            crate::error::SyscityError::Config(crate::error::ConfigError::InvalidValue {
                key: "memory.multimodal.file".to_string(),
                message: format!("Unsupported file type: {}", filename),
            })
        })?;

        let size = data.len() as u64;
        if size > self.config.max_file_bytes {
            return Err(crate::error::SyscityError::Config(
                crate::error::ConfigError::InvalidValue {
                    key: "memory.multimodal.max_file_bytes".to_string(),
                    message: format!(
                        "File too large: {} bytes (max: {})",
                        size, self.config.max_file_bytes
                    ),
                },
            ));
        }

        let dir = self.modality_dir(classification.modality);
        fs::create_dir_all(&dir)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: format!("Failed to create multimodal directory: {:?}", dir),
                details: e.to_string(),
            })?;

        let id = uuid::Uuid::new_v4().to_string();
        let stored_name = format!("{}_{}", id, filename);
        let stored_path = dir.join(&stored_name);

        fs::write(&stored_path, data)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: format!("Failed to write multimodal file: {:?}", stored_path),
                details: e.to_string(),
            })?;

        let relative_path = stored_path
            .strip_prefix(&self.workspace_dir)
            .unwrap_or(&stored_path)
            .to_string_lossy()
            .to_string();

        let label = format!("{} file: {}", classification.modality, filename);

        info!(
            "Stored multimodal file: {} ({} bytes, {:?})",
            filename, size, classification.modality
        );

        Ok(MultimodalFileEntry {
            id,
            filename: filename.to_string(),
            relative_path,
            size,
            modality: classification.modality,
            content_type: content_type.as_ref().to_string(),
            created_at: chrono::Utc::now(),
            label: Some(label),
        })
    }

    /// Scan a modality directory and return all files.
    pub async fn scan_modality(
        &self,
        modality: MemoryMultimodalModality,
    ) -> Vec<MultimodalFileEntry> {
        if !self.config.enabled || !self.config.modalities.contains(&modality) {
            return Vec::new();
        }

        let dir = self.modality_dir(modality);
        let exts: HashSet<String> = match modality {
            MemoryMultimodalModality::Image => {
                IMAGE_EXTENSIONS.iter().map(|e| e.to_lowercase()).collect()
            }
            MemoryMultimodalModality::Audio => {
                AUDIO_EXTENSIONS.iter().map(|e| e.to_lowercase()).collect()
            }
        };

        let mut entries = Vec::new();
        let mut read_dir = match fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(_) => return Vec::new(),
        };

        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let path = entry.path();
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let ext_lower = format!(".{}", ext.to_lowercase());
                if exts.contains(&ext_lower) {
                    if let Ok(metadata) = entry.metadata().await {
                        let filename = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        // Extract original filename after UUID prefix
                        let original = filename
                            .split_once('_')
                            .map(|x| x.1)
                            .unwrap_or(&filename)
                            .to_string();
                        let relative_path = path
                            .strip_prefix(&self.workspace_dir)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .to_string();

                        entries.push(MultimodalFileEntry {
                            id: uuid::Uuid::new_v4().to_string(),
                            filename: original.clone(),
                            relative_path,
                            size: metadata.len(),
                            modality,
                            content_type: guess_mime_from_extension(
                                path.extension().and_then(|e| e.to_str()).unwrap_or(""),
                            ),
                            created_at: chrono::DateTime::from(
                                metadata
                                    .modified()
                                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                            ),
                            label: Some(format!("{} file: {}", modality, original)),
                        });
                    }
                }
            }
        }

        entries
    }

    /// Get the storage directory for a modality.
    fn modality_dir(&self, modality: MemoryMultimodalModality) -> PathBuf {
        self.workspace_dir
            .join("memory")
            .join(modality.to_string().to_lowercase() + "s")
    }

    /// Read a stored file's bytes.
    pub async fn read_file(&self, relative_path: impl AsRef<Path>) -> crate::Result<Vec<u8>> {
        let path = self.workspace_dir.join(relative_path.as_ref());
        fs::read(&path)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: format!("Failed to read multimodal file: {:?}", path),
                details: e.to_string(),
            })
    }

    /// Delete a stored file.
    pub async fn delete_file(&self, relative_path: impl AsRef<Path>) -> crate::Result<()> {
        let path = self.workspace_dir.join(relative_path.as_ref());
        fs::remove_file(&path)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: format!("Failed to delete multimodal file: {:?}", path),
                details: e.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_classify_image() {
        let config = MemoryMultimodalConfig::default();
        assert!(classify_multimodal_file("photo.jpg", &config).is_some());
        assert!(classify_multimodal_file("photo.JPG", &config).is_some());
        assert!(classify_multimodal_file("photo.txt", &config).is_none());
    }

    #[test]
    fn test_classify_audio() {
        let config = MemoryMultimodalConfig::default();
        assert!(classify_multimodal_file("song.mp3", &config).is_some());
        assert!(classify_multimodal_file("song.wav", &config).is_some());
    }

    #[test]
    fn test_build_glob() {
        let g = build_multimodal_glob(MemoryMultimodalModality::Image);
        assert!(g.contains("*.jpg"));
        assert!(g.contains("*.png"));
    }

    #[tokio::test]
    async fn test_store_and_read() {
        let dir = tempdir().unwrap();
        let store = MultimodalStore::new(dir.path(), MemoryMultimodalConfig::default());

        let entry = store
            .store_file("test.png", b"fake image data", "image/png")
            .await
            .unwrap();
        assert_eq!(entry.filename, "test.png");
        assert_eq!(entry.size, 15);
        assert!(entry.label.as_ref().unwrap().contains("image"));

        let data = store.read_file(&entry.relative_path).await.unwrap();
        assert_eq!(data, b"fake image data");
    }

    #[tokio::test]
    async fn test_size_limit() {
        let dir = tempdir().unwrap();
        let mut config = MemoryMultimodalConfig::default();
        config.max_file_bytes = 5;
        let store = MultimodalStore::new(dir.path(), config);

        let result = store
            .store_file("big.png", b"this is too big", "image/png")
            .await;
        assert!(result.is_err());
    }
}
