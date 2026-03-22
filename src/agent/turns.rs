//! Thread + Turn model for conversation branching and undo.
//!
//! A [`Thread`] is a named conversation branch inside a session.  Each
//! [`Turn`] records one user→assistant exchange along with its lifecycle
//! state, allowing turn-level rollback without losing the rest of the
//! conversation.
//!
//! # Relationship to [`super::context::Context`]
//!
//! `Context` manages the raw `Vec<Message>` window sent to the provider.
//! `Thread` wraps a `Context` and adds:
//! - Append-only turn log (`Vec<Turn>`) for rollback
//! - `undo_last_turn()` — removes the last pending/complete turn
//! - Named thread identity for multi-task sessions

use crate::providers::Message;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Lifecycle state of a single turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    /// Waiting to be processed.
    Pending,
    /// Currently being processed by the agent.
    Running,
    /// Completed successfully.
    Complete,
    /// Processing was interrupted (e.g. by a Cancel command).
    Interrupted,
    /// An error occurred during processing.
    Error,
}

/// One user→assistant exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    /// Sequential index within the thread (0-based).
    pub index: usize,
    /// The user's input message.
    pub user_message: String,
    /// The assistant's reply (empty while still running).
    pub assistant_response: String,
    /// Current lifecycle state.
    pub state: TurnState,
    /// When this turn was created.
    pub created_at: SystemTime,
    /// When this turn last changed state.
    pub updated_at: SystemTime,
}

impl Turn {
    /// Create a new turn in the `Pending` state.
    pub fn new(index: usize, user_message: impl Into<String>) -> Self {
        let now = SystemTime::now();
        Self {
            index,
            user_message: user_message.into(),
            assistant_response: String::new(),
            state: TurnState::Pending,
            created_at: now,
            updated_at: now,
        }
    }

    /// Transition to the `Running` state.
    pub fn start(&mut self) {
        self.state = TurnState::Running;
        self.updated_at = SystemTime::now();
    }

    /// Record a completed response and transition to `Complete`.
    pub fn complete(&mut self, response: impl Into<String>) {
        self.assistant_response = response.into();
        self.state = TurnState::Complete;
        self.updated_at = SystemTime::now();
    }

    /// Transition to the `Interrupted` state.
    pub fn interrupt(&mut self) {
        self.state = TurnState::Interrupted;
        self.updated_at = SystemTime::now();
    }

    /// Transition to the `Error` state.
    pub fn mark_error(&mut self) {
        self.state = TurnState::Error;
        self.updated_at = SystemTime::now();
    }
}

/// A named conversation branch holding an ordered log of [`Turn`]s.
///
/// The thread owns a [`super::context::Context`] (the sliding message window
/// sent to the provider) and additionally keeps the full turn log for undo.
#[derive(Debug)]
pub struct Thread {
    /// Thread identifier (e.g. `"main"` or `uuid`).
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Ordered turn log.
    pub turns: Vec<Turn>,
    /// Raw message context for the provider.
    pub context: super::context::Context,
    /// When the thread was created.
    pub created_at: SystemTime,
}

impl Thread {
    /// Create a new thread.
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        system_prompt: impl Into<String>,
        max_tokens: usize,
    ) -> Self {
        let id_str = id.into();
        let context = super::context::Context::new(id_str.clone(), system_prompt, max_tokens);
        Self {
            id: id_str,
            label: label.into(),
            turns: Vec::new(),
            context,
            created_at: SystemTime::now(),
        }
    }

    /// Create a Thread from a pre-built Context (used by Agent integration).
    ///
    /// Unlike [`Thread::new`], which constructs its own `Context`, this
    /// constructor accepts an existing `Context` that already contains a system
    /// prompt, token limits, and any initial messages.  The turn log starts
    /// empty regardless.
    pub fn from_context(
        id: impl Into<String>,
        label: impl Into<String>,
        context: super::context::Context,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            turns: Vec::new(),
            context,
            created_at: SystemTime::now(),
        }
    }

    /// Number of turns recorded.
    pub fn turn_count(&self) -> usize {
        self.turns.len()
    }

    /// Append a new `Pending` turn for `user_message`.
    pub fn push_turn(&mut self, user_message: impl Into<String>) -> usize {
        let index = self.turns.len();
        self.turns.push(Turn::new(index, user_message));
        index
    }

    /// Undo the most recent turn by removing it from the turn log and the
    /// underlying context window.
    ///
    /// Returns `true` if a turn was removed, `false` if the thread was empty.
    pub fn undo_last_turn(&mut self) -> bool {
        match self.turns.pop() {
            None => false,
            Some(turn) => {
                // Mirror the undo in the context by stripping the last
                // user message plus any subsequent messages (assistant reply
                // and tool call/result pairs).
                self.remove_turn_from_context(&turn.user_message);
                true
            }
        }
    }

    // ── Private ──────────────────────────────────────────────────────────────

    /// Remove the last occurrence of a user message with `content` from the
    /// context, along with everything that followed it (assistant reply, tool
    /// calls, tool results).
    fn remove_turn_from_context(&mut self, user_content: &str) {
        let history: &[Message] = self.context.history();
        // Find the last user message that matches.
        let Some(pos) = history
            .iter()
            .rposition(|m| m.role == crate::providers::Role::User && m.content == user_content)
        else {
            return;
        };
        // Collect the new message list: everything before `pos`.
        let kept: Vec<Message> = history[..pos].to_vec();
        self.context.replace_messages(kept);
    }
}

/// Manages all [`Thread`]s for a session.
#[derive(Debug, Default)]
pub struct ThreadManager {
    threads: Vec<Thread>,
}

impl ThreadManager {
    /// Create a new, empty manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a thread, returning its index.
    pub fn push(&mut self, thread: Thread) -> usize {
        let idx = self.threads.len();
        self.threads.push(thread);
        idx
    }

    /// Get a thread by ID (immutable).
    pub fn get(&self, id: &str) -> Option<&Thread> {
        self.threads.iter().find(|t| t.id == id)
    }

    /// Get a thread by ID (mutable).
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == id)
    }

    /// Undo the last turn in the named thread.  Returns `true` if successful.
    pub fn undo(&mut self, thread_id: &str) -> bool {
        self.get_mut(thread_id).map(|t| t.undo_last_turn()).unwrap_or(false)
    }

    /// List all thread IDs.
    pub fn ids(&self) -> Vec<&str> {
        self.threads.iter().map(|t| t.id.as_str()).collect()
    }

    /// Total number of threads.
    pub fn len(&self) -> usize {
        self.threads.len()
    }

    /// Returns `true` if there are no threads.
    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_thread() -> Thread {
        Thread::new("test", "Test Thread", "You are helpful.", 100_000)
    }

    #[test]
    fn test_turn_lifecycle() {
        let mut turn = Turn::new(0, "Hello");
        assert_eq!(turn.state, TurnState::Pending);
        turn.start();
        assert_eq!(turn.state, TurnState::Running);
        turn.complete("Hi there!");
        assert_eq!(turn.state, TurnState::Complete);
        assert_eq!(turn.assistant_response, "Hi there!");
    }

    #[test]
    fn test_thread_undo_empty() {
        let mut thread = make_thread();
        assert!(!thread.undo_last_turn());
    }

    #[test]
    fn test_thread_push_and_undo() {
        let mut thread = make_thread();
        let idx = thread.push_turn("What is 2+2?");
        assert_eq!(idx, 0);
        assert_eq!(thread.turn_count(), 1);

        // Add user message to context to mirror what the agent loop does.
        thread.context.add_message(crate::providers::Message::user("What is 2+2?"));
        thread.context.add_message(crate::providers::Message::assistant("4"));

        assert!(thread.undo_last_turn());
        assert_eq!(thread.turn_count(), 0);
        // Context should be empty after undo.
        assert_eq!(thread.context.message_count(), 0);
    }

    #[test]
    fn test_thread_manager_undo() {
        let mut mgr = ThreadManager::new();
        mgr.push(make_thread());
        // Undo on an unknown id returns false.
        assert!(!mgr.undo("nonexistent"));
        // Undo on empty thread returns false.
        assert!(!mgr.undo("test"));
    }
}
