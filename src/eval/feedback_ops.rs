//! Feedback operations aggregation report (Wave 2 — Agent 3).
//!
//! Rule-based (no-LLM) aggregation over recent Like/Dislike votes plus the
//! pending badcase pool, served read-only over the WS `feedback.ops` method.
//!
//! The report is a snapshot of operational signal for the feedback loop:
//!
//! - `total_votes` / `up` / `down` — Like/Dislike tallies inside the window.
//! - `by_agent` — per-agent vote tallies, sorted by total (descending).
//! - `pending_by_source` — pending badcase counts grouped by collection source
//!   (`online:risk` vs `human:dislike`), sorted by count (descending).
//! - `by_day` — 14-day daily vote series ending today.
//! - `down_votes` — every 👎 vote with the matched turn input (truncated) and
//!   its risk signals, so operators can eyeball why users disliked turns.
//! - `risk_clusters` — risk-signal label → down-vote count (sorted desc), i.e.
//!   the most common reasons behind dislikes.

use std::collections::HashMap;

use serde::Serialize;

use crate::error::Result;
use crate::eval::pending_badcase::{PendingBadcase, PendingBadcaseStore, PendingStatus};
use crate::eval::scorer::{RiskSignalChecker, RiskTurnInput};
use crate::gateway::{FeedbackStore, FeedbackVoteKind};

/// Number of daily buckets in the `by_day` series.
const REPORT_DAYS: usize = 14;
/// Milliseconds per day, for daily bucketing.
const DAY_MS: i64 = 86_400_000;
/// Max rows pulled per store when aggregating (mirrors the eval-dashboard cap).
const AGG_LIMIT: u32 = 50_000;
/// Maximum length of the input preview embedded in a down-vote summary.
const DOWN_INPUT_MAX: usize = 200;

/// Per-agent vote tallies for the report window.
#[derive(Debug, Clone, Serialize)]
pub struct AgentVoteSummary {
    /// Agent id; votes recorded without one are grouped under `"unknown"`.
    pub agent_id: String,
    pub up: u64,
    pub down: u64,
    pub total: u64,
}

/// A pending-badcase count grouped by collection source label.
#[derive(Debug, Clone, Serialize)]
pub struct SourceCount {
    /// Source label, e.g. `online:risk` or `human:dislike`.
    pub source: String,
    pub count: u64,
}

/// One day of the 14-day daily vote series.
#[derive(Debug, Clone, Serialize)]
pub struct DayVoteSummary {
    /// Day label (`YYYY-MM-DD`, UTC).
    pub day: String,
    pub up: u64,
    pub down: u64,
    pub total: u64,
}

/// A single 👎 vote within the report window.
#[derive(Debug, Clone, Serialize)]
pub struct DownVoteSummary {
    pub turn_id: String,
    /// The turn input matched from the pending badcase pool (truncated).
    /// Empty when no pending badcase references the turn.
    pub input: String,
    /// Risk signals associated with the dislike (stored or rule-derived).
    pub risk_signals: Vec<String>,
}

/// A risk-signal label grouped by how many down votes it fired on.
#[derive(Debug, Clone, Serialize)]
pub struct RiskCluster {
    pub label: String,
    pub count: u64,
}

/// Rule-based feedback operations aggregation report.
#[derive(Debug, Clone, Serialize)]
pub struct FeedbackOpsReport {
    /// Start of the aggregation window (unix millis).
    pub since_ms: i64,
    pub total_votes: u64,
    pub up: u64,
    pub down: u64,
    /// Per-agent tallies, sorted by total (descending).
    pub by_agent: Vec<AgentVoteSummary>,
    /// Pending badcase counts by source, sorted by count (descending).
    pub pending_by_source: Vec<SourceCount>,
    /// 14-day daily vote series ending today, ascending.
    pub by_day: Vec<DayVoteSummary>,
    /// Down-vote summaries, newest first.
    pub down_votes: Vec<DownVoteSummary>,
    /// Risk-signal label → down-vote count, sorted by count (descending).
    pub risk_clusters: Vec<RiskCluster>,
}

/// Build the rule-based feedback operations report over votes created at or
/// after `since_ms` plus the pending badcase pool.
///
/// Down-vote summaries and risk clusters are enriched from the pending
/// badcase pool matched by `turn_id`: stored risk signals are reused when
/// present, otherwise `RiskSignalChecker::scan_turn` derives them from the
/// turn's input/response. Down votes without a matching pending badcase have
/// no input/risk signals (nothing reliable to say about them).
pub async fn build_ops_report(
    feedback: &FeedbackStore,
    pending: &PendingBadcaseStore,
    since_ms: i64,
) -> Result<FeedbackOpsReport> {
    let up_votes = feedback
        .list_votes_by(FeedbackVoteKind::Up, since_ms, AGG_LIMIT)
        .await?;
    let down_votes = feedback
        .list_votes_by(FeedbackVoteKind::Down, since_ms, AGG_LIMIT)
        .await?;
    let pending_rows = pending
        .list_pending(PendingStatus::Pending, AGG_LIMIT)
        .await?;

    // ── per-agent tallies (agent_id → (up, down)) ───────────────────────────
    let mut agent_counts: HashMap<String, (u64, u64)> = HashMap::new();
    for v in up_votes.iter().chain(down_votes.iter()) {
        let label = v.agent_id.clone().unwrap_or_else(|| "unknown".to_string());
        let entry = agent_counts.entry(label).or_insert((0, 0));
        if v.vote == FeedbackVoteKind::Up {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
    }
    let mut by_agent: Vec<AgentVoteSummary> = agent_counts
        .into_iter()
        .map(|(agent_id, (up, down))| AgentVoteSummary {
            agent_id,
            up,
            down,
            total: up + down,
        })
        .collect();
    by_agent.sort_by(|a, b| {
        b.total
            .cmp(&a.total)
            .then_with(|| a.agent_id.cmp(&b.agent_id))
    });

    // ── pending badcases: source grouping + per-turn lookup for down votes ──
    let mut source_counts: HashMap<String, u64> = HashMap::new();
    let mut pending_by_turn: HashMap<String, &PendingBadcase> = HashMap::new();
    for b in &pending_rows {
        *source_counts
            .entry(b.source.as_str().to_string())
            .or_insert(0) += 1;
        if let Some(turn_id) = &b.turn_id {
            pending_by_turn.entry(turn_id.clone()).or_insert(b);
        }
    }
    let mut pending_by_source: Vec<SourceCount> = source_counts
        .into_iter()
        .map(|(source, count)| SourceCount { source, count })
        .collect();
    pending_by_source.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.source.cmp(&b.source)));

    // ── down-vote summaries + risk clusters ─────────────────────────────────
    let checker = RiskSignalChecker::default();
    let mut down_votes_out = Vec::with_capacity(down_votes.len());
    let mut cluster_counts: HashMap<String, u64> = HashMap::new();
    for v in &down_votes {
        let (input, signals) = match pending_by_turn.get(&v.turn_id) {
            Some(b) => {
                let input = truncate(&b.input, DOWN_INPUT_MAX);
                let signals = if b.risk_signals.is_empty() {
                    checker.scan_turn(&RiskTurnInput {
                        input: b.input.clone(),
                        response: b.response.clone(),
                        tool_call_count: 0,
                    })
                } else {
                    b.risk_signals.clone()
                };
                (input, signals)
            }
            None => (String::new(), Vec::new()),
        };
        for signal in &signals {
            *cluster_counts.entry(signal.clone()).or_insert(0) += 1;
        }
        down_votes_out.push(DownVoteSummary {
            turn_id: v.turn_id.clone(),
            input,
            risk_signals: signals,
        });
    }
    let mut risk_clusters: Vec<RiskCluster> = cluster_counts
        .into_iter()
        .map(|(label, count)| RiskCluster { label, count })
        .collect();
    risk_clusters.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));

    // ── 14-day daily series (buckets bounded at UTC midnight) ───────────────
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut day_up = [0u64; REPORT_DAYS];
    let mut day_down = [0u64; REPORT_DAYS];
    for v in up_votes.iter().chain(down_votes.iter()) {
        if let Some(i) = day_bucket(v.created_at, now_ms) {
            if v.vote == FeedbackVoteKind::Up {
                day_up[i] += 1;
            } else {
                day_down[i] += 1;
            }
        }
    }
    let by_day = (0..REPORT_DAYS)
        .map(|i| {
            let day_ms = now_ms - (REPORT_DAYS - 1 - i) as i64 * DAY_MS;
            let day = day_label(day_ms);
            let up = day_up[i];
            let down = day_down[i];
            DayVoteSummary {
                day,
                up,
                down,
                total: up + down,
            }
        })
        .collect();

    Ok(FeedbackOpsReport {
        since_ms,
        total_votes: (up_votes.len() + down_votes.len()) as u64,
        up: up_votes.len() as u64,
        down: down_votes.len() as u64,
        by_agent,
        pending_by_source,
        by_day,
        down_votes: down_votes_out,
        risk_clusters,
    })
}

/// Map a unix-ms timestamp to its daily bucket index within the 14-day window
/// (0 = oldest day, `REPORT_DAYS - 1` = today). Rows in the future or older
/// than the window return `None` (skipped).
fn day_bucket(ms: i64, now_ms: i64) -> Option<usize> {
    let ago_days = (now_ms - ms) / DAY_MS;
    if ago_days < 0 || ago_days >= REPORT_DAYS as i64 {
        return None;
    }
    Some(REPORT_DAYS - 1 - ago_days as usize)
}

/// UTC `YYYY-MM-DD` label for a unix-ms timestamp.
fn day_label(day_ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(day_ms / 1000, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// Truncate a string to `max` chars, appending `...` when cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push_str("...");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared in-memory pool wired into both stores so all tables live in one
    /// database (`:memory:` pools are per-connection).
    async fn test_stores() -> (sqlx::SqlitePool, FeedbackStore, PendingBadcaseStore) {
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        let feedback = FeedbackStore::from_pool(pool.clone()).await.unwrap();
        let pending = PendingBadcaseStore::from_pool(pool.clone()).await.unwrap();
        (pool, feedback, pending)
    }

    /// Directly insert a vote row with a chosen `created_at` (the store API
    /// always stamps `now`, which would pin every row to today's bucket).
    async fn seed_vote(
        pool: &sqlx::SqlitePool,
        turn_id: &str,
        agent_id: Option<&str>,
        vote: &str,
        created_at: i64,
    ) {
        sqlx::query(
            "INSERT INTO turn_feedback (turn_id, session_id, agent_id, vote, comment, created_at, updated_at)
             VALUES (?1, NULL, ?2, ?3, NULL, ?4, ?4)",
        )
        .bind(turn_id)
        .bind(agent_id)
        .bind(vote)
        .bind(created_at)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn build_ops_report_aggregates_votes_pending_and_risk_clusters() {
        let (pool, feedback, pending) = test_stores().await;
        let now_ms = chrono::Utc::now().timestamp_millis();

        // Votes spread across today, yesterday and (outside the daily window)
        // 20 days ago. since_ms = 0 keeps everything in the totals.
        seed_vote(&pool, "t1", Some("a1"), "up", now_ms).await;
        seed_vote(&pool, "t2", Some("a1"), "down", now_ms).await;
        seed_vote(&pool, "t3", Some("a2"), "up", now_ms - DAY_MS).await;
        seed_vote(&pool, "t4", Some("a1"), "down", now_ms - DAY_MS).await;
        seed_vote(&pool, "t5", Some("a1"), "up", now_ms - 20 * DAY_MS).await;

        // Pending badcases: a human:dislike row matching the 👎 turn t2 (empty
        // stored signals → rule-derived), plus an online:risk row with stored
        // signals and a second human:dislike row with no matching down vote.
        pending
            .insert_pending(&crate::eval::InsertPendingParams {
                source: crate::eval::PendingSource::HumanDislike,
                turn_id: Some("t2".into()),
                session_id: None,
                agent_id: None,
                input: "what is my balance".into(),
                response: "Your password is 12345".into(),
                risk_signals: vec![],
            })
            .await
            .unwrap();
        pending
            .insert_pending(&crate::eval::InsertPendingParams {
                source: crate::eval::PendingSource::OnlineRisk,
                turn_id: Some("x9".into()),
                session_id: None,
                agent_id: None,
                input: "u".into(),
                response: "r".into(),
                risk_signals: vec!["PII detected".into()],
            })
            .await
            .unwrap();
        pending
            .insert_pending(&crate::eval::InsertPendingParams {
                source: crate::eval::PendingSource::HumanDislike,
                turn_id: Some("x8".into()),
                session_id: None,
                agent_id: None,
                input: "another".into(),
                response: "reply".into(),
                risk_signals: vec![],
            })
            .await
            .unwrap();

        let report = build_ops_report(&feedback, &pending, 0).await.unwrap();

        // ── totals ──
        assert_eq!(report.total_votes, 5);
        assert_eq!(report.up, 3);
        assert_eq!(report.down, 2);

        // ── by_agent: sorted by total desc; a1 has 4, a2 has 1 ──
        assert_eq!(report.by_agent.len(), 2);
        assert_eq!(report.by_agent[0].agent_id, "a1");
        assert_eq!(report.by_agent[0].up, 2);
        assert_eq!(report.by_agent[0].down, 2);
        assert_eq!(report.by_agent[0].total, 4);
        assert_eq!(report.by_agent[1].agent_id, "a2");
        assert_eq!(report.by_agent[1].total, 1);

        // ── pending_by_source: human:dislike (2) before online:risk (1) ──
        assert_eq!(report.pending_by_source.len(), 2);
        assert_eq!(report.pending_by_source[0].source, "human:dislike");
        assert_eq!(report.pending_by_source[0].count, 2);
        assert_eq!(report.pending_by_source[1].source, "online:risk");
        assert_eq!(report.pending_by_source[1].count, 1);

        // ── by_day: 14 buckets; today up=1/down=1, yesterday up=1/down=1,
        //    the 20-day-old vote is excluded from the window. ──
        assert_eq!(report.by_day.len(), 14);
        assert_eq!(report.by_day[13].day, day_label(now_ms));
        assert_eq!(report.by_day[13].up, 1);
        assert_eq!(report.by_day[13].down, 1);
        assert_eq!(report.by_day[12].up, 1);
        assert_eq!(report.by_day[12].down, 1);
        assert_eq!(report.by_day[12].day, day_label(now_ms - DAY_MS));
        let window_up: u64 = report.by_day.iter().map(|d| d.up).sum();
        let window_down: u64 = report.by_day.iter().map(|d| d.down).sum();
        assert_eq!(window_up, 2, "20-day-old vote must not appear in by_day");
        assert_eq!(window_down, 2);

        // ── down_votes: newest first; t2 enriched, t4 unmatched (empty) ──
        assert_eq!(report.down_votes.len(), 2);
        assert_eq!(report.down_votes[0].turn_id, "t2");
        assert_eq!(report.down_votes[0].input, "what is my balance");
        assert!(
            report.down_votes[0]
                .risk_signals
                .iter()
                .any(|s| s.contains("password")),
            "risk signals should be rule-derived for a flagged response"
        );
        assert_eq!(report.down_votes[1].turn_id, "t4");
        assert!(report.down_votes[1].input.is_empty());
        assert!(report.down_votes[1].risk_signals.is_empty());

        // ── risk_clusters: the password signal fires once, sorted desc ──
        assert!(!report.risk_clusters.is_empty());
        assert_eq!(report.risk_clusters[0].count, 1);
        assert!(report.risk_clusters[0].label.contains("password"));
    }

    #[tokio::test]
    async fn build_ops_report_uses_stored_risk_signals_when_present() {
        let (pool, feedback, pending) = test_stores().await;
        let now_ms = chrono::Utc::now().timestamp_millis();
        seed_vote(&pool, "t1", Some("a1"), "down", now_ms).await;

        pending
            .insert_pending(&crate::eval::InsertPendingParams {
                source: crate::eval::PendingSource::OnlineRisk,
                turn_id: Some("t1".into()),
                session_id: None,
                agent_id: None,
                input: "hello".into(),
                response: "hi".into(),
                risk_signals: vec!["unhelpful".into(), "hallucination".into()],
            })
            .await
            .unwrap();

        let report = build_ops_report(&feedback, &pending, 0).await.unwrap();
        assert_eq!(report.down_votes.len(), 1);
        // Stored signals win over re-scanning.
        assert_eq!(
            report.down_votes[0].risk_signals,
            vec!["unhelpful".to_string(), "hallucination".to_string()]
        );
        assert_eq!(report.risk_clusters.len(), 2);
        assert_eq!(report.risk_clusters[0].label, "hallucination");
        assert_eq!(report.risk_clusters[0].count, 1);
    }

    #[tokio::test]
    async fn build_ops_report_empty_stores_returns_zeroed_report() {
        let (_, feedback, pending) = test_stores().await;
        let report = build_ops_report(&feedback, &pending, 0).await.unwrap();
        assert_eq!(report.total_votes, 0);
        assert!(report.by_agent.is_empty());
        assert!(report.pending_by_source.is_empty());
        assert!(report.down_votes.is_empty());
        assert!(report.risk_clusters.is_empty());
        assert_eq!(report.by_day.len(), 14);
    }

    #[tokio::test]
    async fn build_ops_report_respects_since_window() {
        let (pool, feedback, pending) = test_stores().await;
        let now_ms = chrono::Utc::now().timestamp_millis();
        seed_vote(&pool, "t1", None, "up", now_ms).await;
        seed_vote(&pool, "t2", None, "down", now_ms - 40 * DAY_MS).await;

        let report = build_ops_report(&feedback, &pending, now_ms - 10 * DAY_MS)
            .await
            .unwrap();
        assert_eq!(report.total_votes, 1);
        assert_eq!(report.up, 1);
        assert_eq!(report.down, 0);
        assert_eq!(report.down_votes.len(), 0);
    }

    #[test]
    fn truncate_cuts_long_inputs() {
        assert_eq!(truncate("short", 10), "short");
        let long = "x".repeat(300);
        let cut = truncate(&long, 200);
        assert_eq!(cut.chars().count(), 203);
        assert!(cut.ends_with("..."));
    }

    #[test]
    fn day_bucket_maps_within_window() {
        let now_ms = 1_800_000_000_000i64;
        assert_eq!(day_bucket(now_ms, now_ms), Some(13));
        assert_eq!(day_bucket(now_ms - DAY_MS, now_ms), Some(12));
        assert_eq!(day_bucket(now_ms - 13 * DAY_MS, now_ms), Some(0));
        assert_eq!(day_bucket(now_ms - 14 * DAY_MS, now_ms), None);
        assert_eq!(day_bucket(now_ms + DAY_MS, now_ms), None);
    }
}
