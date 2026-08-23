//! Standing Orders — Persistent Background Agent Programs
//!
//! Standing orders are cron-scheduled jobs that periodically send a prompt to
//! a target agent and optionally dispatch the response to a channel.
//!
//! Follows the same lifecycle pattern as `DreamScheduler` in
//! `src/memory/dreaming.rs` and borrows the agent-wake pattern from
//! `src/heartbeat/runner.rs`.

pub mod config;

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use config::{StandingOrderConfig, StandingOrderDef};
use cron::Schedule as CronSchedule;
use tokio::sync::oneshot;
use tokio::time::{sleep_until, Instant as TokioInstant};
use tracing::{error, info, warn};

use crate::channels::{ConversationId, IncomingMessage, InputProvenance, OutgoingMessage};
use crate::gateway::GatewayState;

/// Manages a collection of standing order background tasks.
///
/// Each enabled order in the config spawns a separate tokio task that sleeps
/// until the next cron tick, fires the prompt against the configured agent,
/// and optionally dispatches the response via `ReplyDispatcher`.
pub struct StandingOrderManager {
    config: StandingOrderConfig,
    state: Arc<GatewayState>,
    /// One shutdown sender per enabled order, keyed by order name.
    shutdown_txs: Vec<(String, oneshot::Sender<()>)>,
    /// True after start() has been called (guards against duplicate start).
    started: bool,
}

impl StandingOrderManager {
    /// Create a new manager from the config and gateway state.
    pub fn new(config: StandingOrderConfig, state: Arc<GatewayState>) -> Self {
        Self {
            config,
            state,
            shutdown_txs: Vec::new(),
            started: false,
        }
    }

    /// Start all enabled standing orders.
    ///
    /// Each enabled order spawns a separate tokio task.  Orders whose cron
    /// expression cannot be parsed are silently skipped (a warning is logged).
    ///
    /// Idempotent — calling `start()` a second time is a no-op.
    pub async fn start(&mut self) {
        if self.started {
            info!("Standing orders already started, ignoring duplicate start");
            return;
        }

        if !self.config.enabled {
            info!("Standing orders are disabled globally");
            return;
        }

        for order in &self.config.orders {
            if !order.enabled {
                info!("Standing order '{}' is disabled, skipping", order.name);
                continue;
            }

            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            self.shutdown_txs.push((order.name.clone(), shutdown_tx));

            let handle = tokio::spawn(Self::run_single_order(
                order.clone(),
                Arc::clone(&self.state),
                shutdown_rx,
            ));

            // Register the task handle for coordinated shutdown.
            self.state
                .task_registry
                .insert_join(format!("standing_order:{}", order.name), handle)
                .await;
        }

        self.started = true;
        info!("Started {} standing order(s)", self.shutdown_txs.len());
    }

    /// Stop all running standing orders.
    ///
    /// After `stop()`, the manager can be restarted with a new config
    /// via [`reload_config`] followed by [`start`].
    pub async fn stop(&mut self) {
        for (name, tx) in self.shutdown_txs.drain(..) {
            let _ = tx.send(());
            info!("Standing order '{}' stop signal sent", name);
        }
        self.started = false;
    }

    /// Stop a single standing order by name.
    ///
    /// Returns `true` if the order was found and sent a shutdown signal,
    /// `false` if no order with that name was running.
    pub async fn stop_order(&mut self, name: &str) -> bool {
        let pos = self.shutdown_txs.iter().position(|(n, _)| n == name);
        match pos {
            Some(i) => {
                let (_name, tx) = self.shutdown_txs.remove(i);
                let _ = tx.send(());
                info!("Standing order '{}' stop signal sent", name);
                true
            }
            None => {
                warn!("Standing order '{}' not found or already stopped", name);
                false
            }
        }
    }

    /// Replace the configuration (takes effect on next [`start`]).
    ///
    /// Does **not** stop running orders. Call [`stop`] first if the goal is
    /// to restart with the new config.
    pub fn reload_config(&mut self, config: StandingOrderConfig) {
        info!("Standing order config reloaded (orders={})", config.orders.len());
        self.config = config;
    }

    /// Returns the number of running (enabled) orders.
    pub fn running_count(&self) -> usize {
        self.shutdown_txs.len()
    }

    /// Run a single standing order: parse the cron schedule, then loop,
    /// sleeping until each tick and firing the prompt against the agent.
    async fn run_single_order(
        order: StandingOrderDef,
        state: Arc<GatewayState>,
        mut shutdown_rx: oneshot::Receiver<()>,
    ) {
        let StandingOrderDef {
            name: order_name,
            agent_id,
            prompt,
            schedule: schedule_expr,
            output_channel,
            timeout_secs,
            ..
        } = order;
        let timeout_secs = timeout_secs.unwrap_or(120);
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

            let sleep_deadline = TokioInstant::now() + Duration::from_millis(delay_ms);

            tokio::select! {
                _ = sleep_until(sleep_deadline) => {
                    Self::execute_order(
                        &order_name,
                        &agent_id,
                        &prompt,
                        timeout_secs,
                        &output_channel,
                        &state,
                    ).await;
                }
                _ = &mut shutdown_rx => {
                    info!("Standing order '{}' shutting down", order_name);
                    break;
                }
            }
        }
    }

    /// Fire a single standing order's prompt against its target agent
    /// and optionally dispatch the response.
    async fn execute_order(
        order_name: &str,
        agent_id: &str,
        prompt: &str,
        timeout_secs: u64,
        output_channel: &Option<String>,
        state: &Arc<GatewayState>,
    ) {
        let session_id = format!("standing_order:{}", order_name);
        let message = IncomingMessage::new("system", &session_id, prompt).with_provenance(
            InputProvenance::InternalSystem {
                source: "standing_order".to_string(),
            },
        );

        let agent_handle = {
            let agents = state.agents.agents.read().await;
            agents.get(agent_id).cloned()
        };

        match agent_handle {
            Some(handle) => {
                let result = tokio::time::timeout(
                    Duration::from_secs(timeout_secs),
                    handle.agent.process_message(message),
                )
                .await;

                match result {
                    Ok(Ok(response)) => {
                        info!("Standing order '{}' completed", order_name);

                        if let Some(ref channel) = output_channel {
                            let dispatch_msg =
                                OutgoingMessage::new(ConversationId(session_id), response.content);
                            if let Err(e) = state
                                .channels
                                .reply_dispatcher
                                .dispatch(channel, dispatch_msg)
                                .await
                            {
                                warn!("Failed to dispatch '{}' response: {}", order_name, e);
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        error!("Standing order '{}' agent error: {}", order_name, e);
                    }
                    Err(_) => {
                        warn!("Standing order '{}' timed out after {}s", order_name, timeout_secs);
                    }
                }
            }
            None => {
                warn!("Standing order '{}': agent '{}' not found", order_name, agent_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::config::{StandingOrderConfig, StandingOrderDef};

    // ── Config tests ────────────────────────────────────────────────

    #[test]
    fn test_config_default() {
        let config = StandingOrderConfig::default();
        assert!(config.enabled);
        assert!(config.orders.is_empty());
    }

    #[test]
    fn test_config_round_trip() {
        let toml_str = r#"
enabled = true

[[orders]]
name = "daily-summary"
agent_id = "assistant"
schedule = "0 0 9 * * 1-5"
prompt = "Summarize yesterday's activity"
output_channel = "general"
timeout_secs = 60
"#;
        let config: StandingOrderConfig =
            toml::from_str(toml_str).expect("valid config should parse");
        assert!(config.enabled);
        assert_eq!(config.orders.len(), 1);
        let order = &config.orders[0];
        assert_eq!(order.name, "daily-summary");
        assert_eq!(order.agent_id, "assistant");
        assert_eq!(order.schedule, "0 0 9 * * 1-5");
        assert_eq!(order.prompt, "Summarize yesterday's activity");
        assert_eq!(order.output_channel.as_deref(), Some("general"));
        assert!(order.enabled);
        assert_eq!(order.timeout_secs, Some(60));
    }

    #[test]
    fn test_config_order_default_enabled() {
        let toml_str = r#"
[[orders]]
name = "test"
agent_id = "agent"
schedule = "0 */5 * * * *"
prompt = "hello"
"#;
        let config: StandingOrderConfig =
            toml::from_str(toml_str).expect("minimal order should parse");
        let order = &config.orders[0];
        assert!(order.enabled, "enabled should default to true");
        assert!(order.output_channel.is_none());
        assert!(order.timeout_secs.is_none());
    }

    #[test]
    fn test_config_disabled_global() {
        let toml_str = r#"
enabled = false
"#;
        let config: StandingOrderConfig =
            toml::from_str(toml_str).expect("disabled config should parse");
        assert!(!config.enabled);
        assert!(config.orders.is_empty());
    }

    // ── Cron validation tests ──────────────────────────────────────

    #[test]
    fn test_valid_cron_expression() {
        let toml_str = r#"
[[orders]]
name = "test"
agent_id = "agent"
schedule = "*/5 * * * * *"
prompt = "ping"
"#;
        let config: std::result::Result<StandingOrderConfig, _> = toml::from_str(toml_str);
        assert!(config.is_ok(), "valid cron should parse: {:?}", config.err());
    }

    #[test]
    fn test_valid_cron_six_field() {
        let toml_str = r#"
[[orders]]
name = "test"
agent_id = "agent"
schedule = "0 30 9 * * 1-5"
prompt = "ping"
"#;
        let config: std::result::Result<StandingOrderConfig, _> = toml::from_str(toml_str);
        assert!(config.is_ok(), "six-field cron should parse: {:?}", config.err());
    }

    #[test]
    fn test_invalid_cron_expression() {
        let toml_str = r#"
[[orders]]
name = "test"
agent_id = "agent"
schedule = "not-a-cron"
prompt = "ping"
"#;
        let config: std::result::Result<StandingOrderConfig, _> = toml::from_str(toml_str);
        assert!(config.is_err(), "invalid cron should fail to parse");
        let err = config.unwrap_err().to_string();
        assert!(
            err.contains("invalid cron expression"),
            "error should mention cron validation: {}",
            err
        );
    }

    #[test]
    fn test_invalid_cron_out_of_range() {
        let toml_str = r#"
[[orders]]
name = "test"
agent_id = "agent"
schedule = "99 * * * * *"
prompt = "ping"
"#;
        let config: std::result::Result<StandingOrderConfig, _> = toml::from_str(toml_str);
        assert!(config.is_err(), "out-of-range cron should fail");
    }

    // ── Manager construction tests ─────────────────────────────────

    #[test]
    fn test_standing_order_def_fields() {
        let def = StandingOrderDef {
            name: "test".into(),
            description: Some("a test order".into()),
            agent_id: "agent-1".into(),
            schedule: "0 */5 * * * *".into(),
            prompt: "do something".into(),
            output_channel: Some("general".into()),
            enabled: true,
            timeout_secs: Some(30),
        };
        assert_eq!(def.name, "test");
        assert_eq!(def.description.as_deref(), Some("a test order"));
        assert_eq!(def.schedule, "0 */5 * * * *");
        assert_eq!(def.timeout_secs, Some(30));
    }

    #[test]
    fn test_standing_order_def_defaults() {
        let toml_str = r#"
[[orders]]
name = "minimal"
agent_id = "a1"
schedule = "0 0 * * * *"
prompt = "work"
"#;
        let config: StandingOrderConfig =
            toml::from_str(toml_str).expect("minimal order should parse");
        let order = &config.orders[0];
        assert!(order.enabled);
        assert!(order.description.is_none());
        assert!(order.output_channel.is_none());
        assert!(order.timeout_secs.is_none());
    }
}
