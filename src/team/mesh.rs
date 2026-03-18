//! Team Mesh Integration Module
//!
//! Integrates AssistantMesh with the Team system to enable runtime-enforced
//! communication patterns for agent teams.

use super::{CommunicationPattern, Team, TeamMember};
use crate::assistants::mesh::{AssistantMesh, MeshMessage, MessageType};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Team mesh manager - manages mesh communication for teams
#[derive(Debug)]
pub struct TeamMeshManager {
    /// The underlying assistant mesh
    mesh: AssistantMesh,
    /// Active team sessions (team_id -> TeamMeshSession)
    team_sessions: Arc<RwLock<HashMap<String, TeamMeshSession>>>,
}

/// Team mesh session - tracks active team communication state
#[derive(Debug, Clone)]
pub struct TeamMeshSession {
    /// Team ID
    pub team_id: String,
    /// Team name
    pub team_name: String,
    /// Communication pattern
    pub pattern: CommunicationPattern,
    /// Shared memory/canvas name
    pub shared_memory: Option<String>,
    /// Registered agent IDs
    pub agents: Vec<String>,
    /// Hierarchy: manager -> [workers]
    pub hierarchy: HashMap<String, Vec<String>>,
    /// Team leads (for star pattern)
    pub leads: Vec<String>,
    /// Communication chain order (for chain pattern)
    pub chain_order: Vec<String>,
}

/// Result of sending a team message
#[derive(Debug, Clone)]
pub struct TeamMessageResult {
    /// Message ID
    pub message_id: String,
    /// Recipients who received the message
    pub recipients: Vec<String>,
    /// Pattern used
    pub pattern: CommunicationPattern,
}

impl TeamMeshManager {
    /// Create a new team mesh manager
    pub fn new() -> Self {
        Self {
            mesh: AssistantMesh::new(),
            team_sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Activate a team - register all agents with the mesh
    pub async fn activate_team(&self, team: &Team) -> crate::Result<TeamMeshSession> {
        let team_id = team.name.clone();

        // Check if already active
        {
            let sessions = self.team_sessions.read().await;
            if sessions.contains_key(&team_id) {
                return Err(crate::error::MantaError::Validation(
                    format!("Team '{}' is already active", team_id)
                ));
            }
        }

        // Get team leads (agents at level 0 or those in hierarchy)
        let leads: Vec<String> = team
            .members
            .values()
            .filter(|m| m.level == 0 || team.hierarchy.contains_key(&m.name))
            .map(|m| m.name.clone())
            .collect();

        // Build chain order (sorted by level, then by name for stability)
        let mut chain_order: Vec<(u8, String)> = team
            .members
            .values()
            .map(|m| (m.level, m.name.clone()))
            .collect();
        chain_order.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let chain_order: Vec<String> = chain_order.into_iter().map(|(_, name)| name).collect();

        // Create session
        let session = TeamMeshSession {
            team_id: team_id.clone(),
            team_name: team.name.clone(),
            pattern: team.communication,
            shared_memory: team.shared_memory.clone(),
            agents: team.members.keys().cloned().collect(),
            hierarchy: team.hierarchy.clone(),
            leads,
            chain_order,
        };

        // Register all agents with the mesh
        for agent_name in &session.agents {
            let rx = self.mesh.register(format!("{}:{}", team_id, agent_name)).await;

            // Spawn message receiver for this agent
            let team_id_clone = team_id.clone();
            let agent_name_clone = agent_name.clone();
            tokio::spawn(async move {
                Self::agent_message_receiver(
                    team_id_clone,
                    agent_name_clone,
                    rx,
                ).await;
            });

            info!("Registered agent '{}' with team '{}' mesh", agent_name, team_id);
        }

        // Store session
        {
            let mut sessions = self.team_sessions.write().await;
            sessions.insert(team_id.clone(), session.clone());
        }

        info!(
            "Activated team '{}' with {} agents, pattern: {:?}",
            team_id,
            session.agents.len(),
            session.pattern
        );

        Ok(session)
    }

    /// Deactivate a team - unregister all agents
    pub async fn deactivate_team(&self, team_id: &str) -> crate::Result<()> {
        let session = {
            let mut sessions = self.team_sessions.write().await;
            sessions.remove(team_id)
        };

        if let Some(session) = session {
            for agent_name in &session.agents {
                let mesh_id = format!("{}:{}", team_id, agent_name);
                self.mesh.unregister(&mesh_id).await;
                info!("Unregistered agent '{}' from team '{}' mesh", agent_name, team_id);
            }

            info!("Deactivated team '{}' with {} agents", team_id, session.agents.len());
        }

        Ok(())
    }

    /// Send a message within a team (respects communication pattern)
    pub async fn send_team_message(
        &self,
        team_id: &str,
        from_agent: &str,
        to_agent: Option<&str>,
        content: &str,
    ) -> crate::Result<TeamMessageResult> {
        let session = {
            let sessions = self.team_sessions.read().await;
            sessions
                .get(team_id)
                .cloned()
                .ok_or_else(|| crate::error::MantaError::NotFound {
                    resource: format!("Active team '{}'", team_id),
                })?
        };

        // Validate sender is in team
        if !session.agents.contains(&from_agent.to_string()) {
            return Err(crate::error::MantaError::Validation(format!(
                "Agent '{}' is not a member of team '{}'",
                from_agent, team_id
            )));
        }

        // Apply communication pattern rules
        match session.pattern {
            CommunicationPattern::Mesh => {
                // Full mesh - direct messages allowed to anyone
                self.send_direct_or_broadcast(&session, from_agent, to_agent, content).await
            }
            CommunicationPattern::Broadcast => {
                // Broadcast only - all messages go to everyone
                self.broadcast_to_team(&session, from_agent, content).await
            }
            CommunicationPattern::Star => {
                // Star pattern - messages must go through leads
                self.send_star_pattern(&session, from_agent, to_agent, content).await
            }
            CommunicationPattern::Chain => {
                // Chain pattern - messages only to next in chain
                self.send_chain_pattern(&session, from_agent, content).await
            }
        }
    }

    /// Send a direct message or broadcast (for Mesh pattern)
    async fn send_direct_or_broadcast(
        &self,
        session: &TeamMeshSession,
        from: &str,
        to: Option<&str>,
        content: &str,
    ) -> crate::Result<TeamMessageResult> {
        let from_mesh_id = format!("{}:{}", session.team_id, from);

        if let Some(to_agent) = to {
            // Direct message
            if !session.agents.contains(&to_agent.to_string()) {
                return Err(crate::error::MantaError::Validation(format!(
                    "Recipient '{}' is not in team '{}'",
                    to_agent, session.team_id
                )));
            }

            let to_mesh_id = format!("{}:{}", session.team_id, to_agent);
            let msg_id = self.mesh.send(&from_mesh_id, &to_mesh_id, content).await?;

            Ok(TeamMessageResult {
                message_id: msg_id,
                recipients: vec![to_agent.to_string()],
                pattern: CommunicationPattern::Mesh,
            })
        } else {
            // Broadcast to all
            self.broadcast_to_team(session, from, content).await
        }
    }

    /// Broadcast to all team members
    async fn broadcast_to_team(
        &self,
        session: &TeamMeshSession,
        from: &str,
        content: &str,
    ) -> crate::Result<TeamMessageResult> {
        let from_mesh_id = format!("{}:{}", session.team_id, from);

        // Send to each agent individually (for tracking)
        let mut recipients = vec![];
        for agent in &session.agents {
            if agent != from {
                let to_mesh_id = format!("{}:{}", session.team_id, agent);
                let _ = self.mesh.send(&from_mesh_id, &to_mesh_id, content).await;
                recipients.push(agent.clone());
            }
        }

        let msg_id = uuid::Uuid::new_v4().to_string();

        Ok(TeamMessageResult {
            message_id: msg_id,
            recipients,
            pattern: CommunicationPattern::Broadcast,
        })
    }

    /// Send using star pattern (through leads)
    async fn send_star_pattern(
        &self,
        session: &TeamMeshSession,
        from: &str,
        to: Option<&str>,
        content: &str,
    ) -> crate::Result<TeamMessageResult> {
        let from_mesh_id = format!("{}:{}", session.team_id, from);

        // Check if sender is a lead
        let is_lead = session.leads.contains(&from.to_string());

        if is_lead {
            // Lead can send to anyone
            if let Some(to_agent) = to {
                if !session.agents.contains(&to_agent.to_string()) {
                    return Err(crate::error::MantaError::Validation(format!(
                        "Recipient '{}' is not in team '{}'",
                        to_agent, session.team_id
                    )));
                }
                let to_mesh_id = format!("{}:{}", session.team_id, to_agent);
                let msg_id = self.mesh.send(&from_mesh_id, &to_mesh_id, content).await?;

                Ok(TeamMessageResult {
                    message_id: msg_id,
                    recipients: vec![to_agent.to_string()],
                    pattern: CommunicationPattern::Star,
                })
            } else {
                // Lead broadcasting to all
                self.broadcast_to_team(session, from, content).await
            }
        } else {
            // Non-lead can only send to leads
            if let Some(to_agent) = to {
                if !session.leads.contains(&to_agent.to_string()) {
                    return Err(crate::error::MantaError::Validation(format!(
                        "In star pattern, non-lead agents can only message leads. '{}' is not a lead.",
                        to_agent
                    )));
                }
                let to_mesh_id = format!("{}:{}", session.team_id, to_agent);
                let msg_id = self.mesh.send(&from_mesh_id, &to_mesh_id, content).await?;

                Ok(TeamMessageResult {
                    message_id: msg_id,
                    recipients: vec![to_agent.to_string()],
                    pattern: CommunicationPattern::Star,
                })
            } else {
                // Broadcast to all leads
                let mut recipients = vec![];
                for lead in &session.leads {
                    let to_mesh_id = format!("{}:{}", session.team_id, lead);
                    let _ = self.mesh.send(&from_mesh_id, &to_mesh_id, content).await;
                    recipients.push(lead.clone());
                }

                let msg_id = uuid::Uuid::new_v4().to_string();

                Ok(TeamMessageResult {
                    message_id: msg_id,
                    recipients,
                    pattern: CommunicationPattern::Star,
                })
            }
        }
    }

    /// Send using chain pattern (to next in chain)
    async fn send_chain_pattern(
        &self,
        session: &TeamMeshSession,
        from: &str,
        content: &str,
    ) -> crate::Result<TeamMessageResult> {
        // Find sender position in chain
        let sender_pos = session
            .chain_order
            .iter()
            .position(|a| a == from)
            .ok_or_else(|| crate::error::MantaError::Validation(format!(
                "Agent '{}' not found in team '{}'",
                from, session.team_id
            )))?;

        // Can only send to next in chain
        if sender_pos + 1 >= session.chain_order.len() {
            return Err(crate::error::MantaError::Validation(
                "You are at the end of the chain - no one to send to".to_string()
            ));
        }

        let next_agent = &session.chain_order[sender_pos + 1];
        let from_mesh_id = format!("{}:{}", session.team_id, from);
        let to_mesh_id = format!("{}:{}", session.team_id, next_agent);

        let msg_id = self.mesh.send(&from_mesh_id, &to_mesh_id, content).await?;

        Ok(TeamMessageResult {
            message_id: msg_id,
            recipients: vec![next_agent.clone()],
            pattern: CommunicationPattern::Chain,
        })
    }

    /// Get active team session
    pub async fn get_session(&self, team_id: &str) -> Option<TeamMeshSession> {
        let sessions = self.team_sessions.read().await;
        sessions.get(team_id).cloned()
    }

    /// List all active teams
    pub async fn list_active_teams(&self) -> Vec<String> {
        let sessions = self.team_sessions.read().await;
        sessions.keys().cloned().collect()
    }

    /// Check if a team is active
    pub async fn is_team_active(&self, team_id: &str) -> bool {
        let sessions = self.team_sessions.read().await;
        sessions.contains_key(team_id)
    }

    /// Get message history for a team
    pub async fn get_team_history(&self, team_id: &str) -> Vec<MeshMessage> {
        let session = match self.get_session(team_id).await {
            Some(s) => s,
            None => return vec![],
        };

        let history = self.mesh.get_history().await;

        // Filter to only this team's messages
        history
            .into_iter()
            .filter(|m| {
                m.from.starts_with(&format!("{}:", team_id))
                    || m.to.as_ref().map(|t| t.starts_with(&format!("{}:", team_id))).unwrap_or(false)
            })
            .collect()
    }

    /// Message receiver for an agent
    async fn agent_message_receiver(
        team_id: String,
        agent_name: String,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<MeshMessage>,
    ) {
        debug!("Started message receiver for {}:{} ", team_id, agent_name);

        while let Some(msg) = rx.recv().await {
            info!(
                "Team message to {}:{} from {}: {} chars",
                team_id,
                agent_name,
                msg.from,
                msg.content.len()
            );

            // Here you could:
            // 1. Store in agent's incoming message queue
            // 2. Trigger agent processing
            // 3. Notify via WebSocket/Channel

            // For now, just log - the agent would need to poll or have a callback
            debug!("Message content: {}", msg.content);
        }

        debug!("Message receiver for {}:{} stopped", team_id, agent_name);
    }
}

impl Default for TeamMeshManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Global team mesh manager instance
static TEAM_MESH_MANAGER: tokio::sync::OnceCell<TeamMeshManager> = tokio::sync::OnceCell::const_new();

/// Get or initialize the global team mesh manager
pub async fn get_team_mesh_manager() -> &'static TeamMeshManager {
    TEAM_MESH_MANAGER.get_or_init(|| async { TeamMeshManager::new() }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_team_mesh_manager_creation() {
        let manager = TeamMeshManager::new();
        let teams = manager.list_active_teams().await;
        assert!(teams.is_empty());
    }

    #[test]
    fn test_team_session_creation() {
        let session = TeamMeshSession {
            team_id: "test-team".to_string(),
            team_name: "Test Team".to_string(),
            pattern: CommunicationPattern::Mesh,
            shared_memory: None,
            agents: vec!["agent1".to_string(), "agent2".to_string()],
            hierarchy: HashMap::new(),
            leads: vec!["agent1".to_string()],
            chain_order: vec!["agent1".to_string(), "agent2".to_string()],
        };

        assert_eq!(session.team_id, "test-team");
        assert_eq!(session.agents.len(), 2);
    }
}
