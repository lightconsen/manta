//! Hybrid search combining vector (cosine similarity) and FTS5 (BM25) results.
//!
//! Runs both searches concurrently, normalises scores independently to [0, 1],
//! then merges them using a weighted average:
//!
//! ```text
//! final_score = vector_weight * vector_score + text_weight * fts_score
//! ```
//!
//! Results are deduplicated by a SHA-256 content fingerprint and filtered by
//! `min_score` before being sorted descending and truncated to `max_results`.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use sha2::{Digest, Sha256};
use tracing::warn;

use super::{
    session_search::{SessionSearch, SessionSearchQuery},
    vector::VectorMemoryService,
};

/// Weights and thresholds for hybrid search.
#[derive(Debug, Clone)]
pub struct HybridSearchConfig {
    /// Weight applied to vector (semantic) scores. Default: 0.7.
    pub vector_weight: f32,
    /// Weight applied to FTS5 (BM25) scores. Default: 0.3.
    pub text_weight: f32,
    /// Maximum number of results to return. Default: 6.
    pub max_results: usize,
    /// Minimum combined score to include a result. Default: 0.35.
    pub min_score: f32,
    /// Temporal decay configuration for recency-aware scoring. Default:
    /// disabled.
    pub temporal_decay: TemporalDecayConfig,
    /// MMR configuration for diversity re-ranking. Default: lambda=0.7,
    /// top_k=5.
    pub mmr: MmrConfig,
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            vector_weight: 0.7,
            text_weight: 0.3,
            max_results: 6,
            min_score: 0.35,
            temporal_decay: TemporalDecayConfig::default(),
            mmr: MmrConfig::default(),
        }
    }
}

/// A single result from the hybrid search.
#[derive(Debug, Clone)]
pub struct HybridSearchResult {
    /// The full text content of the result.
    pub content: String,
    /// Combined hybrid score in [0, 1].
    pub score: f32,
    /// Which backend provided this result: `"vector"`, `"fts"`, or
    /// `"combined"`.
    pub source: String,
    /// Human-readable citation, e.g. `"session:abc123#L5-L12"`.
    pub citation: String,
}

// ── Internal accumulator
// ──────────────────────────────────────────────────────

#[derive(Default)]
struct Entry {
    vector_score: Option<f32>,
    fts_score: Option<f32>,
    content: String,
    citation: String,
}

// ── Normalisation
// ─────────────────────────────────────────────────────────────

/// Normalise a slice of (score, key) pairs so that the maximum score maps to
/// 1.0. Returns a `HashMap<key, normalised_score>`.
fn normalise(pairs: &[(f32, String)]) -> HashMap<String, f32> {
    if pairs.is_empty() {
        return HashMap::new();
    }

    let min = pairs
        .iter()
        .map(|(s, _)| *s)
        .fold(f32::INFINITY, f32::min);
    let max = pairs
        .iter()
        .map(|(s, _)| *s)
        .fold(f32::NEG_INFINITY, f32::max);

    // Handle NaN or flat input.
    if max.is_nan() || min.is_nan() || max <= min {
        return pairs.iter().map(|(_, k)| (k.clone(), 0.0)).collect();
    }

    // Min-max normalization to [0, 1] — caller inverts for FTS5 (lower = better).
    let range = max - min;
    pairs
        .iter()
        .map(|(s, k)| (k.clone(), (s - min) / range))
        .collect()
}

/// SHA-256 fingerprint of the first 512 chars of `text` used for dedup.
fn content_key(text: &str) -> String {
    // Use char_indices to safely handle multi-byte UTF-8 characters.
    let end = text
        .char_indices()
        .nth(512)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    let sample = &text[..end];
    let hash = Sha256::digest(sample.as_bytes());
    format!("{:x}", hash)
}

// ── Public search function
// ────────────────────────────────────────────────────

/// Run hybrid search over `vector_service` (semantic) and `session_search`
/// (FTS5), merge results, and return up to `config.max_results` entries.
///
/// Both backends are queried concurrently via `tokio::join!`.
///
/// # Example
///
/// ```rust,no_run
/// # use std::sync::Arc;
/// # use syscity::memory::hybrid::{HybridSearchConfig, hybrid_search};
/// # async fn example(
/// #     vector: Arc<syscity::memory::VectorMemoryService>,
/// #     fts: Arc<syscity::memory::SessionSearch>,
/// # ) {
/// let results = hybrid_search(
///     "what did we decide about the API?",
///     &vector,
///     &fts,
///     &HybridSearchConfig::default(),
/// )
/// .await;
/// for r in results {
///     println!("[{:.2}] {} — {}", r.score, r.citation, &r.content[..80.min(r.content.len())]);
/// }
/// # }
/// ```
pub async fn hybrid_search(
    query: &str,
    vector_service: &VectorMemoryService,
    session_search: &SessionSearch,
    config: &HybridSearchConfig,
) -> Vec<HybridSearchResult> {
    let fetch_limit = config.max_results * 2;
    let threshold = 0.0; // we apply min_score ourselves after merging

    // ── Launch both searches concurrently ─────────────────────────────────────
    let fts_query = SessionSearchQuery::new(query).limit(fetch_limit);

    let (vector_res, fts_res) = tokio::join!(
        vector_service.search(query, fetch_limit, threshold),
        session_search.search(fts_query),
    );

    // Save owned results so they can be iterated twice (once for normalisation
    // key extraction, once for entry population) without re-issuing queries.
    let vector_chunks = match vector_res {
        Ok(v) => v,
        Err(e) => {
            warn!("Vector search failed in hybrid_search: {}", e);
            Vec::new()
        }
    };
    let fts_results = match fts_res {
        Ok(f) => f,
        Err(e) => {
            warn!("FTS search failed in hybrid_search: {}", e);
            Vec::new()
        }
    };

    // ── Collect raw scores ────────────────────────────────────────────────────
    let vector_pairs: Vec<(f32, String)> = vector_chunks
        .iter()
        .map(|(chunk, score)| (*score, content_key(&chunk.text)))
        .collect();

    let fts_pairs: Vec<(f32, String)> = fts_results
        .iter()
        .map(|r| (r.score as f32, content_key(&r.content)))
        .collect();

    // ── Normalise independently ───────────────────────────────────────────────
    let vector_norm = normalise(&vector_pairs);
    let fts_norm = normalise(&fts_pairs);

    // ── Accumulate entries keyed by content fingerprint ───────────────────────
    let mut entries: HashMap<String, Entry> = HashMap::new();

    for (chunk, _raw_score) in vector_chunks {
        let key = content_key(&chunk.text);
        let norm = *vector_norm.get(&key).unwrap_or(&0.0);
        let e = entries.entry(key.clone()).or_default();
        e.vector_score = Some(norm);
        if e.content.is_empty() {
            e.content = chunk.text.clone();
            e.citation = format!("vector:{}", &chunk.id);
        }
    }

    for r in fts_results {
        let key = content_key(&r.content);
        let norm = *fts_norm.get(&key).unwrap_or(&0.0);
        let e = entries.entry(key.clone()).or_default();
        e.fts_score = Some(norm);
        if e.content.is_empty() {
            e.content = r.content.clone();
            e.citation = format!("session:{}#{}", r.conversation_id, r.message_id);
        }
    }

    // ── Merge and filter ──────────────────────────────────────────────────────
    let mut merged: Vec<HybridSearchResult> = entries
        .into_values()
        .filter_map(|e| {
            let vs = e.vector_score.unwrap_or(0.0);
            let fs = e.fts_score.unwrap_or(0.0);
            let combined = config.vector_weight * vs + config.text_weight * (1.0 - fs);

            if combined < config.min_score || e.content.is_empty() {
                return None;
            }

            let source = match (e.vector_score.is_some(), e.fts_score.is_some()) {
                (true, true) => "combined",
                (true, false) => "vector",
                _ => "fts",
            };

            Some(HybridSearchResult {
                content: e.content,
                score: combined,
                source: source.to_string(),
                citation: e.citation,
            })
        })
        .collect();

    // Sort descending by score, then truncate.
    merged.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Apply temporal decay if enabled.
    if config.temporal_decay.enabled {
        apply_temporal_decay(&mut merged, &config.temporal_decay);
    }

    // Apply MMR re-ranking if configured.
    if config.mmr.lambda > 0.0 && config.mmr.top_k > 0 {
        merged = mmr_rerank(merged, &config.mmr);
    } else {
        merged.truncate(config.max_results);
    }

    merged
}

// ── Temporal decay
// ────────────────────────────────────────────────────────────

/// Configuration for exponential temporal decay applied to dated memory files.
///
/// Decay formula: `score *= e^(-λ * age_days)` where `λ = ln(2) /
/// half_life_days`.
///
/// "Evergreen" files — those whose `citation` path does not contain a
/// parseable `YYYY-MM-DD` date — are exempt from decay and returned unchanged.
///
/// Disabled by default (`enabled: false`) for backward compatibility.
#[derive(Debug, Clone)]
pub struct TemporalDecayConfig {
    /// Whether temporal decay is applied. Default: `false`.
    pub enabled: bool,
    /// Exponential half-life in days. Default: 30.0.
    pub half_life_days: f32,
}

impl Default for TemporalDecayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            half_life_days: 30.0,
        }
    }
}

/// Apply exponential temporal decay to `results` in-place, then re-sort
/// descending by score.
///
/// Only results whose `citation` contains a `YYYY-MM-DD` date string (e.g.
/// `"vector:memory/2025-01-15.md"`) are decayed; all others are left
/// unchanged (evergreen).
///
/// This function is a no-op when `config.enabled == false`.
pub fn apply_temporal_decay(results: &mut [HybridSearchResult], config: &TemporalDecayConfig) {
    if !config.enabled {
        return;
    }

    let lambda = std::f32::consts::LN_2 / config.half_life_days;
    let now: DateTime<Utc> = Utc::now();

    for result in results.iter_mut() {
        if let Some(date) = parse_date_from_citation(&result.citation) {
            let Some(midnight) = date.and_hms_opt(0, 0, 0) else {
                continue;
            };
            let age_days = (now - midnight.and_utc()).num_days() as f32;
            let decay = (-lambda * age_days.max(0.0)).exp();
            result.score *= decay;
        }
        // Evergreen: no date found → no decay.
    }

    // Re-sort descending after decay has shifted scores.
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

// ── MMR Re-ranking
// ────────────────────────────────────────────────────────────

/// Configuration for Maximal Marginal Relevance re-ranking.
///
/// MMR selects results that are both relevant to the query and diverse
/// relative to each other, reducing redundancy in search results.
///
/// Formula per step:
/// ```text
/// MMR(d) = λ * relevance(d, query) - (1 - λ) * max_{d' ∈ S} sim(d, d')
/// ```
/// where `S` is the set of already-selected results.
#[derive(Debug, Clone)]
pub struct MmrConfig {
    /// Trade-off between relevance and diversity.
    ///
    /// `1.0` = pure relevance ranking (no diversity benefit).
    /// `0.0` = maximum diversity, ignoring relevance.
    /// Default: `0.7`.
    pub lambda: f32,
    /// Maximum number of results to return after re-ranking. Default: 5.
    pub top_k: usize,
}

impl Default for MmrConfig {
    fn default() -> Self {
        Self { lambda: 0.7, top_k: 5 }
    }
}

/// Re-rank `results` using Maximal Marginal Relevance.
///
/// Requires that `results` are already sorted by relevance score (descending).
/// Uses word-level Jaccard similarity as the inter-document similarity
/// measure, making it embedding-free and fast.
///
/// # Example
///
/// ```rust
/// use syscity::memory::hybrid::{mmr_rerank, HybridSearchResult, MmrConfig};
///
/// let results = vec![
///     HybridSearchResult {
///         content: "Rust ownership model".into(),
///         score: 0.9,
///         source: "vector".into(),
///         citation: "doc:1".into(),
///     },
///     HybridSearchResult {
///         content: "Rust borrowing rules".into(),
///         score: 0.85,
///         source: "fts".into(),
///         citation: "doc:2".into(),
///     },
///     HybridSearchResult {
///         content: "Python async programming".into(),
///         score: 0.7,
///         source: "vector".into(),
///         citation: "doc:3".into(),
///     },
/// ];
/// let reranked = mmr_rerank(results, &MmrConfig::default());
/// assert!(!reranked.is_empty());
/// ```
pub fn mmr_rerank(
    candidates: Vec<HybridSearchResult>,
    config: &MmrConfig,
) -> Vec<HybridSearchResult> {
    if candidates.is_empty() {
        return candidates;
    }

    let n = candidates.len();
    let mut selected: Vec<usize> = Vec::with_capacity(config.top_k);
    let mut remaining: Vec<usize> = (0..n).collect();

    while selected.len() < config.top_k && !remaining.is_empty() {
        let mut best_idx_in_remaining: usize = 0;
        let mut best_score = f32::NEG_INFINITY;

        for (rem_pos, &cand_idx) in remaining.iter().enumerate() {
            let relevance = candidates[cand_idx].score;

            // Max similarity to any already-selected document.
            let max_sim = selected
                .iter()
                .map(|&sel_idx| {
                    jaccard_similarity(&candidates[cand_idx].content, &candidates[sel_idx].content)
                })
                .fold(0.0_f32, f32::max);

            let mmr_score = config.lambda * relevance - (1.0 - config.lambda) * max_sim;

            if mmr_score > best_score {
                best_score = mmr_score;
                best_idx_in_remaining = rem_pos;
            }
        }

        let chosen = remaining.remove(best_idx_in_remaining);
        selected.push(chosen);
    }

    selected
        .into_iter()
        .map(|i| candidates[i].clone())
        .collect()
}

/// Word-level Jaccard similarity: |A ∩ B| / |A ∪ B|.
///
/// Operates on the set of unique words (lowercased, split on whitespace).
fn jaccard_similarity(a: &str, b: &str) -> f32 {
    use std::collections::HashSet;

    let words_a: HashSet<&str> = a.split_whitespace().collect();
    let words_b: HashSet<&str> = b.split_whitespace().collect();

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        return 1.0; // Both empty → identical.
    }

    intersection as f32 / union as f32
}

/// Extract the first `YYYY-MM-DD` date from a citation string.
///
/// Returns `None` for evergreen files that carry no date.
fn parse_date_from_citation(citation: &str) -> Option<NaiveDate> {
    // Scan for a 10-char substring matching `YYYY-MM-DD`.
    // Use char_indices to safely handle multi-byte UTF-8 characters.
    let indices: Vec<usize> = citation.char_indices().map(|(i, _)| i).collect();
    if indices.len() < 10 {
        return None;
    }
    for i in 0..=indices.len().saturating_sub(10) {
        let start = indices[i];
        let end = indices[i + 9]; // 9 chars later = 10-char slice start..end+1
        if end + 1 > citation.len() {
            break;
        }
        let slice = &citation[start..=end];
        if let Ok(date) = NaiveDate::parse_from_str(slice, "%Y-%m-%d") {
            return Some(date);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalise_basic() {
        let pairs = vec![
            (2.0_f32, "a".to_string()),
            (1.0_f32, "b".to_string()),
            (0.0_f32, "c".to_string()),
        ];
        let norm = normalise(&pairs);
        assert!((norm["a"] - 1.0).abs() < 1e-6);
        assert!((norm["b"] - 0.5).abs() < 1e-6);
        assert!((norm["c"]).abs() < 1e-6);
    }

    #[test]
    fn test_normalise_all_zero() {
        let pairs = vec![(0.0_f32, "x".to_string()), (0.0_f32, "y".to_string())];
        let norm = normalise(&pairs);
        assert_eq!(norm["x"], 0.0);
        assert_eq!(norm["y"], 0.0);
    }

    #[test]
    fn test_content_key_is_deterministic() {
        let k1 = content_key("hello world");
        let k2 = content_key("hello world");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_content_key_differs_for_different_text() {
        let k1 = content_key("hello");
        let k2 = content_key("world");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_config_defaults() {
        let cfg = HybridSearchConfig::default();
        assert!((cfg.vector_weight + cfg.text_weight - 1.0).abs() < 1e-6);
        assert_eq!(cfg.max_results, 6);
        assert!(cfg.min_score > 0.0);
    }

    // ── Temporal decay tests ──────────────────────────────────────────────────

    #[test]
    fn test_temporal_decay_disabled_is_noop() {
        let config = TemporalDecayConfig {
            enabled: false,
            half_life_days: 30.0,
        };

        let mut results = vec![HybridSearchResult {
            content: "old content".to_string(),
            score: 0.8,
            source: "vector".to_string(),
            citation: "vector:memory/2020-01-01.md".to_string(),
        }];

        apply_temporal_decay(&mut results, &config);
        assert!((results[0].score - 0.8).abs() < 1e-6, "Score should be unchanged when disabled");
    }

    #[test]
    fn test_temporal_decay_reduces_old_scores() {
        let config = TemporalDecayConfig {
            enabled: true,
            half_life_days: 30.0,
        };

        // A very old citation — score should be significantly reduced.
        let mut results = vec![HybridSearchResult {
            content: "old content".to_string(),
            score: 1.0,
            source: "vector".to_string(),
            citation: "vector:memory/2000-01-01.md".to_string(),
        }];

        apply_temporal_decay(&mut results, &config);
        assert!(results[0].score < 0.01, "Score for 25-year-old memory should approach 0");
    }

    #[test]
    fn test_temporal_decay_spares_evergreen_files() {
        let config = TemporalDecayConfig {
            enabled: true,
            half_life_days: 30.0,
        };

        let mut results = vec![HybridSearchResult {
            content: "evergreen content".to_string(),
            score: 0.9,
            source: "vector".to_string(),
            // No date in citation → evergreen.
            citation: "vector:MEMORY.md".to_string(),
        }];

        apply_temporal_decay(&mut results, &config);
        assert!(
            (results[0].score - 0.9).abs() < 1e-6,
            "Evergreen file score should not be decayed"
        );
    }

    #[test]
    fn test_temporal_decay_sorts_descending() {
        let config = TemporalDecayConfig {
            enabled: true,
            half_life_days: 30.0,
        };

        // Fresh citation (today-ish year) vs very old.
        let fresh_date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let mut results = vec![
            HybridSearchResult {
                content: "old".to_string(),
                score: 0.9,
                source: "fts".to_string(),
                citation: format!("vector:memory/2000-01-01.md"),
            },
            HybridSearchResult {
                content: "fresh".to_string(),
                score: 0.7,
                source: "vector".to_string(),
                citation: format!("vector:memory/{}.md", fresh_date),
            },
        ];

        apply_temporal_decay(&mut results, &config);

        // After decay, the fresh entry (even with lower initial score) should
        // outrank the very old entry.
        assert_eq!(results[0].content, "fresh", "Fresh result should rank first after decay");
    }

    #[test]
    fn test_parse_date_from_citation_finds_date() {
        let date = parse_date_from_citation("vector:memory/2025-03-15.md");
        assert!(date.is_some());
        let d = date.unwrap();
        assert_eq!(d.to_string(), "2025-03-15");
    }

    #[test]
    fn test_parse_date_from_citation_returns_none_for_evergreen() {
        assert!(parse_date_from_citation("vector:MEMORY.md").is_none());
        assert!(parse_date_from_citation("session:abc123#5").is_none());
    }

    #[test]
    fn test_temporal_decay_config_defaults() {
        let cfg = TemporalDecayConfig::default();
        assert!(!cfg.enabled, "Decay should be disabled by default");
        assert!((cfg.half_life_days - 30.0).abs() < 1e-6);
    }

    // ── MMR tests ─────────────────────────────────────────────────────────────

    fn make_result(content: &str, score: f32) -> HybridSearchResult {
        HybridSearchResult {
            content: content.to_string(),
            score,
            source: "vector".to_string(),
            citation: format!("doc:{}", content),
        }
    }

    #[test]
    fn test_mmr_empty_input() {
        let results = mmr_rerank(vec![], &MmrConfig::default());
        assert!(results.is_empty());
    }

    #[test]
    fn test_mmr_top_k_limits_output() {
        let results = vec![
            make_result("a b c", 0.9),
            make_result("d e f", 0.8),
            make_result("g h i", 0.7),
            make_result("j k l", 0.6),
        ];
        let cfg = MmrConfig { lambda: 0.7, top_k: 2 };
        let reranked = mmr_rerank(results, &cfg);
        assert_eq!(reranked.len(), 2);
    }

    #[test]
    fn test_mmr_promotes_diversity() {
        // Two near-identical results plus one diverse result.
        // With lambda < 1.0, the diverse result should be preferred over the
        // duplicate even if it has a lower relevance score.
        let results = vec![
            make_result("rust ownership borrow move copy", 0.9),
            make_result("rust ownership borrow move copy clone", 0.85), // near-duplicate
            make_result("python async await coroutine", 0.7),           // diverse
        ];
        let cfg = MmrConfig { lambda: 0.5, top_k: 2 };
        let reranked = mmr_rerank(results, &cfg);
        // First result: highest score "rust ownership..."
        assert!(reranked[0]
            .content
            .starts_with("rust ownership borrow move copy"));
        // Second result should be the diverse Python one, not the near-duplicate.
        assert_eq!(reranked.len(), 2);
        let has_diverse = reranked.iter().any(|r| r.content.starts_with("python"));
        assert!(has_diverse, "MMR should promote the diverse result over the near-duplicate");
    }

    #[test]
    fn test_mmr_pure_relevance_with_lambda_one() {
        let results = vec![
            make_result("aaa bbb ccc", 0.9),
            make_result("ddd eee fff", 0.8),
            make_result("ggg hhh iii", 0.7),
        ];
        let cfg = MmrConfig { lambda: 1.0, top_k: 3 };
        let reranked = mmr_rerank(results, &cfg);
        // With lambda=1.0, order should be purely by relevance score.
        assert_eq!(reranked[0].score, 0.9);
        assert_eq!(reranked[1].score, 0.8);
        assert_eq!(reranked[2].score, 0.7);
    }

    #[test]
    fn test_jaccard_similarity_identical() {
        assert!((jaccard_similarity("hello world", "hello world") - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_jaccard_similarity_disjoint() {
        assert!((jaccard_similarity("hello world", "foo bar")).abs() < 1e-6);
    }

    #[test]
    fn test_jaccard_similarity_partial_overlap() {
        // "a b c" ∩ "b c d" = {b, c}, union = {a, b, c, d} → 2/4 = 0.5
        let sim = jaccard_similarity("a b c", "b c d");
        assert!((sim - 0.5).abs() < 1e-6);
    }

    // ── HybridSearchConfig with temporal decay tests ─────────────────────────

    #[test]
    fn test_hybrid_search_config_includes_temporal_decay_and_mmr() {
        let cfg = HybridSearchConfig::default();
        assert!(!cfg.temporal_decay.enabled);
        assert!((cfg.temporal_decay.half_life_days - 30.0).abs() < 1e-6);
        assert!((cfg.mmr.lambda - 0.7).abs() < 1e-6);
        assert_eq!(cfg.mmr.top_k, 5);
    }

    #[test]
    fn test_hybrid_search_config_with_custom_temporal_decay() {
        let cfg = HybridSearchConfig {
            temporal_decay: TemporalDecayConfig {
                enabled: true,
                half_life_days: 7.0,
            },
            mmr: MmrConfig { lambda: 0.5, top_k: 3 },
            ..HybridSearchConfig::default()
        };

        assert!(cfg.temporal_decay.enabled);
        assert!((cfg.temporal_decay.half_life_days - 7.0).abs() < 1e-6);
        assert!((cfg.mmr.lambda - 0.5).abs() < 1e-6);
        assert_eq!(cfg.mmr.top_k, 3);
    }

    #[test]
    fn test_apply_temporal_decay_preserves_score_order_for_same_age() {
        let config = TemporalDecayConfig {
            enabled: true,
            half_life_days: 30.0,
        };

        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let mut results = vec![
            HybridSearchResult {
                content: "first".to_string(),
                score: 0.9,
                source: "vector".to_string(),
                citation: format!("vector:memory/{}.md", today),
            },
            HybridSearchResult {
                content: "second".to_string(),
                score: 0.7,
                source: "vector".to_string(),
                citation: format!("vector:memory/{}.md", today),
            },
        ];

        apply_temporal_decay(&mut results, &config);

        // Both have same age, so order should be preserved (both decayed equally)
        // Higher initial score should still be higher after decay
        assert_eq!(results[0].content, "first");
        assert_eq!(results[1].content, "second");
        // First should still have higher score than second (order preserved)
        assert!(results[0].score > results[1].score, "Score order should be preserved");
        // Both scores should be reduced from original (since today's date may have
        // slight age)
        assert!(results[0].score <= 0.9);
        assert!(results[1].score <= 0.7);
    }

    #[test]
    fn test_apply_temporal_decay_half_life_correctness() {
        let config = TemporalDecayConfig {
            enabled: true,
            half_life_days: 30.0,
        };

        // Create a result from exactly 30 days ago
        let old_date = (chrono::Utc::now() - chrono::Duration::days(30))
            .format("%Y-%m-%d")
            .to_string();
        let mut results = vec![HybridSearchResult {
            content: "old".to_string(),
            score: 1.0,
            source: "vector".to_string(),
            citation: format!("vector:memory/{}.md", old_date),
        }];

        apply_temporal_decay(&mut results, &config);

        // After one half-life, score should be approximately 0.5
        assert!(
            results[0].score > 0.45 && results[0].score < 0.55,
            "Score after one half-life should be ~0.5, got {}",
            results[0].score
        );
    }

    #[test]
    fn test_apply_temporal_decay_multiple_half_lives() {
        let config = TemporalDecayConfig {
            enabled: true,
            half_life_days: 30.0,
        };

        // Create a result from 90 days ago (3 half-lives)
        let old_date = (chrono::Utc::now() - chrono::Duration::days(90))
            .format("%Y-%m-%d")
            .to_string();
        let mut results = vec![HybridSearchResult {
            content: "very old".to_string(),
            score: 1.0,
            source: "vector".to_string(),
            citation: format!("vector:memory/{}.md", old_date),
        }];

        apply_temporal_decay(&mut results, &config);

        // After 3 half-lives, score should be approximately 0.125 (1/8)
        assert!(
            results[0].score > 0.10 && results[0].score < 0.15,
            "Score after 3 half-lives should be ~0.125, got {}",
            results[0].score
        );
    }

    #[test]
    fn test_mmr_config_defaults() {
        let cfg = MmrConfig::default();
        assert!((cfg.lambda - 0.7).abs() < 1e-6);
        assert_eq!(cfg.top_k, 5);
    }

    #[test]
    fn test_mmr_with_zero_lambda_pure_diversity() {
        let results = vec![
            make_result("rust programming language", 0.9),
            make_result("rust programming tutorial", 0.85), // similar to first
            make_result("python scripting", 0.5),           // diverse
        ];
        let cfg = MmrConfig { lambda: 0.0, top_k: 2 }; // Pure diversity
        let reranked = mmr_rerank(results, &cfg);

        // With pure diversity, should pick diverse results over similar ones
        assert_eq!(reranked.len(), 2);
        // First should be highest relevance
        assert!(reranked[0].content.contains("rust"));
        // Second should be diverse (Python) not the similar rust one
        let has_diverse = reranked.iter().any(|r| r.content.contains("python"));
        assert!(has_diverse, "Should pick diverse Python result with lambda=0");
    }
}
