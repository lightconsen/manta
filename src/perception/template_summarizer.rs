//! [`TemplateSummarizer`] — zero-LLM, rule-based [`PerceptionSummarizer`].
//!
//! Parses the structured `user` JSON that
//! [`super::MinimalAdapter::summarize`] builds (`{ duration_ms,
//! recent_events, aggregates }`) and emits a deterministic one-line
//! narrative. Used as the default summarizer when
//! [`super::AdapterConfig::enable_summary`] is on but no LLM backend is
//! configured.
//!
//! Rationale: the JSON input is already curated (max 64 events,
//! sliding-window aggregates) and the surrounding `format_for_prompt`
//! already renders `### Recent events` and `### Sensors` blocks. The
//! summary section's incremental value is small enough that a rule-based
//! one-liner is sufficient for most agents — and free.

use async_trait::async_trait;
use serde_json::Value;

use crate::perception::{AdapterError, PerceptionSummarizer};

/// Maximum chars in the emitted summary string. Keeps prompt cost
/// bounded and prevents pathological inputs from blowing up the
/// `### Summary` block.
const MAX_LEN: usize = 200;

/// Rule-based [`PerceptionSummarizer`]. Constructs trivially via
/// [`Self::new`]; no configuration.
#[derive(Debug, Default, Clone, Copy)]
pub struct TemplateSummarizer;

impl TemplateSummarizer {
    /// Construct a new template summarizer.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PerceptionSummarizer for TemplateSummarizer {
    async fn summarize(&self, _system: &str, user: &str) -> Result<String, AdapterError> {
        let v: Value = serde_json::from_str(user)
            .map_err(|e| AdapterError::Summarizer(format!("invalid summarizer payload: {e}")))?;

        let mut parts: Vec<String> = Vec::new();

        // 1. Anomalies (severity >= 128 are surfaced first).
        if let Some(events) = v.get("recent_events").and_then(|x| x.as_array()) {
            let mut high_sev: Vec<(&str, u64)> = events
                .iter()
                .filter(|e| e.get("type").and_then(|t| t.as_str()) == Some("anomaly"))
                .filter_map(|e| {
                    let sev = e.get("severity").and_then(|s| s.as_u64()).unwrap_or(0);
                    let src = e.get("source").and_then(|s| s.as_str()).unwrap_or("?");
                    if sev >= 128 {
                        Some((src, sev))
                    } else {
                        None
                    }
                })
                .collect();
            high_sev.sort_by_key(|a| std::cmp::Reverse(a.1));
            high_sev.dedup_by(|a, b| a.0 == b.0);
            for (src, sev) in high_sev.into_iter().take(3) {
                let label = if sev >= 220 {
                    "quarantined"
                } else {
                    "degraded"
                };
                parts.push(format!("{src} {label} (sev={sev})"));
            }
        }

        // 2. Top-3 changes by absolute numeric delta.
        if let Some(events) = v.get("recent_events").and_then(|x| x.as_array()) {
            let mut changes: Vec<(&str, f64, f64)> = events
                .iter()
                .filter(|e| e.get("type").and_then(|t| t.as_str()) == Some("change"))
                .filter_map(|e| {
                    let src = e.get("source").and_then(|s| s.as_str())?;
                    let from = numeric(e.get("from")?)?;
                    let to = numeric(e.get("to")?)?;
                    Some((src, from, to))
                })
                .collect();
            changes.sort_by(|a, b| {
                (b.2 - b.1)
                    .abs()
                    .partial_cmp(&(a.2 - a.1).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for (src, from, to) in changes.into_iter().take(3) {
                parts.push(format!("{src} {}\u{2192}{}", fmt_num(from), fmt_num(to)));
            }
        }

        // 3. Top aggregates by source (mean/value if available).
        if let Some(aggs) = v.get("aggregates").and_then(|x| x.as_array()) {
            for agg in aggs.iter().take(3) {
                let src = agg.get("source").and_then(|s| s.as_str()).unwrap_or("?");
                let stats = agg.get("stats");
                let descr = stats
                    .and_then(|s| s.get("mean"))
                    .and_then(|m| m.as_f64())
                    .map(|m| format!("avg {}", fmt_num(m)))
                    .or_else(|| {
                        stats
                            .and_then(|s| s.get("count"))
                            .and_then(|c| c.as_u64())
                            .map(|c| format!("count {c}"))
                    });
                if let Some(d) = descr {
                    parts.push(format!("{src} {d}"));
                }
            }
        }

        let mut out = if parts.is_empty() {
            "Environment nominal".to_string()
        } else {
            parts.join("; ")
        };
        if out.len() > MAX_LEN {
            // Truncate on a char boundary, append ellipsis.
            let mut cut = MAX_LEN.saturating_sub(1);
            while !out.is_char_boundary(cut) && cut > 0 {
                cut -= 1;
            }
            out.truncate(cut);
            out.push('\u{2026}');
        }
        Ok(out)
    }
}

/// Best-effort numeric extraction. Accepts JSON numbers directly and
/// the common `{ "value": X }` and `{ "cpu_pct": X }` shapes that
/// adapters emit.
fn numeric(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    if let Some(obj) = v.as_object() {
        for k in ["value", "rms", "cpu_pct", "mem_pct", "level"] {
            if let Some(n) = obj.get(k).and_then(|x| x.as_f64()) {
                return Some(n);
            }
        }
        // Single-numeric-field object → use that.
        if obj.len() == 1 {
            if let Some(n) = obj.values().next().and_then(|x| x.as_f64()) {
                return Some(n);
            }
        }
    }
    None
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e9 {
        format!("{}", n as i64)
    } else {
        format!("{n:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(payload: serde_json::Value) -> String {
        let s = TemplateSummarizer::new();
        let user = payload.to_string();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(s.summarize("", &user)).unwrap()
    }

    #[test]
    fn empty_input_says_nominal() {
        let out = run(serde_json::json!({
            "duration_ms": 60000,
            "recent_events": [],
            "aggregates": [],
        }));
        assert_eq!(out, "Environment nominal");
    }

    #[test]
    fn single_anomaly_and_aggregate_both_surface() {
        let out = run(serde_json::json!({
            "duration_ms": 60000,
            "recent_events": [{
                "type": "anomaly",
                "source": "camera0",
                "reason": "source_fault",
                "severity": 220,
                "at": "2026-01-01T00:00:00Z",
            }],
            "aggregates": [{
                "source": "cpu",
                "modality": "System",
                "window_ms": 60000,
                "stats": {"mean": 45.3},
                "at": "2026-01-01T00:00:00Z",
            }],
        }));
        assert!(out.contains("camera0 quarantined"), "got {out:?}");
        assert!(out.contains("cpu avg 45.3"), "got {out:?}");
    }

    #[test]
    fn many_changes_truncated_to_top_three() {
        let mut events = vec![];
        for i in 0..10 {
            events.push(serde_json::json!({
                "type": "change",
                "source": format!("src{i}"),
                "modality": "System",
                "from": i as f64,
                "to": (i * 100) as f64,
                "at": "2026-01-01T00:00:00Z",
            }));
        }
        let out = run(serde_json::json!({
            "duration_ms": 60000,
            "recent_events": events,
            "aggregates": [],
        }));
        // The largest absolute deltas come from i=9..=7 (delta = i*99).
        assert!(out.contains("src9 9\u{2192}900"), "got {out:?}");
        assert!(out.contains("src8 8\u{2192}800"), "got {out:?}");
        assert!(out.contains("src7 7\u{2192}700"), "got {out:?}");
        assert!(!out.contains("src6"), "should drop tail: {out:?}");
    }

    #[test]
    fn malformed_json_returns_summarizer_error() {
        let s = TemplateSummarizer::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let res = rt.block_on(s.summarize("", "{not json"));
        assert!(matches!(res, Err(AdapterError::Summarizer(_))));
    }

    #[test]
    fn output_is_truncated_at_max_len() {
        let mut events = vec![];
        for i in 0..20 {
            events.push(serde_json::json!({
                "type": "anomaly",
                "source": format!("very-long-source-name-{i}"),
                "reason": "source_fault",
                "severity": 200,
                "at": "2026-01-01T00:00:00Z",
            }));
        }
        let out = run(serde_json::json!({
            "duration_ms": 60000,
            "recent_events": events,
            "aggregates": [],
        }));
        assert!(out.chars().count() <= MAX_LEN, "got {} chars", out.chars().count());
    }
}
