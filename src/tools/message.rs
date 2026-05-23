//! Message delivery tool
//!
//! Send messages through channels via the gateway inbound pipeline.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

use crate::channels::IncomingMessage;
use crate::gateway::GatewayState;

use super::{Tool, ToolContext, ToolExecutionResult};

/// Send a message through a channel.
pub struct MessageTool {
    state: Arc<GatewayState>,
}

impl MessageTool {
    pub fn new(state: Arc<GatewayState>) -> Self {
        Self { state }
    }
}

#[derive(Debug, Deserialize)]
struct MessageArgs {
    channel: String,
    user_id: String,
    content: String,
}

#[async_trait]
impl Tool for MessageTool {
    fn name(&self) -> &str {
        "message"
    }

    fn description(&self) -> &str {
        "Send a message through a channel (e.g., telegram, discord). The message is injected into the inbound pipeline for processing."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "channel": {
                    "type": "string",
                    "description": "Channel name (e.g., 'telegram', 'discord', 'web')"
                },
                "user_id": {
                    "type": "string",
                    "description": "User ID sending the message"
                },
                "content": {
                    "type": "string",
                    "description": "Message content"
                }
            },
            "required": ["channel", "user_id", "content"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();
        let args: MessageArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Invalid arguments: {}", e)),
                    data: None,
                    execution_time: start.elapsed(),
                });
            }
        };

        let incoming = IncomingMessage::new(
            args.user_id.clone(),
            format!("{}:{}", args.channel, args.user_id),
            args.content,
        )
        .with_provenance(crate::channels::InputProvenance::ExternalUser {
            channel: args.channel.clone(),
            is_direct: true,
        });

        match self.state.inbound_pipeline.process(incoming).await {
            Some(_) => Ok(ToolExecutionResult {
                success: true,
                output: format!("Message sent to {} channel", args.channel),
                error: None,
                data: Some(serde_json::json!({
                    "channel": args.channel,
                    "user_id": args.user_id,
                })),
                execution_time: start.elapsed(),
            }),
            None => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some("Failed to route message: pipeline returned None".to_string()),
                data: None,
                execution_time: start.elapsed(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_args_parsing() {
        let args: MessageArgs = serde_json::from_value(serde_json::json!({
            "channel": "telegram",
            "user_id": "user1",
            "content": "hello"
        }))
        .unwrap();
        assert_eq!(args.channel, "telegram");
        assert_eq!(args.user_id, "user1");
        assert_eq!(args.content, "hello");
    }
}
