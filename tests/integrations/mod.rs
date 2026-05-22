//! Integration Tests — Direct Tool::execute() Invocation
//!
//! Each file tests a category of tools without going through Gateway/WebSocket.

pub use manta::tools::{
    AcpSessionTool, AcpSpawnTool, ApplyPatchTool, BrowserTool, CanvasTool, CodeExecutionTool,
    CronTool, DelegateTool, FileEditTool, FileReadTool, FileWriteTool, GlobTool, GrepTool,
    ImageGenerateTool, ImageTool, McpConnectionTool, MemoryGetTool, MemorySearchTool,
    MemoryTool, NodesTool, PdfTool, ProcessTool, SessionStatusTool,
    SessionsHistoryTool, SessionsListTool, SessionsSendTool, SessionsYieldTool, ShellTool,
    TimeTool,
    TodoTool, Tool, ToolContext, TtsTool, UpdatePlanTool, WebFetchTool, WebSearchTool,
};
pub use serde_json::json;
pub use std::sync::Arc;
pub use std::time::Duration;

/// Create a test ToolContext with a unique conversation_id to avoid cross-test pollution.
pub fn test_context() -> ToolContext {
    ToolContext::new("test_user", format!("test-session-{}", std::process::id()))
        .with_timeout(Duration::from_secs(10))
}

mod file_tests;
mod execution_tests;
mod web_tests;
mod task_time_tests;
mod memory_tests;
mod acp_tests;
mod delegate_mcp_plan_tests;
mod media_tests;
mod network_tests;
