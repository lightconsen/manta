//! Per-turn metrics collector.
//!
//! Owned by a single turn in `agent_engine`. Explicitly closed via
//! [`TurnMetricsCollector::finish`] / [`fail`] / [`abort`]; if the turn
//! future is dropped without a terminal call (true abort), `Drop` persists an
//! `aborted` record best-effort.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tracing::warn;

use crate::agent::turns::ToolCallRecord;
use crate::providers::Usage;

use super::record::{
    ErrorSource, LlmRoundRecord, ObservedError, ObservedToolCall, ObservedUsage, TurnEndState,
    TurnRecord, SCHEMA_VERSION,
};
use super::writer::TurnMetricsWriter;
use super::TurnMetricsSink;

/// In-memory cap for accumulated streamed text / reasoning (abort fallback).
const MAX_PARTIAL_BYTES: usize = 64 * 1024;

struct OpenRound {
    round: u32,
    provider: String,
    model: String,
    started_at: String,
    start: Instant,
    ttft_ms: Option<u64>,
}

pub struct TurnMetricsCollector {
    turn_id: String,
    session_id: Option<String>,
    conversation_id: String,
    thread_id: String,
    turn_index: u32,
    start: Instant,
    started_at: String,
    queue_wait_ms: Option<u64>,
    cache_hit: bool,
    model: String,
    user_message: String,
    partial_text: Arc<Mutex<String>>,
    partial_reasoning: Arc<Mutex<String>>,
    rounds: Vec<LlmRoundRecord>,
    open_round: Option<OpenRound>,
    tools: Vec<ObservedToolCall>,
    terminal: bool,
    writer: Arc<TurnMetricsWriter>,
    metrics_sink: Option<Arc<dyn TurnMetricsSink>>,
}

impl TurnMetricsCollector {
    pub fn new(
        session_id: Option<String>,
        conversation_id: String,
        thread_id: String,
        turn_index: usize,
        user_message: &str,
        enqueued_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Self {
        Self::with_writer(
            session_id,
            conversation_id,
            thread_id,
            turn_index,
            user_message,
            enqueued_at,
            Arc::new(TurnMetricsWriter::default_dir()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_writer(
        session_id: Option<String>,
        conversation_id: String,
        thread_id: String,
        turn_index: usize,
        user_message: &str,
        enqueued_at: Option<chrono::DateTime<chrono::Utc>>,
        writer: Arc<TurnMetricsWriter>,
    ) -> Self {
        let now = Instant::now();
        let queue_wait_ms = enqueued_at.map(|t| {
            chrono::Utc::now()
                .signed_duration_since(t)
                .num_milliseconds()
                .max(0) as u64
        });
        Self {
            turn_id: uuid::Uuid::new_v4().to_string(),
            session_id,
            conversation_id,
            thread_id,
            turn_index: turn_index as u32,
            start: now,
            started_at: chrono::Local::now().to_rfc3339(),
            queue_wait_ms,
            cache_hit: false,
            model: String::new(),
            user_message: user_message.to_string(),
            partial_text: Arc::new(Mutex::new(String::new())),
            partial_reasoning: Arc::new(Mutex::new(String::new())),
            rounds: Vec::new(),
            open_round: None,
            tools: Vec::new(),
            terminal: false,
            writer,
            metrics_sink: None,
        }
    }

    /// Attach an optional numeric metrics sink (SQLite rows). JSON persistence
    /// is unaffected.
    pub fn with_metrics_sink(mut self, sink: Option<Arc<dyn TurnMetricsSink>>) -> Self {
        self.metrics_sink = sink;
        self
    }

    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }

    pub fn mark_cache_hit(&mut self) {
        self.cache_hit = true;
    }

    /// Number of LLM rounds begun so far.
    pub fn round_count(&self) -> u32 {
        self.rounds.len() as u32 + u32::from(self.open_round.is_some())
    }

    fn push_partial(buf: &Arc<Mutex<String>>, delta: &str) {
        if let Ok(mut s) = buf.lock() {
            if s.len() < MAX_PARTIAL_BYTES {
                let remaining = MAX_PARTIAL_BYTES - s.len();
                let take = remaining.min(delta.len());
                let mut end = take;
                while !delta.is_char_boundary(end) {
                    end -= 1;
                }
                s.push_str(&delta[..end]);
            }
        }
    }

    /// Append a streamed text delta (abort fallback content).
    pub fn push_text_delta(&self, delta: &str) {
        Self::push_partial(&self.partial_text, delta);
    }

    /// Append a streamed reasoning delta.
    pub fn push_reasoning_delta(&self, delta: &str) {
        Self::push_partial(&self.partial_reasoning, delta);
    }

    /// Begin a new LLM round. Defensively closes any prior open round.
    pub fn begin_round(&mut self, provider: &str, model: &str) {
        if self.open_round.is_some() {
            self.end_round(None, Some("interrupted".to_string()));
        }
        if self.model.is_empty() {
            self.model = model.to_string();
        }
        self.open_round = Some(OpenRound {
            round: self.rounds.len() as u32,
            provider: provider.to_string(),
            model: model.to_string(),
            started_at: chrono::Local::now().to_rfc3339(),
            start: Instant::now(),
            ttft_ms: None,
        });
    }

    /// Record first streamed token for the open round (and turn TTFT on round 0).
    pub fn round_first_token(&mut self) {
        if let Some(open) = &mut self.open_round {
            if open.ttft_ms.is_none() {
                open.ttft_ms = Some(open.start.elapsed().as_millis() as u64);
            }
        }
    }

    /// Close the open round with its usage and finish reason.
    pub fn end_round(&mut self, usage: Option<&Usage>, finish_reason: Option<String>) {
        if let Some(open) = self.open_round.take() {
            self.rounds.push(LlmRoundRecord {
                round: open.round,
                provider: open.provider,
                model: open.model,
                started_at: open.started_at,
                duration_ms: open.start.elapsed().as_millis() as u64,
                ttft_ms: open.ttft_ms,
                usage: usage.map(|u| ObservedUsage {
                    prompt_tokens: u.prompt_tokens,
                    completion_tokens: u.completion_tokens,
                    total_tokens: u.total_tokens,
                    cache_read_tokens: u.cache_read_tokens,
                    cache_creation_tokens: u.cache_creation_tokens,
                }),
                finish_reason,
                error: None,
            });
        }
    }

    /// Close the open round as failed.
    pub fn fail_round(&mut self, err: &str) {
        if let Some(open) = self.open_round.take() {
            self.rounds.push(LlmRoundRecord {
                round: open.round,
                provider: open.provider,
                model: open.model,
                started_at: open.started_at,
                duration_ms: open.start.elapsed().as_millis() as u64,
                ttft_ms: open.ttft_ms,
                usage: None,
                finish_reason: None,
                error: Some(err.to_string()),
            });
        }
    }

    /// Record a tool call (belongs to the round that produced it).
    pub fn record_tool(&mut self, rec: &ToolCallRecord) {
        let round = self.round_count().saturating_sub(1);
        self.tools.push(ObservedToolCall {
            round,
            name: rec.name.clone(),
            args: rec.args.clone(),
            result: rec.result.clone(),
            success: rec.success,
            duration_ms: rec.duration_ms,
            error: if rec.success {
                None
            } else {
                Some(rec.result.clone())
            },
        });
    }

    fn build_record(&self, state: TurnEndState, error: Option<ObservedError>) -> TurnRecord {
        let duration_ms = self.start.elapsed().as_millis() as u64;
        let ttft_ms = self.rounds.first().and_then(|r| r.ttft_ms);
        let usage = self
            .rounds
            .iter()
            .fold(ObservedUsage::default(), |mut acc, r| {
                if let Some(u) = &r.usage {
                    acc.prompt_tokens += u.prompt_tokens;
                    acc.completion_tokens += u.completion_tokens;
                    acc.total_tokens += u.total_tokens;
                    acc.cache_read_tokens += u.cache_read_tokens;
                    acc.cache_creation_tokens += u.cache_creation_tokens;
                }
                acc
            });
        let text = self
            .partial_text
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        let reasoning = self
            .partial_reasoning
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        TurnRecord {
            schema_version: SCHEMA_VERSION,
            turn_id: self.turn_id.clone(),
            session_id: self.session_id.clone(),
            conversation_id: self.conversation_id.clone(),
            thread_id: self.thread_id.clone(),
            turn_index: self.turn_index,
            state,
            started_at: self.started_at.clone(),
            finished_at: chrono::Local::now().to_rfc3339(),
            duration_ms,
            ttft_ms,
            model: self.model.clone(),
            user_message_preview: self.user_message.clone(),
            assistant_text_preview: text,
            reasoning_preview: reasoning,
            queue_wait_ms: self.queue_wait_ms,
            cache_hit: self.cache_hit,
            error,
            usage,
            llm_rounds: self.rounds.clone(),
            tool_calls: self.tools.clone(),
        }
    }

    /// Close any open round with the given error / finish reason (no usage).
    fn close_open_round(&mut self, error: Option<&str>, finish_reason: Option<String>) {
        if let Some(open) = self.open_round.take() {
            self.rounds.push(LlmRoundRecord {
                round: open.round,
                provider: open.provider,
                model: open.model,
                started_at: open.started_at,
                duration_ms: open.start.elapsed().as_millis() as u64,
                ttft_ms: open.ttft_ms,
                usage: None,
                finish_reason,
                error: error.map(|e| e.to_string()),
            });
        }
    }

    async fn persist(&mut self, rec: TurnRecord) {
        self.terminal = true;
        if let Err(e) = self.writer.write(&rec).await {
            super::WRITE_FAILURES.fetch_add(1, Ordering::Relaxed);
            warn!("Failed to write turn observability record {}: {}", rec.turn_id, e);
        }
        if let Some(sink) = &self.metrics_sink {
            if let Err(e) = sink.persist_turn(&rec).await {
                super::WRITE_FAILURES.fetch_add(1, Ordering::Relaxed);
                warn!("Failed to persist metrics for turn {}: {}", rec.turn_id, e);
            }
        }
    }

    /// Turn completed successfully. Awaits the record write.
    pub async fn finish(mut self, final_text: &str) {
        self.close_open_round(None, Some("stop".to_string()));
        if let Ok(mut s) = self.partial_text.lock() {
            *s = final_text.to_string();
        }
        let rec = self.build_record(TurnEndState::Complete, None);
        self.persist(rec).await;
    }

    /// Turn failed. Awaits the record write.
    pub async fn fail(mut self, source: ErrorSource, err: &str) {
        self.close_open_round(Some(err), None);
        let rec = self.build_record(
            TurnEndState::Error,
            Some(ObservedError {
                source,
                message: err.to_string(),
            }),
        );
        self.persist(rec).await;
    }

    /// Turn aborted (cancelled sentinel detected). Awaits the record write.
    pub async fn abort(mut self) {
        self.close_open_round(None, Some("cancelled".to_string()));
        let rec = self.build_record(TurnEndState::Aborted, None);
        self.persist(rec).await;
    }
}

impl Drop for TurnMetricsCollector {
    fn drop(&mut self) {
        if self.terminal {
            return;
        }
        // True future-drop abort: persist best-effort.
        self.close_open_round(None, Some("dropped".to_string()));
        let rec = self.build_record(TurnEndState::Aborted, None);
        let writer = Arc::clone(&self.writer);
        let sink = self.metrics_sink.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            // Best-effort abort write; intentionally not registered in
            // TaskRegistry — there is no shutdown path for a dropped turn.
            handle.spawn(async move {
                if let Err(e) = writer.write(&rec).await {
                    super::WRITE_FAILURES.fetch_add(1, Ordering::Relaxed);
                    warn!("Failed to write aborted turn record {}: {}", rec.turn_id, e);
                }
                if let Some(sink) = sink {
                    if let Err(e) = sink.persist_turn(&rec).await {
                        super::WRITE_FAILURES.fetch_add(1, Ordering::Relaxed);
                        warn!("Failed to persist metrics for aborted turn {}: {}", rec.turn_id, e);
                    }
                }
            });
        } else {
            // No async runtime: JSON only (blocking SQLite writes need a runtime).
            std::thread::spawn(move || {
                if let Err(e) = writer.write_blocking(&rec) {
                    super::WRITE_FAILURES.fetch_add(1, Ordering::Relaxed);
                    warn!("Failed to write aborted turn record {}: {}", rec.turn_id, e);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_collector(dir: &TempDir) -> TurnMetricsCollector {
        TurnMetricsCollector::with_writer(
            Some("s1".into()),
            "c1".into(),
            "main".into(),
            0,
            "hello",
            None,
            Arc::new(TurnMetricsWriter::new(dir.path().to_path_buf())),
        )
    }

    fn read_record(dir: &TempDir, turn_id: &str) -> TurnRecord {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let path = dir.path().join(date).join(format!("{}.json", turn_id));
        let content = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    #[tokio::test]
    async fn finish_writes_complete_record() {
        let dir = TempDir::new().unwrap();
        let mut c = make_collector(&dir);
        c.begin_round("p", "m");
        c.round_first_token();
        c.end_round(None, Some("stop".into()));
        let id = c.turn_id().to_string();
        c.finish("done").await;

        let rec = read_record(&dir, &id);
        assert_eq!(rec.state, TurnEndState::Complete);
        assert_eq!(rec.assistant_text_preview, "done");
        assert_eq!(rec.llm_rounds.len(), 1);
        assert!(rec.ttft_ms.is_some());
    }

    #[tokio::test]
    async fn fail_writes_error_record() {
        let dir = TempDir::new().unwrap();
        let mut c = make_collector(&dir);
        c.begin_round("p", "m");
        let id = c.turn_id().to_string();
        c.fail(ErrorSource::Llm, "boom").await;

        let rec = read_record(&dir, &id);
        assert_eq!(rec.state, TurnEndState::Error);
        assert_eq!(rec.error.as_ref().unwrap().message, "boom");
        assert_eq!(rec.llm_rounds.last().unwrap().error.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn abort_writes_aborted_record_with_partial_text() {
        let dir = TempDir::new().unwrap();
        let mut c = make_collector(&dir);
        c.begin_round("p", "m");
        c.push_text_delta("partial ");
        c.push_text_delta("content");
        c.push_reasoning_delta("thinking");
        let id = c.turn_id().to_string();
        c.abort().await;

        let rec = read_record(&dir, &id);
        assert_eq!(rec.state, TurnEndState::Aborted);
        assert_eq!(rec.assistant_text_preview, "partial content");
        assert_eq!(rec.reasoning_preview, "thinking");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drop_without_terminal_writes_aborted_record() {
        let dir = TempDir::new().unwrap();
        let id;
        {
            let mut c = make_collector(&dir);
            c.begin_round("p", "m");
            c.push_text_delta("partial");
            id = c.turn_id().to_string();
            drop(c); // no terminal call -> Drop persists aborted
        }
        // Give the spawned write task a moment to land.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let rec = read_record(&dir, &id);
        assert_eq!(rec.state, TurnEndState::Aborted);
        assert_eq!(rec.assistant_text_preview, "partial");
        assert_eq!(rec.llm_rounds.last().unwrap().finish_reason.as_deref(), Some("dropped"));
    }

    #[tokio::test]
    async fn drop_after_terminal_is_noop() {
        let dir = TempDir::new().unwrap();
        let mut c = make_collector(&dir);
        c.begin_round("p", "m");
        c.end_round(None, None);
        let id = c.turn_id().to_string();
        c.finish("ok").await; // consumes self; terminal set, Drop must not rewrite
        let rec = read_record(&dir, &id);
        assert_eq!(rec.state, TurnEndState::Complete);
    }

    #[test]
    fn first_token_only_recorded_once() {
        let dir = TempDir::new().unwrap();
        let mut c = make_collector(&dir);
        c.begin_round("p", "m");
        std::thread::sleep(std::time::Duration::from_millis(2));
        c.round_first_token();
        let first = c.open_round.as_ref().unwrap().ttft_ms;
        std::thread::sleep(std::time::Duration::from_millis(5));
        c.round_first_token();
        assert_eq!(c.open_round.as_ref().unwrap().ttft_ms, first);
    }

    #[tokio::test]
    async fn cache_hit_turn_records_flag() {
        let dir = TempDir::new().unwrap();
        let mut c = make_collector(&dir);
        c.mark_cache_hit();
        let id = c.turn_id().to_string();
        c.finish("cached answer").await;

        let rec = read_record(&dir, &id);
        assert!(rec.cache_hit);
        assert!(rec.llm_rounds.is_empty());
    }
}
