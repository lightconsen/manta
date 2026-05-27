use chrono::Timelike;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use super::config::HeartbeatConfig;
use super::events::{HeartbeatEvent, HeartbeatStatus};
use super::parser::{parse_heartbeat_tasks, is_heartbeat_content_empty, HeartbeatTask, TaskDedupTracker};
use super::wake::{WakePriority, WakeRequest};
use crate::channels::IncomingMessage;
use crate::gateway::GatewayState;

/// Default HEARTBEAT.md filename
const HEARTBEAT_FILENAME: &str = "HEARTBEAT.md";

/// State tracked per agent for heartbeat scheduling
struct AgentHeartbeatState {
    consecutive_idle: u32,
    last_run: Option<std::time::Instant>,
    dedup: TaskDedupTracker,
}

impl AgentHeartbeatState {
    fn new() -> Self {
        Self {
            consecutive_idle: 0,
            last_run: None,
            dedup: TaskDedupTracker::new(),
        }
    }
}

/// The heartbeat runner that periodically wakes agents to check HEARTBEAT.md
pub struct HeartbeatRunner {
    state: Arc<GatewayState>,
    config: HeartbeatConfig,
    agent_states: Arc<RwLock<HashMap<String, AgentHeartbeatState>>>,
    pub(crate) event_tx: tokio::sync::broadcast::Sender<HeartbeatEvent>,
    wake_rx: mpsc::Receiver<WakeRequest>,
    wake_tx: mpsc::Sender<WakeRequest>,
}

impl HeartbeatRunner {
    pub fn new(state: Arc<GatewayState>) -> Self {
        let config = {
            let config_lock = state.config.blocking_read();
            config_lock.heartbeat.clone()
        };
        let (wake_tx, wake_rx) = mpsc::channel::<WakeRequest>(64);
        let (event_tx, _) = tokio::sync::broadcast::channel::<HeartbeatEvent>(256);

        Self {
            state,
            config,
            agent_states: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
            wake_rx,
            wake_tx,
        }
    }

    /// Return a sender for external wake requests
    pub fn wake_sender(&self) -> mpsc::Sender<WakeRequest> {
        self.wake_tx.clone()
    }

    /// Return an event subscriber for heartbeat status
    pub fn event_subscribe(&self) -> tokio::sync::broadcast::Receiver<HeartbeatEvent> {
        self.event_tx.subscribe()
    }

    /// Start the heartbeat runner loop
    pub async fn start(mut self) {
        info!(
            "Heartbeat runner started: interval={}s, active_hours={}-{}",
            self.config.interval_seconds,
            self.config.active_hours_start,
            self.config.active_hours_end,
        );

        loop {
            // Run heartbeat cycle
            self._emit_event(HeartbeatEvent::Started);
            self.run_heartbeat_cycle().await;

            // Wait for interval, but process wake requests if they arrive
            tokio::time::sleep(Duration::from_secs(self.config.interval_seconds)).await;

            // Check for pending wake requests after sleep
            while let Ok(req) = self.wake_rx.try_recv() {
                self._emit_event(HeartbeatEvent::Started);
                self.handle_wake_request(&req).await;
            }
        }
    }

    /// Handle a single wake request
    async fn handle_wake_request(&self, req: &WakeRequest) {
        info!(
            "Heartbeat wake request: agent={}, priority={:?}",
            req.agent_id, req.priority
        );

        let agents = self.state.agents.read().await;
        let handle = match agents.get(&req.agent_id) {
            Some(h) => h.clone(),
            None => {
                warn!("Heartbeat wake: agent {} not found", req.agent_id);
                self._emit_event(HeartbeatEvent::Skipped {
                    reason: "agent_not_found".to_string(),
                    agent_id: req.agent_id.clone(),
                });
                return;
            }
        };
        drop(agents);

        if handle.busy {
            if req.priority == WakePriority::Retry {
                debug!("Agent {} busy, skipping retry wake", req.agent_id);
                return;
            }
            let wake_tx = self.wake_tx.clone();
            let req = WakeRequest {
                agent_id: req.agent_id.clone(),
                priority: WakePriority::Retry,
                prompt: req.prompt.clone(),
            };
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let _ = wake_tx.send(req).await;
            });
            return;
        }

        self.run_heartbeat_for_agent(&handle, req.prompt.as_deref()).await;
    }

    /// Run a full heartbeat cycle across all agents
    async fn run_heartbeat_cycle(&self) {
        if !self.is_within_active_hours() {
            debug!("Heartbeat skipped: outside active hours");
            return;
        }

        let handles: Vec<_> = {
            let agents = self.state.agents.read().await;
            agents.values().cloned().collect()
        };

        if handles.is_empty() {
            self._emit_event(HeartbeatEvent::Skipped {
                reason: "no_agents".to_string(),
                agent_id: String::from("*"),
            });
            return;
        }

        for handle in &handles {
            let agent_id = &handle.id;
            let should_run = {
                let states = self.agent_states.read().await;
                if let Some(agent_state) = states.get(agent_id) {
                    agent_state.consecutive_idle < self.config.max_consecutive_idle
                } else {
                    true
                }
            };

            if should_run {
                self.run_heartbeat_for_agent(handle, None).await;
            } else {
                debug!(
                    "Agent {} heartbeat skipped: max consecutive idle reached",
                    agent_id,
                );
                self._emit_event(HeartbeatEvent::Skipped {
                    reason: "max_consecutive_idle".to_string(),
                    agent_id: agent_id.clone(),
                });
            }
        }
    }

    /// Read HEARTBEAT.md content
    async fn read_heartbeat_content(&self, handle: &crate::gateway::AgentHandle) -> Option<String> {
        // Try workspace_dir from agent config
        if let Some(ref workspace_dir) = handle.config.workspace_dir {
            let path = workspace_dir.join(HEARTBEAT_FILENAME);
            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                return Some(content);
            }
        }

        // Try per-agent directory: ~/.manta/agents/{id}/HEARTBEAT.md
        let agent_path = crate::dirs::agents_dir().join(&handle.id).join(HEARTBEAT_FILENAME);
        if let Ok(content) = tokio::fs::read_to_string(&agent_path).await {
            return Some(content);
        }

        // Fallback: workspace-level HEARTBEAT.md
        let workspace_path = crate::dirs::workspace_data_dir().join(HEARTBEAT_FILENAME);
        if let Ok(content) = tokio::fs::read_to_string(&workspace_path).await {
            return Some(content);
        }

        None
    }

    /// Run heartbeat for a single agent
    async fn run_heartbeat_for_agent(&self, handle: &crate::gateway::AgentHandle, custom_prompt: Option<&str>) {
        let agent_id = &handle.id;

        if handle.busy {
            self._emit_event(HeartbeatEvent::Skipped {
                reason: "agent_busy".to_string(),
                agent_id: agent_id.clone(),
            });
            return;
        }

        // Read HEARTBEAT.md content
        let heartbeat_content = self.read_heartbeat_content(handle).await;

        // If HEARTBEAT.md is empty and no custom prompt, mark idle
        if custom_prompt.is_none()
            && heartbeat_content.as_ref().map_or(true, |c| is_heartbeat_content_empty(c))
        {
            self._emit_event(HeartbeatEvent::Completed {
                status: HeartbeatStatus::Idle,
                agent_id: agent_id.clone(),
                session_id: None,
            });
            self.update_consecutive_idle(agent_id, true).await;
            return;
        }

        // Parse tasks and build prompt
        let prompt = match custom_prompt {
            Some(cp) => cp.to_string(),
            None => {
                let tasks = heartbeat_content
                    .as_deref()
                    .map(parse_heartbeat_tasks)
                    .unwrap_or_default();

                let due_tasks: Vec<&HeartbeatTask> = {
                    let states = self.agent_states.read().await;
                    if let Some(agent_state) = states.get(agent_id) {
                        tasks
                            .iter()
                            .filter(|t| agent_state.dedup.is_task_due(t))
                            .collect()
                    } else {
                        tasks.iter().collect()
                    }
                };

                if due_tasks.is_empty() && tasks.is_empty() {
                    "Read HEARTBEAT.md if it exists. Follow it strictly. Do not invent or repeat old tasks from prior chats. If nothing needs attention, reply HEARTBEAT_OK.".to_string()
                } else if due_tasks.is_empty() {
                    "Read HEARTBEAT.md. No tasks are due at this time. If nothing else needs attention, reply HEARTBEAT_OK.".to_string()
                } else {
                    let mut prompt = "Read HEARTBEAT.md. The following tasks are due for execution:\n\n".to_string();
                    for task in &due_tasks {
                        prompt.push_str(&format!("- **{}**: {}\n", task.name, task.prompt));
                    }
                    prompt.push_str("\nExecute the due tasks. For any completed task, note it so it can be tracked.");
                    prompt
                }
            }
        };

        let session_id = format!("heartbeat:{}", agent_id);

        info!("Heartbeat poll for agent {}", agent_id);

        let message = IncomingMessage::new("system", &session_id, &prompt)
            .with_provenance(crate::channels::InputProvenance::InternalSystem {
                source: "heartbeat".to_string(),
            });

        match handle.agent.process_message(message).await {
            Ok(response) => {
                if let Some(ref content) = heartbeat_content {
                    let tasks = parse_heartbeat_tasks(content);
                    if !response.content.contains("HEARTBEAT_OK") && !tasks.is_empty() {
                        let mut states = self.agent_states.write().await;
                        if let Some(agent_state) = states.get_mut(agent_id) {
                            for task in &tasks {
                                agent_state.dedup.mark_executed(&task.name);
                            }
                        }
                    }
                }

                let status = if response.content.contains("HEARTBEAT_OK") {
                    HeartbeatStatus::Idle
                } else {
                    HeartbeatStatus::TaskExecuted
                };

                self.update_consecutive_idle(agent_id, status == HeartbeatStatus::Idle).await;

                self._emit_event(HeartbeatEvent::Completed {
                    status,
                    agent_id: agent_id.clone(),
                    session_id: Some(session_id),
                });

                info!(
                    "Heartbeat response from {}: status={:?}, content_preview: {}",
                    agent_id,
                    status,
                    response.content.chars().take(80).collect::<String>()
                );

                let mut states = self.agent_states.write().await;
                if let Some(agent_state) = states.get_mut(agent_id) {
                    agent_state.last_run = Some(std::time::Instant::now());
                }
            }
            Err(e) => {
                error!("Heartbeat failed for agent {}: {}", agent_id, e);
                self._emit_event(HeartbeatEvent::Failed {
                    error: e.to_string(),
                    agent_id: agent_id.clone(),
                });
                self.update_consecutive_idle(agent_id, false).await;
            }
        }
    }

    /// Update consecutive idle counter for an agent
    async fn update_consecutive_idle(&self, agent_id: &str, is_idle: bool) {
        let mut states = self.agent_states.write().await;
        let agent_state = states.entry(agent_id.to_string()).or_insert_with(AgentHeartbeatState::new);
        if is_idle {
            agent_state.consecutive_idle += 1;
        } else {
            agent_state.consecutive_idle = 0;
        }
    }

    /// Check if current time is within active hours
    fn is_within_active_hours(&self) -> bool {
        let now = chrono::Local::now();
        let current_minutes = now.hour() * 60 + now.minute();

        let start = parse_time(&self.config.active_hours_start);
        let end = parse_time(&self.config.active_hours_end);

        match (start, end) {
            (Some(s), Some(e)) => {
                if s <= e {
                    current_minutes >= s && current_minutes < e
                } else {
                    current_minutes >= s || current_minutes < e
                }
            }
            _ => true,
        }
    }

    fn _emit_event(&self, event: HeartbeatEvent) {
        let _ = self.event_tx.send(event);
    }
}

/// Parse a time string "HH:MM" into minutes since midnight
fn parse_time(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let hour: u32 = parts[0].parse().ok()?;
    let minute: u32 = parts[1].parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 60 + minute)
}
