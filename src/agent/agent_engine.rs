//! Core message-processing loop, LLM completions, and tool-call handling.

use std::sync::Arc;

use tokio_stream::StreamExt;

use tracing::{debug, error, info, instrument, warn};

use crate::agent::turns::ToolCallRecord;
use crate::channels::{IncomingMessage, OutgoingMessage};
use crate::providers::{CompletionRequest, Message, Role, ToolCall, ToolResult};
use crate::tools::{ToolContext, ToolExecutionChunk};

use super::agent_cache::{are_tools_cacheable, should_use_cache_llm};
use super::*;

impl Agent {
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
                            .append_turn(&sid, &tid, t_idx, &user_c, &asst_text, "complete")
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
            (progress_cb)(ProgressEvent::Completed { response: rejection.clone() }).await;
            return Ok(OutgoingMessage::new(
                crate::channels::ConversationId(conversation_id),
                rejection,
            ));
        }

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

                // Notify completed with cached response
                (progress_cb)(ProgressEvent::Completed {
                    response: cached.response.clone(),
                })
                .await;

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

        // Get response from LLM with progress (lock NOT held).
        let llm_result = self
            .get_completion_with_progress(
                &mut thread.context,
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
                    tokio::spawn(async move {
                        if let Err(e) = store
                            .append_turn(&sid, &tid, t_idx, &user_c, &asst_text, "complete")
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
        })
        .await;

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
                let criteria = self
                    .config
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

    /// Get a completion from the LLM, handling tool calls
    async fn get_completion(
        &self,
        context: &mut Context,
        user_id: &str,
    ) -> crate::Result<crate::providers::CompletionResponse> {
        // If the context is over-budget, try to reduce it before sending.
        if context.needs_pruning() {
            if let Some(ref compaction_model) = self.config.compaction_model {
                // LLM-assisted compaction: produce a high-quality summary.
                let compressor = crate::agent::compressor::ContextCompressor::new(
                    self.config.max_context_tokens,
                );
                let history = context.history().to_vec();
                let compacted = compressor
                    .compact_with_llm(
                        &history,
                        &self.provider,
                        Some(compaction_model.as_str()),
                        2,
                        6,
                    )
                    .await;
                context.replace_messages(compacted);
            } else {
                // Fallback: drop middle messages and insert a placeholder summary.
                // This keeps the context coherent without an extra LLM call.
                context.summarize();
            }
        }

        let messages = context.to_messages();

        // Get available tools
        let tool_context =
            self.build_tool_context(user_id, context.id(), context.delegation().cloned());
        let tool_defs = self.tools.get_available(&tool_context);
        let has_tools = !tool_defs.is_empty();

        let extra = self.extra_params.read().await.clone();
        let mut request = CompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            stream: false,
            extra,
            ..Default::default()
        };
        self.patch_request_for_reasoning(&mut request);

        if has_tools && self.provider.supports_tools() {
            // Convert FunctionDefinition to ToolDefinition
            let tools: Vec<crate::providers::ToolDefinition> = tool_defs
                .into_iter()
                .map(|f| crate::providers::ToolDefinition {
                    tool_type: "function".to_string(),
                    function: f,
                })
                .collect();
            request.tools = Some(tools);
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

        // Get completion — use model router when available for key rotation / fallback
        let response = if let Some(ref router) = self.model_router {
            let alias = {
                let guard = self.model_override.read().await;
                guard
                    .as_ref()
                    .cloned()
                    .or(self.model_alias.clone())
                    .or(self.model.clone())
                    .unwrap_or_else(|| self.provider.default_model().to_string())
            };
            let tools = request.tools.take();
            router.complete(&alias, request.messages, tools).await?
        } else {
            self.provider.complete(request).await?
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

    /// Handle tool calls from the LLM
    async fn handle_tool_calls(
        &self,
        context: &mut Context,
        original_response: &crate::providers::CompletionResponse,
        tool_calls: &[ToolCall],
        user_id: &str,
    ) -> crate::Result<crate::providers::CompletionResponse> {
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
            .take(self.config.max_concurrent_tools)
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
        progress_cb: ProgressCallback,
        user_id: &str,
    ) -> crate::Result<crate::providers::CompletionResponse> {
        let messages = context.to_messages();
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
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            stream: true,
            extra,
            ..Default::default()
        };
        self.patch_request_for_reasoning(&mut request);

        if has_tools && self.provider.supports_tools() {
            let tools: Vec<crate::providers::ToolDefinition> = tool_defs
                .into_iter()
                .map(|f| crate::providers::ToolDefinition {
                    tool_type: "function".to_string(),
                    function: f,
                })
                .collect();
            request.tools = Some(tools);
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

        // Get streaming completion — use model router when available
        let (raw_stream, family) = if let Some(ref router) = self.model_router {
            let alias = {
                let guard = self.model_override.read().await;
                guard
                    .as_ref()
                    .cloned()
                    .or(self.model_alias.clone())
                    .or(self.model.clone())
                    .unwrap_or_else(|| self.provider.default_model().to_string())
            };
            let tools = request.tools.take();
            let stream = router.stream(&alias, request.messages, tools).await?;
            // When using model router, fall back to Generic stream family
            (stream, crate::providers::stream_wrappers::ProviderStreamFamily::Generic)
        } else {
            (self.provider.stream(request).await?, self.provider.stream_family())
        };
        let registry = crate::providers::stream_wrappers::StreamFamilyRegistry::default();
        let mut stream = registry.apply(family, raw_stream);

        let mut accumulated_text = String::new();
        let mut accumulated_reasoning = String::new();
        let mut accumulated_tool_calls: Vec<ToolCall> = Vec::new();
        let mut finish_reason: Option<String> = None;
        let mut usage: Option<crate::providers::Usage> = None;

        while let Some(chunk) = stream.next().await {
            // Emit reasoning delta
            if let Some(ref reasoning_delta) = chunk.reasoning_content {
                if !reasoning_delta.is_empty() {
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
            context.accumulate_turn_token_usage(
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens,
            );
        }

        Ok(response)
    }

    /// Handle tool calls with progress callbacks
    async fn handle_tool_calls_with_progress(
        &self,
        context: &mut Context,
        original_response: &crate::providers::CompletionResponse,
        tool_calls: &[ToolCall],
        progress_cb: ProgressCallback,
        user_id: &str,
    ) -> crate::Result<crate::providers::CompletionResponse> {
        // Accumulate token usage from the LLM response that produced these tool calls
        if let Some(ref usage) = original_response.usage {
            context.accumulate_turn_token_usage(
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens,
            );
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
            .take(self.config.max_concurrent_tools)
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

            context.push_tool_call_record(ToolCallRecord {
                name: tool_name.clone(),
                args: tool_call.function.arguments.to_string(),
                result: result.content.clone(),
                success: !result.is_error.unwrap_or(false),
                duration_ms: _start.elapsed().as_millis() as u64,
            });

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
        let mut final_response =
            Box::pin(self.get_completion_with_progress(context, progress_cb, user_id)).await?;
        info!(
            "handle_tool_calls_with_progress: recursive get_completion_with_progress returned in {:?}",
            recursive_llm_start.elapsed()
        );

        // Accumulate token usage from this LLM completion in the tool loop
        if let Some(ref usage) = final_response.usage {
            context.accumulate_turn_token_usage(
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens,
            );
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
