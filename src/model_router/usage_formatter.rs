//! Human-readable usage formatting
//!
//! Converts `ProviderUsageSnapshot` data into formatted strings for CLI
//! display.
//!
//! ```rust,ignore
//! let snapshot = tracker.snapshot("openai").await.unwrap();
//! println!("{}", format_usage_summary(&snapshot));
//! ```

use chrono::{DateTime, Utc};

use crate::model_router::usage_tracker::{ProviderUsageSnapshot, UsageWindow};
use crate::providers::Usage;

/// Format a single usage window into a human-readable summary.
///
/// Example: `today: 1,234 tokens · 3 requests · $0.0042 · resets in 2h 15m`
pub fn format_window(window: &UsageWindow) -> String {
    let reset_in = format_time_until(window.end);
    let cost = if window.estimated_cost_usd > 0.0 {
        format!(" · ${:.4}", window.estimated_cost_usd)
    } else {
        String::new()
    };

    format!(
        "{:>12}: {:>8} tokens · {:>3} requests{} · resets {}",
        window.label, window.tokens.total_tokens, window.requests, cost, reset_in
    )
}

/// Format a usage window as a compact progress-style line.
///
/// Shows percentage of an optional budget limit.
/// Example: `today  87% left  ⏱ 2h 15m`
pub fn format_window_compact(window: &UsageWindow, budget_usd: Option<f64>) -> String {
    let budget_str = if let Some(budget) = budget_usd {
        if budget > 0.0 {
            let pct = ((1.0 - window.estimated_cost_usd / budget) * 100.0).clamp(0.0, 100.0) as u32;
            format!("{:>3}% left", pct)
        } else {
            "no limit".to_string()
        }
    } else {
        format!("${:.4}", window.estimated_cost_usd)
    };

    let reset_in = format_time_until(window.end);
    format!("{:>12}  {}  ⏱ {}", window.label, budget_str, reset_in)
}

/// Format a complete provider snapshot into a multi-line summary.
pub fn format_provider_snapshot(snapshot: &ProviderUsageSnapshot) -> String {
    let mut lines = vec![
        format!("📊  {}  —  {} requests", snapshot.provider, snapshot.total_requests),
        format!(
            "    Tokens: {} prompt / {} completion / {} total",
            snapshot.total_tokens.prompt_tokens,
            snapshot.total_tokens.completion_tokens,
            snapshot.total_tokens.total_tokens,
        ),
    ];

    if snapshot.estimated_cost_usd > 0.0 {
        lines.push(format!("    Estimated cost: ${:.4}", snapshot.estimated_cost_usd));
    }

    if let Some(ref quota) = snapshot.quota {
        lines.push(format!(
            "    Quota:  ${:.2} remaining / ${:.2} limit  ({})",
            quota.remaining,
            quota.limit,
            match quota.source {
                crate::model_router::usage_tracker::QuotaSource::Remote => "remote",
                crate::model_router::usage_tracker::QuotaSource::LocalBudget => "local budget",
                crate::model_router::usage_tracker::QuotaSource::Unknown => "unknown",
            }
        ));
    }

    if !snapshot.windows.is_empty() {
        lines.push(String::new());
        lines.push("    Windows:".to_string());
        for window in &snapshot.windows {
            lines.push(format!("        {}", format_window(window)));
        }
    }

    lines.join("\n")
}

/// Format a compact single-line summary for all providers.
///
/// Example: `📊 Usage: openai $0.42 · anthropic $1.23`
pub fn format_usage_summary_line(snapshots: &[ProviderUsageSnapshot]) -> String {
    if snapshots.is_empty() {
        return "📊  No usage recorded yet".to_string();
    }

    let parts: Vec<String> = snapshots
        .iter()
        .map(|s| {
            if let Some(ref quota) = s.quota {
                if quota.remaining > 0.0 {
                    return format!("{} ${:.2} left", s.provider, quota.remaining);
                }
            }
            let today_cost = s
                .windows
                .iter()
                .find(|w| w.label == "today")
                .map(|w| w.estimated_cost_usd)
                .unwrap_or(0.0);
            if today_cost > 0.0 {
                format!("{} ${:.2}", s.provider, today_cost)
            } else {
                format!("{} {} tokens", s.provider, s.total_tokens.total_tokens)
            }
        })
        .collect();

    format!("📊  Usage: {}", parts.join(" · "))
}

/// Format all snapshots into a full report.
pub fn format_usage_report(snapshots: &[ProviderUsageSnapshot]) -> String {
    if snapshots.is_empty() {
        return "No usage data available.\n".to_string();
    }

    let mut lines = vec![
        "Usage Report".to_string(),
        "============".to_string(),
        String::new(),
    ];

    for snapshot in snapshots {
        lines.push(format_provider_snapshot(snapshot));
        lines.push(String::new());
    }

    lines.join("\n")
}

/// Format a `Usage` struct compactly.
pub fn format_tokens(usage: &Usage) -> String {
    format!(
        "{} prompt / {} completion / {} total",
        usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
    )
}

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

fn format_time_until(target: DateTime<Utc>) -> String {
    let now = Utc::now();
    if target <= now {
        return "soon".to_string();
    }

    let diff = target - now;
    let hours = diff.num_hours();
    let minutes = diff.num_minutes() % 60;

    if hours > 24 {
        let days = hours / 24;
        format!("in {}d", days)
    } else if hours > 0 {
        format!("in {}h {:02}m", hours, minutes)
    } else {
        format!("in {}m", diff.num_minutes().max(1))
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::*;

    fn test_window(label: &str, tokens: u32, requests: u64, cost: f64) -> UsageWindow {
        let now = Utc::now();
        UsageWindow {
            label: label.to_string(),
            start: now,
            end: now + Duration::hours(1),
            requests,
            tokens: Usage {
                prompt_tokens: tokens / 2,
                completion_tokens: tokens / 2,
                total_tokens: tokens,
            },
            estimated_cost_usd: cost,
        }
    }

    fn test_snapshot(provider: &str) -> ProviderUsageSnapshot {
        ProviderUsageSnapshot {
            provider: provider.to_string(),
            windows: vec![
                test_window("today", 1000, 5, 0.0042),
                test_window("this_hour", 200, 2, 0.0008),
            ],
            total_requests: 5,
            total_tokens: Usage {
                prompt_tokens: 500,
                completion_tokens: 500,
                total_tokens: 1000,
            },
            estimated_cost_usd: 0.0042,
            quota: None,
            last_updated: Utc::now(),
        }
    }

    #[test]
    fn test_format_window() {
        let window = test_window("today", 1234, 3, 0.0042);
        let formatted = format_window(&window);
        assert!(formatted.contains("today"));
        assert!(formatted.contains("1234"));
        assert!(formatted.contains("3 requests"));
        assert!(formatted.contains("$0.0042"));
    }

    #[test]
    fn test_format_provider_snapshot() {
        let snapshot = test_snapshot("openai");
        let formatted = format_provider_snapshot(&snapshot);
        assert!(formatted.contains("openai"));
        assert!(formatted.contains("5 requests"));
        assert!(formatted.contains("1000 total"));
    }

    #[test]
    fn test_format_usage_summary_line() {
        let snapshots = vec![test_snapshot("openai"), test_snapshot("anthropic")];
        let line = format_usage_summary_line(&snapshots);
        assert!(line.starts_with("📊  Usage:"));
        assert!(line.contains("openai"));
        assert!(line.contains("anthropic"));
    }

    #[test]
    fn test_format_usage_summary_line_empty() {
        let line = format_usage_summary_line(&[]);
        assert_eq!(line, "📊  No usage recorded yet");
    }

    #[test]
    fn test_format_window_compact_with_budget() {
        let window = test_window("today", 1000, 1, 0.50);
        let formatted = format_window_compact(&window, Some(1.0));
        assert!(formatted.contains("50% left"));
    }

    #[test]
    fn test_format_tokens() {
        let usage = Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        };
        assert_eq!(format_tokens(&usage), "100 prompt / 50 completion / 150 total");
    }
}
