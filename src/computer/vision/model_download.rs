//! ONNX model auto-download — resolve vision models from cache or download.
//!
//! Follows the same pattern as `screen_recorder::resolve_or_download_ffmpeg`:
//! 1. Check cache directory for each model
//! 2. Download missing models from HuggingFace
//! 3. Return paths to all resolved model files
//!
//! Models are cached at `{cache_dir}/syscity/models/vision/`:
//! - macOS:   `~/Library/Caches/syscity/models/vision/`
//! - Linux:   `~/.cache/syscity/models/vision/`
//! - Windows: `{LocalAppData}\syscity\models\vision\`

use std::path::{Path, PathBuf};

/// Paths to all resolved ONNX model files.
#[derive(Debug, Clone)]
pub struct VisionModelPaths {
    /// OmniParser UI element detection model.
    pub omniparser: PathBuf,
    /// RapidOCR text detection model (DBNet).
    pub det: PathBuf,
    /// RapidOCR text recognition model (CRNN).
    pub rec: PathBuf,
    /// RapidOCR direction classification model (optional).
    pub cls: Option<PathBuf>,
}

/// Cache directory for ONNX models: `{cache_dir}/syscity/models/vision/`.
fn model_cache_dir() -> PathBuf {
    let base = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("syscity").join("models").join("vision")
}

/// Resolve ONNX vision models: check cache, download what's missing.
///
/// Returns paths to all required model files.  Models that are already
/// cached are not re-downloaded.  Skips optional models (cls.onnx) that
/// are not found and cannot be downloaded.
pub async fn resolve_or_download_vision_models() -> crate::computer::Result<VisionModelPaths> {
    let cache = model_cache_dir();

    // Ensure cache directory exists
    tokio::fs::create_dir_all(&cache)
        .await
        .map_err(|e| {
            crate::computer::ComputerError::Other(format!(
                "Cannot create model cache dir {}: {}",
                cache.display(),
                e
            ))
        })?;

    // Resolve OmniParser
    let omniparser = resolve_single_model(
        &cache,
        "omniparser.onnx",
        "https://hf-mirror.com/onnx-community/OmniParser-icon_detect_640x640/resolve/main/onnx/model.onnx",
    )
    .await?;

    // Resolve RapidOCR detection
    let det = resolve_single_model(
        &cache,
        "det.onnx",
        "https://hf-mirror.com/monkt/paddleocr-onnx/resolve/main/detection/v5/det.onnx",
    )
    .await?;

    // Resolve RapidOCR recognition
    let rec = resolve_single_model(
        &cache,
        "rec.onnx",
        "https://hf-mirror.com/monkt/paddleocr-onnx/resolve/main/languages/english/rec.onnx",
    )
    .await?;

    // Resolve optional cls model (best-effort)
    let cls = resolve_optional_model(
        &cache,
        "cls.onnx",
        "https://hf-mirror.com/monkt/paddleocr-onnx/resolve/main/languages/english/cls.onnx",
    )
    .await;

    Ok(VisionModelPaths {
        omniparser,
        det,
        rec,
        cls,
    })
}

/// Resolve a single required model: cache hit → return, miss → download.
async fn resolve_single_model(
    cache_dir: &Path,
    filename: &str,
    url: &str,
) -> crate::computer::Result<PathBuf> {
    let path = cache_dir.join(filename);

    // Cache hit: already exists
    if path.exists() {
        tracing::debug!("ONNX model cached: {}", path.display());
        return Ok(path);
    }

    // Cache miss: download
    tracing::info!(
        "Downloading ONNX model '{}' from {} (~{} MB)",
        filename,
        url,
        estimated_size(filename)
    );
    let data = download_bytes(url).await?;

    tokio::fs::write(&path, &data)
        .await
        .map_err(|e| {
            crate::computer::ComputerError::Other(format!(
                "Failed to write model '{}' to {}: {}",
                filename,
                path.display(),
                e
            ))
        })?;

    tracing::info!(
        "ONNX model '{}' saved to {} ({} bytes)",
        filename,
        path.display(),
        data.len()
    );

    Ok(path)
}

/// Resolve an optional model — returns `None` on failure instead of error.
async fn resolve_optional_model(
    cache_dir: &Path,
    filename: &str,
    url: &str,
) -> Option<PathBuf> {
    match resolve_single_model(cache_dir, filename, url).await {
        Ok(path) => Some(path),
        Err(e) => {
            tracing::warn!(
                "Optional ONNX model '{}' not available: {}",
                filename,
                e
            );
            None
        }
    }
}

/// Download raw bytes from a URL (up to 120 s timeout).
async fn download_bytes(url: &str) -> crate::computer::Result<Vec<u8>> {
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .user_agent("syscity/0.1")
        .build()
        .map_err(|e| {
            crate::computer::ComputerError::Other(format!("HTTP client: {}", e))
        })?
        .get(url)
        .send()
        .await
        .map_err(|e| {
            crate::computer::ComputerError::Other(format!(
                "Download failed ({}): {}",
                url, e
            ))
        })?;
    if !response.status().is_success() {
        return Err(crate::computer::ComputerError::Other(format!(
            "Download returned HTTP {} for {}",
            response.status(),
            url
        )));
    }
    response.bytes().await.map(|b| b.to_vec()).map_err(|e| {
        crate::computer::ComputerError::Other(format!("Download body: {}", e))
    })
}

/// Rough size estimate for display purposes.
fn estimated_size(filename: &str) -> &str {
    match filename {
        "omniparser.onnx" => "12",
        "det.onnx" => "88",
        "rec.onnx" => "8",
        "cls.onnx" => "2",
        _ => "?",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_cache_dir_is_absolute() {
        let dir = model_cache_dir();
        assert!(dir.is_absolute(), "cache dir should be absolute: {:?}", dir);
        assert!(dir.ends_with("syscity/models/vision"), "should end with syscity/models/vision: {:?}", dir);
    }

    #[test]
    fn test_estimated_size_known_models() {
        assert_eq!(estimated_size("omniparser.onnx"), "12");
        assert_eq!(estimated_size("det.onnx"), "88");
        assert_eq!(estimated_size("rec.onnx"), "8");
        assert_eq!(estimated_size("cls.onnx"), "2");
    }

    #[test]
    fn test_estimated_size_unknown() {
        assert_eq!(estimated_size("unknown.onnx"), "?");
    }
}
