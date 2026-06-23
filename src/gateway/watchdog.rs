//! Gateway self-repair watchdog loop and per-target restart tracking.
//!
//! Periodically checks agents (closed mpsc tx) and channels (failed
//! `health_check`) and tries to bring them back up to a configured maximum.
//! Per-target restart counters live in [`RepairState`] and are shown via the
//! admin REST surface.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::{spawn_agent_inner, GatewayEvent, GatewayState};
use crate::agent::AgentConfig;
use crate::channels::Channel;

/// Per-target restart tracking record (agent or channel)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairRecord {
    pub target: String,
    pub restart_count: u32,
    pub last_restart_at: Option<chrono::DateTime<chrono::Utc>>,
    pub abandoned: bool,
}

impl RepairRecord {
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            restart_count: 0,
            last_restart_at: None,
            abandoned: false,
        }
    }
}

/// Shared state for the gateway-level self-repair loop — exposed via REST
pub struct RepairState {
    pub last_cycle_at: RwLock<Option<chrono::DateTime<chrono::Utc>>>,
    pub records: RwLock<HashMap<String, RepairRecord>>,
    pub loop_running: std::sync::atomic::AtomicBool,
}

impl Default for RepairState {
    fn default() -> Self {
        Self::new()
    }
}

impl RepairState {
    pub fn new() -> Self {
        Self {
            last_cycle_at: RwLock::new(None),
            records: RwLock::new(HashMap::new()),
            loop_running: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

/// Remove stale repair records for agents/channels that no longer exist.
pub(crate) async fn prune_repair_records(state: &Arc<GatewayState>) {
    let agent_keys: std::collections::HashSet<String> = {
        let agents = state.agents.agents.read().await;
        agents.keys().map(|id| format!("agent:{}", id)).collect()
    };
    let channel_keys: std::collections::HashSet<String> = {
        let channels = state.channels.channels.read().await;
        channels
            .keys()
            .map(|name| format!("channel:{}", name))
            .collect()
    };

    let mut records = state.agents.repair_state.records.write().await;
    let stale: Vec<String> = records
        .keys()
        .filter(|key| {
            if key.starts_with("agent:") {
                !agent_keys.contains(*key)
            } else if key.starts_with("channel:") {
                !channel_keys.contains(*key)
            } else {
                false
            }
        })
        .cloned()
        .collect();
    for key in stale {
        records.remove(&key);
        debug!("Pruned stale repair record '{}'", key);
    }
}
pub(crate) async fn run_agent_watchdog_cycle(state: &Arc<GatewayState>) {
    const MAX_RESTARTS: u32 = 5;
    const COOLDOWN_SECS: i64 = 30;

    let dead: Vec<(String, AgentConfig)> = {
        state
            .agents
            .agents
            .read()
            .await
            .iter()
            .filter(|(_, h)| h.tx.is_closed())
            .map(|(id, h)| (id.clone(), h.config.clone()))
            .collect()
    };
    if dead.is_empty() {
        return;
    }

    for (agent_id, config) in dead {
        let key = format!("agent:{}", agent_id);

        let should_restart = {
            let mut records = state.agents.repair_state.records.write().await;
            let rec = records
                .entry(key.clone())
                .or_insert_with(|| RepairRecord::new(&key));
            if rec.abandoned {
                false
            } else if rec.restart_count >= MAX_RESTARTS {
                error!("Agent {} exceeded max restarts ({}), abandoning", agent_id, MAX_RESTARTS);
                rec.abandoned = true;
                false
            } else {
                !rec.last_restart_at
                    .map(|t| (chrono::Utc::now() - t).num_seconds() < COOLDOWN_SECS)
                    .unwrap_or(false)
            }
        };
        if !should_restart {
            continue;
        }

        warn!("Agent {} tx closed — attempting restart", agent_id);

        match spawn_agent_inner(state.clone(), agent_id.clone(), config).await {
            Ok(_handle) => {
                // Only replace the old handle after a successful spawn so the
                // agent remains visible for retry if spawn fails.
                state.agents.agents.write().await.remove(&agent_id);

                let mut records = state.agents.repair_state.records.write().await;
                let rec = records
                    .entry(key)
                    .or_insert_with(|| RepairRecord::new(&agent_id));
                rec.restart_count += 1;
                rec.last_restart_at = Some(chrono::Utc::now());
                info!("Agent {} restarted (attempt {})", agent_id, rec.restart_count);
                if let Err(e) = state.events.tx.send(GatewayEvent::RepairAction {
                    kind: "agent".into(),
                    target_id: agent_id,
                    description: format!(
                        "Restarted after tx closed (attempt {})",
                        rec.restart_count
                    ),
                    restart_count: rec.restart_count,
                }) {
                    warn!("Failed to broadcast repair event: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to restart agent {}: {}", agent_id, e);
                let mut records = state.agents.repair_state.records.write().await;
                let rec = records
                    .entry(key)
                    .or_insert_with(|| RepairRecord::new(&agent_id));
                rec.restart_count += 1;
                rec.last_restart_at = Some(chrono::Utc::now());
            }
        }
    }
}

/// One watchdog cycle: check each channel's health and call `start()` if
/// unhealthy.
pub(crate) async fn run_channel_watchdog_cycle(state: &Arc<GatewayState>) {
    const MAX_RESTARTS: u32 = 5;
    const COOLDOWN_SECS: i64 = 30;

    let channels: Vec<(String, Arc<dyn Channel>)> = state
        .channels
        .channels
        .read()
        .await
        .iter()
        .map(|(n, c)| (n.clone(), c.clone()))
        .collect();

    for (name, channel) in channels {
        let healthy = match channel.health_check().await {
            Ok(b) => b,
            Err(e) => {
                warn!("Channel {} health_check error: {}", name, e);
                false
            }
        };
        if healthy {
            continue;
        }

        let key = format!("channel:{}", name);
        let should_restart = {
            let mut records = state.agents.repair_state.records.write().await;
            let rec = records
                .entry(key.clone())
                .or_insert_with(|| RepairRecord::new(&key));
            if rec.abandoned {
                false
            } else if rec.restart_count >= MAX_RESTARTS {
                error!("Channel {} exceeded max restarts ({}), abandoning", name, MAX_RESTARTS);
                rec.abandoned = true;
                false
            } else {
                !rec.last_restart_at
                    .map(|t| (chrono::Utc::now() - t).num_seconds() < COOLDOWN_SECS)
                    .unwrap_or(false)
            }
        };
        if !should_restart {
            continue;
        }

        warn!("Channel {} unhealthy — calling start()", name);
        match channel.start().await {
            Ok(()) => {
                let mut records = state.agents.repair_state.records.write().await;
                let rec = records
                    .entry(key)
                    .or_insert_with(|| RepairRecord::new(&name));
                rec.restart_count += 1;
                rec.last_restart_at = Some(chrono::Utc::now());
                info!("Channel {} restarted (attempt {})", name, rec.restart_count);
                if let Err(e) = state.events.tx.send(GatewayEvent::RepairAction {
                    kind: "channel".into(),
                    target_id: name,
                    description: format!(
                        "Restarted after health_check=false (attempt {})",
                        rec.restart_count
                    ),
                    restart_count: rec.restart_count,
                }) {
                    warn!("Failed to broadcast repair event: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to restart channel {}: {}", name, e);
                let mut records = state.agents.repair_state.records.write().await;
                let rec = records
                    .entry(key)
                    .or_insert_with(|| RepairRecord::new(&name));
                rec.restart_count += 1;
                rec.last_restart_at = Some(chrono::Utc::now());
            }
        }
    }
}

/// Gateway-level self-repair loop — runs every 60 seconds, checks agents and
/// channels. Exits promptly when the shutdown token is cancelled.
pub(crate) async fn run_repair_loop(state: Arc<GatewayState>, shutdown_token: CancellationToken) {
    use std::sync::atomic::Ordering;
    state
        .agents
        .repair_state
        .loop_running
        .store(true, Ordering::Relaxed);

    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = shutdown_token.cancelled() => {
                info!("Repair loop received shutdown signal, exiting");
                break;
            }
            _ = ticker.tick() => {
                *state.agents.repair_state.last_cycle_at.write().await = Some(chrono::Utc::now());
                run_agent_watchdog_cycle(&state).await;
                run_channel_watchdog_cycle(&state).await;
                prune_repair_records(&state).await;
            }
        }
    }

    state
        .agents
        .repair_state
        .loop_running
        .store(false, Ordering::Relaxed);
}
