//! Queue Mode Resolver
//!
//! Determines how an incoming message should interact with an ongoing
//! agent execution:
//!
//! - `Interrupt` — New message stops the current agent turn and starts fresh.
//! - `Steer`     — New message is injected into the running context as user
//!                 guidance (the agent changes direction mid-flight).
//! - `FollowUp`  — Collect multiple messages, then process them as a batch.
//! - `Collect`   — Accumulate messages until an explicit trigger.
//!
//! All five modes are now wired into the resolver heuristic.

use crate::channels::IncomingMessage;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Queue mode for message handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueMode {
    /// Interrupt the current agent execution and start a new turn.
    Interrupt,
    /// Steer the running agent (inject guidance mid-flight).
    Steer,
    /// Follow-up: collect messages and process as a batch.
    FollowUp,
    /// Collect messages until an explicit trigger.
    Collect,
    /// No special queue behaviour (default single-turn).
    Normal,
}

impl Default for QueueMode {
    fn default() -> Self {
        QueueMode::Normal
    }
}

/// Per-session tracking for queue-mode heuristics.
#[derive(Debug, Clone)]
struct SessionTiming {
    last_message_at: Instant,
    last_user_id: String,
    message_count: u32,
}

/// Queue mode resolver with per-session timing heuristics.
pub struct QueueModeResolver {
    /// session_id -> last message timing
    sessions: Arc<RwLock<HashMap<String, SessionTiming>>>,
    /// Time window for FollowUp detection (messages within this window are batched).
    follow_up_window: Duration,
}

impl QueueModeResolver {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            follow_up_window: Duration::from_secs(5),
        }
    }

    /// Resolve the queue mode for an incoming message.
    pub async fn resolve(&self, message: &IncomingMessage) -> QueueMode {
        let content = message.content.trim();
        let session_id = message.conversation_id.0.clone();
        let user_id = message.user_id.0.clone();
        let now = Instant::now();

        // ── Explicit interrupt markers ─────────────────────────────────────
        if content.starts_with("!stop")
            || content.starts_with("!interrupt")
            || content.starts_with("/stop")
        {
            self.update_session(&session_id, &user_id, now).await;
            return QueueMode::Interrupt;
        }

        // ── Collect mode trigger ───────────────────────────────────────────
        if content.starts_with("/done") || content.starts_with("!done") {
            self.update_session(&session_id, &user_id, now).await;
            return QueueMode::Collect;
        }

        // ── Steer mode — guidance prefixed with "!" (but not interrupt) ────
        if content.starts_with('!') {
            self.update_session(&session_id, &user_id, now).await;
            return QueueMode::Steer;
        }

        // ── FollowUp detection — rapid succession from same user ───────────
        {
            let sessions = self.sessions.read().await;
            if let Some(timing) = sessions.get(&session_id) {
                let within_window =
                    now.duration_since(timing.last_message_at) < self.follow_up_window;
                let same_user = timing.last_user_id == user_id;
                if within_window && same_user && timing.message_count >= 1 {
                    drop(sessions);
                    self.update_session(&session_id, &user_id, now).await;
                    return QueueMode::FollowUp;
                }
            }
        }

        // Default: Normal single-turn processing
        self.update_session(&session_id, &user_id, now).await;
        QueueMode::Normal
    }

    /// Reset timing for a session (e.g. after /new or explicit flush).
    pub async fn reset_session(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
    }

    /// Update session timing state.
    async fn update_session(&self, session_id: &str, user_id: &str, now: Instant) {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .entry(session_id.to_string())
            .or_insert(SessionTiming {
                last_message_at: now,
                last_user_id: user_id.to_string(),
                message_count: 0,
            });
        entry.last_message_at = now;
        entry.last_user_id = user_id.to_string();
        entry.message_count += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_interrupt_marker() {
        let resolver = QueueModeResolver::new();
        let msg = IncomingMessage::new("u1", "s1", "!stop");
        assert_eq!(resolver.resolve(&msg).await, QueueMode::Interrupt);
    }

    #[tokio::test]
    async fn test_normal_message() {
        let resolver = QueueModeResolver::new();
        let msg = IncomingMessage::new("u1", "s1", "hello");
        assert_eq!(resolver.resolve(&msg).await, QueueMode::Normal);
    }

    #[tokio::test]
    async fn test_steer_mode() {
        let resolver = QueueModeResolver::new();
        let msg = IncomingMessage::new("u1", "s1", "!use rust instead");
        assert_eq!(resolver.resolve(&msg).await, QueueMode::Steer);
    }

    #[tokio::test]
    async fn test_collect_trigger() {
        let resolver = QueueModeResolver::new();
        let msg = IncomingMessage::new("u1", "s1", "/done");
        assert_eq!(resolver.resolve(&msg).await, QueueMode::Collect);
    }

    #[tokio::test]
    async fn test_followup_same_session() {
        let resolver = QueueModeResolver::new();
        // First message in session → Normal
        let msg1 = IncomingMessage::new("u1", "s1", "hello");
        assert_eq!(resolver.resolve(&msg1).await, QueueMode::Normal);

        // Immediate second message from same user → FollowUp
        let msg2 = IncomingMessage::new("u1", "s1", "and also");
        assert_eq!(resolver.resolve(&msg2).await, QueueMode::FollowUp);
    }

    #[tokio::test]
    async fn test_reset_session() {
        let resolver = QueueModeResolver::new();
        let msg1 = IncomingMessage::new("u1", "s1", "hello");
        assert_eq!(resolver.resolve(&msg1).await, QueueMode::Normal);

        resolver.reset_session("s1").await;

        let msg2 = IncomingMessage::new("u1", "s1", "again");
        assert_eq!(resolver.resolve(&msg2).await, QueueMode::Normal);
    }
}
