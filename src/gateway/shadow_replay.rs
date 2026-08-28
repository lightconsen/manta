//! Online shadow replay (N=1) — §09 ShadowTraffic over sampled live turns.
//!
//! The offline quality gates replay a hand-authored regression suite. This
//! module replays *real* sampled production turns ([`crate::eval::TurnSample`])
//! through a candidate agent, decoupled from the gate wiring:
//!
//! - [`samples_to_replay_turns`] maps stored samples to replay turns.
//! - [`replay_shadow`] runs each turn through an [`EvalHarness`] (N=1 trials is
//!   the natural shadow form) and aggregates a [`ShadowReport`] exactly like
//!   `QualityGate::run_shadow`.
//! - [`compare_replays`] bootstrap-compares a baseline vs a candidate pass/fail
//!   sequence, producing a [`VersionComparison`].
//!
//! The `ShadowReport` type and aggregation logic mirror
//! [`crate::gateway::quality_gate::ShadowReport`] / `run_shadow`; this module
//! cannot reuse `QualityGate` directly because it is fed from the sampled-turn
//! store rather than a caller-provided slice of `ProdTurn`s.

use tracing::warn;

use crate::eval::harness::EvalHarness;
use crate::eval::{compare_versions, EvalTask, TurnSample, VersionComparison};
use crate::gateway::quality_gate::ShadowReport;

/// A single production turn selected for online shadow replay.
#[derive(Debug, Clone)]
pub struct ReplayTurn {
    /// The user input sent to the agent.
    pub input: String,
    /// Optional session context (e.g. the conversation id). Carried for
    /// downstream use; `EvalTask` has no context field yet, so replay
    /// currently ignores it.
    pub context: Option<String>,
    /// The sampled turn's stable id (used as the eval task id).
    pub turn_id: String,
    /// The model that served the original turn, if recorded.
    pub model: Option<String>,
    /// When the original turn was sampled (unix millis).
    pub created_at: i64,
}

/// Map stored [`TurnSample`]s to replay turns.
///
/// - `input` ← sample.input
/// - `context` ← `Some(conversation_id)` when non-empty, else `None`
/// - `turn_id` ← sample.turn_id
/// - `model` ← `Some(model)` when non-empty, else `None`
/// - `created_at` ← sample.created_at
pub fn samples_to_replay_turns(samples: &[TurnSample]) -> Vec<ReplayTurn> {
    samples
        .iter()
        .map(|s| ReplayTurn {
            input: s.input.clone(),
            context: if s.conversation_id.is_empty() {
                None
            } else {
                Some(s.conversation_id.clone())
            },
            turn_id: s.turn_id.clone(),
            model: if s.model.is_empty() {
                None
            } else {
                Some(s.model.clone())
            },
            created_at: s.created_at,
        })
        .collect()
}

/// Default bootstrap iterations for [`compare_replays`] (matches the offline
/// gate's default).
const REPLAY_BOOTSTRAP_ITERATIONS: usize = 10_000;
/// Default confidence level for [`compare_replays`].
const REPLAY_CONFIDENCE: f64 = 0.95;

/// Compare a baseline vs a candidate per-turn pass/fail sequence, producing a
/// bootstrap [`VersionComparison`] (via [`compare_versions`]).
///
/// `true` = pass, `false` = fail. The two slices need not have the same
/// length.
pub fn compare_replays(baseline: &[bool], candidate: &[bool]) -> VersionComparison {
    compare_versions(baseline, candidate, REPLAY_BOOTSTRAP_ITERATIONS, REPLAY_CONFIDENCE)
}

/// Replay sampled production turns through a harness and aggregate a
/// [`ShadowReport`].
///
/// Aggregation mirrors `QualityGate::run_shadow`: a turn counts as passed when
/// its summary has `pass_rate > 0.0`, and latency is averaged from the
/// per-turn summaries. `trials` controls the harness trial count per turn —
/// `1` is the natural N=1 online shadow form.
pub async fn replay_shadow(
    harness: &EvalHarness,
    turns: &[ReplayTurn],
    trials: usize,
) -> ShadowReport {
    let total = turns.len();
    if total == 0 {
        return ShadowReport {
            total_turns: 0,
            pass_rate: 1.0,
            avg_latency_ms: 0.0,
            tool_accuracy: 1.0,
        };
    }

    let mut passed = 0usize;
    let mut total_latency = 0u64;

    for turn in turns {
        // Trivial single-task eval whose id traces back to the sampled turn.
        let task = EvalTask {
            id: format!("shadow_{}", turn.turn_id),
            input: turn.input.clone(),
            ..Default::default()
        };
        match harness.run(task, trials).await {
            Ok(summary) => {
                if summary.pass_rate > 0.0 {
                    passed += 1;
                }
                total_latency += summary.avg_duration_ms as u64;
            }
            Err(e) => warn!("shadow replay turn '{}' failed: {}", turn.turn_id, e),
        }
    }

    ShadowReport {
        total_turns: total,
        pass_rate: passed as f64 / total as f64,
        avg_latency_ms: total_latency as f64 / total as f64,
        tool_accuracy: 1.0, // simplified; real impl would check tool call correctness
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::{ComparisonVerdict, SampleVerdict};

    fn sample(turn_id: &str, input: &str, conversation_id: &str, model: &str) -> TurnSample {
        TurnSample {
            turn_id: turn_id.to_string(),
            session_id: Some("s1".into()),
            agent_id: "worker".into(),
            conversation_id: conversation_id.to_string(),
            input: input.to_string(),
            response: "response".into(),
            model: model.to_string(),
            cache_hit: false,
            total_tokens: 10,
            latency_ms: 5,
            verdict: SampleVerdict::Pass,
            risk_signals: vec![],
            created_at: 1_000,
        }
    }

    #[test]
    fn samples_to_replay_turns_maps_fields() {
        let samples = vec![
            sample("t1", "hello", "conv-1", "claude-sonnet-4-6"),
            sample("t2", "world", "", ""),
        ];

        let turns = samples_to_replay_turns(&samples);
        assert_eq!(turns.len(), 2);

        // Full fields map through.
        assert_eq!(turns[0].turn_id, "t1");
        assert_eq!(turns[0].input, "hello");
        assert_eq!(turns[0].context.as_deref(), Some("conv-1"));
        assert_eq!(turns[0].model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(turns[0].created_at, 1_000);

        // Empty conversation id and model map to None.
        assert_eq!(turns[1].context, None);
        assert_eq!(turns[1].model, None);
    }

    #[test]
    fn compare_replays_identifies_clear_regression() {
        let baseline = vec![true, true, true, true, true];
        let candidate = vec![false, false, false, false, false];
        let comp = compare_replays(&baseline, &candidate);
        // Deterministic: every bootstrap draw of the baseline is 1.0 and every
        // draw of the candidate is 0.0, so the CI is entirely negative.
        assert_eq!(comp.verdict, ComparisonVerdict::Regressed);
        assert!((comp.old_pass_rate - 1.0).abs() < 1e-9);
        assert!(comp.new_pass_rate.abs() < 1e-9);
        assert!(comp.delta < 0.0);
    }

    #[test]
    fn compare_replays_equal_is_no_change() {
        let baseline = vec![true, true, false, false];
        let candidate = vec![true, true, false, false];
        let comp = compare_replays(&baseline, &candidate);
        // Observed rates are identical, so delta is exactly 0 and the CI
        // contains 0.
        assert!(comp.delta.abs() < f64::EPSILON);
        assert_eq!(comp.verdict, ComparisonVerdict::NoSignificantChange);
    }

    #[test]
    fn compare_replays_insufficient_data_short_sequences() {
        let comp = compare_replays(&[true], &[false]);
        assert_eq!(comp.verdict, ComparisonVerdict::InsufficientData);
    }
}
