//! Server-Sent Events (SSE) Streamer
//!
//! Streams agent output tokens and events to connected HTTP clients
//! using the SSE protocol for streaming response handlers.
//!
//! Manages per-session broadcast channels so that multiple clients
//! can subscribe to the same agent turn simultaneously.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, warn};

/// A single SSE event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum SseEvent {
 /// A token chunk from the LLM stream.
    Token { text: String },
 /// A tool call started.
    ToolStart { name: String },
 /// A tool call finished.
    ToolEnd {
        name: String,
        result: serde_json::Value,
    },
 /// The stream finished.
    Done,
 /// An error occurred.
    Error { message: String },
 /// A heartbeat to keep the connection alive.
    Heartbeat,
}

/// SSE streamer manages active SSE connections per session.
pub struct SseStreamer {
 /// session_id -> broadcast sender for that session
    sessions: RwLock<HashMap<String, broadcast::Sender<SseEvent>>>,
 /// Channel capacity for each per-session broadcast
    capacity: usize,
}

impl SseStreamer {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            capacity: 256,
        }
    }

 /// Create with a custom broadcast capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            capacity,
        }
    }

 /// Subscribe a new client to a session's SSE stream.
 ///
 /// Returns a receiver that yields [`SseEvent`]s for this session.
 /// If the session does not yet exist, a new broadcast channel is created.
    pub async fn subscribe(&self, session_id: &str) -> broadcast::Receiver<SseEvent> {
        let mut sessions = self.sessions.write().await;
        let sender = sessions
            .entry(session_id.to_string())
            .or_insert_with(|| broadcast::channel(self.capacity).0);
        debug!(
            "SSE subscriber added for session {} (receivers: {})",
            session_id,
            sender.receiver_count()
        );
        sender.subscribe()
    }

 /// Send an SSE event to all subscribers for a session.
    pub async fn send(&self, session_id: &str, event: SseEvent) {
        let sessions = self.sessions.read().await;
        if let Some(sender) = sessions.get(session_id) {
            if sender.receiver_count() == 0 {
                return;
            }
            if let Err(e) = sender.send(event) {
                warn!("SSE send failed for session {} (no active receivers): {:?}", session_id, e);
            } else {
                debug!("SSE event sent to session {}", session_id);
            }
        }
    }

 /// Clean up sessions with no remaining receivers.
    pub async fn gc(&self) {
        let mut sessions = self.sessions.write().await;
        let before = sessions.len();
        sessions.retain(|_id, sender| sender.receiver_count() > 0);
        let after = sessions.len();
        if before != after {
            debug!("SSE gc: removed {} stale sessions", before - after);
        }
    }

 /// Remove a session explicitly (e.g. after turn completion).
    pub async fn remove_session(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
        debug!("SSE session {} removed", session_id);
    }

 /// Number of active sessions.
    pub async fn session_count(&self) -> usize {
        self.sessions.read().await.len()
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
    use std::sync::Arc;

    #[tokio::test]
    async fn test_subscribe_and_send() {
        let streamer = Arc::new(SseStreamer::new());
        let mut rx = streamer.subscribe("s1").await;

        streamer
            .send("s1", SseEvent::Token { text: "hello".into() })
            .await;

        let evt = rx.recv().await.unwrap();
        match evt {
            SseEvent::Token { text } => assert_eq!(text, "hello"),
            _ => panic!("Expected Token event"),
        }
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let streamer = Arc::new(SseStreamer::new());
        let mut rx1 = streamer.subscribe("s1").await;
        let mut rx2 = streamer.subscribe("s1").await;

        streamer.send("s1", SseEvent::Done).await;

        assert!(matches!(rx1.recv().await.unwrap(), SseEvent::Done));
        assert!(matches!(rx2.recv().await.unwrap(), SseEvent::Done));
    }

    #[tokio::test]
    async fn test_gc_removes_stale_sessions() {
        let streamer = SseStreamer::new();
        {
            let _rx = streamer.subscribe("s1").await;
            assert_eq!(streamer.session_count().await, 1);
        }
 // rx dropped — session should be stale
        streamer.gc().await;
        assert_eq!(streamer.session_count().await, 0);
    }

    #[tokio::test]
    async fn test_sse_event_serialization() {
        let event = SseEvent::Token { text: "hello".to_string() };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("token"));
        assert!(json.contains("hello"));
    }
}
