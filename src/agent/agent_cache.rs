//! Response caching: cacheable-query detection and the in-memory
//! [`ResponseCache`] with TTL.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::RwLock;

use crate::providers::{CompletionRequest, Message, Provider};

#[allow(clippy::unwrap_used)] // static regex literals validated at compile-time
pub(super) static RE_CODE_BLOCK: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"```(\w+)?\n(.*?)\n```").unwrap());
#[allow(clippy::unwrap_used)] // static regex literals validated at compile-time
pub(super) static RE_URL: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r#"https?://[^\s)\]>'"`]+"#).unwrap());

/// Fast check for obviously time-sensitive queries
pub(super) fn is_obviously_time_sensitive(message: &str) -> bool {
    let lower = message.to_lowercase();

    // Only check for obvious time keywords that clearly indicate real-time needs
    let obvious_time_queries = [
        "what time is it",
        "current time",
        "what's the time",
        "现在几点",
        "当前时间",
        "现在时间",
    ];

    for query in &obvious_time_queries {
        if lower.contains(query) {
            return true;
        }
    }

    false
}

/// Check if a message should be cached using LLM classification
/// Returns true if the response can be safely cached
pub(super) async fn should_use_cache_llm(
    provider: &Arc<dyn Provider>,
    message: &str,
    model: Option<String>,
) -> bool {
    // Skip LLM check for obviously time-sensitive queries (optimization)
    if is_obviously_time_sensitive(message) {
        return false;
    }

    // Skip LLM check for very short queries (likely conversational)
    if message.len() < 20 {
        return false;
    }

    let prompt = format!(
        r#"Analyze this user query and determine if the response can be safely cached.

A query SHOULD be cached if:
- It's asking for general information, facts, summaries, or research
- The answer won't change significantly in the next hour
- Examples: "explain quantum computing", "summarize news", "how does X work"

A query should NOT be cached if:
- It asks for current time, date, or real-time data
- It asks for stock prices, crypto prices, or financial data
- It asks for current weather or temperature
- It asks "what is happening now" or "latest updates"
- The answer changes frequently (every minute/second)

User query: "{}"

Reply with ONLY "CACHE" or "NOCACHE"."#,
        message.replace('\"', "\\\"")
    );

    let request = CompletionRequest {
        model,
        messages: vec![Message::user(&prompt)],
        temperature: Some(0.0), // Deterministic
        max_tokens: Some(10),
        stream: false,
        ..Default::default()
    };

    match provider.complete(request).await {
        Ok(response) => {
            let content = response.message.content.trim().to_uppercase();
            // Default to not caching if LLM is uncertain
            content == "CACHE"
        }
        Err(_) => {
            // If LLM call fails, default to not caching for safety
            false
        }
    }
}

/// Determine if tools used are cacheable (time-sensitive tools skip caching)
pub(super) fn are_tools_cacheable(tool_names: &[String]) -> bool {
    // Non-cacheable tools that return time-sensitive or real-time data
    let non_cacheable = [
        "datetime",
        "time",
        "clock",
        "date",
        "weather_current",
        "weather_now",
        "stock_price",
        "crypto_price",
    ];

    for tool in tool_names {
        let tool_lower = tool.to_lowercase();
        for nc in &non_cacheable {
            if tool_lower.contains(nc) {
                return false;
            }
        }
    }

    true
}

/// Cached response entry
#[derive(Debug, Clone)]
pub struct CachedResponse {
    pub response: String,
    pub created_at: SystemTime,
    pub tools_used: Vec<String>,
}

/// Simple in-memory response cache with TTL
#[derive(Debug, Clone)]
pub struct ResponseCache {
    pub(super) cache: Arc<RwLock<HashMap<u64, CachedResponse>>>,
    ttl: Duration,
}

impl ResponseCache {
    /// Create a new response cache with specified TTL
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// Generate a cache key from user message and context
    pub(super) fn generate_key(user_id: &str, conversation_id: &str, message: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        user_id.hash(&mut hasher);
        conversation_id.hash(&mut hasher);
        message.trim().hash(&mut hasher);
        hasher.finish()
    }

    /// Get cached response if not expired
    pub async fn get(
        &self,
        user_id: &str,
        conversation_id: &str,
        message: &str,
    ) -> Option<CachedResponse> {
        let key = Self::generate_key(user_id, conversation_id, message);
        let cache = self.cache.read().await;

        if let Some(entry) = cache.get(&key) {
            if let Ok(elapsed) = entry.created_at.elapsed() {
                if elapsed < self.ttl {
                    return Some(entry.clone());
                }
            }
        }
        None
    }

    /// Store a response in cache
    pub async fn set(
        &self,
        user_id: &str,
        conversation_id: &str,
        message: &str,
        response: String,
        tools_used: Vec<String>,
    ) {
        let key = Self::generate_key(user_id, conversation_id, message);
        let entry = CachedResponse {
            response,
            created_at: SystemTime::now(),
            tools_used,
        };

        let mut cache = self.cache.write().await;
        cache.insert(key, entry);

        // Clean up old entries if cache is too large (> 1000 entries)
        if cache.len() > 1000 {
            let keys_to_remove: Vec<u64> = cache
                .iter()
                .filter(|(_, v)| v.created_at.elapsed().unwrap_or(Duration::MAX) > self.ttl)
                .map(|(k, _)| *k)
                .collect();

            for k in keys_to_remove {
                cache.remove(&k);
            }
        }
    }

    /// Clear expired entries
    pub async fn cleanup(&self) {
        let mut cache = self.cache.write().await;
        cache.retain(|_, v| v.created_at.elapsed().unwrap_or(Duration::MAX) < self.ttl);
    }
}
