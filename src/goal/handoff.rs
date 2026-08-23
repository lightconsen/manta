//! Structured handoff between rounds of the fresh-context ("Ralph") goal
//! loop.
//!
//! In fresh-context mode every round runs in a brand-new seedless sub-agent:
//! no parent conversation prefix and no accumulated session. The workspace on
//! disk is the long-term memory, and the *only* state carried between rounds
//! is this bounded JSON handoff.
//!
//! # Extraction contract
//!
//! The round agent must end its final reply with exactly one fenced block
//! tagged [`HANDOFF_FENCE_TAG`] containing this JSON schema:
//!
//! ```text
//! ```handoff
//! {"status": "continue", "summary": "...", "next_steps": ["..."], "evidence": ["..."]}
//! ```
//! ```
//!
//! Validation is strict: an over-limit or schema-invalid handoff fails the
//! whole round. Semantics are never silently truncated or guessed.

use serde::{Deserialize, Serialize};

/// Maximum accepted handoff payload size (characters). A larger block is
/// rejected outright instead of truncated so semantics are never lost.
pub const MAX_HANDOFF_CHARS: usize = 16 * 1024;

/// Fence tag marking the handoff block in the agent's final reply.
pub const HANDOFF_FENCE_TAG: &str = "handoff";

/// Round status reported by the executing agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffStatus {
    /// More work remains; `next_steps` seeds the next fresh round.
    Continue,
    /// The agent believes the goal is met. Deterministic conditions are still
    /// checked by the runner and remain authoritative.
    Complete,
    /// The agent cannot proceed; aborts the loop for human review.
    Failed,
}

/// The bounded structured handoff carried between fresh-context rounds.
///
/// This is the only LLM-produced state that survives a round boundary (besides
/// whatever the agent wrote to the workspace itself). See the module docs for
/// the extraction contract and validation rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundHandoff {
    /// What the agent decided about the loop state.
    pub status: HandoffStatus,
    /// Non-empty human-readable description of what this round did or found.
    pub summary: String,
    /// Concrete tasks for the next round. Required non-empty for
    /// [`HandoffStatus::Continue`].
    #[serde(default)]
    pub next_steps: Vec<String>,
    /// Pointers proving completion (file paths, command output quotes).
    /// Required non-empty for [`HandoffStatus::Complete`].
    #[serde(default)]
    pub evidence: Vec<String>,
}

impl RoundHandoff {
    /// Enforce the semantic rules beyond JSON parsing:
    ///
    /// - `summary` must be a non-empty string;
    /// - `continue` requires non-empty `next_steps`;
    /// - `complete` requires non-empty `evidence`.
    ///
    /// Entries that are empty or whitespace-only count as missing.
    pub fn validate(&self) -> Result<(), String> {
        if self.summary.trim().is_empty() {
            return Err("`summary` must be a non-empty string".to_string());
        }
        let all_blank = |items: &[String]| items.iter().all(|s| s.trim().is_empty());
        match self.status {
            HandoffStatus::Continue if all_blank(&self.next_steps) => {
                Err("status \"continue\" requires a non-empty `next_steps` array".to_string())
            }
            HandoffStatus::Complete if all_blank(&self.evidence) => {
                Err("status \"complete\" requires a non-empty `evidence` array".to_string())
            }
            _ => Ok(()),
        }
    }
}

/// Extract and validate the handoff from a round's final assistant reply.
///
/// Returns the parsed [`RoundHandoff`] or a descriptive rejection reason.
/// Rejections are terminal for the round — callers must not retry with a
/// guessed or truncated handoff.
pub fn extract_handoff(final_text: &str) -> Result<RoundHandoff, String> {
    let raw = extract_handoff_block(final_text).ok_or_else(|| {
        format!("no valid ```{} fenced block found in final reply", HANDOFF_FENCE_TAG)
    })?;
    let char_count = raw.chars().count();
    if char_count > MAX_HANDOFF_CHARS {
        return Err(format!(
            "handoff is {} characters, over the limit of {} — shorten it instead of truncating",
            char_count, MAX_HANDOFF_CHARS
        ));
    }
    let parsed: RoundHandoff = serde_json::from_str(raw.trim())
        .map_err(|e| format!("handoff block is not schema-valid JSON: {}", e))?;
    parsed.validate()?;
    Ok(parsed)
}

/// Pull the body of the first fenced block tagged [`HANDOFF_FENCE_TAG`] out of
/// `text`. Returns `None` when the opening fence is absent or never closed.
fn extract_handoff_block(text: &str) -> Option<String> {
    let open_prefix = format!("```{}", HANDOFF_FENCE_TAG);
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        // The tag must be the whole fence info-string (allow trailing
        // whitespace only, so e.g. ```handoffs does not match).
        let opens = trimmed == open_prefix
            || (trimmed.starts_with(&open_prefix)
                && trimmed[open_prefix.len()..].trim().is_empty());
        if !opens {
            continue;
        }
        let mut content = Vec::new();
        for line in lines.by_ref() {
            if line.trim().starts_with("```") {
                return Some(content.join("\n"));
            }
            content.push(line);
        }
        // Opening fence never closed — malformed output.
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handoff_json(status: &str, summary: &str, steps: &[&str], evidence: &[&str]) -> String {
        let obj = serde_json::json!({
            "status": status,
            "summary": summary,
            "next_steps": steps,
            "evidence": evidence,
        });
        serde_json::to_string_pretty(&obj).unwrap()
    }

    fn fenced(body: &str) -> String {
        format!("work done...\n\n```{}\n{}\n```\n", HANDOFF_FENCE_TAG, body)
    }

    // ── Accept cases ─────────────────────────────────────────────────────

    #[test]
    fn accept_continue_with_next_steps() {
        let text = fenced(&handoff_json(
            "continue",
            "wrote half the report",
            &["write chapter 2", "run linter"],
            &[],
        ));
        let h = extract_handoff(&text).unwrap();
        assert_eq!(h.status, HandoffStatus::Continue);
        assert_eq!(h.next_steps.len(), 2);
        assert_eq!(h.summary, "wrote half the report");
    }

    #[test]
    fn accept_complete_with_evidence() {
        let text = fenced(&handoff_json(
            "complete",
            "all done",
            &[],
            &["report/final.md exists", "`cargo test` exits 0"],
        ));
        let h = extract_handoff(&text).unwrap();
        assert_eq!(h.status, HandoffStatus::Complete);
        assert_eq!(h.evidence.len(), 2);
    }

    #[test]
    fn accept_failed_with_summary_only() {
        let text = fenced(&handoff_json("failed", "toolchain broken", &[], &[]));
        let h = extract_handoff(&text).unwrap();
        assert_eq!(h.status, HandoffStatus::Failed);
    }

    #[test]
    fn accept_block_embedded_in_prose_and_compact_json() {
        let text = format!(
            "# Round 3 notes\nI updated the file.\n```{}\n{}\n```\n\nDone.",
            HANDOFF_FENCE_TAG, r#"{"status":"failed","summary":"blocked"}"#
        );
        let h = extract_handoff(&text).unwrap();
        assert_eq!(h.status, HandoffStatus::Failed);
        assert_eq!(h.summary, "blocked");
    }

    #[test]
    fn accept_optional_arrays_default_to_empty() {
        let text = fenced(r#"{"status": "failed", "summary": "no arrays here"}"#);
        let h = extract_handoff(&text).unwrap();
        assert!(h.next_steps.is_empty());
        assert!(h.evidence.is_empty());
    }

    // ── Reject cases ─────────────────────────────────────────────────────

    #[test]
    fn reject_missing_block() {
        let err = extract_handoff("I finished everything, trust me.").unwrap_err();
        assert!(err.contains("no valid ```handoff"), "{}", err);
    }

    #[test]
    fn reject_unterminated_block() {
        let text = "intro\n```handoff\n{\"status\":\"failed\",\"summary\":\"x\"}\n";
        assert!(extract_handoff(text).is_err());
    }

    #[test]
    fn reject_over_limit_without_truncation() {
        let big_summary = "x".repeat(MAX_HANDOFF_CHARS + 1);
        let json = handoff_json("failed", &big_summary, &[], &[]);
        let text = fenced(&json);
        let err = extract_handoff(&text).unwrap_err();
        assert!(err.contains("over the limit"), "{}", err);
    }

    #[test]
    fn reject_continue_without_next_steps() {
        let json = handoff_json("continue", "halfway", &[], &[]);
        let err = extract_handoff(&fenced(&json)).unwrap_err();
        assert!(err.contains("next_steps"), "{}", err);
    }

    #[test]
    fn reject_continue_with_whitespace_only_next_steps() {
        let json = handoff_json("continue", "halfway", &["   ", ""], &[]);
        let err = extract_handoff(&fenced(&json)).unwrap_err();
        assert!(err.contains("next_steps"), "{}", err);
    }

    #[test]
    fn reject_complete_without_evidence() {
        let json = handoff_json("complete", "done", &["tidy up"], &[]);
        let err = extract_handoff(&fenced(&json)).unwrap_err();
        assert!(err.contains("evidence"), "{}", err);
    }

    #[test]
    fn reject_empty_summary() {
        let json = handoff_json("failed", "  ", &[], &[]);
        let err = extract_handoff(&fenced(&json)).unwrap_err();
        assert!(err.contains("summary"), "{}", err);
    }

    #[test]
    fn reject_invalid_status_value() {
        let json = handoff_json("finished", "done", &[], &[]);
        let err = extract_handoff(&fenced(&json)).unwrap_err();
        assert!(err.contains("schema-valid JSON"), "{}", err);
    }

    #[test]
    fn reject_malformed_json() {
        let err = extract_handoff(&fenced("{not json")).unwrap_err();
        assert!(err.contains("schema-valid JSON"), "{}", err);
    }

    #[test]
    fn reject_unknown_fields() {
        let json = r#"{"status":"failed","summary":"s","bonus":"surprise"}"#;
        let err = extract_handoff(&fenced(json)).unwrap_err();
        assert!(err.contains("schema-valid JSON"), "{}", err);
    }

    #[test]
    fn reject_similarly_tagged_fence() {
        // A fence tagged `handoffs` must not satisfy the contract.
        let text = "```handoffs\n{\"status\":\"failed\",\"summary\":\"s\"}\n```";
        assert!(extract_handoff(text).is_err());
    }
}
