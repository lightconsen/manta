//! Visual perception layer — OCR and UI element detection from screenshots.
//!
//! This module provides computer-vision-based fallbacks when native
//! accessibility APIs return empty or incomplete results (games, image-based
//! UIs, remote desktops, etc.).
//!
//! Features:
//! - **RapidOCR** (`ocr_rapid`): ONNX-based text detection + recognition
//! - **OmniParser** (`ui_onnx`): ONNX-based UI element detection (buttons,
//!   inputs, checkboxes, icons, etc.)
//!
//! Both are gated behind the `vision` Cargo feature.

use crate::computer::types::Rect;

/// A block of detected text with its location and confidence.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBlock {
    pub text: String,
    pub confidence: f32,
    pub bounds: Rect,
}

/// A UI element detected visually (as opposed to via accessibility API).
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedElement {
    pub role: String, // "button", "text_field", "checkbox", "icon", etc.
    pub label: Option<String>,
    pub bounds: Rect,
    pub confidence: f32,
}

#[cfg(feature = "vision")]
pub mod model_download;
#[cfg(feature = "vision")]
pub mod ocr_rapid;
#[cfg(feature = "vision")]
pub mod ui_onnx;

#[cfg(feature = "vision")]
mod preprocess;

#[cfg(feature = "vision")]
pub use model_download::{resolve_or_download_vision_models, VisionModelPaths};
#[cfg(feature = "vision")]
pub use preprocess::{decode_screenshot, image_to_nchw_tensor, normalize_image, resize_with_pad};
