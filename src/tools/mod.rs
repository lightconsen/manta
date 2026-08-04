//! Tool abstractions for Syscity
//!
//! Tools are capabilities that the AI assistant can use to interact
//! with the world (execute shell commands, read files, search the web, etc.).

pub mod approval;
pub mod eval;
pub mod rbac;

// Re-export approval types for convenience
pub use approval::{
    ApprovalDecision, ApprovalFilter, ApprovalLevel, ApprovalQueue, ApprovalRequiredEvent,
    PendingApproval, PendingApprovalSummary, RiskLevel,
};
// Re-export RBAC types for convenience
pub use rbac::{
    ModelCapabilities, PolicyEvaluationContext, Role, SandboxPolicy, ToolPolicy, UserContext,
};

mod registrar;
mod registry;
mod types;
mod util;
mod validators;

pub use registrar::ToolRegistrar;
pub use registry::{ToolRegistry, WebSearchProviders};
pub use types::{
    BoxedTool, SharedTool, SkillTrust, Tool, ToolContext, ToolExecutionChunk, ToolExecutionResult,
    ToolId, ToolIdentity, ToolModel, ToolSandbox,
};
pub use util::create_schema;
pub use validators::{
    NameValidator, SchemaValidator, SecurityValidator, ToolValidationError, ToolValidator,
};

pub mod acp_tool;
pub mod agents_list;
pub mod browser;
pub mod canvas;
pub mod code_exec;
pub mod command_detector;
pub mod command_gate;
pub mod computer;
pub mod cron_tool;
pub mod delegate_tool;
pub mod file;
pub mod gateway;
pub mod grep;
pub mod heartbeat_tool;
pub mod hooks;
pub mod image;
pub mod list_capabilities;
pub mod memory;
pub mod message;
pub mod nodes;
pub mod patch;
pub mod pdf;
pub mod planner;
pub mod process;
pub mod process_runner;
pub mod report;
pub mod sandbox;
pub mod sandbox_interceptor;
pub mod screen_state;
pub mod sdk;
pub mod session;
pub mod shell;
pub mod shell_safety;
pub mod stt;
pub mod time;
pub mod todo_tool;
pub mod tts;
pub mod update_plan;
pub mod web;

pub use acp_tool::{AcpSessionTool, AcpSpawnTool};
pub use agents_list::AgentsListTool;
pub use browser::BrowserTool;
pub use canvas::CanvasTool;
pub use code_exec::CodeExecutionTool;
pub use cron_tool::CronTool;
pub use delegate_tool::{AgentResolver, DelegateTool};
pub use file::{FileEditTool, FileReadTool, FileWriteTool, GlobTool};
pub use gateway::GatewayTool;
pub use grep::GrepTool;
pub use heartbeat_tool::HeartbeatTool;
pub use hooks::{ToolHooks, ToolPolicyDecision};
pub use image::{ImageGenerateTool, ImageTool};
pub use list_capabilities::ListCapabilitiesTool;
pub use memory::{MemoryGetTool, MemorySearchTool, MemoryTool};
pub use message::MessageTool;
pub use nodes::NodesTool;
pub use patch::ApplyPatchTool;
pub use pdf::PdfTool;
pub use process::ProcessTool;
pub use process_runner::{CommandOutput, ProcessError, ProcessRequest, ProcessRunner, StdioMode};
pub use report::WriteReportTool;
pub use sandbox::{SandboxConfig, SandboxedTool};
pub use sdk::{
    CapabilityFilter, SyncResult, ToolCapabilities, ToolMetadata, ToolPack, ToolSdk, ToolSdkError,
};
pub use session::{
    SessionStatusTool, SessionsHistoryTool, SessionsListTool, SessionsSendTool, SessionsYieldTool,
};
pub use shell::ShellTool;
pub use stt::SttTool;
pub use time::TimeTool;
pub use todo_tool::TodoTool;
pub use tts::TtsTool;
pub use update_plan::UpdatePlanTool;
pub use web::{WebFetchTool, WebSearchTool};

// Re-exported from the top-level mcp module for backward compatibility.
pub use crate::mcp::{McpConnectionTool, McpManager};

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use async_trait::async_trait;
    use serde_json::Value;

    use super::*;

    #[test]
    fn test_tool_id() {
        let id = ToolId::new("test_tool");
        assert_eq!(id.0, "test_tool");
    }

    #[test]
    fn test_tool_context() {
        let ctx = ToolContext::new("user1", "conv1")
            .with_timeout(Duration::from_secs(60))
            .allow_path("/tmp")
            .allow_command("ls");

        assert_eq!(ctx.user_id, "user1");
        assert_eq!(ctx.timeout(), Duration::from_secs(60));
        assert!(ctx.is_command_allowed("ls"));
        assert!(!ctx.is_command_allowed("rm"));
    }

    #[test]
    fn test_tool_execution_result() {
        let success = ToolExecutionResult::success("Done!");
        assert!(success.success);
        assert_eq!(success.output, "Done!");

        let error = ToolExecutionResult::error("Failed!");
        assert!(!error.success);
        assert_eq!(error.error, Some("Failed!".to_string()));
    }

    #[test]
    fn test_tool_registry() {
        let registry = ToolRegistry::new();
        assert!(registry.list().is_empty());
        assert!(!registry.has("test"));
    }

    #[test]
    fn test_create_schema() {
        let schema = create_schema(
            "A test tool",
            serde_json::json!({
                "name": { "type": "string" },
                "count": { "type": "integer" }
            }),
            vec!["name"],
        );

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["description"], "A test tool");
        assert_eq!(schema["required"], serde_json::json!(["name"]));
    }

    // ToolRegistrar tests

    #[test]
    fn test_tool_registrar_creation() {
        let registrar = ToolRegistrar::new();
        assert!(registrar.list().is_empty());
    }

    #[test]
    fn test_name_validator_valid() {
        struct ValidTool;

        #[async_trait]
        impl Tool for ValidTool {
            fn name(&self) -> &str {
                "valid_tool"
            }
            fn description(&self) -> &str {
                "A valid test tool"
            }
            fn parameters_schema(&self) -> Value {
                create_schema("Test", serde_json::json!({}), Vec::<String>::new())
            }
            async fn execute(
                &self,
                _args: Value,
                _ctx: &ToolContext,
            ) -> crate::Result<ToolExecutionResult> {
                Ok(ToolExecutionResult::success("ok"))
            }
        }

        let validator = NameValidator;
        assert!(validator.validate(&ValidTool).is_ok());
    }

    #[test]
    fn test_name_validator_invalid() {
        struct InvalidTool;

        #[async_trait]
        impl Tool for InvalidTool {
            fn name(&self) -> &str {
                "123_invalid"
            }
            fn description(&self) -> &str {
                "A test tool"
            }
            fn parameters_schema(&self) -> Value {
                create_schema("Test", serde_json::json!({}), Vec::<String>::new())
            }
            async fn execute(
                &self,
                _args: Value,
                _ctx: &ToolContext,
            ) -> crate::Result<ToolExecutionResult> {
                Ok(ToolExecutionResult::success("ok"))
            }
        }

        let validator = NameValidator;
        let result = validator.validate(&InvalidTool);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ToolValidationError::InvalidName(_)));
    }

    #[test]
    fn test_security_validator_path_traversal() {
        let validator = SecurityValidator;

        // Valid paths
        assert!(validator
            .check_path_traversal("/home/user/file.txt")
            .is_ok());
        assert!(validator.check_path_traversal("./file.txt").is_ok());

        // Invalid paths with traversal
        assert!(validator.check_path_traversal("../etc/passwd").is_err());
        assert!(validator
            .check_path_traversal("foo/../../../etc/passwd")
            .is_err());
    }

    #[test]
    fn test_security_validator_command_injection() {
        let validator = SecurityValidator;

        // Valid commands
        assert!(validator.check_command_injection("ls -la").is_ok());
        assert!(validator.check_command_injection("cat file.txt").is_ok());

        // Invalid commands with injection
        assert!(validator.check_command_injection("ls; rm -rf /").is_err());
        assert!(validator
            .check_command_injection("cat file | grep test")
            .is_err());
        assert!(validator.check_command_injection("echo $(whoami)").is_err());
    }

    #[test]
    fn test_security_validator_input_validation() {
        let validator = SecurityValidator;

        // Valid input
        let valid_args = serde_json::json!({
            "path": "/home/user/file.txt",
            "content": "hello world"
        });
        assert!(validator.validate_input("test", &valid_args).is_ok());

        // Invalid input with path traversal
        let invalid_args = serde_json::json!({
            "path": "../../../etc/passwd",
            "content": "malicious"
        });
        assert!(validator.validate_input("test", &invalid_args).is_err());

        // Invalid input with command injection
        let cmd_inject_args = serde_json::json!({
            "command": "ls; rm -rf /"
        });
        assert!(validator.validate_input("test", &cmd_inject_args).is_err());
    }

    // ── Path sandbox penetration tests ───────────────────────────────────────

    use tempfile::tempdir;

    fn workspace_context(root: &std::path::Path) -> ToolContext {
        ToolContext::new("user", "conv1")
            .with_workspace_root(root)
            .with_workspace_only(true)
    }

    #[test]
    fn test_path_allowed_relative_within_workspace() {
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir(&ws).unwrap();
        let ctx = workspace_context(&ws);
        assert!(ctx.is_path_allowed(std::path::Path::new("notes.txt")));
    }

    #[test]
    fn test_path_allowed_relative_traversal_outside_workspace() {
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir(&ws).unwrap();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        let ctx = workspace_context(&ws);
        assert!(!ctx.is_path_allowed(std::path::Path::new("../outside.txt")));
    }

    #[test]
    fn test_path_allowed_dotdot_combo_traversal() {
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir_all(ws.join("subdir")).unwrap();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        let ctx = workspace_context(&ws);
        assert!(!ctx.is_path_allowed(std::path::Path::new("subdir/../../outside.txt")));
    }

    #[test]
    fn test_path_allowed_absolute_outside_workspace() {
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir(&ws).unwrap();
        let ctx = workspace_context(&ws);
        assert!(!ctx.is_path_allowed(std::path::Path::new("/etc/passwd")));
    }

    #[test]
    fn test_path_allowed_absolute_inside_workspace() {
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir(&ws).unwrap();
        let inside = ws.join("file.txt");
        std::fs::write(&inside, "ok").unwrap();
        let ctx = workspace_context(&ws);
        assert!(ctx.is_path_allowed(&inside));
    }

    #[cfg(unix)]
    #[test]
    fn test_path_allowed_symlink_escape() {
        use std::os::unix::fs::symlink;
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir(&ws).unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let secret = outside.join("secret.txt");
        std::fs::write(&secret, "secret").unwrap();
        symlink(&outside, ws.join("link")).unwrap();
        let ctx = workspace_context(&ws);
        assert!(!ctx.is_path_allowed(std::path::Path::new("link/secret.txt")));
    }

    #[test]
    fn test_path_allowed_tilde_expansion_outside_workspace() {
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir(&ws).unwrap();
        let ctx = workspace_context(&ws);
        // `~` resolves to the user's home directory, which is outside the workspace.
        assert!(!ctx.is_path_allowed(std::path::Path::new("~/anything.txt")));
    }

    #[test]
    fn test_path_allowed_with_allowlist() {
        let tmp = tempdir().unwrap();
        let allowed = tmp.path().join("allowed");
        std::fs::create_dir(&allowed).unwrap();
        let ctx = ToolContext::new("user", "conv1")
            .with_workspace_only(false)
            .allow_path(&allowed);
        let inside = allowed.join("file.txt");
        assert!(ctx.is_path_allowed(&inside));
        assert!(!ctx.is_path_allowed(std::path::Path::new("/etc/passwd")));
    }

    // ── Skill trust boundary tests ───────────────────────────────────────────

    struct ReadTool;

    #[async_trait]
    impl Tool for ReadTool {
        fn name(&self) -> &str {
            "read"
        }

        fn description(&self) -> &str {
            "Read a file"
        }

        fn parameters_schema(&self) -> Value {
            create_schema(
                "Read a file",
                serde_json::json!({"path": {"type": "string"}}),
                vec!["path"],
            )
        }

        async fn execute(
            &self,
            _args: Value,
            _ctx: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            Ok(ToolExecutionResult::success("ok"))
        }
    }

    #[test]
    fn test_skill_trust_boundary_excludes_privileged_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ShellTool::new()));
        registry.register(Box::new(ReadTool));
        registry.mark_privileged("shell");

        let trusted_ctx = ToolContext::new("user", "conv1").with_skill_trust(SkillTrust::Trusted);
        let community_ctx =
            ToolContext::new("user", "conv1").with_skill_trust(SkillTrust::Community);

        let trusted: Vec<String> = registry
            .get_available(&trusted_ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();
        let community: Vec<String> = registry
            .get_available(&community_ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();

        assert!(trusted.contains(&"shell".to_string()), "Trusted context should see shell");
        assert!(trusted.contains(&"read".to_string()), "Trusted context should see read");
        assert!(
            !community.contains(&"shell".to_string()),
            "Community context must not see shell"
        );
        assert!(community.contains(&"read".to_string()), "Community context should see read");
    }

    // ── RBAC policy tests ────────────────────────────────────────────────────

    struct HighRiskTool;

    #[async_trait]
    impl Tool for HighRiskTool {
        fn name(&self) -> &str {
            "high_risk"
        }

        fn description(&self) -> &str {
            "A high-risk tool"
        }

        fn parameters_schema(&self) -> Value {
            create_schema(
                "A high-risk tool",
                serde_json::json!({"x": {"type": "string"}}),
                vec!["x"],
            )
        }

        fn capabilities(&self) -> crate::tools::sdk::ToolCapabilities {
            crate::tools::sdk::ToolCapabilities {
                risk_level: crate::tools::approval::RiskLevel::High,
                categories: vec!["system".to_string()],
                ..Default::default()
            }
        }

        async fn execute(
            &self,
            _args: Value,
            _ctx: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            Ok(ToolExecutionResult::success("ok"))
        }
    }

    #[test]
    fn test_rbac_policy_denies_by_name() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ShellTool::new()));
        registry.register(Box::new(ReadTool));

        let policy = crate::tools::rbac::ToolPolicy {
            denied_tools: vec!["shell".to_string()],
            ..Default::default()
        };
        let ctx = ToolContext::new("user", "conv1")
            .with_user_context(crate::tools::rbac::UserContext::owner())
            .with_tool_policy(policy);

        let available: Vec<String> = registry
            .get_available(&ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();

        assert!(!available.contains(&"shell".to_string()));
        assert!(available.contains(&"read".to_string()));
    }

    #[test]
    fn test_rbac_policy_denies_by_role() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ShellTool::new()));

        let policy = crate::tools::rbac::ToolPolicy {
            required_role: crate::tools::rbac::Role::Admin,
            ..Default::default()
        };
        let admin_ctx = ToolContext::new("admin", "conv1")
            .with_user_context(crate::tools::rbac::UserContext {
                roles: vec![crate::tools::rbac::Role::Admin],
                ..Default::default()
            })
            .with_tool_policy(policy.clone());
        let user_ctx = ToolContext::new("user", "conv1")
            .with_user_context(crate::tools::rbac::UserContext::user())
            .with_tool_policy(policy);

        let admin_available: Vec<String> = registry
            .get_available(&admin_ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();
        let user_available: Vec<String> = registry
            .get_available(&user_ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();

        assert!(admin_available.contains(&"shell".to_string()));
        assert!(!user_available.contains(&"shell".to_string()));
    }

    #[test]
    fn test_rbac_policy_denies_by_risk_level() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(HighRiskTool));
        registry.register(Box::new(ReadTool));

        let policy = crate::tools::rbac::ToolPolicy {
            max_risk_level: Some(crate::tools::approval::RiskLevel::Medium),
            ..Default::default()
        };
        let ctx = ToolContext::new("user", "conv1")
            .with_user_context(crate::tools::rbac::UserContext::owner())
            .with_tool_policy(policy);

        let available: Vec<String> = registry
            .get_available(&ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();

        assert!(!available.contains(&"high_risk".to_string()));
        assert!(available.contains(&"read".to_string()));
    }

    #[test]
    fn test_rbac_skill_trust_backward_compatible() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ShellTool::new()));
        registry.register(Box::new(ReadTool));
        registry.mark_privileged("shell");

        // No user_context/tool_policy: SkillTrust alone governs availability.
        let community_ctx =
            ToolContext::new("user", "conv1").with_skill_trust(SkillTrust::Community);
        let available: Vec<String> = registry
            .get_available(&community_ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();

        assert!(!available.contains(&"shell".to_string()));
        assert!(available.contains(&"read".to_string()));
    }

    // ── Tool gating tests ────────────────────────────────────────────────────

    #[test]
    fn test_plugin_allowlist_gating() {
        let registry = ToolRegistry::new();
        registry.register_dynamic(std::sync::Arc::new(ReadTool));
        registry.register_dynamic(std::sync::Arc::new(PluginTool));
        registry.register_dynamic(std::sync::Arc::new(BlockedPluginTool));

        let ctx =
            ToolContext::new("user", "conv1").with_plugin_allowlist(vec!["allowed__".to_string()]);

        let available: Vec<String> = registry
            .get_available(&ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();

        assert!(available.contains(&"read".to_string()));
        assert!(available.contains(&"allowed__foo".to_string()));
        assert!(!available.contains(&"blocked__bar".to_string()));
    }

    struct PluginTool;

    #[async_trait]
    impl Tool for PluginTool {
        fn name(&self) -> &str {
            "allowed__foo"
        }

        fn description(&self) -> &str {
            "An allowed plugin tool"
        }

        fn parameters_schema(&self) -> Value {
            create_schema("Plugin", serde_json::json!({}), Vec::<String>::new())
        }

        async fn execute(
            &self,
            _args: Value,
            _ctx: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            Ok(ToolExecutionResult::success("plugin ok"))
        }
    }

    struct BlockedPluginTool;

    #[async_trait]
    impl Tool for BlockedPluginTool {
        fn name(&self) -> &str {
            "blocked__bar"
        }

        fn description(&self) -> &str {
            "A blocked plugin tool"
        }

        fn parameters_schema(&self) -> Value {
            create_schema("Blocked", serde_json::json!({}), Vec::<String>::new())
        }

        async fn execute(
            &self,
            _args: Value,
            _ctx: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            Ok(ToolExecutionResult::success("blocked"))
        }
    }

    // ── execute()/execute_call() integration tests ───────────────────────────

    #[tokio::test]
    async fn test_execute_simple_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadTool));

        let ctx = ToolContext::new("user", "conv1");
        let result = registry
            .execute("read", serde_json::json!({"path": "/tmp/test"}), &ctx)
            .await;

        assert!(result.is_some(), "execute should return Some for known tools");
        let inner = result.unwrap();
        assert!(inner.is_ok(), "ReadTool should succeed");
        assert_eq!(inner.unwrap().output, "ok");
    }

    #[tokio::test]
    async fn test_execute_unknown_tool() {
        let registry = ToolRegistry::new();
        let ctx = ToolContext::new("user", "conv1");
        let result = registry
            .execute("nonexistent", serde_json::json!({}), &ctx)
            .await;

        assert!(result.is_none(), "execute should return None for unknown tools");
    }

    #[tokio::test]
    async fn test_execute_blocked_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadTool));

        // Block the tool by prefix
        registry.deregister_prefix("read");

        let ctx = ToolContext::new("user", "conv1");
        let result = registry
            .execute("read", serde_json::json!({"path": "/tmp/test"}), &ctx)
            .await;

        assert!(result.is_none(), "execute should return None for blocked tools");
    }

    #[tokio::test]
    async fn test_execute_no_cache_skips_cache() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadTool));

        let ctx = ToolContext::new("user", "conv1");
        // Execute once to warm cache
        let r1 = registry
            .execute("read", serde_json::json!({"path": "/tmp/test"}), &ctx)
            .await;
        assert!(r1.is_some());
        assert!(r1.unwrap().is_ok());

        // execute_no_cache should bypass cache
        let r2 = registry
            .execute_no_cache("read", serde_json::json!({"path": "/tmp/test"}), &ctx)
            .await;
        assert!(r2.is_some());
        assert!(r2.unwrap().is_ok());
    }

    #[tokio::test]
    async fn test_execute_call_simple() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadTool));

        let call = crate::providers::FunctionCall {
            name: "read".to_string(),
            arguments: "{\"path\": \"/tmp/test\"}".to_string(),
        };
        let ctx = ToolContext::new("user", "conv1");

        let result = registry.execute_call(&call, &ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().output, "ok");
    }

    // ── Streaming tool tests ─────────────────────────────────────────────────

    struct StreamingTool;

    #[async_trait]
    impl Tool for StreamingTool {
        fn name(&self) -> &str {
            "streaming_test"
        }

        fn description(&self) -> &str {
            "A tool that streams output"
        }

        fn parameters_schema(&self) -> Value {
            create_schema("Stream", serde_json::json!({}), Vec::<String>::new())
        }

        fn capabilities(&self) -> crate::tools::sdk::ToolCapabilities {
            crate::tools::sdk::ToolCapabilities {
                streaming: true,
                ..Default::default()
            }
        }

        async fn execute(
            &self,
            _args: Value,
            _ctx: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            Ok(ToolExecutionResult::success("final"))
        }
    }

    #[tokio::test]
    async fn test_tool_execute_stream_default() {
        let tool = StreamingTool;
        let ctx = ToolContext::new("user", "conv1");
        let mut stream = tool.execute_stream(serde_json::json!({}), &ctx);

        let chunks: Vec<ToolExecutionChunk> = tokio_stream::StreamExt::collect(&mut stream).await;
        assert_eq!(chunks.len(), 1);
        assert!(matches!(chunks[0], ToolExecutionChunk::Output(ref s) if s == "final"));
    }

    #[tokio::test]
    async fn test_registry_execute_call_streaming() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(StreamingTool));

        let call = crate::providers::FunctionCall {
            name: "streaming_test".to_string(),
            arguments: "{}".to_string(),
        };
        let ctx = ToolContext::new("user", "conv1");

        let mut chunks = Vec::new();
        let result = registry
            .execute_call_streaming(&call, &ctx, |chunk| {
                chunks.push(chunk.clone());
                async move {}
            })
            .await;

        assert!(result.is_ok(), "streaming execution should succeed");
        let result = result.unwrap();
        assert_eq!(result.output, "final");
        assert!(chunks
            .iter()
            .any(|c| matches!(c, ToolExecutionChunk::Output(s) if s == "final")));
    }

    #[test]
    fn test_tool_execution_chunk_serialization() {
        let chunk = ToolExecutionChunk::Output("hello".to_string());
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("\"kind\":\"output\""));
        assert!(json.contains("\"payload\":\"hello\""));

        let decoded: ToolExecutionChunk = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, ToolExecutionChunk::Output(s) if s == "hello"));
    }

    #[test]
    fn test_parse_tool_args_clean_json() {
        let registry = ToolRegistry::new();
        let result = registry.parse_tool_args(r#"{"cmd": "echo hello"}"#, "test_tool");
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["cmd"], "echo hello");
    }

    #[test]
    fn test_parse_tool_args_trailing_text() {
        let registry = ToolRegistry::new();
        // DeepSeek appends text after JSON
        let result =
            registry.parse_tool_args(r#"{"cmd": "echo hello"} some trailing text"#, "test_tool");
        assert!(result.is_ok(), "should handle trailing text: {:?}", result);
        assert_eq!(result.unwrap()["cmd"], "echo hello");
    }

    #[test]
    fn test_parse_tool_args_multiple_objects() {
        let registry = ToolRegistry::new();
        // LLM produces two consecutive JSON objects
        let result = registry.parse_tool_args(r#"{"cmd": "first"} {"cmd": "second"}"#, "test_tool");
        assert!(result.is_ok(), "should handle multiple objects: {:?}", result);
        // Should return only the first value
        assert_eq!(result.unwrap()["cmd"], "first");
    }

    #[test]
    fn test_parse_tool_args_empty() {
        let registry = ToolRegistry::new();
        let result = registry.parse_tool_args("", "test_tool");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::json!({}));
    }

    #[test]
    fn test_parse_tool_args_whitespace_only() {
        let registry = ToolRegistry::new();
        let result = registry.parse_tool_args("   ", "test_tool");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::json!({}));
    }

    #[test]
    fn test_parse_tool_args_invalid_json() {
        let registry = ToolRegistry::new();
        let result = registry.parse_tool_args("not valid json", "test_tool");
        assert!(result.is_err());
    }
}
