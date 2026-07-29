//! Context window management for RAG results.
//!
//! Provides token-budget-aware selection of retrieved chunks so they fit
//! within the LLM's context window, preventing overflow.

/// Approximate characters per token (same heuristic used throughout the
/// codebase: `src/providers/mod.rs`, `src/agent/context.rs`).
pub const CHARS_PER_TOKEN: f64 = 4.0;

/// Estimate the token count of a text string using the standard 4-char
/// heuristic.
pub fn estimate_tokens(text: &str) -> usize {
    (text.len() as f64 / CHARS_PER_TOKEN).ceil() as usize
}

/// Configuration for context-window-aware result selection.
#[derive(Debug, Clone)]
pub struct ContextWindowConfig {
    /// Maximum total tokens the LLM context can hold.
    /// Default: 128_000 (common for GPT-4 / Claude).
    pub max_tokens: usize,
    /// Tokens reserved for the LLM's response generation.
    /// Default: 4096.
    pub reserved_for_response: usize,
    /// Minimum number of chunks to retain, even if they exceed the budget.
    /// Default: 1.
    pub min_chunks: usize,
}

impl Default for ContextWindowConfig {
    fn default() -> Self {
        Self {
            max_tokens: 128_000,
            reserved_for_response: 4096,
            min_chunks: 1,
        }
    }
}

/// Select chunks greedily by relevance score, stopping when the token budget
/// is exhausted.
///
/// # Arguments
///
/// * `items` — Chunks sorted descending by relevance score.
/// * `config` — Context window constraints.
/// * `current_context_tokens` — Estimated tokens already consumed by other
///   parts of the context (system prompt, conversation history, etc.).
///
/// # Returns
///
/// The subset of `items` that fits within the token budget, with at least
/// `config.min_chunks` items retained.
pub fn select_by_token_budget<T>(
    items: Vec<T>,
    config: &ContextWindowConfig,
    current_context_tokens: usize,
) -> Vec<T>
where
    T: AsRef<str>,
{
    if items.is_empty() {
        return items;
    }

    let budget = config
        .max_tokens
        .saturating_sub(config.reserved_for_response)
        .saturating_sub(current_context_tokens);

    let mut result: Vec<T> = Vec::new();
    let mut used: usize = 0;

    for item in items {
        let tokens = estimate_tokens(item.as_ref());
        if used + tokens <= budget || result.len() < config.min_chunks {
            result.push(item);
            used += tokens;
        } else {
            break;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn test_estimate_tokens_short() {
        // "abc" = 3 chars → ceil(3/4) = 1
        assert_eq!(estimate_tokens("abc"), 1);
    }

    #[test]
    fn test_estimate_tokens_exact() {
        // "abcd" = 4 chars → ceil(4/4) = 1
        assert_eq!(estimate_tokens("abcd"), 1);
        // "abcdefgh" = 8 chars → ceil(8/4) = 2
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }

    #[test]
    fn test_estimate_tokens_rounds_up() {
        // "abcde" = 5 chars → ceil(5/4) = 2
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn test_select_by_token_budget_empty() {
        let result: Vec<String> =
            select_by_token_budget(vec![], &ContextWindowConfig::default(), 0);
        assert!(result.is_empty());
    }

    #[test]
    fn test_select_by_token_budget_all_fit() {
        let items: Vec<String> = vec!["a".repeat(4), "b".repeat(4), "c".repeat(4)];
        let cfg = ContextWindowConfig {
            max_tokens: 100,
            reserved_for_response: 0,
            min_chunks: 0,
        };
        let result = select_by_token_budget(items.clone(), &cfg, 0);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_select_by_token_budget_some_truncated() {
        let items: Vec<String> = vec![
            "a".repeat(40), // 10 tokens
            "b".repeat(40), // 10 tokens
            "c".repeat(40), // 10 tokens
        ];
        let cfg = ContextWindowConfig {
            max_tokens: 15, // fits only the first chunk
            reserved_for_response: 0,
            min_chunks: 0,
        };
        let result = select_by_token_budget(items, &cfg, 0);
        assert_eq!(result.len(), 1);
        assert!(result[0].starts_with('a'));
    }

    #[test]
    fn test_select_by_token_budget_min_chunks() {
        let items: Vec<String> = vec![
            "a".repeat(400), // 100 tokens — exceeds budget alone
            "b".repeat(400), // 100 tokens
        ];
        let cfg = ContextWindowConfig {
            max_tokens: 50,
            reserved_for_response: 0,
            min_chunks: 2, // keep both even if over budget
        };
        let result = select_by_token_budget(items, &cfg, 0);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_select_by_token_budget_respects_reserved() {
        let items: Vec<String> = vec!["a".repeat(40)]; // 10 tokens
        let cfg = ContextWindowConfig {
            max_tokens: 15,
            reserved_for_response: 10, // only 5 tokens available
            min_chunks: 1,
        };
        // min_chunks keeps it
        let result = select_by_token_budget(items.clone(), &cfg, 0);
        assert_eq!(result.len(), 1);

        let cfg2 = ContextWindowConfig {
            max_tokens: 15,
            reserved_for_response: 10,
            min_chunks: 0,
        };
        let result2 = select_by_token_budget(items, &cfg2, 0);
        assert_eq!(result2.len(), 0);
    }

    #[test]
    fn test_select_preserves_ordering() {
        let items: Vec<String> = vec![
            "first chunk with some content".to_string(),  // ~5 tokens
            "second chunk with some content".to_string(), // ~5 tokens
            "third chunk with some content".to_string(),  // ~5 tokens
        ];
        let cfg = ContextWindowConfig {
            max_tokens: 8, // fits first but not second
            reserved_for_response: 0,
            min_chunks: 0,
        };
        let result = select_by_token_budget(items, &cfg, 0);
        assert_eq!(result.len(), 1);
        assert!(result[0].starts_with("first"));
    }

    /// Helper struct to test AsRef<str> generic.
    struct HasContent(String);

    impl AsRef<str> for HasContent {
        fn as_ref(&self) -> &str {
            &self.0
        }
    }

    #[test]
    fn test_select_works_with_custom_types() {
        let items = vec![
            HasContent("short".to_string()),
            HasContent("also short".to_string()),
        ];
        let cfg = ContextWindowConfig {
            max_tokens: 100,
            reserved_for_response: 0,
            min_chunks: 0,
        };
        let result = select_by_token_budget(items, &cfg, 0);
        assert_eq!(result.len(), 2);
    }
}
