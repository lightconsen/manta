//! Team Communication Tool
//!
//! Enables agents to send messages to other agents within their team,
//! respecting the team's configured communication pattern.

use async_trait::async_trait;
use serde_json::json;
use tracing::{debug, error, info, warn};

use crate::team::mesh::{get_team_mesh_manager, TeamMeshManager};
use crate::tools::{Tool, ToolContext, ToolExecutionResult};

/// Tool for team communication
#[derive(Debug)]
pub struct TeamCommunicateTool;

impl TeamCommunicateTool {
    /// Create a new team communication tool
    pub fn new() -> Self {
        Self
    }
}

impl Default for TeamCommunicateTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TeamCommunicateTool {
    fn name(&self) -> &str {
        "team_communicate"
    }

    fn description(&self) -> &str {
        r#"Send messages to other agents within your team.

This tool respects your team's communication pattern:
- Mesh: Send to any team member
- Star: Non-leads can only message leads; leads can message anyone
- Chain: Messages flow sequentially through team members
- Broadcast: All messages go to entire team

Use this to coordinate with teammates, delegate tasks, or share information.

Examples:
- Send update to lead: {"action": "send", "to": "lead-agent", "message": "Task complete"}
- Broadcast to all: {"action": "broadcast", "message": "Starting phase 2"}
- Check team status: {"action": "status"}
- List teammates: {"action": "list"}"#
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["send", "broadcast", "status", "list", "history"],
                    "description": "Action to perform"
                },
                "to": {
                    "type": "string",
                    "description": "Recipient agent name (for send action)"
                },
                "message": {
                    "type": "string",
                    "description": "Message content to send"
                },
                "team": {
                    "type": "string",
                    "description": "Team name (optional, auto-detected from context if not provided)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let action = args["action"].as_str().ok_or_else(|| {
            crate::error::MantaError::Validation("action is required".to_string())
        })?;

        // Get team name from args or context
        let team_name = args["team"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                crate::error::MantaError::Validation(
                    "Team name is required. Use 'team' parameter.".to_string(),
                )
            })?;

        // Get sender agent name from context
        let sender = context.conversation_id.clone();
        // In a real implementation, you'd map conversation_id to agent name
        // For now, we'll use a simplified approach
        let sender_agent = sender.split(':').last().unwrap_or(&sender).to_string();

        let mesh_manager = get_team_mesh_manager().await;

        match action {
            "send" => {
                let to = args["to"].as_str().ok_or_else(|| {
                    crate::error::MantaError::Validation(
                        "'to' is required for send action".to_string(),
                    )
                })?;

                let message = args["message"].as_str().ok_or_else(|| {
                    crate::error::MantaError::Validation(
                        "'message' is required for send action".to_string(),
                    )
                })?;

                info!(
                    "Team message from {} to {} in team {}: {} chars",
                    sender_agent,
                    to,
                    team_name,
                    message.len()
                );

                let result = mesh_manager
                    .send_team_message(&team_name, &sender_agent, Some(to), message)
                    .await?;

                Ok(ToolExecutionResult::success(format!(
                    "Message sent to {} using {:?} pattern",
                    result.recipients.join(", "),
                    result.pattern
                ))
                .with_data(json!({
                    "message_id": result.message_id,
                    "recipients": result.recipients,
                    "pattern": format!("{:?}", result.pattern)
                })))
            }

            "broadcast" => {
                let message = args["message"].as_str().ok_or_else(|| {
                    crate::error::MantaError::Validation(
                        "'message' is required for broadcast action".to_string(),
                    )
                })?;

                info!(
                    "Team broadcast from {} in team {}: {} chars",
                    sender_agent,
                    team_name,
                    message.len()
                );

                let result = mesh_manager
                    .send_team_message(&team_name, &sender_agent, None, message)
                    .await?;

                Ok(ToolExecutionResult::success(format!(
                    "Broadcast sent to {} team members using {:?} pattern",
                    result.recipients.len(),
                    result.pattern
                ))
                .with_data(json!({
                    "message_id": result.message_id,
                    "recipient_count": result.recipients.len(),
                    "recipients": result.recipients,
                    "pattern": format!("{:?}", result.pattern)
                })))
            }

            "status" => {
                let session = mesh_manager.get_session(&team_name).await;

                if let Some(session) = session {
                    Ok(ToolExecutionResult::success(format!(
                        "Team '{}' is active with {} agents, pattern: {:?}",
                        session.team_name,
                        session.agents.len(),
                        session.pattern
                    ))
                    .with_data(json!({
                        "team_id": session.team_id,
                        "team_name": session.team_name,
                        "pattern": format!("{:?}", session.pattern),
                        "agent_count": session.agents.len(),
                        "agents": session.agents,
                        "leads": session.leads,
                        "active": true
                    })))
                } else {
                    Ok(ToolExecutionResult::success(format!(
                        "Team '{}' is not currently active in the mesh",
                        team_name
                    ))
                    .with_data(json!({
                        "team_id": team_name,
                        "active": false
                    })))
                }
            }

            "list" => {
                let session = mesh_manager.get_session(&team_name).await;

                if let Some(session) = session {
                    let agent_details: Vec<serde_json::Value> = session
                        .agents
                        .iter()
                        .map(|a| {
                            let is_lead = session.leads.contains(a);
                            json!({
                                "name": a,
                                "role": if is_lead { "lead" } else { "member" },
                                "can_message": match session.pattern {
                                    crate::team::CommunicationPattern::Star if !is_lead => {
                                        "leads only"
                                    }
                                    _ => "anyone"
                                }
                            })
                        })
                        .collect();

                    Ok(ToolExecutionResult::success(format!(
                        "Team '{}' has {} agents",
                        session.team_name,
                        session.agents.len()
                    ))
                    .with_data(json!({
                        "team": session.team_name,
                        "pattern": format!("{:?}", session.pattern),
                        "agents": agent_details
                    })))
                } else {
                    // Try to load from storage
                    match crate::team::Team::load(&team_name).await {
                        Ok(team) => Ok(ToolExecutionResult::success(format!(
                            "Team '{}' found (not active). Activate with 'manta team activate {}'",
                            team.name, team.name
                        ))
                        .with_data(json!({
                            "team": team.name,
                            "active": false,
                            "member_count": team.members.len(),
                            "members": team.members.keys().cloned().collect::<Vec<_>>()
                        }))),
                        Err(e) => Err(crate::error::MantaError::NotFound {
                            resource: format!("Team '{}'", team_name),
                        }),
                    }
                }
            }

            "history" => {
                let history = mesh_manager.get_team_history(&team_name).await;

                if history.is_empty() {
                    Ok(ToolExecutionResult::success("No messages in team history yet")
                        .with_data(json!({"messages": []})))
                } else {
                    let messages: Vec<serde_json::Value> = history
                        .into_iter()
                        .map(|m| {
                            json!({
                                "id": m.id,
                                "from": m.from,
                                "to": m.to,
                                "content": m.content,
                                "type": format!("{:?}", m.msg_type),
                                "timestamp": m.timestamp
                            })
                        })
                        .collect();

                    Ok(ToolExecutionResult::success(format!(
                        "Retrieved {} messages from team history",
                        messages.len()
                    ))
                    .with_data(json!({"messages": messages})))
                }
            }

            _ => Err(crate::error::MantaError::Validation(format!(
                "Unknown action: '{}'. Use: send, broadcast, status, list, history",
                action
            ))),
        }
    }
}
