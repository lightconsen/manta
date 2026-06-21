//! [`AgentPerceptionAdapter`] — the per-agent facade over the perception
//! pipeline.
//!
//! The adapter is **the only contact surface** an agent has with the
//! perception layer. It owns the per-agent state (current focus, diff
//! baselines, dedup cache, event queue) and presents three usage modes:
//!
//! | Method            | Shape         | When to call                                  |
//! |-------------------|---------------|-----------------------------------------------|
//! | [`now`]           | sync snapshot | "what does the world look like right now"     |
//! | [`next_event`]    | async stream  | "wake me when something interesting happens"  |
//! | [`summarize`]     | LLM-generated | "give me a sentence I can drop into a prompt" |
//!
//! Implementations are constructed via
//! [`super::PerceptionContext::new_adapter`] (preferred) or directly
//! with [`MinimalAdapter::new`].
//!
//! [`now`]: AgentPerceptionAdapter::now
//! [`next_event`]: AgentPerceptionAdapter::next_event
//! [`summarize`]: AgentPerceptionAdapter::summarize

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::perception::{Event, Focus, Snapshot};

/// Errors returned by [`AgentPerceptionAdapter`] methods.
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// The adapter has been shut down — no further operations will succeed.
    #[error("perception adapter is shut down")]
    Shutdown,

    /// The pipeline has been torn down upstream (hub or processor closed).
    #[error("perception pipeline closed")]
    PipelineClosed,

    /// LLM-side failure during [`AgentPerceptionAdapter::summarize`].
    #[error("summarizer error: {0}")]
    Summarizer(String),

    /// Catch-all for adapter-internal failures.
    #[error("{0}")]
    Other(String),
}

/// Minimal LLM interface used by [`AgentPerceptionAdapter::summarize`].
///
/// We intentionally don't depend on `crate::model_router::LlmProvider`
/// here — perception is a low-level module and shouldn't pull in the
/// full provider/routing stack. Callers wrap their `Arc<dyn LlmProvider>`
/// in a thin adapter that implements this trait.
#[async_trait]
pub trait PerceptionSummarizer: Send + Sync {
    /// Send `system` and `user` messages, return the assistant text.
    /// Implementations should pick a fast model — summarize calls are
    /// frequent and latency-sensitive.
    async fn summarize(&self, system: &str, user: &str) -> Result<String, AdapterError>;
}

/// Per-agent facade over the perception pipeline.
#[async_trait]
pub trait AgentPerceptionAdapter: Send + Sync {
    /// Replace the current [`Focus`]. Re-configures the underlying
    /// [`super::AttentionGate`] and [`super::SalienceFilter`] for this
    /// agent only — other agents are unaffected.
    ///
    /// Side-effects on per-agent state when focus changes:
    /// - AttentionGate whitelist: replaced.
    /// - Frequency budget token buckets: reset.
    /// - Salience diff baselines: kept (subject to `baseline_max_age`).
    /// - Dedup cache: cleared.
    async fn focus(&self, focus: Focus);

    /// Read the current [`Focus`].
    async fn current_focus(&self) -> Focus;

    /// Synchronous, cheap snapshot of the world. Does not consume queued
    /// events. Safe to call repeatedly.
    fn now(&self) -> Snapshot;

    /// Wait for the next event in this agent's queue.
    ///
    /// Returns `None` if the underlying pipeline is closed. Cancellable
    /// via the caller's task / select!.
    async fn next_event(&self) -> Option<Event>;

    /// Generate a natural-language summary of the past `dur` of perception.
    ///
    /// Implementations call the configured [`PerceptionSummarizer`].
    /// Returns [`AdapterError::Summarizer`] on LLM failure.
    async fn summarize(&self, dur: Duration) -> Result<String, AdapterError>;

    /// Tear down the per-agent state. After shutdown, all methods
    /// return [`AdapterError::Shutdown`] (or empty results).
    async fn shutdown(self: Arc<Self>);
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::perception::Snapshot;

    /// Minimal stub adapter purely for trait-shape testing.
    struct StubAdapter {
        focus: Mutex<Focus>,
        shut: std::sync::atomic::AtomicBool,
    }

    impl StubAdapter {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                focus: Mutex::new(Focus::default()),
                shut: false.into(),
            })
        }
    }

    #[async_trait]
    impl AgentPerceptionAdapter for StubAdapter {
        async fn focus(&self, focus: Focus) {
            *self.focus.lock().unwrap() = focus;
        }
        async fn current_focus(&self) -> Focus {
            self.focus.lock().unwrap().clone()
        }
        fn now(&self) -> Snapshot {
            Snapshot::empty()
        }
        async fn next_event(&self) -> Option<Event> {
            None
        }
        async fn summarize(&self, _dur: Duration) -> Result<String, AdapterError> {
            Err(AdapterError::Summarizer("stub".into()))
        }
        async fn shutdown(self: Arc<Self>) {
            self.shut.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn test_stub_round_trips_focus() {
        use crate::perception::Modality;
        let a: Arc<dyn AgentPerceptionAdapter> = StubAdapter::new();
        a.focus(Focus::default().with_modalities([Modality::System]))
            .await;
        let f = a.current_focus().await;
        assert!(f.admits_modality(Modality::System));
        assert!(!f.admits_modality(Modality::Audio));
    }

    #[tokio::test]
    async fn test_stub_now_returns_empty() {
        let a: Arc<dyn AgentPerceptionAdapter> = StubAdapter::new();
        assert_eq!(a.now().item_count(), 0);
    }
}
