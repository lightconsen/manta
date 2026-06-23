use std::collections::{HashMap, HashSet};

use tracing::{info, warn};

use crate::acp::config::SubagentStatus;
use crate::acp::subagent::{SubagentCommand, SubagentHandle};
use crate::channels::IncomingMessage;

use super::{AcpControlPlane, AcpSessionId, AcpSessionInfo, SubagentTreeNode};

impl AcpControlPlane {
    /// Send a message to a subagent
    pub async fn send_message(
        &self,
        subagent_id: &str,
        message: IncomingMessage,
    ) -> crate::Result<String> {
        let subagents = self.subagents.read().await;
        let subagent =
            subagents
                .get(subagent_id)
                .ok_or_else(|| crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", subagent_id),
                })?;

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        subagent
            .command_tx
            .send(SubagentCommand::ProcessMessage {
                message: Box::new(message),
                response_tx,
            })
            .await
            .map_err(|_| {
                crate::error::SyscityError::Internal("Subagent command channel closed".to_string())
            })?;

        let result = response_rx.await.map_err(|_| {
            crate::error::SyscityError::Internal("Subagent response channel closed".to_string())
        })??;

        Ok(result)
    }

    /// Shutdown a subagent
    pub async fn shutdown_subagent(&self, subagent_id: &str) -> crate::Result<bool> {
        let mut subagents = self.subagents.write().await;

        if let Some(subagent) = subagents.get_mut(subagent_id) {
            subagent.status = SubagentStatus::ShuttingDown;
            if let Err(e) = subagent.command_tx.send(SubagentCommand::Shutdown).await {
                warn!("Failed to send shutdown command to subagent {}: {}", subagent_id, e);
            }
            // Watchdog task will update status to Terminated once the task exits.
            drop(subagents);
            if let Some(ref store) = self.store {
                if let Err(e) = store
                    .update_subagent_run_status(subagent_id, "shutting_down")
                    .await
                {
                    warn!("Failed to persist shutting_down status for {}: {}", subagent_id, e);
                }
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Kill a subagent immediately (force abort)
    pub async fn kill_subagent(&self, subagent_id: &str) -> crate::Result<bool> {
        let mut subagents = self.subagents.write().await;

        if let Some(subagent) = subagents.get_mut(subagent_id) {
            if let Err(e) = subagent.command_tx.send(SubagentCommand::Shutdown).await {
                warn!("Failed to send shutdown command to subagent {}: {}", subagent_id, e);
            }
            subagent.abort_handle.abort();
            subagent.status = SubagentStatus::Terminated;
            info!("Killed subagent {} (force abort)", subagent_id);
            drop(subagents);
            if let Some(ref store) = self.store {
                if let Err(e) = store.kill_subagent_run(subagent_id, "user").await {
                    warn!("Failed to persist kill event for {}: {}", subagent_id, e);
                }
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Steer a subagent — cancel current execution and send a new message
    pub async fn steer_subagent(
        &self,
        subagent_id: &str,
        message: String,
    ) -> crate::Result<String> {
        let subagents = self.subagents.read().await;
        let subagent =
            subagents
                .get(subagent_id)
                .ok_or_else(|| crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", subagent_id),
                })?;

        // 1. Cancel any in-progress execution
        if let Err(e) = subagent.command_tx.send(SubagentCommand::Cancel).await {
            warn!("Failed to send cancel command to subagent {}: {}", subagent_id, e);
        }

        // 2. Build steer message
        let steer_msg = IncomingMessage::new(
            "user".to_string(),
            format!("steer-{}", subagent_id),
            message.clone(),
        );

        // 3. Send steer message as new ProcessMessage
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        subagent
            .command_tx
            .send(SubagentCommand::ProcessMessage {
                message: Box::new(steer_msg),
                response_tx,
            })
            .await
            .map_err(|_| {
                crate::error::SyscityError::Internal("Subagent command channel closed".to_string())
            })?;

        drop(subagents);

        // Persist steer event
        if let Some(ref store) = self.store {
            if let Err(e) = store.append_steer_to_run(subagent_id, &message).await {
                warn!("Failed to persist steer event for {}: {}", subagent_id, e);
            }
        }

        match response_rx.await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                Err(crate::error::SyscityError::Internal("Steer response dropped".to_string()))
            }
        }
    }

    /// Terminate all subagents in a session
    pub async fn terminate_session(&self, session_id: &AcpSessionId) -> crate::Result<usize> {
        let sessions = self.sessions.read().await;
        let session =
            sessions
                .get(session_id)
                .ok_or_else(|| crate::error::SyscityError::NotFound {
                    resource: format!("Session '{}'", session_id),
                })?;

        let subagent_ids: Vec<String> = session.subagents.clone();
        drop(sessions);

        let mut count = 0;
        for subagent_id in subagent_ids {
            if self.shutdown_subagent(&subagent_id).await.unwrap_or(false) {
                count += 1;
            }
        }

        // Remove session
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
        drop(sessions);

        // Delete from persistent store
        if let Some(ref store) = self.store {
            if let Err(e) = store.delete_acp_session(&session_id.0).await {
                warn!("Failed to delete ACP session {} from store: {}", session_id.0, e);
            }
        }

        info!("Terminated {} subagents in session {}", count, session_id);
        Ok(count)
    }

    /// Get subagent status
    pub async fn get_subagent_status(&self, subagent_id: &str) -> Option<SubagentStatus> {
        let subagents = self.subagents.read().await;
        subagents.get(subagent_id).map(|s| s.status)
    }

    /// List all subagents
    pub async fn list_subagents(&self) -> Vec<SubagentHandle> {
        let subagents = self.subagents.read().await;
        subagents.values().cloned().collect()
    }

    /// List subagents in a session
    pub async fn list_session_subagents(&self, session_id: &AcpSessionId) -> Vec<SubagentHandle> {
        let subagent_ids = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .map(|s| s.subagents.clone())
                .unwrap_or_default()
        };

        let subagents = self.subagents.read().await;
        subagent_ids
            .into_iter()
            .filter_map(|id| subagents.get(&id).cloned())
            .collect()
    }

    /// Get session info
    pub async fn get_session_info(&self, session_id: &AcpSessionId) -> Option<AcpSessionInfo> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).map(|s| AcpSessionInfo {
            id: s.id.clone(),
            parent_agent_id: s.parent_agent_id.clone(),
            subagent_count: s.subagents.len(),
            created_at: s.created_at,
        })
    }

    /// Get subagent tree for a session (recursive parent-child hierarchy)
    pub async fn get_subagent_tree(&self, session_id: &AcpSessionId) -> Vec<SubagentTreeNode> {
        let (root_parent_id, session_subagent_ids) = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .map(|s| (s.parent_agent_id.clone(), s.subagents.clone()))
                .unwrap_or_default()
        };

        let subagents = self.subagents.read().await;
        let mut by_parent: HashMap<String, Vec<SubagentHandle>> = HashMap::new();
        let mut all_ids = HashSet::new();

        for id in session_subagent_ids {
            if let Some(subagent) = subagents.get(&id) {
                all_ids.insert(subagent.id.clone());
                by_parent
                    .entry(subagent.parent_id.clone())
                    .or_default()
                    .push(subagent.clone());
            }
        }
        drop(subagents);

        fn build_tree(
            parent_id: &str,
            by_parent: &HashMap<String, Vec<SubagentHandle>>,
            all_ids: &HashSet<String>,
        ) -> Vec<SubagentTreeNode> {
            by_parent
                .get(parent_id)
                .map(|children| {
                    children
                        .iter()
                        .map(|s| SubagentTreeNode {
                            id: s.id.clone(),
                            parent_id: s.parent_id.clone(),
                            status: s.status,
                            mode: s.mode,
                            thread_id: s.thread_id.clone(),
                            children: if all_ids.contains(&s.id) {
                                build_tree(&s.id, by_parent, all_ids)
                            } else {
                                vec![]
                            },
                        })
                        .collect()
                })
                .unwrap_or_default()
        }

        build_tree(&root_parent_id, &by_parent, &all_ids)
    }
}
