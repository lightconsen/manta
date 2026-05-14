//! Media Understanding Pipeline
//!
//! Pre-processes inbound media attachments (images, audio, video, files)
//! before they reach the agent.  Converts raw media into text descriptions
//! or transcripts that can be injected into the agent context.
//!
//! This is a **stub** framework.  Full implementation requires:
//! - Vision-capable model integration (Claude, GPT-4V, Gemini)
//! - STT providers (Whisper, local CLI)
//! - Image optimisation pipeline (resize, compress, HEIC conversion)
//! - Video frame extraction
//!
//! Design matches OpenClaw's `src/media-understanding/`.

use crate::channels::{Attachment, IncomingMessage};
use base64::{engine::general_purpose, Engine as _};
use std::sync::Arc;

/// Capability types supported by the media understanding pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCapability {
    Image,
    Audio,
    Video,
    File,
}

/// Result of processing a single attachment.
#[derive(Debug, Clone)]
pub struct AttachmentResult {
    pub attachment_id: String,
    pub capability: MediaCapability,
    pub description: String,
    pub transcript: Option<String>,
}

/// Result of processing all attachments in a message.
#[derive(Debug, Clone)]
pub struct MediaUnderstandingResult {
    pub message_id: String,
    pub attachment_results: Vec<AttachmentResult>,
    /// Combined text to inject into the agent context.
    pub combined_text: String,
}

/// Cache for media attachments to avoid re-processing.
#[derive(Debug, Default, Clone)]
pub struct MediaAttachmentCache {
    // Future: store processed results keyed by attachment hash.
}

/// Media understanding pipeline.
///
/// Processes inbound media attachments by routing them to appropriate
/// providers (vision models for images, STT for audio, etc.).
pub struct MediaUnderstandingPipeline {
    /// Optional model router for vision-capable provider queries.
    model_router: Option<Arc<crate::model_router::ModelRouter>>,
}

impl MediaUnderstandingPipeline {
    pub fn new() -> Self {
        Self { model_router: None }
    }

    /// Attach a model router to enable vision model routing.
    pub fn with_model_router(mut self, router: Arc<crate::model_router::ModelRouter>) -> Self {
        self.model_router = Some(router);
        self
    }

    /// Process all attachments on an incoming message.
    ///
    /// Returns `MediaUnderstandingResult` containing text descriptions
    /// that should be injected into the agent context.
    pub async fn process(&self, message: &IncomingMessage) -> MediaUnderstandingResult {
        let mut results = Vec::new();

        for attachment in &message.attachments {
            let capability = Self::classify_attachment(attachment);
            let result = self.process_attachment(attachment, capability).await;
            results.push(result);
        }

        let combined_text = Self::format_combined_text(&results);

        MediaUnderstandingResult {
            message_id: message.id.to_string(),
            attachment_results: results,
            combined_text,
        }
    }

    fn classify_attachment(attachment: &Attachment) -> MediaCapability {
        let mime = attachment.content_type.to_lowercase();
        if mime.starts_with("image/") {
            MediaCapability::Image
        } else if mime.starts_with("audio/") {
            MediaCapability::Audio
        } else if mime.starts_with("video/") {
            MediaCapability::Video
        } else {
            MediaCapability::File
        }
    }

    async fn process_attachment(
        &self,
        attachment: &Attachment,
        capability: MediaCapability,
    ) -> AttachmentResult {
        let description = match capability {
            MediaCapability::Image => {
                if let Some(ref router) = self.model_router {
                    self.describe_image_with_vision(attachment, router).await
                } else {
                    format!(
                        "[Image attachment: {} ({} bytes)]",
                        attachment.filename, attachment.size
                    )
                }
            }
            MediaCapability::Audio => {
                format!("[Audio attachment: {} ({} bytes)]", attachment.filename, attachment.size)
            }
            MediaCapability::Video => {
                format!("[Video attachment: {} ({} bytes)]", attachment.filename, attachment.size)
            }
            MediaCapability::File => {
                format!("[File attachment: {} ({} bytes)]", attachment.filename, attachment.size)
            }
        };

        AttachmentResult {
            attachment_id: attachment.id.to_string(),
            capability,
            description,
            transcript: None,
        }
    }

    /// Route an image to the default provider for description.
    ///
    /// Constructs a prompt that includes the image filename, MIME type, and
    /// either a URL or base64 data URL. Falls back to a placeholder if the
    /// provider call fails.
    async fn describe_image_with_vision(
        &self,
        attachment: &Attachment,
        router: &crate::model_router::ModelRouter,
    ) -> String {
        let image_ref = if let Some(ref url) = attachment.url {
            format!("URL: {}", url)
        } else if let Some(ref data) = attachment.data {
            let b64 = general_purpose::STANDARD.encode(data);
            format!(
                "data:{};base64,{}... ({} bytes)",
                attachment.content_type,
                &b64[..b64.len().min(32)],
                attachment.size
            )
        } else {
            format!("filename: {}", attachment.filename)
        };

        let prompt = format!(
            "Describe the following image briefly.\n\nImage: {} ({})",
            attachment.filename, image_ref
        );

        let messages = vec![crate::providers::Message::user(prompt)];
        match router.complete("default", messages).await {
            Ok(resp) => resp.message.content.trim().to_string(),
            Err(e) => {
                tracing::warn!("Vision provider failed for image {}: {}", attachment.filename, e);
                format!("[Image attachment: {} ({} bytes)]", attachment.filename, attachment.size)
            }
        }
    }

    fn format_combined_text(results: &[AttachmentResult]) -> String {
        if results.is_empty() {
            return String::new();
        }

        let mut parts = vec!["The user sent the following attachments:".to_string()];
        for result in results {
            parts.push(format!("- {}", result.description));
            if let Some(ref transcript) = result.transcript {
                parts.push(format!("  Transcript: {}", transcript));
            }
        }
        parts.join("\n")
    }
}

impl Default for MediaUnderstandingPipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_classify_image() {
        let pipeline = MediaUnderstandingPipeline::new();
        let attachment = Attachment::new("photo.png", "image/png").with_data(vec![1, 2, 3]);
        let result = pipeline
            .process_attachment(&attachment, MediaCapability::Image)
            .await;
        assert!(result.description.contains("Image"));
    }

    #[tokio::test]
    async fn test_format_combined() {
        let results = vec![AttachmentResult {
            attachment_id: "a1".to_string(),
            capability: MediaCapability::Image,
            description: "An image of a cat".to_string(),
            transcript: None,
        }];
        let text = MediaUnderstandingPipeline::format_combined_text(&results);
        assert!(text.contains("attachments"));
        assert!(text.contains("cat"));
    }
}
