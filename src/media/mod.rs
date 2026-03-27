//! Media Pipeline for Voice/TTS and Image Processing
//!
//! Provides abstractions for:
//! - Text-to-Speech (TTS) synthesis
//! - Speech-to-Text (STT) transcription
//! - Image generation and vision model support
//! - Media storage (S3-compatible or local filesystem)

use crate::error::{MantaError, Result};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

// ── Media Content Types ──────────────────────────────────────────────────────

/// Represents different types of media content that can flow through the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
pub enum MediaContent {
    /// Plain text content
    Text(String),
    /// Audio data with format metadata
    Audio(AudioData),
    /// Image data with format and dimensions
    Image(ImageData),
    /// Video data (for future expansion)
    Video(VideoData),
}

impl MediaContent {
    /// Returns true if this is text content.
    pub fn is_text(&self) -> bool {
        matches!(self, MediaContent::Text(_))
    }

    /// Returns true if this is audio content.
    pub fn is_audio(&self) -> bool {
        matches!(self, MediaContent::Audio(_))
    }

    /// Returns true if this is image content.
    pub fn is_image(&self) -> bool {
        matches!(self, MediaContent::Image(_))
    }

    /// Get text content if this is text.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MediaContent::Text(t) => Some(t),
            _ => None,
        }
    }
}

// ── Audio Types ───────────────────────────────────────────────────────────────

/// Audio format enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormat {
    Mp3,
    Wav,
    Ogg,
    Flac,
    Aac,
    Opus,
    Pcm,
}

impl fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioFormat::Mp3 => write!(f, "mp3"),
            AudioFormat::Wav => write!(f, "wav"),
            AudioFormat::Ogg => write!(f, "ogg"),
            AudioFormat::Flac => write!(f, "flac"),
            AudioFormat::Aac => write!(f, "aac"),
            AudioFormat::Opus => write!(f, "opus"),
            AudioFormat::Pcm => write!(f, "pcm"),
        }
    }
}

/// Audio data container with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioData {
    /// Raw audio bytes
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    /// Audio format
    pub format: AudioFormat,
    /// Sample rate in Hz (e.g., 44100, 48000)
    pub sample_rate: u32,
    /// Number of channels (1 = mono, 2 = stereo)
    pub channels: u8,
    /// Duration in seconds
    pub duration_secs: f64,
    /// Storage reference if persisted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_ref: Option<StorageReference>,
}

impl AudioData {
    /// Create audio data from raw bytes.
    pub fn new(
        bytes: Vec<u8>,
        format: AudioFormat,
        sample_rate: u32,
        channels: u8,
        duration_secs: f64,
    ) -> Self {
        Self {
            bytes,
            format,
            sample_rate,
            channels,
            duration_secs,
            storage_ref: None,
        }
    }

    /// Size of audio data in bytes.
    pub fn size_bytes(&self) -> usize {
        self.bytes.len()
    }
}

// ── Image Types ───────────────────────────────────────────────────────────────

/// Image format enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    Png,
    Jpeg,
    Webp,
    Gif,
    Bmp,
    Svg,
    Avif,
}

impl fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageFormat::Png => write!(f, "png"),
            ImageFormat::Jpeg => write!(f, "jpeg"),
            ImageFormat::Webp => write!(f, "webp"),
            ImageFormat::Gif => write!(f, "gif"),
            ImageFormat::Bmp => write!(f, "bmp"),
            ImageFormat::Svg => write!(f, "svg"),
            ImageFormat::Avif => write!(f, "avif"),
        }
    }
}

/// Image data container with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageData {
    /// Raw image bytes
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    /// Image format
    pub format: ImageFormat,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Alt text or description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_text: Option<String>,
    /// Storage reference if persisted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_ref: Option<StorageReference>,
}

impl ImageData {
    /// Create image data from raw bytes.
    pub fn new(bytes: Vec<u8>, format: ImageFormat, width: u32, height: u32) -> Self {
        Self {
            bytes,
            format,
            width,
            height,
            alt_text: None,
            storage_ref: None,
        }
    }

    /// Size of image data in bytes.
    pub fn size_bytes(&self) -> usize {
        self.bytes.len()
    }

    /// Set alt text for accessibility.
    pub fn with_alt_text(mut self, alt: impl Into<String>) -> Self {
        self.alt_text = Some(alt.into());
        self
    }
}

/// Video data container (placeholder for future expansion).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoData {
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub duration_secs: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_ref: Option<StorageReference>,
}

// ── Storage Reference ────────────────────────────────────────────────────────

/// Reference to stored media (avoids duplicating large byte arrays).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageReference {
    /// Storage backend (s3, gcs, file, etc.)
    pub backend: String,
    /// Bucket or root path
    pub bucket: String,
    /// Object key or file path
    pub key: String,
    /// Content hash for verification
    pub hash: String,
    /// Content size in bytes
    pub size: usize,
    /// URL for access (if public)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl StorageReference {
    /// Create a new storage reference.
    pub fn new(
        backend: impl Into<String>,
        bucket: impl Into<String>,
        key: impl Into<String>,
        hash: impl Into<String>,
        size: usize,
    ) -> Self {
        Self {
            backend: backend.into(),
            bucket: bucket.into(),
            key: key.into(),
            hash: hash.into(),
            size,
            url: None,
        }
    }

    /// Add public URL.
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
}

// ── Media Attachment ─────────────────────────────────────────────────────────

/// Media attachment for inclusion in messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAttachment {
    /// Unique attachment ID
    pub id: String,
    /// MIME type (e.g., "image/png", "audio/mpeg")
    pub mime_type: String,
    /// Original filename
    pub filename: String,
    /// Content or reference
    #[serde(flatten)]
    pub content: MediaContent,
    /// Size in bytes
    pub size_bytes: usize,
}

impl MediaAttachment {
    /// Create a new media attachment.
    pub fn new(
        id: impl Into<String>,
        mime_type: impl Into<String>,
        filename: impl Into<String>,
        content: MediaContent,
    ) -> Self {
        let size_bytes = match &content {
            MediaContent::Text(t) => t.len(),
            MediaContent::Audio(a) => a.size_bytes(),
            MediaContent::Image(i) => i.size_bytes(),
            MediaContent::Video(v) => v.bytes.len(),
        };

        Self {
            id: id.into(),
            mime_type: mime_type.into(),
            filename: filename.into(),
            content,
            size_bytes,
        }
    }
}

// ── Provider Traits ──────────────────────────────────────────────────────────

/// Text-to-Speech provider trait.
#[async_trait]
pub trait TtsProvider: Send + Sync {
    /// Synthesize text into audio.
    ///
    /// # Arguments
    /// * `text` - The text to speak
    /// * `voice` - Voice identifier (provider-specific)
    /// * `speed` - Speech speed multiplier (0.5 - 2.0)
    async fn synthesize(&self, text: &str, voice: &str, speed: f64) -> Result<AudioData>;

    /// List available voices.
    async fn list_voices(&self) -> Result<Vec<VoiceInfo>>;

    /// Provider name.
    fn name(&self) -> &str;
}

/// Voice information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub language: String,
    pub gender: Option<String>,
    pub preview_url: Option<String>,
}

/// Speech-to-Text provider trait.
#[async_trait]
pub trait SttProvider: Send + Sync {
    /// Transcribe audio to text.
    ///
    /// # Arguments
    /// * `audio` - The audio data to transcribe
    /// * `language` - Expected language code (e.g., "en-US")
    async fn transcribe(
        &self,
        audio: &AudioData,
        language: Option<&str>,
    ) -> Result<TranscriptionResult>;

    /// Provider name.
    fn name(&self) -> &str;
}

/// Transcription result with confidence scores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResult {
    pub text: String,
    pub confidence: f64,
    pub language: Option<String>,
    pub words: Vec<WordTiming>,
    pub duration_secs: f64,
}

/// Word-level timing information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordTiming {
    pub word: String,
    pub start_secs: f64,
    pub end_secs: f64,
    pub confidence: f64,
}

/// Image generation/modification provider trait.
#[async_trait]
pub trait ImageProvider: Send + Sync {
    /// Generate an image from a text prompt.
    ///
    /// # Arguments
    /// * `prompt` - Text description of desired image
    /// * `size` - Target dimensions (width, height)
    /// * `format` - Output format
    async fn generate(
        &self,
        prompt: &str,
        size: (u32, u32),
        format: ImageFormat,
    ) -> Result<ImageData>;

    /// Edit/modify an existing image.
    ///
    /// # Arguments
    /// * `image` - Base image to edit
    /// * `prompt` - Edit instructions
    /// * `mask` - Optional mask for selective editing
    async fn edit(
        &self,
        image: &ImageData,
        prompt: &str,
        mask: Option<&ImageData>,
    ) -> Result<ImageData>;

    /// Create image variations.
    ///
    /// # Arguments
    /// * `image` - Base image
    /// * `n` - Number of variations to generate
    async fn variations(&self, image: &ImageData, n: u32) -> Result<Vec<ImageData>>;

    /// Provider name.
    fn name(&self) -> &str;
}

/// Vision model provider for image understanding.
#[async_trait]
pub trait VisionProvider: Send + Sync {
    /// Describe/analyze an image.
    ///
    /// # Arguments
    /// * `image` - The image to analyze
    /// * `prompt` - Specific question or instructions
    async fn describe(&self, image: &ImageData, prompt: &str) -> Result<String>;

    /// Extract text from image (OCR).
    async fn ocr(&self, image: &ImageData) -> Result<String>;

    /// Provider name.
    fn name(&self) -> &str;
}

// ── Media Storage Trait ───────────────────────────────────────────────────────

/// Storage backend for media files.
#[async_trait]
pub trait MediaStorage: Send + Sync {
    /// Store media and return a reference.
    async fn store(&self, key: &str, data: Bytes, content_type: &str) -> Result<StorageReference>;

    /// Retrieve media by reference.
    async fn retrieve(&self, reference: &StorageReference) -> Result<Bytes>;

    /// Delete media by reference.
    async fn delete(&self, reference: &StorageReference) -> Result<()>;

    /// Generate temporary access URL.
    async fn presigned_url(&self, reference: &StorageReference, expiry_secs: u64)
        -> Result<String>;

    /// Storage backend name.
    fn name(&self) -> &str;
}

// ── Media Pipeline ────────────────────────────────────────────────────────────

/// Central media pipeline coordinator.
pub struct MediaPipeline {
    tts: Option<Arc<dyn TtsProvider>>,
    stt: Option<Arc<dyn SttProvider>>,
    image: Option<Arc<dyn ImageProvider>>,
    vision: Option<Arc<dyn VisionProvider>>,
    storage: Option<Arc<dyn MediaStorage>>,
}

impl MediaPipeline {
    /// Create a new media pipeline (all components optional).
    pub fn new() -> Self {
        Self {
            tts: None,
            stt: None,
            image: None,
            vision: None,
            storage: None,
        }
    }

    /// Add TTS provider.
    pub fn with_tts(mut self, provider: Arc<dyn TtsProvider>) -> Self {
        self.tts = Some(provider);
        self
    }

    /// Add STT provider.
    pub fn with_stt(mut self, provider: Arc<dyn SttProvider>) -> Self {
        self.stt = Some(provider);
        self
    }

    /// Add image generation provider.
    pub fn with_image(mut self, provider: Arc<dyn ImageProvider>) -> Self {
        self.image = Some(provider);
        self
    }

    /// Add vision provider.
    pub fn with_vision(mut self, provider: Arc<dyn VisionProvider>) -> Self {
        self.vision = Some(provider);
        self
    }

    /// Add storage backend.
    pub fn with_storage(mut self, storage: Arc<dyn MediaStorage>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Check if TTS is available.
    pub fn has_tts(&self) -> bool {
        self.tts.is_some()
    }

    /// Check if STT is available.
    pub fn has_stt(&self) -> bool {
        self.stt.is_some()
    }

    /// Check if image generation is available.
    pub fn has_image(&self) -> bool {
        self.image.is_some()
    }

    /// Check if vision is available.
    pub fn has_vision(&self) -> bool {
        self.vision.is_some()
    }

    /// Check if storage is configured.
    pub fn has_storage(&self) -> bool {
        self.storage.is_some()
    }

    /// Synthesize text to speech.
    pub async fn synthesize_speech(
        &self,
        text: &str,
        voice: &str,
        speed: f64,
    ) -> Result<AudioData> {
        match &self.tts {
            Some(provider) => provider.synthesize(text, voice, speed).await,
            None => Err(MantaError::Validation("TTS provider not configured".into())),
        }
    }

    /// Transcribe speech to text.
    pub async fn transcribe_audio(
        &self,
        audio: &AudioData,
        language: Option<&str>,
    ) -> Result<TranscriptionResult> {
        match &self.stt {
            Some(provider) => provider.transcribe(audio, language).await,
            None => Err(MantaError::Validation("STT provider not configured".into())),
        }
    }

    /// Generate image from prompt.
    pub async fn generate_image(
        &self,
        prompt: &str,
        size: (u32, u32),
        format: ImageFormat,
    ) -> Result<ImageData> {
        match &self.image {
            Some(provider) => provider.generate(prompt, size, format).await,
            None => Err(MantaError::Validation("Image provider not configured".into())),
        }
    }

    /// Describe/analyze image.
    pub async fn describe_image(&self, image: &ImageData, prompt: &str) -> Result<String> {
        match &self.vision {
            Some(provider) => provider.describe(image, prompt).await,
            None => Err(MantaError::Validation("Vision provider not configured".into())),
        }
    }

    /// Store media and return reference.
    pub async fn store(
        &self,
        key: &str,
        data: Bytes,
        content_type: &str,
    ) -> Result<StorageReference> {
        match &self.storage {
            Some(storage) => storage.store(key, data, content_type).await,
            None => Err(MantaError::Validation("Media storage not configured".into())),
        }
    }

    /// Retrieve media by reference.
    pub async fn retrieve(&self, reference: &StorageReference) -> Result<Bytes> {
        match &self.storage {
            Some(storage) => storage.retrieve(reference).await,
            None => Err(MantaError::Validation("Media storage not configured".into())),
        }
    }
}

impl Default for MediaPipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_content_types() {
        let text = MediaContent::Text("hello".into());
        assert!(text.is_text());
        assert!(!text.is_audio());

        let audio =
            MediaContent::Audio(AudioData::new(vec![1, 2, 3], AudioFormat::Mp3, 44100, 2, 1.0));
        assert!(audio.is_audio());
        assert!(!audio.is_text());
    }

    #[test]
    fn test_audio_data_creation() {
        let audio = AudioData::new(vec![1, 2, 3, 4, 5], AudioFormat::Wav, 48000, 2, 5.5);
        assert_eq!(audio.size_bytes(), 5);
        assert_eq!(audio.sample_rate, 48000);
        assert_eq!(audio.channels, 2);
    }

    #[test]
    fn test_image_data_creation() {
        let image = ImageData::new(vec![0u8; 1024], ImageFormat::Png, 1920, 1080)
            .with_alt_text("Test image");

        assert_eq!(image.width, 1920);
        assert_eq!(image.height, 1080);
        assert_eq!(image.alt_text, Some("Test image".to_string()));
    }

    #[test]
    fn test_media_attachment_size() {
        let attachment = MediaAttachment::new(
            "123",
            "image/png",
            "test.png",
            MediaContent::Text("hello world".into()),
        );
        assert_eq!(attachment.size_bytes, 11);
    }

    #[test]
    fn test_storage_reference() {
        let storage_ref =
            StorageReference::new("s3", "my-bucket", "path/to/file.mp3", "abc123", 1024)
                .with_url("https://example.com/file.mp3");

        assert_eq!(storage_ref.backend, "s3");
        assert_eq!(storage_ref.url, Some("https://example.com/file.mp3".to_string()));
    }
}
