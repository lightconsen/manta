//! Dream phase configuration, budget, metrics, checkpoint types.

use super::*;

/// Dream execution speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DreamSpeed {
    Fast,
    Balanced,
    Slow,
}

/// Dream thinking depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DreamThinking {
    Low,
    Medium,
    High,
}

/// Dream budget level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DreamBudget {
    Cheap,
    Medium,
    Expensive,
}

/// Configuration for the dreaming engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamConfig {
    /// Whether dreaming is enabled.
    pub enabled: bool,
    /// Cron expression for scheduling.
    pub frequency: String,
    /// Execution speed.
    pub speed: DreamSpeed,
    /// Thinking depth.
    pub thinking: DreamThinking,
    /// Budget level.
    pub budget: DreamBudget,
    /// Similarity threshold for deduplication (0.0-1.0).
    pub dedup_similarity_threshold: f32,
    /// Minimum memories to trigger a dream.
    pub min_memories: usize,
    /// Maximum memories to process per dream cycle.
    pub max_memories_per_cycle: usize,
}

impl Default for DreamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            frequency: DEFAULT_MEMORY_DREAMING_FREQUENCY.to_string(),
            speed: DreamSpeed::Balanced,
            thinking: DreamThinking::Medium,
            budget: DreamBudget::Medium,
            dedup_similarity_threshold: 0.95,
            min_memories: 10,
            max_memories_per_cycle: 500,
        }
    }
}

/// Dream phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DreamPhase {
    Light,
    Deep,
    Rem,
}

impl std::fmt::Display for DreamPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DreamPhase::Light => write!(f, "light"),
            DreamPhase::Deep => write!(f, "deep"),
            DreamPhase::Rem => write!(f, "rem"),
        }
    }
}

/// Result of a single dream cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamResult {
    /// Unique dream run ID.
    pub dream_id: String,
    /// Phase that ran.
    pub phase: DreamPhase,
    /// When the dream started.
    pub started_at: SystemTime,
    /// When the dream finished.
    pub finished_at: SystemTime,
    /// Duration of the dream cycle in milliseconds.
    pub duration_ms: u64,
    /// Number of memories processed.
    pub memories_processed: u32,
    /// Number of memories created (summaries, merged, etc.).
    pub memories_created: u32,
    /// Number of memories deduplicated/removed.
    pub memories_removed: u32,
    /// Number of memories promoted.
    pub memories_promoted: u32,
    /// Number of memories demoted.
    pub memories_demoted: u32,
    /// Peak memory usage observed during the cycle, in megabytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_memory_mb: Option<f64>,
    /// Estimated LLM input tokens consumed during the cycle.
    pub llm_tokens_input: u32,
    /// Estimated LLM output tokens produced during the cycle.
    pub llm_tokens_output: u32,
    /// Human-readable summary.
    pub summary: String,
    /// Errors encountered (non-fatal).
    pub errors: Vec<String>,
    /// Whether the dream was cancelled mid-execution.
    pub cancelled: bool,
}

/// Observability counters for dream activity.
///
/// All counters use relaxed ordering — they are meant for observability,
/// not for synchronisation.
#[derive(Debug, Default)]
pub struct DreamMetrics {
    /// Total number of dream cycles started.
    pub dreams_total: AtomicU64,
    /// Total number of dream cycles that failed.
    pub dreams_failed: AtomicU64,
    /// Total memories processed across all dreams.
    pub memories_processed_total: AtomicU64,
    /// Total memories created across all dreams.
    pub memories_created_total: AtomicU64,
    /// Total memories removed across all dreams.
    pub memories_removed_total: AtomicU64,
    /// Total memories promoted across all dreams.
    pub memories_promoted_total: AtomicU64,
    /// Total memories demoted across all dreams.
    pub memories_demoted_total: AtomicU64,
    /// Total dream duration across all cycles, in milliseconds.
    pub dream_duration_ms_total: AtomicU64,
    /// Total estimated LLM input tokens consumed during dreams.
    pub llm_tokens_input_total: AtomicU64,
    /// Total estimated LLM output tokens produced during dreams.
    pub llm_tokens_output_total: AtomicU64,
}

impl DreamMetrics {
    /// Record a completed dream cycle.
    pub fn record(&self, result: &DreamResult, failed: bool) {
        self.dreams_total.fetch_add(1, Ordering::Relaxed);
        if failed {
            self.dreams_failed.fetch_add(1, Ordering::Relaxed);
        }
        self.memories_processed_total
            .fetch_add(result.memories_processed as u64, Ordering::Relaxed);
        self.memories_created_total
            .fetch_add(result.memories_created as u64, Ordering::Relaxed);
        self.memories_removed_total
            .fetch_add(result.memories_removed as u64, Ordering::Relaxed);
        self.memories_promoted_total
            .fetch_add(result.memories_promoted as u64, Ordering::Relaxed);
        self.memories_demoted_total
            .fetch_add(result.memories_demoted as u64, Ordering::Relaxed);
        self.dream_duration_ms_total
            .fetch_add(result.duration_ms, Ordering::Relaxed);
        self.llm_tokens_input_total
            .fetch_add(result.llm_tokens_input as u64, Ordering::Relaxed);
        self.llm_tokens_output_total
            .fetch_add(result.llm_tokens_output as u64, Ordering::Relaxed);
    }
}

/// Recovery checkpoint for resuming interrupted dreams.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamCheckpoint {
    /// Last processed memory ID.
    pub last_memory_id: Option<String>,
    /// Phase that was interrupted.
    pub phase: Option<DreamPhase>,
    /// Timestamp of the checkpoint.
    pub timestamp: SystemTime,
}

impl Default for DreamCheckpoint {
    fn default() -> Self {
        Self {
            last_memory_id: None,
            phase: None,
            timestamp: SystemTime::UNIX_EPOCH,
        }
    }
}
