//! Tool learning — learn from execution failures and suggest alternatives.
//!
//! The [`ToolLearningEngine`] records successes and failures of tool usage,
//! storing experiences in the memory system. When a tool fails, it searches
//! past experiences to suggest known workarounds or alternative tools.
//!
//! Example:
//! ```text
//! xdotool click failed (X11 unavailable) → suggest "ydotool" on Wayland
//! npm install failed (lockfile conflict) → suggest "rm node_modules && npm i"
//! ```

use crate::computer::DesktopAction;
use crate::memory::{Memory, MemoryQuery, MemoryStore};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

/// A recorded experience using a specific tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExperience {
    /// Name of the tool / action variant (e.g. "LaunchApp:xdotool").
    pub tool_name: String,
    /// The action that was attempted.
    pub action: DesktopAction,
    /// Whether the execution succeeded.
    pub success: bool,
    /// Error message if failed.
    pub error: Option<String>,
    /// Alternative tool/action that worked, if any.
    pub alternative: Option<String>,
    /// Context (goal, OS, desktop environment) for matching.
    pub context: ExperienceContext,
    /// How many times this experience has been reinforced.
    #[serde(default)]
    pub reinforcement_count: u32,
}

/// Context in which a tool experience was recorded.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExperienceContext {
    /// The high-level goal being pursued.
    pub goal: String,
    /// Operating system ("linux", "macos", "windows").
    pub os: String,
    /// Desktop environment ("wayland", "x11", "aqua", "win32").
    pub desktop_env: String,
    /// Additional tags (e.g. "docker", "vpn", "proxy").
    pub tags: Vec<String>,
}

impl ExperienceContext {
    /// Build context from the current environment.
    pub fn current(goal: impl Into<String>) -> Self {
        Self {
            goal: goal.into(),
            os: std::env::consts::OS.to_string(),
            desktop_env: detect_desktop_env(),
            tags: Vec::new(),
        }
    }

    /// Add a tag.
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Compute similarity score [0.0, 1.0] with another context.
    pub fn similarity(&self, other: &ExperienceContext) -> f32 {
        let mut score = 0.0;
        let mut weights = 0.0;

        // OS match (weight 0.3).
        weights += 0.3;
        if self.os == other.os {
            score += 0.3;
        }

        // Desktop env match (weight 0.2).
        weights += 0.2;
        if self.desktop_env == other.desktop_env {
            score += 0.2;
        }

        // Goal text similarity via simple word overlap (weight 0.4).
        weights += 0.4;
        score += 0.4 * word_overlap(&self.goal, &other.goal);

        // Tag overlap (weight 0.1).
        weights += 0.1;
        if !self.tags.is_empty() || !other.tags.is_empty() {
            let common: HashMap<_, _> = self.tags.iter().map(|t| (t.to_lowercase(), ())).collect();
            let overlap = other
                .tags
                .iter()
                .filter(|t| common.contains_key(&t.to_lowercase()))
                .count() as f32;
            let max_tags = self.tags.len().max(other.tags.len()) as f32;
            if max_tags > 0.0 {
                score += 0.1 * (overlap / max_tags);
            } else {
                score += 0.1;
            }
        } else {
            score += 0.1;
        }

        if weights > 0.0 {
            score / weights
        } else {
            0.0
        }
    }
}

/// Suggestion produced by the learning engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSuggestion {
    /// The alternative tool or approach.
    pub alternative: String,
    /// Description of why this might work.
    pub reasoning: String,
    /// Confidence based on past success rate.
    pub confidence: f32,
    /// How many times this alternative succeeded in the past.
    pub success_count: u32,
    /// The original failure pattern this addresses.
    pub failure_pattern: String,
}

const DEFAULT_EXPERIENCE_TTL_SECS: u64 = 90 * 24 * 60 * 60; // 90 days

/// Engine that learns from tool execution outcomes.
#[derive(Clone)]
pub struct ToolLearningEngine {
    memory: Arc<dyn MemoryStore>,
    experience_ttl_secs: u64,
}

impl ToolLearningEngine {
    /// Create a new learning engine backed by the given memory store.
    pub fn new(memory: Arc<dyn MemoryStore>) -> Self {
        Self {
            memory,
            experience_ttl_secs: DEFAULT_EXPERIENCE_TTL_SECS,
        }
    }

    /// Set the TTL for stored experiences (default: 90 days).
    /// Set to 0 for no expiry.
    #[allow(dead_code)]
    pub fn with_experience_ttl_secs(mut self, secs: u64) -> Self {
        self.experience_ttl_secs = secs;
        self
    }

    /// Record the outcome of a tool execution.
    pub async fn record_experience(
        &self,
        tool_name: impl Into<String>,
        action: &DesktopAction,
        success: bool,
        error: Option<&str>,
        alternative: Option<&str>,
        context: &ExperienceContext,
    ) -> crate::Result<()> {
        let exp = ToolExperience {
            tool_name: tool_name.into(),
            action: action.clone(),
            success,
            error: error.map(String::from),
            alternative: alternative.map(String::from),
            context: context.clone(),
            reinforcement_count: 1,
        };

        // Check if we already have a similar experience and reinforce it.
        let existing = self.find_similar_experience(&exp).await?;
        if let Some(mut existing_mem) = existing {
            if let Ok(mut existing_exp) =
                serde_json::from_str::<ToolExperience>(&existing_mem.content)
            {
                existing_exp.reinforcement_count += 1;
                existing_mem.content = serde_json::to_string(&existing_exp).unwrap_or_default();
                self.memory.update(existing_mem).await?;
                info!(
                    "Reinforced tool experience for '{}' (count: {})",
                    existing_exp.tool_name, existing_exp.reinforcement_count
                );
                return Ok(());
            }
        }

        let mem = Memory::new(
            "agent",
            serde_json::to_string(&exp).unwrap_or_default(),
            "tool_experience",
        )
        .with_importance_score(if success { 0.6 } else { 0.85 })
        .with_source("tool_learning");

        // Apply TTL to prevent unbounded storage growth.
        let mem = if self.experience_ttl_secs > 0 {
            mem.with_ttl(self.experience_ttl_secs)
        } else {
            mem
        };

        self.memory.store(mem).await?;
        info!("Recorded new tool experience for '{}'", exp.tool_name);
        Ok(())
    }

    /// Given a failed tool and error, search memory for known alternatives.
    pub async fn suggest_alternative(
        &self,
        tool_name: &str,
        error: &str,
        context: &ExperienceContext,
    ) -> crate::Result<Option<ToolSuggestion>> {
        // Search for past experiences with this tool that failed.
        let query = MemoryQuery::new()
            .of_type("tool_experience")
            .with_content(tool_name)
            .limit(20);

        let results = self.memory.search(query).await?;
        if results.is_empty() {
            return Ok(None);
        }

        let mut candidates: Vec<(f32, ToolExperience)> = Vec::new();

        for mem in results {
            let Ok(exp) = serde_json::from_str::<ToolExperience>(&mem.content) else {
                continue;
            };

            // Must be a failure experience with an alternative.
            if exp.success || exp.alternative.is_none() {
                continue;
            }

            // Check if error message similarity is high enough.
            let error_sim = word_overlap(error, exp.error.as_deref().unwrap_or(""));
            if error_sim < 0.3 {
                continue;
            }

            let context_sim = context.similarity(&exp.context);
            let combined_score = error_sim * 0.6 + context_sim * 0.4;

            candidates.push((combined_score, exp));
        }

        // Sort by score descending.
        candidates.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        if let Some((score, best)) = candidates.into_iter().next() {
            let alt = best.alternative.unwrap_or_default();
            let confidence = (score * best.reinforcement_count as f32).min(1.0);

            return Ok(Some(ToolSuggestion {
                alternative: alt.clone(),
                reasoning: format!(
                    "Previously '{}' failed with a similar error; '{}' succeeded instead \
                     (reinforced {} times)",
                    best.tool_name, alt, best.reinforcement_count
                ),
                confidence,
                success_count: best.reinforcement_count,
                failure_pattern: best.error.unwrap_or_default(),
            }));
        }

        Ok(None)
    }

    /// Learn from a failure: record it and return any known alternative.
    pub async fn learn_from_failure(
        &self,
        tool_name: impl Into<String>,
        action: &DesktopAction,
        error: &str,
        context: &ExperienceContext,
    ) -> crate::Result<Option<ToolSuggestion>> {
        let tool_name = tool_name.into();

        // First record the failure.
        self.record_experience(
            &tool_name,
            action,
            false,
            Some(error),
            None,
            context,
        )
        .await?;

        // Then search for alternatives.
        self.suggest_alternative(&tool_name, error, context).await
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    async fn find_similar_experience(
        &self,
        exp: &ToolExperience,
    ) -> crate::Result<Option<crate::memory::Memory>> {
        let query = MemoryQuery::new()
            .of_type("tool_experience")
            .with_content(&exp.tool_name)
            .limit(10);

        let results = self.memory.search(query).await?;
        for mem in results {
            let Ok(existing) = serde_json::from_str::<ToolExperience>(&mem.content) else {
                continue;
            };
            if existing.tool_name == exp.tool_name
                && existing.success == exp.success
                && word_overlap(
                    existing.error.as_deref().unwrap_or(""),
                    exp.error.as_deref().unwrap_or(""),
                ) > 0.7
            {
                return Ok(Some(mem));
            }
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn detect_desktop_env() -> String {
    if let Ok(de) = std::env::var("XDG_SESSION_TYPE") {
        return de.to_lowercase();
    }
    if let Ok(wayland) = std::env::var("WAYLAND_DISPLAY") {
        if !wayland.is_empty() {
            return "wayland".to_string();
        }
    }
    if let Ok(display) = std::env::var("DISPLAY") {
        if !display.is_empty() {
            return "x11".to_string();
        }
    }
    match std::env::consts::OS {
        "macos" => "aqua".to_string(),
        "windows" => "win32".to_string(),
        _ => "unknown".to_string(),
    }
}

/// Simple word-overlap similarity [0.0, 1.0].
fn word_overlap(a: &str, b: &str) -> f32 {
    let a_words: std::collections::HashSet<String> = a
        .to_lowercase()
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| !w.is_empty() && w.len() > 2)
        .collect();
    let b_words: std::collections::HashSet<String> = b
        .to_lowercase()
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| !w.is_empty() && w.len() > 2)
        .collect();

    if a_words.is_empty() || b_words.is_empty() {
        return 0.0;
    }

    let common = a_words.intersection(&b_words).count() as f32;
    let union = a_words.union(&b_words).count() as f32;

    if union > 0.0 {
        common / union
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_experience_context_similarity() {
        let a = ExperienceContext {
            goal: "deploy app to server".to_string(),
            os: "linux".to_string(),
            desktop_env: "wayland".to_string(),
            tags: vec!["docker".to_string()],
        };
        let b = ExperienceContext {
            goal: "deploy app to remote server".to_string(),
            os: "linux".to_string(),
            desktop_env: "wayland".to_string(),
            tags: vec!["docker".to_string()],
        };
        let sim = a.similarity(&b);
        assert!(sim > 0.7, "similar contexts should have high similarity: {}", sim);
    }

    #[test]
    fn test_experience_context_dissimilar() {
        let a = ExperienceContext {
            goal: "deploy app".to_string(),
            os: "linux".to_string(),
            desktop_env: "x11".to_string(),
            tags: vec![],
        };
        let b = ExperienceContext {
            goal: "edit a photo".to_string(),
            os: "windows".to_string(),
            desktop_env: "win32".to_string(),
            tags: vec!["gaming".to_string()],
        };
        let sim = a.similarity(&b);
        assert!(sim < 0.5, "dissimilar contexts should have low similarity: {}", sim);
    }

    #[test]
    fn test_word_overlap() {
        let a = "command not found: xdotool";
        let b = "command not found: ydotool";
        let sim = word_overlap(a, b);
        assert!(sim > 0.3, "overlap should detect similar error patterns: {}", sim);
    }

    #[test]
    fn test_detect_desktop_env() {
        let env = detect_desktop_env();
        assert!(
            ["wayland", "x11", "aqua", "win32", "unknown"].contains(&env.as_str()),
            "desktop env should be one of known values: {}",
            env
        );
    }

    #[test]
    fn test_tool_suggestion_display() {
        let sugg = ToolSuggestion {
            alternative: "ydotool".to_string(),
            reasoning: "Use ydotool on Wayland".to_string(),
            confidence: 0.85,
            success_count: 3,
            failure_pattern: "command not found: xdotool".to_string(),
        };
        assert_eq!(sugg.alternative, "ydotool");
        assert!(sugg.confidence > 0.0);
    }
}
