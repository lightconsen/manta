//! Structured request context for tracing and logging.
//!
//! [`RequestContext`] captures the identity and tracing identifiers that
//! should follow a request across all async boundaries.  Attach it at
//! the **entry point** (WebSocket handler, webhook receiver, …) and the
//! fields will propagate automatically to every `#[instrument]`-ed
//! function in the call tree.
//!
//! # Usage
//!
//! ```ignore
//! let ctx = RequestContext::new("session-1", "user-42");
//! let _guard = ctx.attach_to_span().entered();
//!
//! // All tracing events and instrumented functions below this
//! // point will carry session_id and user_id fields.
//! ```

use crate::core::models::Id;
use tracing::Span;

/// Contextual metadata that follows a request across async boundaries.
///
/// Each field is attached to the current [`tracing::Span`] so that every
/// `info!` / `warn!` / `#[instrument]` call automatically carries these
/// values in its output (JSON log lines, etc.).
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// Opaque trace identifier — generated once at the entry point.
    pub trace_id: String,
    /// Conversation / session identifier (if available).
    pub session_id: Option<String>,
    /// User or caller identifier (if available).
    pub user_id: Option<String>,
    /// The primary entity being operated on (if applicable).
    pub entity_id: Option<Id>,
}

impl RequestContext {
    /// Create a new context with a fresh `trace_id`.
    pub fn new(session_id: Option<String>, user_id: Option<String>) -> Self {
        Self {
            trace_id: uuid::Uuid::new_v4().to_string(),
            session_id,
            user_id,
            entity_id: None,
        }
    }

    /// Set the entity this request is operating on.
    pub fn with_entity(mut self, entity_id: Id) -> Self {
        self.entity_id = Some(entity_id);
        self
    }

    /// Attach this context to the current tracing span.
    ///
    /// Returns the span — call `.entered()` on it to activate:
    ///
    /// ```ignore
    /// let _guard = ctx.attach_to_span().entered();
    /// ```
    ///
    /// The guard lives for the scope of the request; when it drops the
    /// span is exited.
    pub fn attach_to_span(&self) -> Span {
        let entity_display = self
            .entity_id
            .as_ref()
            .map(|id| id.to_string())
            .unwrap_or_default();

        tracing::info_span!(
            "request",
            trace_id = %self.trace_id,
            session_id = %self.session_id.as_deref().unwrap_or("-"),
            user_id = %self.user_id.as_deref().unwrap_or("-"),
            entity_id = %entity_display,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_generates_trace_id() {
        let ctx1 = RequestContext::new(Some("s1".into()), Some("u1".into()));
        let ctx2 = RequestContext::new(Some("s1".into()), Some("u1".into()));
        // Each context gets a unique trace_id
        assert_ne!(ctx1.trace_id, ctx2.trace_id);
    }

    #[test]
    fn test_with_entity() {
        let ctx = RequestContext::new(Some("s1".into()), None).with_entity(Id::new());
        assert!(ctx.entity_id.is_some());
    }

    #[test]
    fn test_attach_to_span_returns_span() {
        let ctx = RequestContext::new(Some("sess".into()), Some("usr".into()));
        let span = ctx.attach_to_span();
        // Just verify it creates a span without panicking
        let _child = tracing::info_span!(parent: &span, "child");
    }
}
