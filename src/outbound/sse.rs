//! Server-Sent Events (SSE) Streamer
//!
//! Streams agent output tokens and events to connected HTTP clients
//! using the SSE protocol.  This is the OpenClaw equivalent of
//! streaming response handlers.
//!
//! Stub — full implementation would manage per-session Event streams
//! and handle back-pressure.

use serde::{Deserialize, Serialize};

/// A single SSE event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum SseEvent {
    /// A token chunk from the LLM stream.
    Token { text: String },
    /// A tool call started.
    ToolStart { name: String },
    /// A tool call finished.
    ToolEnd { name: String, result: serde_json::Value },
    /// The stream finished.
    Done,
    /// An error occurred.
    Error { message: String },
    /// A heartbeat to keep the connection alive.
    Heartbeat,
}

/// SSE streamer manages active SSE connections per session.
pub struct SseStreamer {
    // Future: HashMap<session_id, Vec<Sender>>
}

impl SseStreamer {
    pub fn new() -> Self {
        Self {}
    }

    /// Send an SSE event to all subscribers for a session.
    pub async fn send(&self, _session_id: &str, _event: SseEvent) {
        // Stub: would look up subscribers and broadcast.
    }

    /// Subscribe a new client to a session's SSE stream.
    pub async fn subscribe(&self, _session_id: &str) {
        // Stub: would create a channel and return a receiver.
    }
}

impl Default for SseStreamer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sse_event_serialization() {
        let event = SseEvent::Token {
            text: "hello".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("token"));
        assert!(json.contains("hello"));
    }
}
