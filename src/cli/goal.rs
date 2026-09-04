//! Goal management commands for Syscity

use clap::Subcommand;
use serde_json::json;

use crate::cli::ws;
use crate::error::Result;

#[derive(Debug, Subcommand)]
pub enum GoalCommands {
    /// List running and suspended goals
    List,
    /// Resume a suspended goal from its checkpoint
    Resume {
        /// Goal ID (e.g. goal_ab12cd34-...)
        id: String,
    },
    /// Cancel a running goal, or discard a suspended one's checkpoint
    Cancel {
        /// Goal ID (e.g. goal_ab12cd34-...)
        id: String,
    },
}

/// Run goal commands (over WebSocket, via the `/goal` slash command).
pub async fn run_goal_command(command: &GoalCommands) -> Result<()> {
    let args = match command {
        GoalCommands::List => "list".to_string(),
        GoalCommands::Resume { id } => format!("resume {}", id),
        GoalCommands::Cancel { id } => format!("cancel {}", id),
    };
    match ws::call("commands.execute", json!({ "command": "goal", "args": args })).await {
        Ok(payload) => {
            println!("{}", payload);
            Ok(())
        }
        Err(e) => {
            eprintln!("Failed to execute goal command: {}", e);
            Err(e)
        }
    }
}
