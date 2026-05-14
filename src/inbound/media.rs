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
/// Stub implementation — returns placeholder descriptions.
/// Full implementation would:
/// 1. Resolve the best provider for each capability
/// 2. Apply image optimisation (resize, compress)
/// 3. Call vision/STT providers
/// 4. Cache results
/// 5. Format combined text for agent context
pub struct MediaUnderstandingPipeline {
    // Future: provider registry, config, cache
}

impl MediaUnderstandingPipeline {
    pub fn new() -> Self {
        Self {}
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
        // Stub: return placeholder descriptions.
        // Full implementation would call the appropriate provider.
        let description = match capability {
            MediaCapability::Image => format!(
                "[Image attachment: {} ({} bytes)]",
                attachment.filename, attachment.size
            ),
            MediaCapability::Audio => format!(
                "[Audio attachment: {} ({} bytes)]",
                attachment.filename, attachment.size
            ),
            MediaCapability::Video => format!(
                "[Video attachment: {} ({} bytes)]",
                attachment.filename, attachment.size
            ),
            MediaCapability::File => format!(
                "[File attachment: {} ({} bytes)]",
                attachment.filename, attachment.size
            ),
        };

        AttachmentResult {
            attachment_id: attachment.id.to_string(),
            capability,
            description,
            transcript: None,
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
        let result = pipeline.process_attachment(&attachment, MediaCapability::Image).await;
        assert!(result.description.contains("Image"));
    }

    #[tokio::test]
    async fn test_format_combined() {
        let results = vec![
            AttachmentResult {
                attachment_id: "a1".to_string(),
                capability: MediaCapability::Image,
                description: "An image of a cat".to_string(),
                transcript: None,
            },
        ];
        let text = MediaUnderstandingPipeline::format_combined_text(&results);
        assert!(text.contains("attachments"));
        assert!(text.contains("cat"));
    }
}
