//! Per-provider usage tracking with time-window snapshots
//!
//! Tracks requests, tokens, and estimated cost per provider across
//! configurable time windows (today, this_hour, this_month).
//!
//! Usage:
//!   tracker.record("openai", usage, "gpt-4o").await;
//!   let snapshot = tracker.snapshot("openai").await;

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, Months, Timelike, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, trace};

use crate::providers::Usage;

/// Remote usage quota fetched from a provider's API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageQuota {
    /// Estimated remaining budget / quota (in USD or tokens,
    /// provider-dependent).
    pub remaining: f64,
    /// Total quota limit.
    pub limit: f64,
    /// When the quota window resets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<DateTime<Utc>>,
    /// Unit of the quota values: "usd", "tokens", "requests".
    pub unit: String,
    /// Where this quota came from.
    pub source: QuotaSource,
}

/// Source of a usage quota reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuotaSource {
    /// Quota was fetched from the provider's remote API.
    Remote,
    /// Quota was derived from local budget configuration.
    LocalBudget,
    /// No quota information available.
    Unknown,
}

/// A single usage window (e.g. "today", "this_hour").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageWindow {
    pub label: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub requests: u64,
    pub tokens: Usage,
    pub estimated_cost_usd: f64,
}

impl UsageWindow {
    fn new(label: impl Into<String>, start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self {
            label: label.into(),
            start,
            end,
            requests: 0,
            tokens: Usage::default(),
            estimated_cost_usd: 0.0,
        }
    }

    fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now < self.start || now >= self.end
    }

    fn record(&mut self, usage: Usage, cost: f64) {
        self.requests += 1;
        self.tokens.prompt_tokens += usage.prompt_tokens;
        self.tokens.completion_tokens += usage.completion_tokens;
        self.tokens.total_tokens += usage.total_tokens;
        self.estimated_cost_usd += cost;
    }
}

/// Snapshot of all usage data for a single provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUsageSnapshot {
    pub provider: String,
    pub windows: Vec<UsageWindow>,
    pub total_requests: u64,
    pub total_tokens: Usage,
    pub estimated_cost_usd: f64,
    /// Optional remote quota information (remaining budget / limit).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota: Option<UsageQuota>,
    pub last_updated: DateTime<Utc>,
}

/// Configured per-model price for usage estimation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPrice {
    /// Input token cost in USD per 1 million tokens.
    pub input_cpm: f64,
    /// Output token cost in USD per 1 million tokens.
    pub output_cpm: f64,
}

/// Configuration for the usage tracker.
#[derive(Debug, Clone)]
pub struct UsageTrackerConfig {
    /// Daily budget per provider in USD (0 = unlimited).
    pub daily_budget_usd: f64,
    /// Monthly budget per provider in USD (0 = unlimited).
    pub monthly_budget_usd: f64,
    /// Optional per-model price overrides for cost estimation.
    pub model_prices: Option<HashMap<String, ModelPrice>>,
}

impl Default for UsageTrackerConfig {
    fn default() -> Self {
        Self {
            daily_budget_usd: 0.0,
            monthly_budget_usd: 0.0,
            model_prices: None,
        }
    }
}

/// Per-provider usage tracker with time-window support.
pub struct ProviderUsageTracker {
    snapshots: RwLock<HashMap<String, ProviderUsageSnapshot>>,
    config: UsageTrackerConfig,
}

impl ProviderUsageTracker {
    pub fn new(config: UsageTrackerConfig) -> Self {
        Self {
            snapshots: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Access the tracker configuration.
    pub fn config(&self) -> &UsageTrackerConfig {
        &self.config
    }

    /// Record usage for a provider.
    pub async fn record(&self, provider: &str, usage: Usage, model: &str) {
        let cost = self.estimate_cost(usage, model);
        let now = Utc::now();

        let mut snapshots = self.snapshots.write().await;
        let snapshot =
            snapshots
                .entry(provider.to_string())
                .or_insert_with(|| ProviderUsageSnapshot {
                    provider: provider.to_string(),
                    windows: Self::build_windows(now),
                    total_requests: 0,
                    total_tokens: Usage::default(),
                    estimated_cost_usd: 0.0,
                    quota: None,
                    last_updated: now,
                });

        // Reset any expired windows
        for window in &mut snapshot.windows {
            if window.is_expired(now) {
                *window = Self::build_window(&window.label, now);
            }
            window.record(usage, cost);
        }

        snapshot.total_requests += 1;
        snapshot.total_tokens.prompt_tokens += usage.prompt_tokens;
        snapshot.total_tokens.completion_tokens += usage.completion_tokens;
        snapshot.total_tokens.total_tokens += usage.total_tokens;
        snapshot.estimated_cost_usd += cost;
        snapshot.last_updated = now;

        trace!("Recorded usage for {}: {} tokens, ${:.4}", provider, usage.total_tokens, cost);
    }

    /// Get the current snapshot for a provider.
    pub async fn snapshot(&self, provider: &str) -> Option<ProviderUsageSnapshot> {
        let snapshots = self.snapshots.read().await;
        snapshots.get(provider).cloned()
    }

    /// Get snapshots for all providers.
    pub async fn all_snapshots(&self) -> Vec<ProviderUsageSnapshot> {
        let snapshots = self.snapshots.read().await;
        snapshots.values().cloned().collect()
    }

    /// Check whether the provider is within its configured budget.
    pub async fn is_within_budget(&self, provider: &str) -> bool {
        let snapshots = self.snapshots.read().await;
        let Some(snapshot) = snapshots.get(provider) else {
            return true;
        };

        let today_cost: f64 = snapshot
            .windows
            .iter()
            .filter(|w| w.label == "today")
            .map(|w| w.estimated_cost_usd)
            .sum();

        let month_cost: f64 = snapshot
            .windows
            .iter()
            .filter(|w| w.label == "this_month")
            .map(|w| w.estimated_cost_usd)
            .sum();

        if self.config.daily_budget_usd > 0.0 && today_cost >= self.config.daily_budget_usd {
            debug!(
                "Provider {} daily budget exceeded: ${:.2} / ${:.2}",
                provider, today_cost, self.config.daily_budget_usd
            );
            return false;
        }

        if self.config.monthly_budget_usd > 0.0 && month_cost >= self.config.monthly_budget_usd {
            debug!(
                "Provider {} monthly budget exceeded: ${:.2} / ${:.2}",
                provider, month_cost, self.config.monthly_budget_usd
            );
            return false;
        }

        true
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn build_windows(now: DateTime<Utc>) -> Vec<UsageWindow> {
        vec![
            Self::build_window("today", now),
            Self::build_window("this_hour", now),
            Self::build_window("this_month", now),
        ]
    }

    fn build_window(label: &str, now: DateTime<Utc>) -> UsageWindow {
        let (start, end) = match label {
            "today" => {
                let start = now
                    .with_hour(0)
                    .and_then(|d| d.with_minute(0))
                    .and_then(|d| d.with_second(0))
                    .and_then(|d| d.with_nanosecond(0))
                    .unwrap_or(now);
                let end = start + Duration::days(1);
                (start, end)
            }
            "this_hour" => {
                let start = now
                    .with_minute(0)
                    .and_then(|d| d.with_second(0))
                    .and_then(|d| d.with_nanosecond(0))
                    .unwrap_or(now);
                let end = start + Duration::hours(1);
                (start, end)
            }
            "this_month" => {
                let start = now
                    .with_day(1)
                    .and_then(|d| d.with_hour(0))
                    .and_then(|d| d.with_minute(0))
                    .and_then(|d| d.with_second(0))
                    .and_then(|d| d.with_nanosecond(0))
                    .unwrap_or(now);
                // End on the first day of the next calendar month.
                let end = start
                    .date_naive()
                    .checked_add_months(Months::new(1))
                    .and_then(|d| {
                        d.and_hms_opt(0, 0, 0)
                            .map(|ndt| ndt.and_local_timezone(Utc).single())
                    })
                    .flatten()
                    .unwrap_or(start + Duration::days(31));
                (start, end)
            }
            _ => (now, now + Duration::days(1)),
        };
        UsageWindow::new(label, start, end)
    }

    /// Estimate cost in USD from token usage and model name.
    fn estimate_cost(&self, usage: Usage, model: &str) -> f64 {
        let model_lower = model.to_lowercase();
        let (input_cpm, output_cpm) = self
            .config
            .model_prices
            .as_ref()
            .and_then(|prices| prices.get(&model_lower))
            .map(|p| (p.input_cpm, p.output_cpm))
            .unwrap_or_else(|| Self::default_price_for_model(&model_lower));

        let input_cost = usage.prompt_tokens as f64 * input_cpm / 1_000_000.0;
        let output_cost = usage.completion_tokens as f64 * output_cpm / 1_000_000.0;
        input_cost + output_cost
    }

    /// Fallback hard-coded pricing when no configured price exists.
    fn default_price_for_model(model_lower: &str) -> (f64, f64) {
        if model_lower.contains("opus") {
            (15.0, 75.0)
        } else if model_lower.contains("sonnet") && model_lower.contains("4") {
            // claude-4-sonnet
            (3.0, 15.0)
        } else if model_lower.contains("sonnet") {
            (3.0, 15.0)
        } else if model_lower.contains("haiku") {
            (0.25, 1.25)
        } else if model_lower.contains("gpt-4o") || model_lower.contains("gpt-4-turbo") {
            (2.50, 10.0)
        } else if model_lower.contains("gpt-4") {
            (30.0, 60.0)
        } else if model_lower.contains("gpt-3.5") || model_lower.contains("gpt-3") {
            (0.50, 1.50)
        } else if model_lower.contains("o1") || model_lower.contains("o3") {
            (15.0, 60.0)
        } else {
            (3.0, 15.0) // default
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn test_usage_window_record() {
        let mut window = UsageWindow::new("today", Utc::now(), Utc::now() + Duration::days(1));
        let usage = Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            ..Default::default()
        };
        window.record(usage, 0.001);
        assert_eq!(window.requests, 1);
        assert_eq!(window.tokens.total_tokens, 150);
        assert!((window.estimated_cost_usd - 0.001).abs() < f64::EPSILON);
    }

    #[test]
    fn test_estimate_cost_gpt4o() {
        let tracker = ProviderUsageTracker::new(UsageTrackerConfig::default());
        let usage = Usage {
            prompt_tokens: 1_000_000,
            completion_tokens: 500_000,
            total_tokens: 1_500_000,
            ..Default::default()
        };
        let cost = tracker.estimate_cost(usage, "gpt-4o");
        // (1M * $2.50 + 0.5M * $10.00) / 1M = $2.50 + $5.00 = $7.50
        assert!((cost - 7.5).abs() < 0.01);
    }

    #[test]
    fn test_estimate_cost_claude_opus() {
        let tracker = ProviderUsageTracker::new(UsageTrackerConfig::default());
        let usage = Usage {
            prompt_tokens: 1_000_000,
            completion_tokens: 500_000,
            total_tokens: 1_500_000,
            ..Default::default()
        };
        let cost = tracker.estimate_cost(usage, "claude-3-opus");
        // (1M * $15 + 0.5M * $75) / 1M = $15 + $37.5 = $52.5
        assert!((cost - 52.5).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_tracker_record_and_snapshot() {
        let tracker = ProviderUsageTracker::new(UsageTrackerConfig::default());
        let usage = Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            ..Default::default()
        };
        tracker.record("openai", usage, "gpt-4o").await;

        let snapshot = tracker.snapshot("openai").await.unwrap();
        assert_eq!(snapshot.provider, "openai");
        assert_eq!(snapshot.total_requests, 1);
        assert_eq!(snapshot.total_tokens.total_tokens, 150);
        assert!(snapshot.estimated_cost_usd > 0.0);

        assert_eq!(snapshot.windows.len(), 3);
        let today = snapshot
            .windows
            .iter()
            .find(|w| w.label == "today")
            .unwrap();
        assert_eq!(today.requests, 1);
    }

    #[tokio::test]
    async fn test_tracker_all_snapshots() {
        let tracker = ProviderUsageTracker::new(UsageTrackerConfig::default());
        let usage = Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            ..Default::default()
        };
        tracker.record("openai", usage, "gpt-4o").await;
        tracker.record("anthropic", usage, "claude-sonnet").await;

        let all = tracker.all_snapshots().await;
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_tracker_budget_enforcement() {
        let config = UsageTrackerConfig {
            daily_budget_usd: 0.001, // very low budget
            monthly_budget_usd: 0.0,
            model_prices: None,
        };
        let tracker = ProviderUsageTracker::new(config);
        let usage = Usage {
            prompt_tokens: 1_000_000,
            completion_tokens: 0,
            total_tokens: 1_000_000,
            ..Default::default()
        };
        tracker.record("openai", usage, "gpt-4o").await;

        assert!(!tracker.is_within_budget("openai").await);
        assert!(tracker.is_within_budget("unknown").await);
    }

    #[test]
    fn test_this_month_window_end_boundary() {
        let now = Utc.with_ymd_and_hms(2024, 2, 15, 12, 0, 0).unwrap();
        let window = ProviderUsageTracker::build_window("this_month", now);
        assert_eq!(window.start, Utc.with_ymd_and_hms(2024, 2, 1, 0, 0, 0).unwrap());
        assert_eq!(window.end, Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap());

        // 30-day month.
        let now = Utc.with_ymd_and_hms(2024, 4, 10, 0, 0, 0).unwrap();
        let window = ProviderUsageTracker::build_window("this_month", now);
        assert_eq!(window.end, Utc.with_ymd_and_hms(2024, 5, 1, 0, 0, 0).unwrap());

        // December -> January rollover.
        let now = Utc.with_ymd_and_hms(2024, 12, 31, 23, 59, 59).unwrap();
        let window = ProviderUsageTracker::build_window("this_month", now);
        assert_eq!(window.end, Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap());
    }

    #[test]
    fn test_configured_price_overrides_default() {
        let mut prices = HashMap::new();
        prices.insert(
            "custom-model".to_string(),
            ModelPrice {
                input_cpm: 1.0,
                output_cpm: 2.0,
            },
        );
        let config = UsageTrackerConfig {
            daily_budget_usd: 0.0,
            monthly_budget_usd: 0.0,
            model_prices: Some(prices),
        };
        let tracker = ProviderUsageTracker::new(config);
        let usage = Usage {
            prompt_tokens: 1_000_000,
            completion_tokens: 500_000,
            total_tokens: 1_500_000,
            ..Default::default()
        };
        let cost = tracker.estimate_cost(usage, "custom-model");
        // $1.00 input + $1.00 output = $2.00
        assert!((cost - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_unconfigured_model_uses_default_price() {
        let config = UsageTrackerConfig {
            daily_budget_usd: 0.0,
            monthly_budget_usd: 0.0,
            model_prices: Some(HashMap::new()),
        };
        let tracker = ProviderUsageTracker::new(config);
        let usage = Usage {
            prompt_tokens: 1_000_000,
            completion_tokens: 500_000,
            total_tokens: 1_500_000,
            ..Default::default()
        };
        let cost = tracker.estimate_cost(usage, "gpt-4o");
        assert!((cost - 7.5).abs() < 0.01);
    }
}
