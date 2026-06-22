use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::channels::IncomingMessage;

use super::config::{AcpSessionId, SpawnMode, SubagentStatus};
use super::controller::ExecutionController;

/// Subagent handle - reference to a spawned subagent
#[derive(Debug, Clone)]
pub struct SubagentHandle {
    /// Subagent ID
    pub id: String,
    /// Parent agent ID
    pub parent_id: String,
    /// ACP Session ID
    pub session_id: AcpSessionId,
    /// Spawn mode
    pub mode: SpawnMode,
    /// Thread ID this agent is bound to
    pub thread_id: String,
    /// Command channel to subagent
    pub command_tx: mpsc::Sender<SubagentCommand>,
    /// Current status
    pub status: SubagentStatus,
    /// Execution controller for runtime pause/resume/step/cancel
    pub controller: Arc<ExecutionController>,
    /// Abort handle for force-killing the subagent task
    pub abort_handle: tokio::task::AbortHandle,
    /// Number of times this subagent has been restarted after a crash
    pub crash_count: u32,
}

/// Commands that can be sent to a subagent
#[derive(Debug)]
pub enum SubagentCommand {
    /// Process a message
    ProcessMessage {
        message: Box<IncomingMessage>,
        response_tx: oneshot::Sender<crate::Result<String>>,
    },
    /// Cancel current operation
    Cancel,
    /// Shutdown the subagent
    Shutdown,
}
