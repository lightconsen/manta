//! Remote usage quota fetching from provider APIs
//!
//! Defines the [`UsageFetcher`] trait for querying provider-specific usage
//! endpoints, plus concrete implementations for providers that expose them.
//!
//! ```rust,ignore
//! let fetcher = OpenAiUsageFetcher::new("sk-xxx".to_string());
//! let quota = fetcher.fetch().await?;
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Datelike, Utc};

use crate::model_router::usage_tracker::{QuotaSource, UsageQuota};

/// Fetch remote usage quota from a provider's API.
#[async_trait]
pub trait UsageFetcher: Send + Sync {
    /// Provider name this fetcher targets.
    fn provider(&self) -> &str;

    /// Query the provider's API for current usage quota.
    ///
    /// Returns `None` when the provider does not expose quota information
    /// or the request fails.
    async fn fetch(&self) -> crate::Result<Option<UsageQuota>>;
}

// ------------------------------------------------------------------
// OpenAI Usage Fetcher
// ------------------------------------------------------------------

/// Fetches usage quota from OpenAI's dashboard API.
///
/// Uses the non-official `https://api.openai.com/dashboard/billing/usage`
/// endpoint when an organization-level key is available.
#[derive(Debug, Clone)]
pub struct OpenAiUsageFetcher {
    api_key: String,
    org_id: Option<String>,
    client: reqwest::Client,
}

impl OpenAiUsageFetcher {
    /// Create a new fetcher.
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            org_id: None,
            client: reqwest::Client::new(),
        }
    }

    /// Attach an OpenAI organization ID (required for some usage endpoints).
    pub fn with_org_id(mut self, org_id: impl Into<String>) -> Self {
        self.org_id = Some(org_id.into());
        self
    }
}

#[async_trait]
impl UsageFetcher for OpenAiUsageFetcher {
    fn provider(&self) -> &str {
        "openai"
    }

    async fn fetch(&self) -> crate::Result<Option<UsageQuota>> {
        // Build the billing/subscription endpoint to get hard limit
        let mut req = self
            .client
            .get("https://api.openai.com/dashboard/billing/subscription")
            .header("Authorization", format!("Bearer {}", self.api_key));

        if let Some(ref org) = self.org_id {
            req = req.header("OpenAI-Organization", org.clone());
        }

        // Fetch current usage for the month
        let now = Utc::now();
        let start_date = format!("{}-{:02}-{:02}", now.year(), now.month(), 1);
        let end_date = format!("{}-{:02}-{:02}", now.year(), now.month(), now.day());

        let mut usage_req = self
            .client
            .get(format!(
                "https://api.openai.com/dashboard/billing/usage?start_date={}&end_date={}",
                start_date, end_date
            ))
            .header("Authorization", format!("Bearer {}", self.api_key));

        if let Some(ref org) = self.org_id {
            usage_req = usage_req.header("OpenAI-Organization", org.clone());
        }

        // Query subscription and usage endpoints concurrently.
        let (sub_result, usage_result) = tokio::join!(req.send(), usage_req.send());

        let sub_resp = sub_result.map_err(|e| crate::error::SyscityError::ExternalService {
            source: format!("OpenAI usage fetch request failed: {}", e),
            cause: Some(Box::new(e)),
        })?;

        if !sub_resp.status().is_success() {
            // Most API keys don't have dashboard access — silently return None
            return Ok(None);
        }

        let body: serde_json::Value =
            sub_resp
                .json()
                .await
                .map_err(|e| crate::error::SyscityError::ExternalService {
                    source: format!("OpenAI usage response invalid: {}", e),
                    cause: Some(Box::new(e)),
                })?;

        let limit = body
            .get("hard_limit_usd")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let used_this_month = match usage_result {
            Ok(r) if r.status().is_success() => {
                r.json::<serde_json::Value>()
                    .await
                    .ok()
                    .and_then(|v| v.get("total_usage").and_then(|u| u.as_f64()))
                    .unwrap_or(0.0)
                    / 100.0
            } // convert cents to USD
            _ => 0.0,
        };

        let remaining = (limit - used_this_month).max(0.0);

        Ok(Some(UsageQuota {
            remaining,
            limit,
            reset_at: None, // OpenAI billing is calendar month
            unit: "usd".to_string(),
            source: QuotaSource::Remote,
        }))
    }
}

// ------------------------------------------------------------------
// Local-budget fallback fetcher
// ------------------------------------------------------------------

/// Derives a quota from the local [`UsageTrackerConfig`] budget settings.
#[derive(Debug, Clone)]
pub struct LocalBudgetFetcher {
    provider: String,
    daily_budget: f64,
    monthly_budget: f64,
    today_cost: f64,
    month_cost: f64,
}

impl LocalBudgetFetcher {
    pub fn new(
        provider: impl Into<String>,
        daily_budget: f64,
        monthly_budget: f64,
        today_cost: f64,
        month_cost: f64,
    ) -> Self {
        Self {
            provider: provider.into(),
            daily_budget,
            monthly_budget,
            today_cost,
            month_cost,
        }
    }
}

#[async_trait]
impl UsageFetcher for LocalBudgetFetcher {
    fn provider(&self) -> &str {
        &self.provider
    }

    async fn fetch(&self) -> crate::Result<Option<UsageQuota>> {
        let (limit, used) = if self.monthly_budget > 0.0 {
            (self.monthly_budget, self.month_cost)
        } else if self.daily_budget > 0.0 {
            (self.daily_budget, self.today_cost)
        } else {
            return Ok(None);
        };

        Ok(Some(UsageQuota {
            remaining: (limit - used).max(0.0),
            limit,
            reset_at: None,
            unit: "usd".to_string(),
            source: QuotaSource::LocalBudget,
        }))
    }
}

// ------------------------------------------------------------------
// Registry
// ------------------------------------------------------------------

/// A registry of [`UsageFetcher`] instances keyed by provider name.
#[derive(Default)]
pub struct UsageFetcherRegistry {
    fetchers: std::collections::HashMap<String, Arc<dyn UsageFetcher>>,
}

impl UsageFetcherRegistry {
    /// Register a fetcher for a provider.
    pub fn register(&mut self, provider: impl Into<String>, fetcher: Arc<dyn UsageFetcher>) {
        self.fetchers.insert(provider.into(), fetcher);
    }

    /// Get a fetcher by provider name.
    pub fn get(&self, provider: &str) -> Option<Arc<dyn UsageFetcher>> {
        self.fetchers.get(provider).cloned()
    }

    /// Remove a fetcher.
    pub fn remove(&mut self, provider: &str) -> Option<Arc<dyn UsageFetcher>> {
        self.fetchers.remove(provider)
    }

    /// List all registered provider names.
    pub fn list(&self) -> Vec<&str> {
        self.fetchers.keys().map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockFetcher;

    #[async_trait]
    impl UsageFetcher for MockFetcher {
        fn provider(&self) -> &str {
            "mock"
        }

        async fn fetch(&self) -> crate::Result<Option<UsageQuota>> {
            Ok(Some(UsageQuota {
                remaining: 42.0,
                limit: 100.0,
                reset_at: None,
                unit: "usd".to_string(),
                source: QuotaSource::Remote,
            }))
        }
    }

    #[test]
    fn test_registry_register_and_get() {
        let mut registry = UsageFetcherRegistry::default();
        registry.register("mock", Arc::new(MockFetcher));
        assert!(registry.get("mock").is_some());
        assert!(registry.get("other").is_none());
    }

    #[tokio::test]
    async fn test_local_budget_fetcher_with_budget() {
        let fetcher = LocalBudgetFetcher::new("test", 10.0, 0.0, 3.5, 0.0);
        let quota = fetcher.fetch().await.unwrap().unwrap();
        assert_eq!(quota.remaining, 6.5);
        assert_eq!(quota.limit, 10.0);
        assert_eq!(quota.source, QuotaSource::LocalBudget);
    }

    #[tokio::test]
    async fn test_local_budget_fetcher_no_budget() {
        let fetcher = LocalBudgetFetcher::new("test", 0.0, 0.0, 0.0, 0.0);
        assert!(fetcher.fetch().await.unwrap().is_none());
    }
}
