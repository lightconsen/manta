//! Session compaction (LLM and naive) and context-cache invalidation.

use super::*;

impl MemoryManager {
    // -------------------------------------------------------------------------
    // Cache invalidation (fix 2.4)
    // -------------------------------------------------------------------------

    /// Invalidate the context cache so the next call to `session_context` or
    /// `retrieve` re-fetches from the store rather than serving stale data.
    pub(super) async fn invalidate_cache(&self) {
        *self.context_cache.write().await = None;
    }

    /// Compact a session: extract key facts from old messages into semantic
    /// memories.
    ///
    /// This is called when a session is closed or exceeds thresholds
    /// (>50 turns or >7 days old).
    ///
    /// When an LLM provider is attached, uses the model to extract facts,
    /// preferences, decisions, and important context.  Falls back to naive
    /// sampling when no provider is configured.
    pub async fn compact_session(
        &self,
        conversation_id: impl AsRef<str>,
        model: Option<&str>,
    ) -> crate::Result<Vec<MemoryId>> {
        let conversation_id = conversation_id.as_ref();
        info!("Compacting session: {}", conversation_id);

        // Get full session history
        let messages = self
            .chat_history
            .get_conversation_history(conversation_id, 1000)
            .await?;

        if messages.len() < 10 {
            debug!("Session too short to compact: {} messages", messages.len());
            return Ok(vec![]);
        }

        let user_id = messages
            .iter()
            .find(|m| !m.user_id.is_empty())
            .map(|m| m.user_id.clone())
            .unwrap_or_else(|| "unknown".to_string());

        let stored_ids = if let Some(ref provider) = self.llm_provider {
            self.compact_with_llm(provider, model, &user_id, conversation_id, &messages)
                .await?
        } else {
            self.compact_naive(&user_id, &messages).await?
        };

        // Mark session as compacted
        if !stored_ids.is_empty() {
            let marker = Memory::new(
                &user_id,
                format!("Session compacted: {}", conversation_id),
                "compaction",
            )
            .with_conversation(conversation_id)
            .with_metadata(serde_json::json!({
                "compacted_at": chrono::Utc::now().to_rfc3339(),
                "message_count": messages.len(),
                "extracted_memories": stored_ids.len(),
            }))
            .with_source("compaction");

            self.store.store(marker).await?;
        }

        // Invalidate context cache so the next retrieval accounts for newly
        // extracted facts (fix 2.4).
        self.invalidate_cache().await;

        let session_key = format!("{}:{}", user_id, conversation_id);

        // Log compact event
        if let Some(ref event_log) = self.event_log {
            let event = MemoryEventBuilder::new().compact(
                &session_key,
                format!("compact-{}", uuid::Uuid::new_v4()),
                messages.len() as u32,
                stored_ids.len() as u32,
            );
            if let Err(e) = event_log.append(&event).await {
                warn!("Failed to append compact event: {}", e);
            }
        }

        // Auto-generate SOUL.md fields from conversation patterns
        if let Some(ref personality) = self.personality_memory {
            match personality.analyze_conversation_patterns(&messages) {
                Ok(analysis) => {
                    if let Err(e) = personality.update_soul_from_analysis(&analysis).await {
                        warn!("Failed to update SOUL.md from analysis: {}", e);
                    }
                }
                Err(e) => warn!("Failed to analyze conversation patterns: {}", e),
            }
        }

        info!("Session {} compacted: {} facts extracted", conversation_id, stored_ids.len());
        Ok(stored_ids)
    }

    /// Compact a session using an LLM to extract facts, preferences, decisions,
    /// and important context from the conversation history.
    async fn compact_with_llm(
        &self,
        provider: &Arc<dyn Provider>,
        model: Option<&str>,
        user_id: &str,
        conversation_id: &str,
        messages: &[ChatMessage],
    ) -> crate::Result<Vec<MemoryId>> {
        let transcript: String = messages
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n\n");

        let request = CompletionRequest {
            messages: vec![
                Message::system(
                    "Extract key facts, preferences, decisions, and context from conversations. \
                     Return ONLY a JSON array.",
                ),
                Message::user(format!(
                    "Analyze the conversation and extract structured information.\n\n\
                     Each entry: {{\"content\": \"...\", \"type\": \"fact\"|\"preference\"|\"decision\"|\"context\", \
                     \"importance\": 0.0..1.0}}\n\n\
                     Only extract clearly stated or strongly implied info. \
                     Do not invent. Return ONLY the JSON array.\n\nConversation:\n{transcript}"
                )),
            ],
            model: model.map(String::from),
            temperature: Some(0.3),
            max_tokens: Some(4096),
            stream: false,
            ..Default::default()
        };

        let response = match provider.complete(request).await {
            Ok(resp) => resp,
            Err(e) => {
                warn!("LLM compaction failed, falling back to naive extraction: {}", e);
                return self.compact_naive(user_id, messages).await;
            }
        };

        let extracted: Vec<serde_json::Value> =
            match serde_json::from_str(&response.message.content) {
                Ok(vals) => vals,
                Err(e) => {
                    warn!("Failed to parse LLM extraction JSON, falling back: {}", e);
                    return self.compact_naive(user_id, messages).await;
                }
            };

        let mut stored_ids = Vec::new();
        for item in extracted {
            let content = item.get("content").and_then(|v| v.as_str());
            let memory_type = item
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("semantic");
            let importance = item
                .get("importance")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.5) as f32;

            if let Some(content) = content {
                if content.len() >= 5 {
                    let id = self
                        .observe(user_id, content, memory_type, importance)
                        .await?;
                    stored_ids.push(id);
                }
            }
        }

        debug!(
            "LLM extraction yielded {} memories from session {}",
            stored_ids.len(),
            conversation_id
        );
        Ok(stored_ids)
    }

    /// Fallback naive compaction: sample every 5th user message.
    async fn compact_naive(
        &self,
        user_id: &str,
        messages: &[ChatMessage],
    ) -> crate::Result<Vec<MemoryId>> {
        let mut stored_ids = vec![];

        for (i, msg) in messages.iter().enumerate() {
            if msg.role != "user" {
                continue;
            }
            if i % 5 != 0 {
                continue;
            }
            let fact = msg.content.clone();
            if fact.len() < 20 {
                continue;
            }

            let id = self.observe(user_id, fact, "semantic", 0.6).await?;

            stored_ids.push(id);
        }

        Ok(stored_ids)
    }
}
