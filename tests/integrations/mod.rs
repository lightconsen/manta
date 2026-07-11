//! Integration Tests — Direct Tool::execute() Invocation
//!
//! Each file tests a category of tools without going through Gateway/WebSocket.

pub use std::sync::Arc;
pub use std::time::Duration;

pub use serde_json::json;
pub use syscity::tools::web::SearchProvider;
pub use syscity::tools::{
    AcpSessionTool, AcpSpawnTool, ApplyPatchTool, BrowserTool, CanvasTool, CodeExecutionTool,
    CronTool, DelegateTool, FileEditTool, FileReadTool, FileWriteTool, GlobTool, GrepTool,
    ImageGenerateTool, ImageTool, McpConnectionTool, MemoryGetTool, MemorySearchTool, MemoryTool,
    NodesTool, PdfTool, ProcessTool, SessionStatusTool, SessionsHistoryTool, SessionsListTool,
    SessionsSendTool, SessionsYieldTool, ShellTool, TimeTool, TodoTool, Tool, ToolContext, TtsTool,
    UpdatePlanTool, WebFetchTool, WebSearchTool,
};

/// Create a test ToolContext with a unique conversation_id to avoid cross-test
/// pollution.
pub fn test_context() -> ToolContext {
    ToolContext::new("test_user", format!("test-session-{}", std::process::id()))
        .with_timeout(Duration::from_secs(10))
        .with_workspace_only(false)
}

mod acp_tests;
#[cfg(feature = "browser")]
mod browser_tests;
mod computer_adapter_e2e_tests;
mod computer_modules_tests;
mod delegate_mcp_plan_tests;
mod execution_tests;
mod file_tests;
mod media_tests;
mod memory_tests;
mod message_tool_tests;
mod network_tests;
mod task_time_tests;
mod vision_tests;
mod web_tests;
