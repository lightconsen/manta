//! `ModelRouter` cost-aware automatic model selection.

use super::*;

impl ModelRouter {
    // ==================== COST-AWARE ROUTING ====================

    /// Complete a request with cost-aware automatic model selection
    pub async fn complete_auto(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> crate::Result<CompletionResponse> {
        let config = self.config.read().await;

        let model_id = if let Some(ref cost_aware) = config.cost_aware {
            if cost_aware.enabled {
                drop(config);
                return self.complete_with_cost_routing(messages, tools).await;
            }
            cost_aware.default_model.clone()
        } else {
            config.default_model.clone()
        };
        drop(config);

        self.complete(&model_id, messages, tools).await
    }

    /// Internal: route based on task classification and cost
    async fn complete_with_cost_routing(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> crate::Result<CompletionResponse> {
        let task_type = self.classifier.classify(&messages);
        info!("Task classified as: {:?}", task_type);

        let config = self.config.read().await;
        let Some(cost_aware) = config.cost_aware.as_ref() else {
            let default = config.default_model.clone();
            drop(config);
            return self.complete(&default, messages, tools).await;
        };

        // Check budget limit
        if let Some(cheapest) = Self::cheapest_model_on_budget_exceeded(cost_aware) {
            drop(config);
            return self.complete(&cheapest, messages, tools).await;
        }

        // Resolve model from routing rules
        let model_id = Self::resolve_model_for_task(cost_aware, &task_type, &messages);
        drop(config);

        // Complete and track cost
        let model_id_for_cost = model_id.clone();
        let response = self.complete(&model_id, messages, tools).await?;

        // Track cost: config is lock #1 (per doc at line 48-58) and we hold
        // no other locks at this point, so acquiring config.write() is safe.
        if let Some(ref usage) = response.usage {
            let mut config = self.config.write().await;
            if let Some(ref mut cost_aware) = config.cost_aware {
                if let Some(cost) = cost_aware.model_costs.get(&model_id_for_cost) {
                    let estimated = cost.estimate(usage);
                    cost_aware.daily_spend_usd += estimated;
                    info!(
                        "Cost tracked: ${estimated:.4} for '{model_id_for_cost}' (task: \
                         {task_type:?})"
                    );
                }
            }
        }

        Ok(response)
    }

    /// Get cheapest model when budget is exceeded, or `None` if within
    /// budget.
    fn cheapest_model_on_budget_exceeded(cost_aware: &CostAwareConfig) -> Option<String> {
        let budget = cost_aware.budget_limit_usd?;
        let current_spend = cost_aware.daily_spend_usd;
        if current_spend < budget {
            return None;
        }
        warn!(
            "Daily budget exceeded: ${:.2} / ${:.2}. Falling back to cheapest model.",
            current_spend, budget
        );
        let cheapest = cost_aware
            .model_costs
            .iter()
            .min_by(|a, b| {
                let a_total = a.1.input_cost_per_1k + a.1.output_cost_per_1k;
                let b_total = b.1.input_cost_per_1k + b.1.output_cost_per_1k;
                a_total
                    .partial_cmp(&b_total)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| cost_aware.default_model.clone());
        Some(cheapest)
    }

    /// Resolve the model to use for a given task type based on routing rules.
    fn resolve_model_for_task(
        cost_aware: &CostAwareConfig,
        task_type: &TaskType,
        messages: &[Message],
    ) -> String {
        let rule = cost_aware
            .routing_rules
            .iter()
            .find(|r| r.task_type == *task_type)
            .or_else(|| {
                cost_aware
                    .routing_rules
                    .iter()
                    .find(|r| r.task_type == TaskType::Unknown)
            });

        let Some(rule) = rule else {
            return cost_aware.default_model.clone();
        };

        let estimated_tokens: u32 = messages.iter().map(|m| m.content.len() as u32 / 4).sum();
        if let Some(max_tokens) = rule.max_input_tokens {
            if estimated_tokens > max_tokens {
                info!(
                    "Estimated tokens ({estimated_tokens}) exceeds max for '{}' ({max_tokens}), \
                     using fallback",
                    rule.preferred_model
                );
                return rule
                    .fallback_model
                    .clone()
                    .unwrap_or_else(|| rule.preferred_model.clone());
            }
        }
        rule.preferred_model.clone()
    }

    /// Get current daily spend
    pub async fn get_daily_spend(&self) -> f64 {
        let config = self.config.read().await;
        config
            .cost_aware
            .as_ref()
            .map(|c| c.daily_spend_usd)
            .unwrap_or(0.0)
    }

    /// Reset daily spend counter
    pub async fn reset_daily_spend(&self) {
        let mut config = self.config.write().await;
        if let Some(ref mut cost_aware) = config.cost_aware {
            cost_aware.daily_spend_usd = 0.0;
            info!("Daily spend counter reset");
        }
    }
}
