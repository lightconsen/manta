//! Core message-processing loop, LLM completions, and tool-call handling.

use std::sync::Arc;

use tokio_stream::StreamExt;

use tracing::{debug, error, info, instrument, warn};

use crate::agent::reflection::critic::Critic;
use crate::agent::reflection::types::{Critique, QualityCriteria};
use crate::agent::turns::ToolCallRecord;
use crate::channels::{IncomingMessage, OutgoingMessage};
use crate::observe::{
    ChannelObservation, ErrorSource, TurnContext, TurnMetricsCollector, TurnMetricsSink,
};
use crate::providers::{CompletionRequest, Message, Role, ToolCall, ToolResult};
use crate::tools::{ToolContext, ToolExecutionChunk};

use super::agent_cache::{are_tools_cacheable, should_use_cache_llm};
use super::*;

impl Agent {
    /// Begin-of-turn reset for the active plan surfaces.
    ///
    /// The currently effective plan is the todo list written during the most
    /// recent turn; a new user turn automatically clears it so the UI never
    /// shows a stale checklist. Concretely this:
    ///
    /// 1. Drops the conversation's `ActivePlan` (and its persisted snapshot)
    ///    so the prompt no longer injects stale task context, and
    /// 2. Clears the `todo` tool's whole-snapshot state for the conversation.
    ///
    /// Called before any planning runs for the new message, so a plan the
    /// new turn creates is written after this reset and survives it.
    async fn begin_user_turn_reset(&self, conversation_id: &str) {
        let had_plan = {
            let mut plans = self.active_plans.write().await;
            plans.remove(conversation_id).is_some()
        };
        if had_plan {
            // Best-effort removal of the persisted plan snapshot so a daemon
            // restart cannot resurrect the cleared plan.
            if let Some(ref dir) = self.plans_dir {
                let path = dir.join(format!("{}.json", conversation_id));
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        warn!("Failed to remove persisted plan {:?}: {}", path, e);
                    }
                }
            }
            info!("New turn: cleared active plan for conversation {}", conversation_id);
        }

        if let Some(todo_state) = self.tools.todo_state() {
            todo_state.clear_conversation(conversation_id).await;
        }
    }

    /// Process an incoming message
    #[instrument(skip(self, message))]
    pub async fn process_message(
        &self,
        message: IncomingMessage,
    ) -> crate::Result<OutgoingMessage> {
        debug!("Processing message from user: {}", message.user_id);

        let conversation_id = message.conversation_id.0.clone();
        let user_id = message.user_id.0.clone();
        let content = message.content.clone();

        // ── Prompt-injection guard ────────────────────────────────────────────
        let input_scan = crate::skills::guard::scan_input(&content);
        if !input_scan.passed {
            warn!("Blocked suspicious input from user {}: {:?}", user_id, input_scan.issues);
            return Ok(OutgoingMessage::new(
                crate::channels::ConversationId(conversation_id),
                "I'm unable to process this request as it contains potentially unsafe content. If \
                 you believe this is a mistake, please rephrase your message."
                    .to_string(),
            ));
        }

        // ── New-turn reset ────────────────────────────────────────────────────
        // A new user turn clears the previous turn's active plan/todo list.
        self.begin_user_turn_reset(&conversation_id).await;

        // ── Thread binding check ──────────────────────────────────────────────
        if let Some(ref manager) = self.thread_binding_manager {
            // Check if a binding exists and is still valid
            if manager.is_valid(&conversation_id).await {
                // Record activity on the existing binding
                manager.record_activity(&conversation_id).await;
            } else if manager.get(&conversation_id).await.is_some() {
                // Binding exists but is expired — remove it and warn
                warn!(
                    "Thread binding expired/session {} for conversation {}",
                    conversation_id, conversation_id
                );
                manager.remove(&conversation_id).await;
            }
            // Reap any idle bindings periodically (best-effort)
            let _reaped = manager.reap().await;
        }

        // Check cache for identical prompt (only for non-follow-up, non-time-sensitive
        // messages) Skip cache if this looks like a follow-up (short message
        // referring to previous context)
        let is_follow_up = content.len() < 50
            && (content.contains("it")
                || content.contains("that")
                || content.contains("this")
                || content.contains("上面的")
                || content.contains("这个")
                || content.contains("那个"));

        // Use LLM to determine if query should be cached
        let should_cache = !is_follow_up
            && should_use_cache_llm(&self.provider, &content, self.model.clone()).await;

        if should_cache {
            if let Some(cached) = self
                .response_cache
                .get(&user_id, &conversation_id, &content)
                .await
            {
                info!("Cache hit for user {} - returning cached response", user_id);

                // Store user message in chat history
                if let Some(ref store) = self.chat_history {
                    use crate::memory::ChatMessage;
                    let chat_msg = ChatMessage::new(&conversation_id, &user_id, "user", &content);
                    if let Err(e) = store.store_message(chat_msg).await {
                        error!("Failed to store user message: {}", e);
                    }
                }

                // Store cached assistant response in chat history
                if let Some(ref store) = self.chat_history {
                    use crate::memory::ChatMessage;
                    let chat_msg =
                        ChatMessage::new(&conversation_id, &user_id, "assistant", &cached.response);
                    if let Err(e) = store.store_message(chat_msg).await {
                        error!("Failed to store assistant message: {}", e);
                    }
                }

                // Return cached response
                return Ok(OutgoingMessage::new(
                    crate::channels::ConversationId(conversation_id),
                    cached.response.clone(),
                ));
            }
        }

        // Store user message in chat history and index for search
        let message_id = uuid::Uuid::new_v4().to_string();

        // Persist user message via MemoryManager (episodic memory)
        if let Some(ref mm) = self.memory_manager {
            if let Err(e) = mm
                .remember_message(&user_id, &conversation_id, "user", &content)
                .await
            {
                warn!("MemoryManager: failed to store user message: {}", e);
            }
        }

        if let Some(ref store) = self.chat_history {
            use crate::memory::ChatMessage;
            let chat_msg = ChatMessage::new(&conversation_id, &user_id, "user", &content);
            // Clone message_id before moving chat_msg
            let msg_id = chat_msg.id.clone();
            if let Err(e) = store.store_message(chat_msg).await {
                error!("Failed to store user message: {}", e);
            }
            // Index for session search
            if let Some(ref search) = self.session_search {
                if let Err(e) = search
                    .index_message(&msg_id, &conversation_id, &user_id, &content, "user")
                    .await
                {
                    error!("Failed to index user message for search: {}", e);
                }
            }
        } else if let Some(ref search) = self.session_search {
            // Even if chat history is not enabled, index for search
            if let Err(e) = search
                .index_message(&message_id, &conversation_id, &user_id, &content, "user")
                .await
            {
                error!("Failed to index user message for search: {}", e);
            }
        }

        // Record user message in transcript
        if let Some(ref transcript_store) = self.transcript_store {
            transcript_store.append(
                &conversation_id,
                "agent",
                &user_id,
                &conversation_id,
                TranscriptMessage::new("user", &content),
            );
            // Track transcript size in disk budget
            if let Some(ref budget) = self.disk_budget {
                let transcript_size = content.len();
                if let Err(e) = budget.track_item(
                    &conversation_id,
                    format!("transcript-user-{}", message_id),
                    BudgetCategory::Transcript,
                    transcript_size,
                ) {
                    warn!("Failed to track user transcript in disk budget: {}", e);
                }
            }
        }

        // Check if we need task planning
        let needs_planning = self.task_planner.needs_planning(&content).await;

        if needs_planning {
            info!("Complex task detected, creating plan for: {}", conversation_id);

            // Create a plan
            match self.task_planner.create_plan(&content).await {
                Ok(plan) => {
                    let summary = plan.format_summary();
                    info!("Created plan with {} tasks", plan.tasks.len());

                    // Plan turns return before any LLM round, so they produce
                    // no round record. Persist a lightweight turn record whose
                    // whole observable payload is the plan DAG snapshot (closes
                    // the §五 planner blind spot).
                    let mut collector = TurnMetricsCollector::new(TurnContext {
                        session_id: self.session_id.clone(),
                        conversation_id: conversation_id.clone(),
                        agent_id: self.agent_id.clone(),
                        thread_id: format!("thread-{}", conversation_id),
                        turn_index: 0,
                        user_message: content.clone(),
                        enqueued_at: None,
                    })
                    .with_metrics_sink(
                        self.session_store
                            .clone()
                            .map(|s| -> Arc<dyn TurnMetricsSink> { s }),
                    );
                    collector
                        .record_plan_snapshot(crate::observe::record::PlanSnapshot::from(&plan));
                    collector.finish(&summary).await;

                    // Convert to todos
                    let todos = self.task_planner.plan_to_todos(&plan);

                    // Store active plan
                    let active_plan = ActivePlan {
                        plan,
                        todos,
                        completed_tasks: Vec::new(),
                    };

                    let mut plans = self.active_plans.write().await;
                    plans.insert(conversation_id.clone(), active_plan);
                    drop(plans);

                    // Persist the plan if plans_dir is configured
                    if let Some(ref dir) = self.plans_dir {
                        let plans = self.active_plans.read().await;
                        if let Some(active) = plans.get(&conversation_id) {
                            let snapshot = PersistedPlan::from_active(active);
                            let path = dir.join(format!("{}.json", conversation_id));
                            if let Err(e) = snapshot.persist_to(&path).await {
                                warn!("Failed to persist plan: {}", e);
                            }
                        }
                    }

                    // Return the plan to the user
                    return Ok(OutgoingMessage::new(
                        crate::channels::ConversationId(conversation_id),
                        format!("I'll break this down into steps:\n\n{}", summary),
                    ));
                }
                Err(e) => {
                    warn!("Failed to create plan: {}, proceeding without planning", e);
                }
            }
        }

        // ── Per-conversation concurrency guard ──────────────────────────────
        // Prevents reentrant processing: if a second message arrives for the
        // same conversation_id while one is in-flight, it waits here.
        let sem = {
            let mut guards = self.concurrency_guards.lock().await;
            guards
                .entry(conversation_id.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(1)))
                .clone()
        };
        let _permit = match sem.acquire().await {
            Ok(p) => p,
            Err(_) => {
                return Err(crate::error::SyscityError::Internal(
                    "concurrency semaphore closed".into(),
                ));
            }
        };

        // ── Thread take-out (panic-safe via ThreadGuard) ────────────────────
        // ThreadGuard reinserts the thread into thread_map on Drop, preventing
        // thread loss if processing panics between take-out and reinsertion.
        let mut guard = ThreadGuard::take(&self.thread_map, &conversation_id).await;
        if guard.thread.is_none() {
            // First message for this conversation — build initial Context.
            let ctx = self
                .build_fresh_context(&conversation_id, &user_id, &content)
                .await;
            let thread_id = format!("thread-{}", conversation_id);
            // Persist the new thread record (fire-and-forget).
            if let (Some(store), Some(sid)) = (self.session_store.clone(), self.session_id.clone())
            {
                let tid = thread_id.clone();
                let label = conversation_id.clone();
                tokio::spawn(async move {
                    if let Err(e) = store
                        .save_thread(&sid, &tid, &label, chrono::Utc::now().timestamp_millis())
                        .await
                    {
                        warn!("Failed to persist thread {} for session {}: {}", tid, sid, e);
                    }
                });
            }
            guard.thread = Some(Thread::from_context(thread_id, &conversation_id, ctx));
        }
        let thread = guard.get_mut();
        // Safe: from here on, guard.thread is always Some until into_thread().

        // Apply ACP max iteration override for existing threads
        let override_opt = *self.max_tool_iterations_override.read().await;
        if let Some(max_iter) = override_opt {
            thread.context.set_max_tool_iterations(max_iter);
            info!(
                "Applied ACP max iteration override to existing thread: {} for conversation {}",
                max_iter, conversation_id
            );
        }

        // Reset tool tracking and add user message for this turn.
        thread.context.clear_tools_used();
        thread
            .context
            .add_message(Message::user_named(&user_id, &content));

        // Track this turn in the turn log.
        let turn_idx = thread.push_turn(&content);
        thread.turns[turn_idx].start();

        // Check if we're executing an active plan
        let active_plan_check = {
            let plans = self.active_plans.read().await;
            plans.get(&conversation_id).map(|p| {
                (p.plan.progress_percent(), p.plan.current_task().map(|t| t.description.clone()))
            })
        };

        if let Some((progress, Some(current_task))) = active_plan_check {
            info!("Executing plan: {}% - Task: {}", progress, current_task);
        }

        // Get response from LLM (lock NOT held during this await).
        let llm_result = self
            .get_completion(&mut thread.context, &message.user_id.0)
            .await;

        // Complete or interrupt the turn based on result.
        let llm_result = match llm_result {
            Ok(resp) => {
                let asst_text = resp.message.content.clone();
                thread.turns[turn_idx].complete(asst_text.clone());
                // Persist the turn asynchronously (fire-and-forget).
                if let (Some(store), Some(sid)) =
                    (self.session_store.clone(), self.session_id.clone())
                {
                    let tid = thread.id.clone();
                    let user_c = content.clone();
                    let t_idx = turn_idx as i64;
                    tokio::spawn(async move {
                        if let Err(e) = store
                            .append_turn(&sid, &tid, t_idx, &user_c, &asst_text, "complete", None)
                            .await
                        {
                            warn!("Failed to persist turn {} for session {}: {}", t_idx, sid, e);
                        }
                    });
                }
                Ok(resp)
            }
            Err(e) => {
                thread.turns[turn_idx].mark_error();
                Err(e)
            }
        };

        // Collect tools_used BEFORE putting thread back (needed for cache logic below).
        let tools_used_this_turn = thread.context.tools_used().to_vec();

        // ── Put thread back ───────────────────────────────────────────────────
        {
            let mut map = self.thread_map.lock().await;
            map.insert(conversation_id.clone(), guard.into_thread());
        }

        let response = llm_result?;

        // Mark memory hits based on response content
        if let Some(ref mm) = self.memory_manager {
            let session_key = format!("{}:{}", user_id, conversation_id);
            mm.evaluate_response_hits(&session_key, &response.message.content)
                .await;
            // Close the effectiveness feedback loop
            mm.apply_effectiveness_adjustments().await;
        }

        // Store assistant response in chat history and index for search
        let assistant_message_id = uuid::Uuid::new_v4().to_string();

        // Record assistant message in transcript
        if let Some(ref transcript_store) = self.transcript_store {
            transcript_store.append(
                &conversation_id,
                "agent",
                &user_id,
                &conversation_id,
                TranscriptMessage::new("assistant", &response.message.content),
            );
            // Track transcript size in disk budget
            if let Some(ref budget) = self.disk_budget {
                let transcript_size = response.message.content.len();
                if let Err(e) = budget.track_item(
                    &conversation_id,
                    format!("transcript-assistant-{}", assistant_message_id),
                    BudgetCategory::Transcript,
                    transcript_size,
                ) {
                    warn!("Failed to track assistant transcript in disk budget: {}", e);
                }
            }
        }

        // Persist assistant response via MemoryManager (episodic memory)
        if let Some(ref mm) = self.memory_manager {
            if let Err(e) = mm
                .remember_message(
                    &user_id,
                    &conversation_id,
                    "assistant",
                    &response.message.content,
                )
                .await
            {
                warn!("MemoryManager: failed to store assistant message: {}", e);
            }
        }

        if let Some(ref store) = self.chat_history {
            use crate::memory::ChatMessage;
            let chat_msg = ChatMessage::new(
                &conversation_id,
                &user_id,
                "assistant",
                &response.message.content,
            );
            let msg_id = chat_msg.id.clone();
            if let Err(e) = store.store_message(chat_msg).await {
                error!("Failed to store assistant message: {}", e);
            }
            // Index for session search
            if let Some(ref search) = self.session_search {
                if let Err(e) = search
                    .index_message(
                        &msg_id,
                        &conversation_id,
                        &user_id,
                        &response.message.content,
                        "assistant",
                    )
                    .await
                {
                    error!("Failed to index assistant message for search: {}", e);
                }
            }
        } else if let Some(ref search) = self.session_search {
            // Even if chat history is not enabled, index for search
            if let Err(e) = search
                .index_message(
                    &assistant_message_id,
                    &conversation_id,
                    &user_id,
                    &response.message.content,
                    "assistant",
                )
                .await
            {
                error!("Failed to index assistant message for search: {}", e);
            }
        }

        // Only cache the response if it should be cached
        if should_cache {
            // Check if tools used are cacheable (skip cache for time-sensitive tools)
            if are_tools_cacheable(&tools_used_this_turn) {
                self.response_cache
                    .set(
                        &user_id,
                        &conversation_id,
                        &content,
                        response.message.content.clone(),
                        tools_used_this_turn,
                    )
                    .await;
            }
        }

        // ── PII output filtering ─────────────────────────────────────────────
        let filtered_content = if let Some(ref detector) = self.pii_detector {
            match detector.filter_response(&response.message.content) {
                crate::security::FilterResult::Clean(text) => text,
                crate::security::FilterResult::Redacted(text, findings) => {
                    tracing::info!(
                        "Redacted {} PII findings from response for conversation {}",
                        findings.len(),
                        conversation_id
                    );
                    text
                }
                crate::security::FilterResult::Blocked(findings) => {
                    let restricted_count = findings
                        .iter()
                        .filter(|f| {
                            f.classification == crate::security::DataClassification::Restricted
                        })
                        .count();
                    tracing::warn!(
                        "Blocked response containing {} restricted PII items for conversation {}",
                        restricted_count,
                        conversation_id
                    );
                    "⚠️ This response contains sensitive personal information and has been \
                     blocked. Please review the content before sharing."
                        .to_string()
                }
            }
        } else {
            response.message.content.clone()
        };

        // Create outgoing message with usage tracking
        let mut outgoing = OutgoingMessage::new(
            crate::channels::ConversationId(conversation_id),
            filtered_content,
        );
        if let Some(ref usage) = response.usage {
            outgoing.usage = Some(*usage);
        }

        // ── Note: trajectory reflection (retrospect) runs in
        // process_message_with_progress ──

        Ok(outgoing)
    }

    /// Process an incoming message with progress callbacks
    #[instrument(skip(self, message, progress_cb))]
    pub async fn process_message_with_progress(
        &self,
        message: IncomingMessage,
        progress_cb: ProgressCallback,
    ) -> crate::Result<OutgoingMessage> {
        debug!("Processing message with progress from user: {}", message.user_id);

        let cfg = self.config_snapshot();

        let conversation_id = message.conversation_id.0.clone();
        let user_id = message.user_id.0.clone();
        let content = message.content.clone();

        // Notify started
        (progress_cb)(ProgressEvent::Started).await;

        // ── Prompt-injection guard ────────────────────────────────────────────
        let input_scan = crate::skills::guard::scan_input(&content);
        if !input_scan.passed {
            warn!("Blocked suspicious input from user {}: {:?}", user_id, input_scan.issues);
            let rejection = "I'm unable to process this request as it contains potentially unsafe \
                             content. If you believe this is a mistake, please rephrase your \
                             message."
                .to_string();
            // Guard rejection happens before a turn collector exists, so the
            // turn_id is empty (no persisted turn to vote on).
            (progress_cb)(ProgressEvent::Completed {
                response: rejection.clone(),
                turn_id: String::new(),
            })
            .await;
            return Ok(OutgoingMessage::new(
                crate::channels::ConversationId(conversation_id),
                rejection,
            ));
        }

        // ── New-turn reset ────────────────────────────────────────────────────
        // A new user turn clears the previous turn's active plan/todo list.
        self.begin_user_turn_reset(&conversation_id).await;

        // Check cache for identical prompt (only for non-follow-up, non-time-sensitive
        // messages)
        let is_follow_up = content.len() < 50
            && (content.contains("it")
                || content.contains("that")
                || content.contains("this")
                || content.contains("上面的")
                || content.contains("这个")
                || content.contains("那个"));

        // Use LLM to determine if query should be cached
        let should_cache = !is_follow_up
            && should_use_cache_llm(&self.provider, &content, self.model.clone()).await;

        if should_cache {
            if let Some(cached) = self
                .response_cache
                .get(&user_id, &conversation_id, &content)
                .await
            {
                info!("Cache hit for user {} - returning cached response", user_id);

                // Notify cache hit
                (progress_cb)(ProgressEvent::ToolCalling {
                    name: "cache".to_string(),
                    arguments: "{\"hit\": true}".to_string(),
                })
                .await;

                // Store user message in chat history
                if let Some(ref store) = self.chat_history {
                    use crate::memory::ChatMessage;
                    let chat_msg = ChatMessage::new(&conversation_id, &user_id, "user", &content);
                    if let Err(e) = store.store_message(chat_msg).await {
                        error!("Failed to store user message: {}", e);
                    }
                }

                // Store cached assistant response in chat history
                if let Some(ref store) = self.chat_history {
                    use crate::memory::ChatMessage;
                    let chat_msg =
                        ChatMessage::new(&conversation_id, &user_id, "assistant", &cached.response);
                    if let Err(e) = store.store_message(chat_msg).await {
                        error!("Failed to store assistant message: {}", e);
                    }
                }

                // Record the cache hit as a completed turn with no LLM rounds so
                // cache-hit rate / cost statistics stay accurate. The collector
                // must be created BEFORE emitting Completed so the cache-hit
                // turn has a stable turn_id for feedback.vote.
                let mut cache_collector = TurnMetricsCollector::new(TurnContext {
                    session_id: self.session_id.clone(),
                    conversation_id: conversation_id.clone(),
                    agent_id: self.agent_id.clone(),
                    thread_id: format!("thread-{}", conversation_id),
                    turn_index: 0,
                    user_message: content.clone(),
                    enqueued_at: Some(message.metadata.timestamp),
                })
                .with_metrics_sink(
                    self.session_store
                        .clone()
                        .map(|s| -> Arc<dyn TurnMetricsSink> { s }),
                );
                let cache_turn_id = cache_collector.turn_id().to_string();

                // Notify completed with cached response
                (progress_cb)(ProgressEvent::Completed {
                    response: cached.response.clone(),
                    turn_id: cache_turn_id.clone(),
                })
                .await;
                cache_collector.mark_cache_hit();
                cache_collector.finish(&cached.response).await;

                // Online risk scan for the cache-hit turn (no tools ran).
                self.scan_turn_for_badcase(
                    &content,
                    &cached.response,
                    0,
                    &cache_turn_id,
                    &conversation_id,
                );

                // Return cached response
                return Ok(OutgoingMessage::new(
                    crate::channels::ConversationId(conversation_id),
                    cached.response.clone(),
                ));
            }
        }

        // Persist user message via MemoryManager (episodic memory)
        if let Some(ref mm) = self.memory_manager {
            if let Err(e) = mm
                .remember_message(&user_id, &conversation_id, "user", &content)
                .await
            {
                warn!("MemoryManager: failed to store user message: {}", e);
            }
        }

        // Store user message in chat history and index for search
        let message_id = uuid::Uuid::new_v4().to_string();
        if let Some(ref store) = self.chat_history {
            use crate::memory::ChatMessage;
            let chat_msg = ChatMessage::new(&conversation_id, &user_id, "user", &content);
            let msg_id = chat_msg.id.clone();
            if let Err(e) = store.store_message(chat_msg).await {
                error!("Failed to store user message: {}", e);
            }
            if let Some(ref search) = self.session_search {
                if let Err(e) = search
                    .index_message(&msg_id, &conversation_id, &user_id, &content, "user")
                    .await
                {
                    error!("Failed to index user message for search: {}", e);
                }
            }
        } else if let Some(ref search) = self.session_search {
            if let Err(e) = search
                .index_message(&message_id, &conversation_id, &user_id, &content, "user")
                .await
            {
                error!("Failed to index user message for search: {}", e);
            }
        }

        // Record user message in transcript
        if let Some(ref transcript_store) = self.transcript_store {
            transcript_store.append(
                &conversation_id,
                "agent",
                &user_id,
                &conversation_id,
                TranscriptMessage::new("user", &content),
            );
            if let Some(ref budget) = self.disk_budget {
                if let Err(e) = budget.track_item(
                    &conversation_id,
                    format!("transcript-user-{}", message_id),
                    BudgetCategory::Transcript,
                    content.len(),
                ) {
                    warn!("Failed to track user transcript in disk budget: {}", e);
                }
            }
        }

        // ── Per-conversation concurrency guard ──────────────────────────────
        let sem = {
            let mut guards = self.concurrency_guards.lock().await;
            guards
                .entry(conversation_id.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(1)))
                .clone()
        };
        let _permit = match sem.acquire().await {
            Ok(p) => p,
            Err(_) => {
                return Err(crate::error::SyscityError::Internal(
                    "concurrency semaphore closed".into(),
                ));
            }
        };

        // ── Thread take-out (panic-safe via ThreadGuard) ────────────────────
        let mut guard = ThreadGuard::take(&self.thread_map, &conversation_id).await;
        if guard.thread.is_none() {
            let ctx = self
                .build_fresh_context(&conversation_id, &user_id, &content)
                .await;
            let thread_id = format!("thread-{}", conversation_id);
            if let (Some(store), Some(sid)) = (self.session_store.clone(), self.session_id.clone())
            {
                let tid = thread_id.clone();
                let label = conversation_id.clone();
                tokio::spawn(async move {
                    if let Err(e) = store
                        .save_thread(&sid, &tid, &label, chrono::Utc::now().timestamp_millis())
                        .await
                    {
                        warn!("Failed to persist thread {} for session {}: {}", tid, sid, e);
                    }
                });
            }
            guard.thread = Some(Thread::from_context(thread_id, &conversation_id, ctx));
        }
        let thread = guard.get_mut();

        // Apply delegation scope from message metadata (delegated child agents).
        // Applied after the thread is available so both freshly built and resumed
        // delegation conversations get the scope before any LLM call.
        if let Some(scope_value) = message
            .metadata
            .extra
            .get(crate::delegation::DELEGATION_SCOPE_KEY)
        {
            match serde_json::from_value::<crate::delegation::DelegationScope>(scope_value.clone())
            {
                Ok(scope) => {
                    thread.context.set_delegation(Some(scope.clone()));
                    if let Some(max_iter) = scope.max_iterations {
                        thread.context.set_max_tool_iterations(max_iter);
                    }
                    info!(
                        delegation_root = %scope.root_id,
                        delegation_task = %scope.task_id,
                        depth = scope.depth,
                        "Applied delegation scope to conversation {}",
                        conversation_id
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to parse delegation scope for conversation {}: {}",
                        conversation_id, e
                    );
                }
            }
        }

        // Apply ACP max iteration override for existing threads
        let override_opt = *self.max_tool_iterations_override.read().await;
        if let Some(max_iter) = override_opt {
            thread.context.set_max_tool_iterations(max_iter);
            info!(
                "Applied ACP max iteration override to existing thread: {} for conversation {}",
                max_iter, conversation_id
            );
        }

        // Reset tool tracking and add user message for this turn.
        thread.context.clear_tools_used();
        thread
            .context
            .add_message(Message::user_named(&user_id, &content));

        // Track this turn.
        let turn_idx = thread.push_turn(&content);
        thread.turns[turn_idx].start();

        // Per-turn observability collector (persisted on finish/fail/abort).
        let mut collector = TurnMetricsCollector::new(TurnContext {
            session_id: self.session_id.clone(),
            conversation_id: conversation_id.clone(),
            agent_id: self.agent_id.clone(),
            thread_id: thread.id.clone(),
            turn_index: turn_idx,
            user_message: content.clone(),
            enqueued_at: Some(message.metadata.timestamp),
        })
        .with_metrics_sink(
            self.session_store
                .clone()
                .map(|s| -> Arc<dyn TurnMetricsSink> { s }),
        );

        // Capture the stable turn id BEFORE the collector is consumed by
        // finish()/fail()/abort(); it is threaded into the Completed event.
        let turn_id = collector.turn_id().to_string();

        // Drain the inbound channel-layer observation (debounce/enrich/route)
        // attached by the gateway dispatch layer, if the message carried one.
        if let Some(obs) = message
            .metadata
            .extra
            .get("channel_observation")
            .and_then(|v| serde_json::from_value::<ChannelObservation>(v.clone()).ok())
        {
            collector.record_channel(obs);
        }

        // Get response from LLM with progress (lock NOT held).
        let llm_result = self
            .get_completion_with_progress(
                &mut thread.context,
                &mut collector,
                progress_cb.clone(),
                &message.user_id.0,
            )
            .await;

        // Complete or interrupt the turn.
        let llm_result = match llm_result {
            Ok(resp) => {
                let asst_text = resp.message.content.clone();
                thread.turns[turn_idx].complete(asst_text.clone());
                if let (Some(store), Some(sid)) =
                    (self.session_store.clone(), self.session_id.clone())
                {
                    let tid = thread.id.clone();
                    let user_c = content.clone();
                    let t_idx = turn_idx as i64;
                    let asst_text_spawn = asst_text.clone();
                    let turn_id_spawn = turn_id.clone();
                    tokio::spawn(async move {
                        if let Err(e) = store
                            .append_turn(
                                &sid,
                                &tid,
                                t_idx,
                                &user_c,
                                &asst_text_spawn,
                                "complete",
                                Some(&turn_id_spawn),
                            )
                            .await
                        {
                            warn!("Failed to persist turn {} for session {}: {}", t_idx, sid, e);
                        }
                    });
                }
                // Controller checkpoints return Ok with finish_reason="cancelled";
                // treat that as an abort, not a successful completion.
                if resp.finish_reason.as_deref() == Some("cancelled") {
                    collector.abort().await;
                } else {
                    collector.finish(&asst_text).await;
                }
                Ok(resp)
            }
            Err(e) => {
                thread.turns[turn_idx].mark_error();
                collector.fail(ErrorSource::Llm, &e.to_string()).await;
                Err(e)
            }
        };

        // Transfer accumulated tool call records and token usage from context to this
        // turn
        let tool_records = thread.context.take_tool_call_records();
        if !tool_records.is_empty() {
            thread.turns[turn_idx].tool_calls = tool_records;
        }
        thread.turns[turn_idx].token_usage = thread.context.take_turn_token_usage();

        let tools_used_this_turn = thread.context.tools_used().to_vec();

        // Snapshot turns for retrospect engine before moving the thread.
        let retrospect_turns: Vec<crate::agent::turns::Turn> = thread.turns.clone();
        let retrospect_turn_count = thread.turn_count();

        // ── Put thread back ───────────────────────────────────────────────────
        {
            let mut map = self.thread_map.lock().await;
            map.insert(conversation_id.clone(), guard.into_thread());
        }

        let response = llm_result?;

        // Store assistant response
        let assistant_message_id = uuid::Uuid::new_v4().to_string();

        // Record assistant message in transcript
        if let Some(ref transcript_store) = self.transcript_store {
            transcript_store.append(
                &conversation_id,
                "agent",
                &user_id,
                &conversation_id,
                TranscriptMessage::new("assistant", &response.message.content),
            );
            if let Some(ref budget) = self.disk_budget {
                if let Err(e) = budget.track_item(
                    &conversation_id,
                    format!("transcript-assistant-{}", assistant_message_id),
                    BudgetCategory::Transcript,
                    response.message.content.len(),
                ) {
                    warn!("Failed to track assistant transcript in disk budget: {}", e);
                }
            }
        }

        // Persist assistant response via MemoryManager (episodic memory)
        if let Some(ref mm) = self.memory_manager {
            if let Err(e) = mm
                .remember_message(
                    &user_id,
                    &conversation_id,
                    "assistant",
                    &response.message.content,
                )
                .await
            {
                warn!("MemoryManager: failed to store assistant message: {}", e);
            }
        }

        if let Some(ref store) = self.chat_history {
            use crate::memory::ChatMessage;
            let chat_msg = ChatMessage::new(
                &conversation_id,
                &user_id,
                "assistant",
                &response.message.content,
            );
            let msg_id = chat_msg.id.clone();
            if let Err(e) = store.store_message(chat_msg).await {
                error!("Failed to store assistant message: {}", e);
            }
            if let Some(ref search) = self.session_search {
                if let Err(e) = search
                    .index_message(
                        &msg_id,
                        &conversation_id,
                        &user_id,
                        &response.message.content,
                        "assistant",
                    )
                    .await
                {
                    error!("Failed to index assistant message for search: {}", e);
                }
            }
        } else if let Some(ref search) = self.session_search {
            if let Err(e) = search
                .index_message(
                    &assistant_message_id,
                    &conversation_id,
                    &user_id,
                    &response.message.content,
                    "assistant",
                )
                .await
            {
                error!("Failed to index assistant message for search: {}", e);
            }
        }

        // Only cache the response if it should be cached
        let tool_call_count = tools_used_this_turn.len();
        if should_cache && are_tools_cacheable(&tools_used_this_turn) {
            self.response_cache
                .set(
                    &user_id,
                    &conversation_id,
                    &content,
                    response.message.content.clone(),
                    tools_used_this_turn,
                )
                .await;
        }

        // Notify completed
        let response_content = response.message.content.clone();
        (progress_cb)(ProgressEvent::Completed {
            response: response_content.clone(),
            turn_id: turn_id.clone(),
        })
        .await;

        // ── Online risk scan: auto-collect badcases into the pending pool ──
        self.scan_turn_for_badcase(
            &content,
            &response_content,
            tool_call_count,
            &turn_id,
            &conversation_id,
        );

        // Create outgoing message with full metadata
        let mut outgoing = OutgoingMessage::new(
            crate::channels::ConversationId(conversation_id),
            response_content,
        );
        if let Some(ref reasoning) = response.message.reasoning_content {
            if !reasoning.is_empty() {
                outgoing.reasoning_content = Some(reasoning.clone());
            }
        }
        if let Some(ref calls) = response.message.tool_calls {
            if !calls.is_empty() {
                outgoing.tool_calls = Some(calls.clone());
            }
        }
        outgoing.usage = response.usage;

        // ── Retrospect: background trajectory reflection ─────────────────
        if let Some(ref engine) = self.retrospect_engine {
            let counter = self
                .retrospect_counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                + 1;
            let interval = engine.config.interval as u64;
            let min_turns = engine.config.min_turns as u64;

            if counter >= min_turns && counter.is_multiple_of(interval) {
                let engine = engine.clone();
                let mm = self.memory_manager.clone();
                let uid = user_id.clone();
                let criteria = cfg
                    .reflection_config
                    .as_ref()
                    .map(|rc| rc.criteria.clone())
                    .unwrap_or_default();
                let turns = retrospect_turns.clone();
                let total = retrospect_turn_count;

                tokio::spawn(async move {
                    match engine.retrospect(&turns, total, &criteria).await {
                        Ok(retrospect_result) => {
                            info!(
                                "Retrospect trajectory reflection at turn {}: {}",
                                retrospect_result.turn_count, retrospect_result.observation
                            );

                            // Compute dynamic importance from critique scores
                            let importance =
                                crate::agent::reflection::critic::compute_retrospect_importance(
                                    &retrospect_result.critique,
                                );

                            // Build metadata from critique for richer memory browsing
                            let metadata = serde_json::json!({
                                "turn_count": retrospect_result.turn_count,
                                "dimension_scores": retrospect_result.critique.dimension_scores,
                                "weaknesses": retrospect_result.critique.weaknesses,
                                "suggested_improvements": retrospect_result.critique.suggested_improvements,
                            });

                            // Write observation to memory.
                            if let Some(ref mm) = mm {
                                if let Err(e) = mm
                                    .observe_with_metadata(
                                        &uid,
                                        retrospect_result.observation,
                                        "interaction_pattern",
                                        importance,
                                        metadata,
                                    )
                                    .await
                                {
                                    warn!("Failed to persist interaction pattern: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Retrospect trajectory reflection failed: {}", e);
                        }
                    }
                });
            }
        }

        Ok(outgoing)
    }

    /// Attach the 在线质量监控（§八）config to this agent.
    ///
    /// Snapshot from `GatewayConfig.eval.online_monitoring` at spawn time; the
    /// judge trigger in [`scan_turn_for_badcase`] reads it without holding any
    /// lock across an await.
    pub fn with_online_monitoring(
        mut self,
        online_monitoring: crate::gateway::config::OnlineMonitoringConfig,
    ) -> Self {
        self.online_monitoring = online_monitoring;
        self
    }

    /// Post-turn online risk scan: when a completed turn trips a risk signal,
    /// insert it into the pending-badcase pool (source `online:risk`). Runs
    /// fire-and-forget, mirroring the retrospect hook above.
    ///
    /// §八 在线质量监控: when the risk-signal count reaches the configured
    /// `online_monitoring.llm_judge_risk_threshold`, the flagged turn is also
    /// deep-judged by an LLM [`Critic`] before it is inserted, and a compact
    /// verdict summary is appended to the badcase's `risk_signals`. The judge
    /// runs in the same fire-and-forget task; any failure is `warn!`ed and
    /// never breaks the turn.
    fn scan_turn_for_badcase(
        &self,
        input: &str,
        response: &str,
        tool_call_count: usize,
        turn_id: &str,
        conversation_id: &str,
    ) {
        let (Some(checker), Some(store)) =
            (self.risk_checker.as_ref(), self.pending_badcase_store.as_ref())
        else {
            return;
        };
        // No persisted turn to key the badcase on — nothing to collect.
        if turn_id.is_empty() || response.is_empty() {
            return;
        }
        let record = crate::eval::RiskTurnInput {
            input: input.to_string(),
            response: response.to_string(),
            tool_call_count,
        };
        let risks = checker.scan_turn(&record);
        if risks.is_empty() {
            return;
        }

        // ── §八 在线质量监控: snapshot config eagerly (no lock across await) ──
        // The config is cloned here (never held as a lock) and moved into the
        // fire-and-forget task below.
        let monitoring = self.online_monitoring.clone();
        let deep_judge = if should_deep_judge(
            risks.len(),
            monitoring.enabled,
            monitoring.llm_judge_risk_threshold,
        ) {
            Some((monitoring.llm_judge_risk_threshold.max(1), monitoring.judge_model))
        } else {
            None
        };

        let provider = self.provider.clone();
        let store = Arc::clone(store);
        let session_id = self
            .session_id
            .clone()
            .unwrap_or_else(|| conversation_id.to_string());
        let agent_id = self.agent_id.clone();
        let turn_id = turn_id.to_string();
        tokio::spawn(async move {
            let mut risk_signals = risks;
            // Deep LLM judge on the flagged turn. Runs before the pending insert
            // so the verdict can ride along on the badcase row.
            if let Some((threshold, judge_model)) = deep_judge {
                let mut critic = Critic::new(provider);
                if let Some(model) = judge_model {
                    critic = critic.with_model(model);
                }
                let trajectory = format!(
                    "=== TURN (high-risk) ===\nUser: {}\n\nAssistant: {}",
                    record.input, record.response
                );
                let criteria = QualityCriteria::default();
                match critic
                    .evaluate_trajectory(&trajectory, &criteria, None)
                    .await
                {
                    Ok(critique) => {
                        let summary = judge_summary(&critique);
                        info!(
                            "Online monitoring: LLM judge verdict for turn {} ({} risk signals >= threshold {}): {}",
                            turn_id, risk_signals.len(), threshold, summary
                        );
                        risk_signals.push(summary);
                    }
                    Err(e) => {
                        warn!(
                            "Online monitoring: LLM judge failed for turn {} ({} risk signals): {}",
                            turn_id,
                            risk_signals.len(),
                            e
                        );
                    }
                }
            }

            let params = crate::eval::InsertPendingParams {
                source: crate::eval::PendingSource::OnlineRisk,
                turn_id: Some(turn_id),
                session_id: Some(session_id),
                agent_id: Some(agent_id),
                input: record.input,
                response: record.response,
                risk_signals,
            };
            if let Err(e) = store.insert_pending(&params).await {
                warn!("Failed to record online risk badcase: {}", e);
            }
        });
    }

    /// Process a message in persistent session mode with an execution
    /// controller.
    ///
    /// The controller is attached before processing and detached afterward,
    /// enabling pause/resume/step/cancel during the tool-call loop.
    pub async fn process_message_with_controller(
        &self,
        message: IncomingMessage,
        controller: Arc<ExecutionController>,
        max_iterations: usize,
    ) -> crate::Result<OutgoingMessage> {
        {
            let mut ctrl = self.execution_controller.write().await;
            *ctrl = Some(controller);
            let mut max_iter = self.max_tool_iterations_override.write().await;
            *max_iter = Some(max_iterations);
        }

        let result = self.process_message(message).await;

        {
            let mut ctrl = self.execution_controller.write().await;
            *ctrl = None;
            let mut max_iter = self.max_tool_iterations_override.write().await;
            *max_iter = None;
        }

        result
    }

    /// Execute a single tool call, using streaming when the tool advertises
    /// `capabilities.streaming`.
    async fn execute_single_tool(
        &self,
        tool_call: &ToolCall,
        tool_context: &ToolContext,
        progress_cb: &ProgressCallback,
        context_id: &str,
    ) -> ToolResult {
        let tool_name = tool_call.function.name.clone();
        let capabilities = self.tools.get_capabilities(&tool_name);

        if capabilities.streaming {
            self.execute_single_tool_stream(tool_call, tool_context, progress_cb, context_id)
                .await
        } else {
            self.execute_single_tool_buffered(tool_call, tool_context, progress_cb, context_id)
                .await
        }
    }

    /// Buffered execution path for tools that do not support streaming.
    async fn execute_single_tool_buffered(
        &self,
        tool_call: &ToolCall,
        tool_context: &ToolContext,
        progress_cb: &ProgressCallback,
        context_id: &str,
    ) -> ToolResult {
        let tool_name = tool_call.function.name.clone();

        let _start = std::time::Instant::now();
        let result = self
            .tools
            .execute_call(&tool_call.function, tool_context)
            .await;
        let execution_time_ms = _start.elapsed().as_millis() as u64;
        match result {
            Ok(exec_result) => {
                // Reset circuit-breaker on success
                self.tools.reset_failure(&tool_name);
                let tool_data = exec_result.data.clone();
                let tool_result = exec_result.to_tool_result(&tool_call.id);
                let result_str = tool_result.content.clone();

                // Extract artifacts from successful tool results
                self.extract_and_store_artifacts(context_id, &result_str, &tool_name);

                // Notify tool result
                (progress_cb)(ProgressEvent::ToolResult {
                    name: tool_name.clone(),
                    result: result_str.chars().take(200).collect(), // Truncate for display
                    data: tool_data,
                    execution_time_ms,
                })
                .await;

                info!("Tool {} executed successfully", tool_name);
                tool_result
            }
            Err(e) => {
                // Record failure for circuit-breaker
                self.tools.record_failure(&tool_name);
                let error_msg = format!("Tool execution failed: {}", e);

                // Notify tool error
                (progress_cb)(ProgressEvent::ToolResult {
                    name: tool_name.clone(),
                    result: error_msg.clone(),
                    data: None,
                    execution_time_ms,
                })
                .await;

                error!("Tool {} failed: {}", tool_name, e);
                ToolResult::error(&tool_call.id, error_msg)
            }
        }
    }

    /// Streaming execution path for tools that advertise streaming support.
    async fn execute_single_tool_stream(
        &self,
        tool_call: &ToolCall,
        tool_context: &ToolContext,
        progress_cb: &ProgressCallback,
        context_id: &str,
    ) -> ToolResult {
        let tool_name = tool_call.function.name.clone();
        let progress_cb = progress_cb.clone();

        let _start = std::time::Instant::now();
        let result = self
            .tools
            .execute_call_streaming(&tool_call.function, tool_context, |chunk| {
                let progress_cb = progress_cb.clone();
                let tool_name = tool_name.clone();
                async move {
                    let (chunk_text, is_error) = match chunk {
                        ToolExecutionChunk::Output(text) => (text, false),
                        ToolExecutionChunk::Error(text) => (text, true),
                        ToolExecutionChunk::Data(_) | ToolExecutionChunk::Done => return,
                    };
                    (progress_cb)(ProgressEvent::ToolResultDelta {
                        name: tool_name,
                        chunk: chunk_text,
                        is_error,
                    })
                    .await;
                }
            })
            .await;
        let execution_time_ms = _start.elapsed().as_millis() as u64;

        match result {
            Ok(exec_result) => {
                self.tools.reset_failure(&tool_name);
                let tool_data = exec_result.data.clone();
                let tool_result = exec_result.to_tool_result(&tool_call.id);
                let result_str = tool_result.content.clone();

                self.extract_and_store_artifacts(context_id, &result_str, &tool_name);

                (progress_cb)(ProgressEvent::ToolResult {
                    name: tool_name.clone(),
                    result: result_str.chars().take(200).collect(),
                    data: tool_data,
                    execution_time_ms,
                })
                .await;

                info!("Streaming tool {} executed successfully", tool_name);
                tool_result
            }
            Err(e) => {
                self.tools.record_failure(&tool_name);
                let error_msg = format!("Tool execution failed: {}", e);
                (progress_cb)(ProgressEvent::ToolResult {
                    name: tool_name.clone(),
                    result: error_msg.clone(),
                    data: None,
                    execution_time_ms,
                })
                .await;
                error!("Streaming tool {} failed: {}", tool_name, e);
                ToolResult::error(&tool_call.id, error_msg)
            }
        }
    }

    /// Process a message with progress callbacks and an execution controller.
    pub async fn process_message_with_progress_and_controller(
        &self,
        message: IncomingMessage,
        progress_cb: ProgressCallback,
        controller: Arc<ExecutionController>,
        max_iterations: usize,
    ) -> crate::Result<OutgoingMessage> {
        {
            let mut ctrl = self.execution_controller.write().await;
            *ctrl = Some(controller);
            let mut max_iter = self.max_tool_iterations_override.write().await;
            *max_iter = Some(max_iterations);
        }

        let result = self
            .process_message_with_progress(message, progress_cb)
            .await;

        {
            let mut ctrl = self.execution_controller.write().await;
            *ctrl = None;
            let mut max_iter = self.max_tool_iterations_override.write().await;
            *max_iter = None;
        }

        result
    }

    /// Run a message in one-shot mode (no persistence) with an execution
    /// controller.
    ///
    /// The thread context is discarded after execution completes.
    pub async fn run_message_with_controller(
        &self,
        message: IncomingMessage,
        controller: Arc<ExecutionController>,
        max_iterations: usize,
    ) -> crate::Result<OutgoingMessage> {
        let conversation_id = message.conversation_id.0.clone();

        {
            let mut ctrl = self.execution_controller.write().await;
            *ctrl = Some(controller);
            let mut max_iter = self.max_tool_iterations_override.write().await;
            *max_iter = Some(max_iterations);
        }

        let result = self.process_message(message).await;

        {
            let mut ctrl = self.execution_controller.write().await;
            *ctrl = None;
            let mut max_iter = self.max_tool_iterations_override.write().await;
            *max_iter = None;
        }

        // Run mode: discard the thread after execution
        {
            let mut map = self.thread_map.lock().await;
            map.remove(&conversation_id);
        }

        result
    }

    /// Resolve the effective model id for a conversation, using the same
    /// precedence as the send path: per-session binding > temporary override >
    /// agent default > provider default.
    async fn resolve_model_id(&self, conversation_id: &str) -> String {
        let session_model = self
            .session_models
            .read()
            .await
            .get(conversation_id)
            .cloned();
        let override_model = self.model_override.read().await.clone();
        session_model
            .or(override_model)
            .or(self.model.clone())
            .unwrap_or_else(|| self.provider.default_model().to_string())
    }

    /// Persist a compact snapshot of one outgoing LLM request — resolved model
    /// id, system prompt, and the tool names/schemas offered — into the
    /// `request_snapshots` side table, for post-hoc debugging of turns where
    /// the model behaved oddly. Fire-and-forget: insert failures are logged,
    /// never propagated. Only the first send attempt of a request is
    /// snapshotted (a compaction retry changes the messages, not the header).
    fn persist_request_snapshot(
        &self,
        context: &Context,
        model_id: &str,
        tools: &[crate::providers::ToolDefinition],
    ) {
        let Some(store) = self.session_store.clone() else {
            return;
        };
        // Copy everything the snapshot needs into owned data before spawning:
        // `RequestSnapshot` borrows, and borrowed locals cannot escape into a
        // `'static` task.
        let session_id = self.session_id.clone();
        let conversation_id = context.id().to_string();
        let agent_id = self.agent_id.clone();
        let model = model_id.to_string();
        let system_prompt = context.system_prompt().to_string();
        let tools_json = super::session_store::compact_tools_json(tools);
        tokio::spawn(async move {
            let snapshot = super::session_store::RequestSnapshot {
                session_id: session_id.as_deref(),
                conversation_id: Some(&conversation_id),
                agent_id: Some(&agent_id),
                model: &model,
                system_prompt: &system_prompt,
                tools_json: &tools_json,
            };
            if let Err(e) = store.save_request_snapshot(&snapshot).await {
                warn!("Failed to save request snapshot: {}", e);
            }
        });
    }

    /// Get a completion from the LLM, handling tool calls
    async fn get_completion(
        &self,
        context: &mut Context,
        user_id: &str,
    ) -> crate::Result<crate::providers::CompletionResponse> {
        let cfg = self.config_snapshot();
        // If the context is over-budget, reduce it before sending (persisting
        // the compaction mask so a restart can rehydrate the same boundary).
        // No observability collector exists on this (non-progress) path, so
        // compression events here are not recorded.
        self.compact_context_if_needed(context, &cfg, None).await;

        // Attachment refs in tool results: current turn materializes to
        // images, older turns degrade to placeholders (request clone only).
        let mut messages = context.to_messages();
        crate::attachments::materialize_history(&mut messages);

        // Get available tools
        let tool_context =
            self.build_tool_context(user_id, context.id(), context.delegation().cloned());
        let tool_defs = self.tools.get_available(&tool_context);
        let has_tools = !tool_defs.is_empty();

        // Convert FunctionDefinition to ToolDefinition. Kept owned so the
        // request can be re-armed after a compaction retry below.
        let tools: Vec<crate::providers::ToolDefinition> =
            if has_tools && self.provider.supports_tools() {
                tool_defs
                    .into_iter()
                    .map(|f| crate::providers::ToolDefinition {
                        tool_type: "function".to_string(),
                        function: f,
                    })
                    .collect()
            } else {
                Vec::new()
            };

        let extra = self.extra_params.read().await.clone();
        let mut request = CompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(cfg.temperature),
            max_tokens: Some(cfg.max_tokens),
            stream: false,
            extra,
            ..Default::default()
        };
        self.patch_request_for_reasoning(&mut request);
        if !tools.is_empty() {
            request.tools = Some(tools.clone());
        }

        // Check live cost guard before calling provider
        if let Some(ref guard) = self.cost_guard {
            if guard.is_exceeded() {
                return Err(crate::error::SyscityError::Validation(
                    "Budget limit exceeded — refusing provider call. Adjust daily_limit_cents or \
                     hourly_action_limit in config."
                        .to_string(),
                ));
            }
        }

        // Snapshot what is about to be sent (one row per LLM request).
        let model_id = self.resolve_model_id(context.id()).await;
        self.persist_request_snapshot(context, &model_id, &tools);

        // Get completion — use model router when available for key rotation /
        // fallback. If the provider rejects the request as over its context
        // window, compact the context and retry once instead of failing.
        let mut retried = false;
        let response = loop {
            let outcome = if let Some(ref router) = self.model_router {
                let req_tools = request.tools.take();
                match router
                    .complete_with_route(&model_id, request.messages, req_tools)
                    .await
                {
                    Ok((resp, rec)) => {
                        // Non-progress path has no turn collector; surface the
                        // route decision at debug level for replay/debugging.
                        debug!(
                            "[observe] route record: {}",
                            serde_json::to_string(&rec).unwrap_or_default()
                        );
                        Ok(resp)
                    }
                    Err(e) => Err(e),
                }
            } else {
                self.provider.complete(request.clone()).await
            };

            match outcome {
                Ok(r) => break r,
                Err(e)
                    if !retried
                        && crate::model_router::FailureClass::from_error(&e, None)
                            == crate::model_router::FailureClass::ContextLength =>
                {
                    retried = true;
                    info!(
                        "[compaction] provider rejected context as too long — compacting and \
                         retrying once"
                    );
                    self.compact_context_forced(context, &cfg, None).await;
                    let mut messages = context.to_messages();
                    crate::attachments::materialize_history(&mut messages);
                    request.messages = messages;
                    if !tools.is_empty() {
                        request.tools = Some(tools.clone());
                    }
                }
                Err(e) => return Err(e),
            }
        };

        // Record token usage in cost guard
        if let Some(ref guard) = self.cost_guard {
            if let Some(ref usage) = response.usage {
                guard.record_usage(
                    usage.prompt_tokens as u64,
                    usage.completion_tokens as u64,
                    response.model.as_str(),
                );
            }
        }

        // Handle tool calls if present
        if let Some(tool_calls) = &response.message.tool_calls {
            if !tool_calls.is_empty() {
                debug!("Processing {} tool calls", tool_calls.len());
                return self
                    .handle_tool_calls(context, &response, tool_calls, user_id)
                    .await;
            }
        }

        // Add assistant message to context
        context.add_message(response.message.clone());

        Ok(response)
    }

    /// If `context` is over-budget, compact it and persist a durable
    /// compaction record so a later restart can rehydrate `[summary] + tail`
    /// instead of replaying the full history.
    async fn compact_context_if_needed(
        &self,
        context: &mut Context,
        cfg: &crate::agent::AgentConfig,
        collector: Option<&mut TurnMetricsCollector>,
    ) {
        // If the context is over-budget, try to reduce it before sending.
        if context.needs_pruning() {
            self.compact_context_forced(context, cfg, collector).await;
        }
    }

    /// Force a compaction regardless of the local token budget, then persist
    /// the boundary. Used by the overflow retry: when the provider rejects the
    /// request as over its real context window, `needs_pruning()` may still be
    /// false (our estimate is larger than the model's limit), so compaction
    /// must not be gated on it.
    async fn compact_context_forced(
        &self,
        context: &mut Context,
        cfg: &crate::agent::AgentConfig,
        collector: Option<&mut TurnMetricsCollector>,
    ) {
        let tokens_before = context.token_count();
        if let Some(ref compaction_model) = cfg.compaction_model {
            // LLM-assisted compaction: produce a high-quality summary.
            let compressor =
                crate::agent::compressor::ContextCompressor::new(cfg.max_context_tokens);
            let history = context.history().to_vec();
            let compacted = compressor
                .compact_with_llm(&history, &self.provider, Some(compaction_model.as_str()), 2, 6)
                .await;
            context.replace_messages(compacted);
            if let Some(c) = collector {
                c.record_compression(tokens_before, context.token_count(), "llm_summary");
            }
        } else {
            // Fallback: drop middle messages and insert a placeholder summary.
            // This keeps the context coherent without an extra LLM call.
            context.summarize();
            if let Some(c) = collector {
                c.record_compression(tokens_before, context.token_count(), "heuristic_summary");
            }
        }
        self.record_compaction_boundary(context).await;
    }

    /// Persist the current compaction mask.
    ///
    /// The boundary anchor is the first message after the summary that is safe
    /// to replay from persistence: a user message or an assistant message
    /// without tool calls. Tool results and tool-calling assistant turns are
    /// not stored in `chat_messages`, so anchoring on them would leave a broken
    /// pair on rehydration.
    async fn record_compaction_boundary(&self, context: &Context) {
        let Some(summary_idx) = context
            .history()
            .iter()
            .position(|m| m.name.as_deref() == Some("compaction_summary"))
        else {
            return;
        };
        let summary = context.history()[summary_idx].content.clone();
        let Some(boundary) = context.history()[summary_idx + 1..].iter().find(|m| {
            m.role != crate::providers::Role::Tool
                && !(m.role == crate::providers::Role::Assistant && m.tool_calls.is_some())
        }) else {
            return;
        };
        let Some(ref store) = self.chat_history else {
            return;
        };
        if let Err(e) = store
            .record_compaction(
                context.id(),
                &boundary.role.to_string(),
                &boundary.content,
                &summary,
            )
            .await
        {
            warn!("[compaction] failed to persist compaction boundary: {}", e);
        }
    }

    /// Handle tool calls from the LLM
    async fn handle_tool_calls(
        &self,
        context: &mut Context,
        original_response: &crate::providers::CompletionResponse,
        tool_calls: &[ToolCall],
        user_id: &str,
    ) -> crate::Result<crate::providers::CompletionResponse> {
        let cfg = self.config_snapshot();
        // Check iteration limit before processing
        if !context.increment_tool_iteration() {
            warn!("Tool iteration limit reached ({}), stopping", context.tool_iterations());

            // Return a response indicating the limit was reached
            return Ok(crate::providers::CompletionResponse {
                message: Message {
                    role: Role::Assistant,
                    content: format!(
                        "I've reached the maximum number of tool calls ({}) for this request. The \
                         task may be too complex or the tools may not be providing the expected \
                         results. Please try a more specific request or break the task into \
                         smaller steps.",
                        Context::DEFAULT_MAX_TOOL_ITERATIONS
                    ),
                    content_blocks: None,
                    reasoning_content: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    metadata: None,
                },
                usage: None,
                model: "system".to_string(),
                finish_reason: Some("tool_limit".to_string()),
            });
        }

        // Filter out duplicate tool calls before adding assistant message
        // This ensures the tool_call count matches the tool result count,
        // which is required by APIs like DeepSeek that enforce strict pairing.
        let filtered_tool_calls: Vec<ToolCall> = tool_calls
            .iter()
            .take(cfg.max_concurrent_tools)
            .filter(|tc| {
                let tool_name = &tc.function.name;
                let tool_args = &tc.function.arguments;
                if context.is_tool_call_duplicate(tool_name, tool_args) {
                    warn!("Duplicate tool call detected: {} with same args, skipping", tool_name);
                    false
                } else {
                    true
                }
            })
            .filter(|tc| {
                if !context.is_tool_allowed(&tc.function.name) {
                    warn!(
                        "Tool '{}' is not allowed in this delegation scope, skipping",
                        tc.function.name
                    );
                    false
                } else {
                    true
                }
            })
            .cloned()
            .collect();

        if filtered_tool_calls.is_empty() {
            // All tool calls were duplicates or disallowed; return the original
            // response as-is (the assistant message with tool_calls was never
            // added to context)
            return Ok(original_response.clone());
        }

        // Execute tools FIRST, before adding the assistant message.
        // This avoids context.prune_if_needed() removing the assistant because
        // its tool_call IDs don't yet have matching tool results.
        let tool_context = self
            .build_tool_context(user_id, context.id(), context.delegation().cloned())
            .with_timeout(std::time::Duration::from_secs(120));

        let mut results = Vec::new();

        for tool_call in &filtered_tool_calls {
            let tool_name = tool_call.function.name.clone();
            let tool_args = tool_call.function.arguments.clone();

            // Record this tool call before executing
            context.record_tool_call(&tool_name, &tool_args);

            debug!("Executing tool: {}", tool_name);

            let result = match self
                .tools
                .execute_call(&tool_call.function, &tool_context)
                .await
            {
                Ok(exec_result) => {
                    // Reset circuit-breaker on success
                    self.tools.reset_failure(&tool_call.function.name);
                    let tool_result = exec_result.to_tool_result(&tool_call.id);
                    // Extract artifacts from successful tool results
                    self.extract_and_store_artifacts(
                        context.id(),
                        &tool_result.content,
                        &tool_call.function.name,
                    );
                    info!("Tool {} executed successfully", tool_call.function.name);
                    tool_result
                }
                Err(e) => {
                    // Record failure for circuit-breaker
                    self.tools.record_failure(&tool_call.function.name);
                    error!("Tool {} failed: {}", tool_call.function.name, e);
                    ToolResult::error(&tool_call.id, format!("Tool execution failed: {}", e))
                }
            };

            results.push(result);
        }

        // NOW add assistant message with ONLY non-duplicate tool calls,
        // immediately followed by tool results as a single atomic batch.
        // This prevents prune_if_needed() from removing the assistant before
        // its tool results are added.
        let mut assistant_msg = original_response.message.clone();
        assistant_msg.tool_calls = Some(filtered_tool_calls.clone());

        let mut batch = Vec::with_capacity(1 + results.len());
        batch.push(assistant_msg);
        for result in &results {
            batch.push(Message {
                role: Role::Tool,
                content: result.content.clone(),
                content_blocks: None,
                reasoning_content: None,
                name: None,
                tool_calls: None,
                tool_call_id: Some(result.tool_call_id.clone()),
                metadata: None,
            });
        }
        context.add_batch(batch);

        // Check execution controller before next iteration
        {
            let ctrl_guard = self.execution_controller.read().await;
            if let Some(ref ctrl) = *ctrl_guard {
                if let Err(reason) = ctrl.check_and_wait().await {
                    return Ok(crate::providers::CompletionResponse {
                        message: Message {
                            role: Role::Assistant,
                            content: format!("Execution halted: {}", reason),
                            content_blocks: None,
                            reasoning_content: None,
                            name: None,
                            tool_calls: None,
                            tool_call_id: None,
                            metadata: None,
                        },
                        usage: None,
                        model: "system".to_string(),
                        finish_reason: Some("cancelled".to_string()),
                    });
                }
            }
        }

        // Get final response (boxed to avoid recursive async issue)
        Box::pin(self.get_completion(context, user_id)).await
    }

    /// Get a completion from the LLM with progress callbacks
    async fn get_completion_with_progress(
        &self,
        context: &mut Context,
        collector: &mut TurnMetricsCollector,
        progress_cb: ProgressCallback,
        user_id: &str,
    ) -> crate::Result<crate::providers::CompletionResponse> {
        let cfg = self.config_snapshot();
        // Attachment refs in tool results: current turn materializes to
        // images, older turns degrade to placeholders (request clone only).
        let mut messages = context.to_messages();
        crate::attachments::materialize_history(&mut messages);
        let user_msg_count = messages.iter().filter(|m| m.role == Role::User).count();
        let assistant_msg_count = messages
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .count();
        let tool_msg_count = messages.iter().filter(|m| m.role == Role::Tool).count();
        info!(
            "get_completion_with_progress: entry — msgs={} (user={}, asst={}, tool={})",
            messages.len(),
            user_msg_count,
            assistant_msg_count,
            tool_msg_count
        );

        // Get available tools
        let tool_context =
            self.build_tool_context(user_id, context.id(), context.delegation().cloned());
        let tool_defs = self.tools.get_available(&tool_context);
        let has_tools = !tool_defs.is_empty();

        let extra = self.extra_params.read().await.clone();
        let mut request = CompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(cfg.temperature),
            max_tokens: Some(cfg.max_tokens),
            stream: true,
            extra,
            ..Default::default()
        };
        self.patch_request_for_reasoning(&mut request);

        // Convert FunctionDefinition to ToolDefinition. Kept owned so the
        // request can be re-armed after a compaction retry below.
        let tools: Vec<crate::providers::ToolDefinition> =
            if has_tools && self.provider.supports_tools() {
                tool_defs
                    .into_iter()
                    .map(|f| crate::providers::ToolDefinition {
                        tool_type: "function".to_string(),
                        function: f,
                    })
                    .collect()
            } else {
                Vec::new()
            };
        if !tools.is_empty() {
            request.tools = Some(tools.clone());
        }

        // Check live cost guard before calling provider
        if let Some(ref guard) = self.cost_guard {
            if guard.is_exceeded() {
                return Err(crate::error::SyscityError::Validation(
                    "Budget limit exceeded — refusing provider call. Adjust daily_limit_cents or \
                     hourly_action_limit in config."
                        .to_string(),
                ));
            }
        }

        // Notify generating (starting)
        (progress_cb)(ProgressEvent::Generating { content: None }).await;

        // Snapshot what is about to be sent (one row per LLM request).
        let model_id = self.resolve_model_id(context.id()).await;
        self.persist_request_snapshot(context, &model_id, &tools);

        // Snapshot the full request messages (untruncated) for the observability
        // full-trace input. Captured before the stream-setup loop below because
        // the model-router branch moves `request.messages`.
        let input_json = serde_json::to_string(&request.messages).map_err(|e| {
            crate::error::SyscityError::Internal(format!(
                "Failed to serialize request messages: {}",
                e
            ))
        })?;

        // Get streaming completion — use model router when available. If the
        // provider rejects the request as over its context window at stream
        // setup (before any bytes are emitted), compact and retry once.
        let mut retried = false;
        let (raw_stream, family, round_model, round_provider, route_record) = loop {
            let setup = if let Some(ref router) = self.model_router {
                let req_tools = request.tools.take();
                let stream = router
                    .stream_with_route(&model_id, request.messages, req_tools)
                    .await;
                let provider = router
                    .provider_for_model(&model_id)
                    .await
                    .unwrap_or_else(|| "router".to_string());
                // Capture the route decision from the successful setup attempt.
                let route_record = stream.as_ref().ok().map(|(_, rec)| rec.clone());
                let stream = stream.map(|(s, _)| s);
                // When using model router, fall back to Generic stream family
                (
                    stream,
                    crate::providers::stream_wrappers::ProviderStreamFamily::Generic,
                    model_id.clone(),
                    provider,
                    route_record,
                )
            } else {
                let round_model = request
                    .model
                    .clone()
                    .unwrap_or_else(|| self.provider.default_model().to_string());
                let round_provider = self.provider.name().to_string();
                (
                    self.provider.stream(request.clone()).await,
                    self.provider.stream_family(),
                    round_model,
                    round_provider,
                    None,
                )
            };

            match setup {
                (Ok(stream), family, round_model, round_provider, route_record) => {
                    break (stream, family, round_model, round_provider, route_record);
                }
                (Err(e), _, _, _, _)
                    if !retried
                        && crate::model_router::FailureClass::from_error(&e, None)
                            == crate::model_router::FailureClass::ContextLength =>
                {
                    retried = true;
                    info!(
                        "[compaction] provider rejected streaming context as too long — \
                         compacting and retrying once"
                    );
                    self.compact_context_forced(context, &cfg, Some(&mut *collector))
                        .await;
                    let mut messages = context.to_messages();
                    crate::attachments::materialize_history(&mut messages);
                    request.messages = messages;
                    if !tools.is_empty() {
                        request.tools = Some(tools.clone());
                    }
                }
                (Err(e), _, _, _, _) => return Err(e),
            }
        };
        let registry = crate::providers::stream_wrappers::StreamFamilyRegistry::default();
        let mut stream = registry.apply(family, raw_stream);

        // Begin an observability round for this LLM call.
        collector.begin_round(&round_provider, &round_model, Some(input_json));
        // Persist the route decision on the turn record.
        if let Some(rec) = route_record {
            collector.record_route(rec);
        }

        let mut accumulated_text = String::new();
        let mut accumulated_reasoning = String::new();
        let mut accumulated_tool_calls: Vec<ToolCall> = Vec::new();
        let mut finish_reason: Option<String> = None;
        let mut usage: Option<crate::providers::Usage> = None;

        while let Some(chunk) = stream.next().await {
            // Emit reasoning delta
            if let Some(ref reasoning_delta) = chunk.reasoning_content {
                if !reasoning_delta.is_empty() {
                    collector.round_first_token();
                    collector.push_reasoning_delta(reasoning_delta);
                    accumulated_reasoning.push_str(reasoning_delta);
                    (progress_cb)(ProgressEvent::Generating {
                        content: Some(reasoning_delta.clone()),
                    })
                    .await;
                }
            }

            // Emit text delta
            if let Some(ref text_delta) = chunk.content {
                if !text_delta.is_empty() {
                    collector.round_first_token();
                    collector.push_text_delta(text_delta);
                    accumulated_text.push_str(text_delta);
                    (progress_cb)(ProgressEvent::ContentDelta { text: text_delta.clone() }).await;
                }
            }

            // Accumulate tool calls from stream
            if let Some(ref calls) = chunk.tool_calls {
                for call in calls {
                    // Merge partial tool calls by index (streaming deltas use index as key)
                    let key = call.index.unwrap_or(0);
                    if let Some(existing) = accumulated_tool_calls
                        .iter_mut()
                        .find(|c| c.index == Some(key) || (c.index.is_none() && c.id == call.id))
                    {
                        // Fill in id/type/name from first chunk if they were empty.
                        // Note: some providers (DeepSeek) send the function name
                        // in every delta chunk; unconditionally appending with
                        // push_str would duplicate it (file_readfile_read).
                        if existing.id.is_empty() && !call.id.is_empty() {
                            existing.id = call.id.clone();
                        }
                        if existing.call_type.is_empty() && !call.call_type.is_empty() {
                            existing.call_type = call.call_type.clone();
                        }
                        if existing.function.name.is_empty() && !call.function.name.is_empty() {
                            existing.function.name = call.function.name.clone();
                        }
                        // Some providers (DeepSeek) send the complete JSON
                        // arguments in every delta chunk. If the incoming
                        // chunk looks like a complete JSON value (starts
                        // with `{`/`[` and ends with `}`/`]`), replace the
                        // existing arguments rather than appending.
                        let incoming = &call.function.arguments;
                        let is_complete_json =
                            incoming.starts_with(['{', '[']) && incoming.ends_with(['}', ']']);
                        if is_complete_json && !existing.function.arguments.is_empty() {
                            existing.function.arguments = incoming.clone();
                        } else {
                            existing.function.arguments.push_str(incoming);
                        }
                    } else {
                        accumulated_tool_calls.push(call.clone());
                    }
                }
            }

            if chunk.is_done {
                finish_reason = Some("stop".to_string());
                usage = chunk.usage;
                break;
            }
        }

        // Close the observability round with usage and an accurate finish reason.
        let round_finish_reason = if accumulated_tool_calls.is_empty() {
            "stop".to_string()
        } else {
            "tool_calls".to_string()
        };
        collector.end_round(usage.as_ref(), Some(round_finish_reason));

        // Build the final message
        let final_message = Message {
            role: Role::Assistant,
            content: accumulated_text.clone(),
            content_blocks: None,
            reasoning_content: if accumulated_reasoning.is_empty() {
                None
            } else {
                Some(accumulated_reasoning.clone())
            },
            name: None,
            tool_calls: if accumulated_tool_calls.is_empty() {
                None
            } else {
                Some(accumulated_tool_calls.clone())
            },
            tool_call_id: None,
            metadata: None,
        };

        let response = crate::providers::CompletionResponse {
            message: final_message,
            usage,
            model: self
                .model
                .clone()
                .unwrap_or_else(|| self.provider.default_model().to_string()),
            finish_reason,
        };

        // Record token usage in cost guard (approximate from accumulated text if no
        // usage provided)
        if let Some(ref guard) = self.cost_guard {
            let prompt_tokens = context
                .to_messages()
                .iter()
                .map(|m| m.content.len() / 4)
                .sum::<usize>() as u64;
            let completion_tokens =
                (accumulated_text.len() + accumulated_reasoning.len()) as u64 / 4;
            guard.record_usage(prompt_tokens, completion_tokens, response.model.as_str());
        }

        // Handle tool calls if present
        if let Some(ref tool_calls) = response.message.tool_calls {
            if !tool_calls.is_empty() {
                debug!("Processing {} tool calls with progress", tool_calls.len());
                return self
                    .handle_tool_calls_with_progress(
                        context,
                        &mut *collector,
                        &response,
                        tool_calls,
                        progress_cb,
                        user_id,
                    )
                    .await;
            }
        }

        // Add assistant message to context
        context.add_message(response.message.clone());

        // Accumulate token usage for non-tool-call responses
        if let Some(ref usage) = response.usage {
            context.accumulate_turn_token_usage(usage);
        }

        Ok(response)
    }

    /// Handle tool calls with progress callbacks
    async fn handle_tool_calls_with_progress(
        &self,
        context: &mut Context,
        collector: &mut TurnMetricsCollector,
        original_response: &crate::providers::CompletionResponse,
        tool_calls: &[ToolCall],
        progress_cb: ProgressCallback,
        user_id: &str,
    ) -> crate::Result<crate::providers::CompletionResponse> {
        let cfg = self.config_snapshot();
        // Accumulate token usage from the LLM response that produced these tool calls
        if let Some(ref usage) = original_response.usage {
            context.accumulate_turn_token_usage(usage);
        }

        // Check iteration limit before processing
        if !context.increment_tool_iteration() {
            warn!("Tool iteration limit reached ({}), stopping", context.tool_iterations());

            // Notify user about the limit
            (progress_cb)(ProgressEvent::Error {
                message: format!(
                    "Tool iteration limit reached ({}) - the agent was taking too many steps. \
                     Please try a more specific request.",
                    Context::DEFAULT_MAX_TOOL_ITERATIONS
                ),
            })
            .await;

            // Return a response indicating the limit was reached
            return Ok(crate::providers::CompletionResponse {
                message: Message {
                    role: Role::Assistant,
                    content: format!(
                        "I've reached the maximum number of tool calls ({}) for this request. The \
                         task may be too complex or the tools may not be providing the expected \
                         results. Please try a more specific request or break the task into \
                         smaller steps.",
                        Context::DEFAULT_MAX_TOOL_ITERATIONS
                    ),
                    content_blocks: None,
                    reasoning_content: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    metadata: None,
                },
                usage: None,
                model: "system".to_string(),
                finish_reason: Some("tool_limit".to_string()),
            });
        }

        // Filter out duplicate tool calls before adding assistant message
        // This ensures the tool_call count matches the tool result count,
        // which is required by APIs like DeepSeek that enforce strict pairing.
        let filtered_tool_calls: Vec<ToolCall> = tool_calls
            .iter()
            .take(cfg.max_concurrent_tools)
            .filter(|tc| {
                let tool_name = &tc.function.name;
                let tool_args = &tc.function.arguments;
                if context.is_tool_call_duplicate(tool_name, tool_args) {
                    warn!("Duplicate tool call detected: {} with same args, skipping", tool_name);

                    // Notify about duplicate via progress callback
                    let cb = progress_cb.clone();
                    let name = tool_name.clone();
                    tokio::spawn(async move {
                        (cb)(ProgressEvent::ToolResult {
                            name,
                            result: "[Duplicate tool call skipped - already executed with same \
                                     parameters]"
                                .to_string(),
                            data: None,
                            execution_time_ms: 0,
                        })
                        .await;
                    });

                    false
                } else {
                    true
                }
            })
            .filter(|tc| {
                if !context.is_tool_allowed(&tc.function.name) {
                    warn!(
                        "Tool '{}' is not allowed in this delegation scope, skipping",
                        tc.function.name
                    );

                    // Notify about the disallowed tool via progress callback
                    let cb = progress_cb.clone();
                    let name = tc.function.name.clone();
                    tokio::spawn(async move {
                        (cb)(ProgressEvent::ToolResult {
                            name,
                            result: "[Tool skipped - not allowed in this delegation scope]"
                                .to_string(),
                            data: None,
                            execution_time_ms: 0,
                        })
                        .await;
                    });

                    false
                } else {
                    true
                }
            })
            .cloned()
            .collect();

        if filtered_tool_calls.is_empty() {
            // All tool calls were duplicates or disallowed; return the original
            // response as-is
            return Ok(original_response.clone());
        }

        // Execute tools with progress FIRST, before adding the assistant message.
        // This avoids a critical issue: context.prune_if_needed() removes any
        // assistant message whose tool_call IDs don't yet appear in tool results.
        // By adding the assistant AFTER execution and immediately followed by
        // tool results, the tool_call/tool_result pairing is preserved.
        let tool_context = self
            .build_tool_context(user_id, context.id(), context.delegation().cloned())
            .with_timeout(std::time::Duration::from_secs(120));

        let mut results = Vec::new();

        for tool_call in &filtered_tool_calls {
            let tool_name = tool_call.function.name.clone();
            let tool_args = tool_call.function.arguments.clone();

            // Record this tool call before executing
            context.record_tool_call(&tool_name, &tool_args);

            // Notify tool calling
            (progress_cb)(ProgressEvent::ToolCalling {
                name: tool_name.clone(),
                arguments: tool_args,
            })
            .await;

            debug!("Executing tool: {}", tool_name);

            let _start = std::time::Instant::now();
            let result = self
                .execute_single_tool(tool_call, &tool_context, &progress_cb, context.id())
                .await;

            info!(
                "handle_tool_calls_with_progress: tool={} executed in {:?}",
                tool_name,
                _start.elapsed()
            );

            let rec = ToolCallRecord {
                name: tool_name.clone(),
                args: tool_call.function.arguments.to_string(),
                result: result.content.clone(),
                success: !result.is_error.unwrap_or(false),
                duration_ms: _start.elapsed().as_millis() as u64,
            };
            collector.record_tool(&rec);
            context.push_tool_call_record(rec);

            results.push(result);
        }

        // Build a map of tool_call_id -> result content for history persistence
        let tool_result_map: std::collections::HashMap<String, String> = results
            .iter()
            .map(|r| (r.tool_call_id.clone(), r.content.clone()))
            .collect();

        // NOW add assistant message with ONLY non-duplicate tool calls,
        // immediately followed by tool results as a single atomic batch.
        // This prevents prune_if_needed() from removing the assistant before
        // its tool results are added.
        let mut assistant_msg = original_response.message.clone();
        assistant_msg.tool_calls = Some(filtered_tool_calls.clone());

        let mut batch = Vec::with_capacity(1 + results.len());
        batch.push(assistant_msg);
        for result in &results {
            batch.push(Message {
                role: Role::Tool,
                content: result.content.clone(),
                content_blocks: None,
                reasoning_content: None,
                name: None,
                tool_calls: None,
                tool_call_id: Some(result.tool_call_id.clone()),
                metadata: None,
            });
        }
        context.add_batch(batch);

        // Check execution controller before next iteration
        {
            let ctrl_guard = self.execution_controller.read().await;
            if let Some(ref ctrl) = *ctrl_guard {
                if let Err(reason) = ctrl.check_and_wait().await {
                    return Ok(crate::providers::CompletionResponse {
                        message: Message {
                            role: Role::Assistant,
                            content: format!("Execution halted: {}", reason),
                            content_blocks: None,
                            reasoning_content: None,
                            name: None,
                            tool_calls: None,
                            tool_call_id: None,
                            metadata: None,
                        },
                        usage: None,
                        model: "system".to_string(),
                        finish_reason: Some("cancelled".to_string()),
                    });
                }
            }
        }

        // Get final response with progress (recursive LLM call)
        let recursive_llm_start = std::time::Instant::now();
        info!("handle_tool_calls_with_progress: calling recursive get_completion_with_progress ({} tools executed)", results.len());
        let mut final_response = Box::pin(self.get_completion_with_progress(
            context,
            &mut *collector,
            progress_cb,
            user_id,
        ))
        .await?;
        info!(
            "handle_tool_calls_with_progress: recursive get_completion_with_progress returned in {:?}",
            recursive_llm_start.elapsed()
        );

        // Accumulate token usage from this LLM completion in the tool loop
        if let Some(ref usage) = final_response.usage {
            context.accumulate_turn_token_usage(usage);
        }

        // Preserve tool calls from the original assistant message so that
        // downstream consumers (session_store, etc.) can see what tools were invoked.
        if let Some(ref original_calls) = original_response.message.tool_calls {
            match final_response.message.tool_calls {
                None => final_response.message.tool_calls = Some(original_calls.clone()),
                Some(ref mut existing) => {
                    let mut merged = original_calls.clone();
                    merged.append(existing);
                    final_response.message.tool_calls = Some(merged);
                }
            }
        }

        // Attach execution results to the preserved tool calls so that history
        // replay can show "Done" instead of "Running".
        if let Some(ref mut calls) = final_response.message.tool_calls {
            for call in calls.iter_mut() {
                if call.result.is_none() {
                    if let Some(result_content) = tool_result_map.get(&call.id) {
                        call.result = Some(result_content.clone());
                    }
                }
            }
        }

        Ok(final_response)
    }
}

/// Decide whether a post-turn risk scan should trigger the deep LLM Judge
/// (§八 在线质量监控).
///
/// Returns `true` only when online monitoring is enabled AND the number of
/// deterministic risk signals found on the turn is at least the configured
/// threshold. The threshold is floored at 1 so a `0` in config never silently
/// disables the judge.
fn should_deep_judge(risk_count: usize, enabled: bool, threshold: usize) -> bool {
    enabled && risk_count >= threshold.max(1)
}

/// Compact single-line summary of an LLM judge critique, used to surface the
/// deep-evaluation verdict in the pending badcase row and the log.
fn judge_summary(critique: &Critique) -> String {
    let mut parts = Vec::new();
    if !critique.dimension_scores.is_empty() {
        let mut scores = critique
            .dimension_scores
            .iter()
            .map(|(k, v)| format!("{k}={v:.2}"))
            .collect::<Vec<_>>();
        // Stable ordering so identical verdicts render identically.
        scores.sort();
        parts.push(format!("scores[{}]", scores.join(", ")));
    }
    if let Some(obs) = critique.observation.as_deref() {
        parts.push(format!("observation: {obs}"));
    }
    if parts.is_empty() {
        format!("llm judge overall_score={:.2}", critique.overall_score)
    } else {
        format!("llm judge {}", parts.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use crate::error::SyscityError;
    use crate::memory::ChatHistoryStore;
    use crate::providers::{
        CompletionChunk, CompletionRequest, CompletionResponse, CompletionStream, Message,
        Provider, Usage,
    };

    /// A provider that fails the first completion with a context-length error
    /// and succeeds afterwards — models the "model real limit < our estimate"
    /// overflow that should trigger a compact-and-retry.
    struct ContextLengthThenOk {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Provider for ContextLengthThenOk {
        fn name(&self) -> &str {
            "ctx-overflow-test"
        }

        fn default_model(&self) -> &str {
            "test-model"
        }

        fn supports_tools(&self) -> bool {
            false
        }

        fn max_context(&self) -> usize {
            128_000
        }

        async fn complete(&self, _request: CompletionRequest) -> crate::Result<CompletionResponse> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return Err(SyscityError::ExternalService {
                    source: "Test provider: this model's maximum context length is 2048 tokens"
                        .into(),
                    cause: None,
                });
            }
            Ok(CompletionResponse {
                message: Message::assistant("ok"),
                model: self.default_model().to_string(),
                usage: Some(Usage::default()),
                finish_reason: Some("stop".to_string()),
            })
        }

        async fn stream(&self, _request: CompletionRequest) -> crate::Result<CompletionStream> {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let _ = tx.send(CompletionChunk {
                content: Some("ok".to_string()),
                reasoning_content: None,
                tool_calls: None,
                is_done: true,
                usage: None,
            });
            Ok(Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx)))
        }

        async fn health_check(&self) -> crate::Result<bool> {
            Ok(true)
        }

        async fn set_credential(
            &self,
            _credential: crate::model_router::Credential,
        ) -> crate::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_get_completion_retries_once_on_context_length() {
        let provider = Arc::new(ContextLengthThenOk { calls: AtomicUsize::new(0) });
        let store = Arc::new(crate::memory::DatabaseStore::new_in_memory().await.unwrap());
        let agent = crate::agent::Agent::new(
            crate::agent::AgentConfig::default(),
            provider.clone(),
            Arc::new(crate::tools::ToolRegistry::new()),
        )
        .with_chat_history(store.clone());

        let mut context =
            crate::agent::Context::new("conv-overflow", "You are a helpful assistant", 100_000);
        // Enough messages (> KEEP_FIRST + KEEP_LAST + 1) that a forced
        // `summarize()` actually shrinks the history, but short enough that the
        // local budget check says no pre-flight pruning is needed.
        for i in 0..10 {
            context.add_message(Message::user(format!("user message {}", i)));
            context.add_message(Message::assistant(format!("assistant message {}", i)));
        }
        assert!(!context.needs_pruning());

        let response = agent.get_completion(&mut context, "user1").await.unwrap();
        assert_eq!(response.message.content, "ok");
        // First call failed, second call (after compaction) succeeded.
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        // The durable compaction record was written for this conversation.
        let record = store
            .get_compaction("conv-overflow")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.conversation_id, "conv-overflow");
        assert!(!record.summary.is_empty());
        // The in-memory context was actually compacted: it now carries the
        // named summary and is much smaller than the original 20 messages.
        assert!(context
            .history()
            .iter()
            .any(|m| m.name.as_deref() == Some("compaction_summary")));
        assert!(context.message_count() < 20);
    }

    /// A provider whose completions always succeed.
    struct AlwaysOk;

    #[async_trait::async_trait]
    impl Provider for AlwaysOk {
        fn name(&self) -> &str {
            "always-ok-test"
        }

        fn default_model(&self) -> &str {
            "test-model"
        }

        fn supports_tools(&self) -> bool {
            false
        }

        fn max_context(&self) -> usize {
            128_000
        }

        async fn complete(&self, _request: CompletionRequest) -> crate::Result<CompletionResponse> {
            Ok(CompletionResponse {
                message: Message::assistant("ok"),
                model: self.default_model().to_string(),
                usage: Some(Usage::default()),
                finish_reason: Some("stop".to_string()),
            })
        }

        async fn stream(&self, _request: CompletionRequest) -> crate::Result<CompletionStream> {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let _ = tx.send(CompletionChunk {
                content: Some("ok".to_string()),
                reasoning_content: None,
                tool_calls: None,
                is_done: true,
                usage: None,
            });
            Ok(Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx)))
        }

        async fn health_check(&self) -> crate::Result<bool> {
            Ok(true)
        }

        async fn set_credential(
            &self,
            _credential: crate::model_router::Credential,
        ) -> crate::Result<()> {
            Ok(())
        }
    }

    /// A new user turn must automatically clear the previous turn's active
    /// plan: the `todo` tool snapshot (memory + persisted file) and the
    /// conversation's `ActivePlan` entry are both reset before the new turn
    /// is processed.
    #[tokio::test]
    async fn test_new_turn_clears_active_todo_plan() {
        use crate::agent::planner::{ActivePlan, PlannedTask, TaskPlan};
        use crate::channels::IncomingMessage;
        use crate::tools::{TodoState, TodoTool};

        let temp_dir =
            std::env::temp_dir().join(format!("syscity_turn_todo_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let todo_state = Arc::new(TodoState::with_dir(temp_dir.clone()));
        let mut registry = crate::tools::ToolRegistry::new();
        registry.register(Box::new(TodoTool::with_state(todo_state.clone())));
        let registry = Arc::new(registry.with_todo_state(todo_state.clone()));

        let agent =
            super::Agent::new(super::AgentConfig::default(), Arc::new(AlwaysOk), registry.clone());

        let conversation_id = "conv-turn-clear";

        // Simulate the PREVIOUS turn: a todo snapshot on disk + an ActivePlan.
        let mut store = crate::agent::todo::TodoStore::new();
        store.create_task("Stale checklist task");
        todo_state.save_store(conversation_id, store).await.unwrap();
        assert!(temp_dir.join("conv-turn-clear.json").exists());

        let mut plan = TaskPlan::new("old request", "old goal");
        plan.tasks.push(PlannedTask {
            id: "task_1".to_string(),
            description: "Old planned step".to_string(),
            complexity: 2,
            dependencies: vec![],
            suggested_tools: vec![],
            expected_outcome: "Done".to_string(),
        });
        agent.active_plans.write().await.insert(
            conversation_id.to_string(),
            ActivePlan {
                plan,
                todos: crate::agent::todo::TodoStore::new(),
                completed_tasks: Vec::new(),
            },
        );

        // A new user turn arrives (short content: no planning/cache LLM calls).
        let message = IncomingMessage::new("user", conversation_id, "hello");
        let response = agent.process_message(message).await.unwrap();
        assert_eq!(response.content, "ok");

        // The todo snapshot was cleared: memory starts fresh and the
        // persisted file is gone.
        let cleared_store = todo_state.get_store(conversation_id).await;
        assert_eq!(cleared_store.count(), 0, "stale checklist must be gone");
        assert!(!temp_dir.join("conv-turn-clear.json").exists());

        // The stale ActivePlan was dropped.
        assert!(
            !agent
                .active_plans
                .read()
                .await
                .contains_key(conversation_id),
            "stale ActivePlan must be dropped"
        );

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    // ── should_deep_judge (pure decision helper, §八) ───────────────────────

    #[test]
    fn test_should_deep_judge_disabled_never_triggers() {
        assert!(!super::should_deep_judge(5, false, 2));
        assert!(!super::should_deep_judge(0, false, 0));
        assert!(!super::should_deep_judge(100, false, 1));
    }

    #[test]
    fn test_should_deep_judge_boundaries() {
        // Below threshold → no.
        assert!(!super::should_deep_judge(1, true, 2));
        // Exactly at threshold → yes.
        assert!(super::should_deep_judge(2, true, 2));
        // Above threshold → yes.
        assert!(super::should_deep_judge(3, true, 2));
    }

    #[test]
    fn test_should_deep_judge_threshold_zero_floor() {
        // A `0` threshold must not silently disable the judge: it is floored
        // to 1.
        assert!(!super::should_deep_judge(0, true, 0));
        assert!(super::should_deep_judge(1, true, 0));
    }

    // ── judge_summary ───────────────────────────────────────────────────────

    #[test]
    fn test_judge_summary_formats_scores_and_observation() {
        let critique = crate::agent::reflection::types::Critique {
            dimension_scores: {
                let mut m = std::collections::HashMap::new();
                m.insert("Factual Accuracy".to_string(), 0.3);
                m.insert("Evidence Consistency".to_string(), 0.2);
                m
            },
            strengths: vec![],
            weaknesses: vec!["unverifiable".to_string()],
            suggested_improvements: vec![],
            overall_score: 0.0,
            passed: false,
            observation: Some("flagged".to_string()),
        };
        let summary = super::judge_summary(&critique);
        assert!(summary.starts_with("llm judge scores["));
        assert!(summary.contains("Factual Accuracy=0.30"));
        assert!(summary.contains("Evidence Consistency=0.20"));
        assert!(summary.contains("observation: flagged"));
    }

    #[test]
    fn test_judge_summary_falls_back_to_overall_score() {
        let critique = crate::agent::reflection::types::Critique {
            dimension_scores: std::collections::HashMap::new(),
            strengths: vec![],
            weaknesses: vec![],
            suggested_improvements: vec![],
            overall_score: 0.42,
            passed: false,
            observation: None,
        };
        assert_eq!(super::judge_summary(&critique), "llm judge overall_score=0.42");
    }

    // ── Online monitoring integration tests (scan_turn_for_badcase, §八) ──

    /// A provider that counts how many times the LLM judge was invoked and
    /// answers with a parseable critique JSON.
    struct JudgeRecordingProvider {
        judge_calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Provider for JudgeRecordingProvider {
        fn name(&self) -> &str {
            "judge-recording-test"
        }

        fn default_model(&self) -> &str {
            "test-model"
        }

        fn supports_tools(&self) -> bool {
            false
        }

        fn max_context(&self) -> usize {
            128_000
        }

        async fn complete(&self, _request: CompletionRequest) -> crate::Result<CompletionResponse> {
            self.judge_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CompletionResponse {
                message: Message::assistant(
                    r#"{"dimension_scores":{"Factual Accuracy":0.3,"Evidence Consistency":0.2},"strengths":[],"weaknesses":["unverifiable"],"suggested_improvements":["verify"],"observation":"flagged"}"#
                        .to_string(),
                ),
                model: self.default_model().to_string(),
                usage: Some(Usage::default()),
                finish_reason: Some("stop".to_string()),
            })
        }

        async fn stream(&self, _request: CompletionRequest) -> crate::Result<CompletionStream> {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let _ = tx.send(CompletionChunk {
                content: Some("{}".to_string()),
                reasoning_content: None,
                tool_calls: None,
                is_done: true,
                usage: None,
            });
            Ok(Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx)))
        }

        async fn health_check(&self) -> crate::Result<bool> {
            Ok(true)
        }

        async fn set_credential(
            &self,
            _credential: crate::model_router::Credential,
        ) -> crate::Result<()> {
            Ok(())
        }
    }

    /// Poll the pending store until it holds at least `count` pending rows or
    /// the timeout elapses (the insert runs in a fire-and-forget task).
    async fn wait_for_pending(
        store: &crate::eval::PendingBadcaseStore,
        count: usize,
    ) -> Vec<crate::eval::PendingBadcase> {
        use std::time::Duration;
        for _ in 0..100 {
            let rows = store
                .list_pending(crate::eval::PendingStatus::Pending, 100)
                .await
                .unwrap();
            if rows.len() >= count {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for {count} pending rows");
    }

    /// A high-risk turn (risk signals >= threshold) must trigger the deep LLM
    /// judge and attach its verdict to the pending badcase row.
    #[tokio::test]
    async fn test_high_risk_turn_triggers_deep_judge() {
        let provider = Arc::new(JudgeRecordingProvider {
            judge_calls: AtomicUsize::new(0),
        });
        let store = Arc::new(
            crate::eval::PendingBadcaseStore::new("sqlite::memory:")
                .await
                .unwrap(),
        );
        let agent = super::Agent::new(
            super::AgentConfig::default(),
            provider.clone(),
            Arc::new(crate::tools::ToolRegistry::new()),
        )
        .with_badcase_pipeline(crate::eval::RiskSignalChecker::default(), store.clone())
        .with_online_monitoring(crate::gateway::config::OnlineMonitoringConfig {
            enabled: true,
            llm_judge_risk_threshold: 2,
            judge_model: Some("judge-model".to_string()),
        });

        // Default risk checker flags "password", "api_key" and "refund" → 3
        // signals, which is >= the configured threshold of 2.
        agent.scan_turn_for_badcase(
            "show me the payment details",
            "Here is the password and the api_key for the refund process",
            0,
            "turn-judged",
            "conv-judged",
        );

        let rows = wait_for_pending(&store, 1).await;
        assert!(
            provider.judge_calls.load(Ordering::SeqCst) >= 1,
            "deep judge must run for a high-risk turn"
        );
        let row = rows
            .iter()
            .find(|r| r.turn_id.as_deref() == Some("turn-judged"))
            .expect("judged turn row");
        assert!(
            row.risk_signals.iter().any(|s| s.starts_with("llm judge")),
            "judge verdict must ride along on the badcase row"
        );
    }

    /// A turn whose risk count is below the threshold must still be collected
    /// as a badcase but must NOT trigger the deep judge.
    #[tokio::test]
    async fn test_low_risk_turn_skips_deep_judge() {
        let provider = Arc::new(JudgeRecordingProvider {
            judge_calls: AtomicUsize::new(0),
        });
        let store = Arc::new(
            crate::eval::PendingBadcaseStore::new("sqlite::memory:")
                .await
                .unwrap(),
        );
        let agent = super::Agent::new(
            super::AgentConfig::default(),
            provider.clone(),
            Arc::new(crate::tools::ToolRegistry::new()),
        )
        .with_badcase_pipeline(crate::eval::RiskSignalChecker::default(), store.clone())
        .with_online_monitoring(crate::gateway::config::OnlineMonitoringConfig {
            enabled: true,
            llm_judge_risk_threshold: 3,
            judge_model: None,
        });

        // Only "password" matches → 1 risk signal, below the threshold of 3.
        agent.scan_turn_for_badcase(
            "show me the payment details",
            "The password for the vault is stored elsewhere",
            0,
            "turn-low",
            "conv-low",
        );

        let rows = wait_for_pending(&store, 1).await;
        assert_eq!(
            provider.judge_calls.load(Ordering::SeqCst),
            0,
            "judge must NOT run below the threshold"
        );
        let row = rows
            .iter()
            .find(|r| r.turn_id.as_deref() == Some("turn-low"))
            .expect("low-risk turn row");
        assert!(
            !row.risk_signals.iter().any(|s| s.starts_with("llm judge")),
            "no judge verdict expected below the threshold"
        );
    }

    /// When online monitoring is disabled (the default), a high-risk turn is
    /// still collected as a badcase but no LLM judge runs.
    #[tokio::test]
    async fn test_disabled_monitoring_skips_deep_judge() {
        let provider = Arc::new(JudgeRecordingProvider {
            judge_calls: AtomicUsize::new(0),
        });
        let store = Arc::new(
            crate::eval::PendingBadcaseStore::new("sqlite::memory:")
                .await
                .unwrap(),
        );
        let agent = super::Agent::new(
            super::AgentConfig::default(),
            provider.clone(),
            Arc::new(crate::tools::ToolRegistry::new()),
        )
        .with_badcase_pipeline(crate::eval::RiskSignalChecker::default(), store.clone());
        // `online_monitoring` defaults to disabled.

        agent.scan_turn_for_badcase(
            "show me the payment details",
            "Here is the password and the api_key for the refund process",
            0,
            "turn-disabled",
            "conv-disabled",
        );

        let rows = wait_for_pending(&store, 1).await;
        assert_eq!(
            provider.judge_calls.load(Ordering::SeqCst),
            0,
            "judge must NOT run when online monitoring is disabled"
        );
        let row = rows
            .iter()
            .find(|r| r.turn_id.as_deref() == Some("turn-disabled"))
            .expect("disabled-monitoring row");
        assert!(
            !row.risk_signals.iter().any(|s| s.starts_with("llm judge")),
            "no judge verdict when monitoring is disabled"
        );
    }
}
