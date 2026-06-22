//! Agent Control Plane (ACP) - Subagent Spawning System
//!
//! This provides:
//! - Subagent spawning with thread binding
//! - Runtime modes: "run" (one-shot) vs "session" (persistent)
//! - Session actor queue for serialized execution
//! - Parent-child agent communication

pub mod bus;
pub mod config;
pub mod control_plane;
pub mod controller;
pub mod session;
pub mod subagent;

pub use bus::{AcpBus, BusMessage};
pub use config::{
    AcpSessionId, AcpSessionStatus, CrashRecoveryConfig, ExecutionMode, RuntimeState, SpawnMode,
    SubagentConfig, SubagentStatus, ThreadBinding, ThreadContext, ThreadContextSummary,
};
pub use control_plane::{
    AcpAgentExt, AcpControlPlane, AcpSession, AcpSessionInfo, SubagentTreeNode,
};
pub use controller::ExecutionController;
pub use session::AcpCommand;
pub use subagent::{SubagentCommand, SubagentHandle, SubagentResponse};
