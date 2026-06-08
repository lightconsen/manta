//! Screenshot encoding optimization — resize and compress screenshots
//! based on network conditions to reduce payload size.
//!
//! On macOS `sips` is already used inline.  This module provides
//! cross-platform encoding for Linux (X11/Wayland) and Windows.
//!
//! # Usage
//!
//! ```rust,no_run
//! use syscity::computer::screenshot_encoder::{ScreenshotEncoder, NetworkCondition};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let encoder = ScreenshotEncoder::detect().await?;
//! let optimized = encoder.encode("/tmp/capture.png", NetworkCondition::Remote).await?;
//! # Ok(())
//! # }
//! ```

use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{info, warn};

/// Detected network quality drives compression aggressiveness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkCondition {
    /// Local loopback / same machine — minimal compression, keep quality.
    Local,
    /// LAN or fast internet — moderate compression.
    Normal,
    /// Slow / metered / remote — aggressive compression.
    Remote,
}

impl NetworkCondition {
    /// Detect from environment or fallback to Normal.
    pub fn detect() -> Self {
        // If a remote endpoint is configured, assume Remote.
        if std::env::var("SYSCITY_REMOTE_ENDPOINT").is_ok() {
            return Self::Remote;
        }
        // If explicitly local.
        if std::env::var("SYSCITY_LOCAL_MODE").is_ok() {
            return Self::Local;
        }
        Self::Normal
    }

    /// Target maximum width in pixels.
    pub fn max_width(self) -> u32 {
        match self {
            Self::Local => 1920,
            Self::Normal => 1600,
            Self::Remote => 1280,
        }
    }

    /// JPEG quality (1-100).
    pub fn jpeg_quality(self) -> u8 {
        match self {
            Self::Local => 90,
            Self::Normal => 80,
            Self::Remote => 65,
        }
    }

    /// Preferred output format.
    pub fn preferred_format(self) -> &'static str {
        match self {
            Self::Local => "png",
            _ => "jpeg",
        }
    }
}

/// Backend used for image encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeBackend {
    /// ImageMagick `convert`.
    ImageMagick,
    /// ffmpeg with image2 filter.
    Ffmpeg,
    /// macOS `sips` (already used inline in macos screenshot tool).
    Sips,
}

/// Cross-platform screenshot encoder.
#[derive(Debug, Clone)]
pub struct ScreenshotEncoder {
    backend: EncodeBackend,
}

impl ScreenshotEncoder {
    /// Detect the best available backend on this system.
    pub async fn detect() -> Option<Self> {
        if Self::has_cmd("convert").await {
            Some(Self {
                backend: EncodeBackend::ImageMagick,
            })
        } else if Self::has_cmd("ffmpeg").await {
            Some(Self {
                backend: EncodeBackend::Ffmpeg,
            })
        } else if cfg!(target_os = "macos") && Self::has_cmd("sips").await {
            Some(Self {
                backend: EncodeBackend::Sips,
            })
        } else {
            None
        }
    }

    /// Encode / compress a screenshot file in-place.
    ///
    /// Returns the path to the optimized file (may be the same as input if
    /// compression failed or the backend is unavailable).
    pub async fn encode(
        &self,
        input: &Path,
        network: NetworkCondition,
    ) -> crate::computer::Result<PathBuf> {
        let out_path = input.with_extension("opt.jpg");
        let max_w = network.max_width().to_string();
        let quality = network.jpeg_quality().to_string();

        let result = match self.backend {
            EncodeBackend::ImageMagick => {
                Command::new("convert")
                    .arg(input)
                    .arg("-resize")
                    .arg(format!("{}x{}>", max_w, max_w))
                    .arg("-quality")
                    .arg(&quality)
                    .arg("-strip")
                    .arg(&out_path)
                    .output()
                    .await
            }
            EncodeBackend::Ffmpeg => {
                Command::new("ffmpeg")
                    .arg("-y")
                    .arg("-i")
                    .arg(input)
                    .arg("-vf")
                    .arg(format!("scale=min({},iw):-1", max_w))
                    .arg("-q:v")
                    .arg(format!("{}", (100 - network.jpeg_quality()) / 10 + 1))
                    .arg(&out_path)
                    .output()
                    .await
            }
            EncodeBackend::Sips => {
                Command::new("sips")
                    .arg("-Z")
                    .arg(&max_w)
                    .arg("-s")
                    .arg("format")
                    .arg("jpeg")
                    .arg("-s")
                    .arg("formatOptions")
                    .arg(&quality)
                    .arg(input)
                    .arg("--out")
                    .arg(&out_path)
                    .output()
                    .await
            }
        };

        match result {
            Ok(out) if out.status.success() => {
                let in_size = tokio::fs::metadata(input).await.map(|m| m.len()).unwrap_or(0);
                let out_size = tokio::fs::metadata(&out_path).await.map(|m| m.len()).unwrap_or(0);
                info!(
                    "Screenshot encoded: {} → {} bytes ({:.0}% of original, {} @ quality {})",
                    in_size,
                    out_size,
                    if in_size > 0 {
                        (out_size as f64 / in_size as f64) * 100.0
                    } else {
                        0.0
                    },
                    network.preferred_format(),
                    network.jpeg_quality()
                );
                Ok(out_path)
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                warn!("Screenshot encoding failed: {}", stderr);
                Ok(input.to_path_buf())
            }
            Err(e) => {
                warn!("Screenshot encoding command failed: {}", e);
                Ok(input.to_path_buf())
            }
        }
    }

    async fn has_cmd(name: &str) -> bool {
        Command::new("which")
            .arg(name)
            .output()
            .await
            .ok()
            .is_some_and(|o| o.status.success())
    }
}

/// Convenience: encode a screenshot if an encoder is available.
///
/// Returns the optimized path, or the original path if encoding is unavailable
/// or fails.
pub async fn maybe_encode_screenshot(input: &Path) -> PathBuf {
    if let Some(encoder) = ScreenshotEncoder::detect().await {
        let network = NetworkCondition::detect();
        match encoder.encode(input, network).await {
            Ok(path) => path,
            Err(_) => input.to_path_buf(),
        }
    } else {
        input.to_path_buf()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_condition_values() {
        assert_eq!(NetworkCondition::Local.max_width(), 1920);
        assert_eq!(NetworkCondition::Normal.max_width(), 1600);
        assert_eq!(NetworkCondition::Remote.max_width(), 1280);

        assert_eq!(NetworkCondition::Local.jpeg_quality(), 90);
        assert_eq!(NetworkCondition::Normal.jpeg_quality(), 80);
        assert_eq!(NetworkCondition::Remote.jpeg_quality(), 65);
    }

    #[test]
    fn test_network_condition_env() {
        // Default without env vars.
        assert_eq!(NetworkCondition::detect(), NetworkCondition::Normal);
    }

    #[tokio::test]
    async fn test_encoder_detect_does_not_panic() {
        // Just ensure detect() runs without panic.
        let _ = ScreenshotEncoder::detect().await;
    }
}
