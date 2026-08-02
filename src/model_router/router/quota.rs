//! `ModelRouter` usage snapshots enriched with remote/local quota.

use super::*;

impl ModelRouter {
    // ==================== USAGE SNAPSHOTS WITH QUOTA ====================

    /// Get a usage snapshot enriched with remote quota.
    pub async fn snapshot_with_quota(&self, provider: &str) -> Option<ProviderUsageSnapshot> {
        let mut snapshot = self.usage_tracker.snapshot(provider).await?;

        let fetchers = self.usage_fetchers.read().await;
        if let Some(fetcher) = fetchers.get(provider) {
            match fetcher.fetch().await {
                Ok(Some(quota)) => {
                    snapshot.quota = Some(quota);
                }
                Ok(None) => {
                    drop(fetchers);
                    snapshot.quota = self.local_budget_quota(provider).await;
                }
                Err(e) => {
                    warn!(
                        "Failed to fetch usage quota for {}: {}; falling back to local budget",
                        provider, e
                    );
                    drop(fetchers);
                    snapshot.quota = self.local_budget_quota(provider).await;
                }
            }
        } else {
            drop(fetchers);
            snapshot.quota = self.local_budget_quota(provider).await;
        }

        snapshot.last_updated = Utc::now();
        Some(snapshot)
    }

    /// Get all usage snapshots enriched with remote quota.
    pub async fn all_snapshots_with_quota(&self) -> Vec<ProviderUsageSnapshot> {
        let base_snapshots = self.usage_tracker.all_snapshots().await;

        let fetchers = self.usage_fetchers.read().await;
        let futures = base_snapshots
            .into_iter()
            .map(|snapshot| {
                let provider = snapshot.provider.clone();
                let fetcher = fetchers.get(&provider).clone();
                async move {
                    let provider = snapshot.provider.clone();
                    let quota = if let Some(fetcher) = fetcher {
                        match fetcher.fetch().await {
                            Ok(Some(q)) => Some(q),
                            Ok(None) => None,
                            Err(e) => {
                                warn!("Failed to fetch usage quota for {}: {}", provider, e);
                                None
                            }
                        }
                    } else {
                        None
                    };
                    (snapshot, quota)
                }
            })
            .collect::<Vec<_>>();
        drop(fetchers);

        let results = join_all(futures).await;
        let mut enriched = Vec::with_capacity(results.len());
        for (mut snapshot, remote_quota) in results {
            let provider = snapshot.provider.clone();
            snapshot.quota = if let Some(q) = remote_quota {
                Some(q)
            } else {
                self.local_budget_quota(&provider).await
            };
            snapshot.last_updated = Utc::now();
            enriched.push(snapshot);
        }

        enriched
    }

    async fn local_budget_quota(&self, provider: &str) -> Option<UsageQuota> {
        let snapshot = self.usage_tracker.snapshot(provider).await?;
        let config = self.usage_tracker.config();

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

        let fetcher = LocalBudgetFetcher::new(
            provider,
            config.daily_budget_usd,
            config.monthly_budget_usd,
            today_cost,
            month_cost,
        );
        fetcher.fetch().await.ok().flatten()
    }
}
