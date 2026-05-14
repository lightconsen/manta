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
//! This is a **stub** implementation.  Only `Interrupt` is fully wired;
//! `Steer`, `FollowUp`, and `Collect` require deeper agent runtime changes.

use crate::channels::IncomingMessage;

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

/// Queue mode resolver.
///
/// Currently a simple heuristic-based resolver:
/// - Messages starting with "!" or explicit interrupt markers → `Interrupt`
/// - Messages in rapid succession from the same user → `FollowUp` (stub)
/// - Everything else → `Normal`
pub struct QueueModeResolver {
    // Future: track session state (busy/idle) and recent message timing.
}

impl QueueModeResolver {
    pub fn new() -> Self {
        Self {}
    }

    /// Resolve the queue mode for an incoming message.
    pub async fn resolve(&self, message: &IncomingMessage) -> QueueMode {
        let content = message.content.trim();

        // Interrupt markers
        if content.starts_with("!stop")
            || content.starts_with("!interrupt")
            || content.starts_with("/stop")
        {
            return QueueMode::Interrupt;
        }

        // TODO: Steer mode — detect "@bot do X instead" patterns while agent
        // is running.  Requires querying agent busy state.

        // TODO: FollowUp mode — detect rapid succession messages from same
        // user within a time window.  Requires per-session message history.

        // TODO: Collect mode — explicit batch trigger (e.g. "/done").

        QueueMode::Normal
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
}
