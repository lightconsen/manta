//! Standing Orders — Persistent Background Agent Programs
//!
//! Standing orders are cron-scheduled jobs that periodically send a prompt to
//! a target agent and optionally dispatch the response to a channel.
//!
//! Follows the same lifecycle pattern as `DreamScheduler` in
//! `src/memory/dreaming.rs` and borrows the agent-wake pattern from
//! `src/heartbeat/runner.rs`.

pub mod config;

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use cron::Schedule as CronSchedule;
use std::str::FromStr;
use tokio::sync::mpsc;
use tokio::time::{sleep_until, Instant as TokioInstant};
use tracing::{error, info, warn};

use crate::channels::{ConversationId, IncomingMessage, InputProvenance, OutgoingMessage};
use crate::gateway::GatewayState;
use config::StandingOrderConfig;

/// Manages a collection of standing order background tasks.
///
/// Each enabled order in the config spawns a separate tokio task that sleeps
/// until the next cron tick, fires the prompt against the configured agent,
/// and optionally dispatches the response via `ReplyDispatcher`.
pub struct StandingOrderManager {
    config: StandingOrderConfig,
    state: Arc<GatewayState>,
    /// One shutdown sender per enabled order, keyed by order name.
    shutdown_txs: Vec<(String, mpsc::Sender<()>)>,
}

impl StandingOrderManager {
    /// Create a new manager from the config and gateway state.
    pub fn new(config: StandingOrderConfig, state: Arc<GatewayState>) -> Self {
        Self {
            config,
            state,
            shutdown_txs: Vec::new(),
        }
    }

    /// Start all enabled standing orders.
    ///
    /// Each enabled order spawns a separate tokio task.  Orders whose cron
    /// expression cannot be parsed are silently skipped (a warning is logged).
    pub fn start(&mut self) {
        if !self.config.enabled {
            info!("Standing orders are disabled globally");
            return;
        }

        for order in &self.config.orders {
            if !order.enabled {
                info!("Standing order '{}' is disabled, skipping", order.name);
                continue;
            }

            let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
            self.shutdown_txs.push((order.name.clone(), shutdown_tx));

            let order_name = order.name.clone();
            let agent_id = order.agent_id.clone();
            let prompt = order.prompt.clone();
            let schedule_expr = order.schedule.clone();
            let output_channel = order.output_channel.clone();
            let timeout = order.timeout_secs.unwrap_or(120);
            let state = Arc::clone(&self.state);

            tokio::spawn(async move {
                let schedule = match CronSchedule::from_str(&schedule_expr) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(
                            "Invalid cron expression '{}' for standing order '{}': {}",
                            schedule_expr, order_name, e
                        );
                        return;
                    }
                };

                info!(
                    "Standing order '{}' started (agent={}, schedule='{}')",
                    order_name, agent_id, schedule_expr
                );

                loop {
                    // Calculate next execution time
                    let next = match schedule.upcoming(Utc).next() {
                        Some(dt) => dt,
                        None => {
                            warn!(
                                "No upcoming times for standing order '{}' cron '{}'",
                                order_name, schedule_expr
                            );
                            break;
                        }
                    };

                    let now = Utc::now();
                    let delay_ms = if next > now {
                        (next - now).num_milliseconds().max(0) as u64
                    } else {
                        0
                    };

                    let sleep_deadline =
                        TokioInstant::now() + Duration::from_millis(delay_ms);

                    tokio::select! {
                        _ = sleep_until(sleep_deadline) => {
                            // ── Fire the standing order ────────────────
                            let session_id = format!("standing_order:{}", order_name);
                            let message = IncomingMessage::new("system", &session_id, &prompt)
                                .with_provenance(InputProvenance::InternalSystem {
                                    source: "standing_order".to_string(),
                                });

                            // Find the target agent
                            let agent_handle = {
                                let agents = state.agents.read().await;
                                agents.get(&agent_id).cloned()
                            };

                            match agent_handle {
                                Some(handle) => {
                                    let result = tokio::time::timeout(
                                        Duration::from_secs(timeout),
                                        handle.agent.process_message(message),
                                    )
                                    .await;

                                    match result {
                                        Ok(Ok(response)) => {
                                            info!("Standing order '{}' completed", order_name);

                                            // Optionally dispatch to a channel
                                            if let Some(ref channel) = output_channel {
                                                let dispatch_msg = OutgoingMessage::new(
                                                    ConversationId(session_id),
                                                    response.content,
                                                );
                                                if let Err(e) = state
                                                    .reply_dispatcher
                                                    .dispatch(channel, dispatch_msg)
                                                    .await
                                                {
                                                    warn!(
                                                        "Failed to dispatch '{}' response: {}",
                                                        order_name, e
                                                    );
                                                }
                                            }
                                        }
                                        Ok(Err(e)) => {
                                            error!(
                                                "Standing order '{}' agent error: {}",
                                                order_name, e
                                            );
                                        }
                                        Err(_) => {
                                            warn!(
                                                "Standing order '{}' timed out after {}s",
                                                order_name, timeout
                                            );
                                        }
                                    }
                                }
                                None => {
                                    warn!(
                                        "Standing order '{}': agent '{}' not found",
                                        order_name, agent_id
                                    );
                                }
                            }
                        }
                        _ = shutdown_rx.recv() => {
                            info!("Standing order '{}' shutting down", order_name);
                            break;
                        }
                    }
                }
            });
        }

        info!(
            "Started {} standing order(s)",
            self.shutdown_txs.len()
        );
    }

    /// Stop all running standing orders.
    pub async fn stop(&mut self) {
        for (name, tx) in self.shutdown_txs.drain(..) {
            let _ = tx.send(()).await;
            info!("Standing order '{}' stop signal sent", name);
        }
    }

    /// Returns the number of running (enabled) orders.
    pub fn running_count(&self) -> usize {
        self.shutdown_txs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::config::StandingOrderConfig;

    #[test]
    fn test_config_default() {
        let config = StandingOrderConfig::default();
        assert!(config.enabled);
        assert!(config.orders.is_empty());
    }
}
