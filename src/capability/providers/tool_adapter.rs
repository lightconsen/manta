//! Bridge [`Tool`](crate::tools::Tool) → [`Capability`](super::super::Capability).
//!
//! [`ToolCapability`] wraps any `Arc<dyn Tool>` and implements the unified
//! [`Capability`] trait by delegating to the tool's methods.

use crate::capability::{Capability, CapabilityResult};
use crate::tools::{Tool, ToolContext};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

/// A [`Capability`] backed by an existing [`Tool`].
///
/// The adapter uses a default [`ToolContext`] during execution. Callers that
/// need a custom context should construct their own and pass it to the
/// underlying tool directly.
pub struct ToolCapability {
    tool: Arc<dyn Tool>,
}

impl ToolCapability {
    /// Wrap a tool as a capability.
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// use syscity::capability::providers::tool_adapter::ToolCapability;
    /// use syscity::tools::ShellTool;
    ///
    /// let cap = ToolCapability::new(Arc::new(ShellTool::new()));
    /// assert_eq!(cap.name(), "shell");
    /// ```
    pub fn new(tool: Arc<dyn Tool>) -> Self {
        Self { tool }
    }

    /// Access the inner tool.
    pub fn inner(&self) -> &Arc<dyn Tool> {
        &self.tool
    }
}

#[async_trait]
impl Capability for ToolCapability {
    fn name(&self) -> &str {
        self.tool.name()
    }

    fn param_schema(&self) -> Value {
        self.tool.parameters_schema()
    }

    async fn execute(&self, params: Value) -> CapabilityResult {
        let start = std::time::Instant::now();
        let ctx = ToolContext::default();

        match self.tool.execute(params, &ctx).await {
            Ok(result) => CapabilityResult {
                success: result.success,
                output: Some(serde_json::json!({ "output": result.output })),
                error: result.error,
                duration_ms: start.elapsed().as_millis() as u64,
            },
            Err(err) => CapabilityResult {
                success: false,
                output: None,
                error: Some(err.to_string()),
                duration_ms: start.elapsed().as_millis() as u64,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::create_schema;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes input back"
        }
        fn parameters_schema(&self) -> Value {
            create_schema(
                "Echo",
                serde_json::json!({"text": {"type": "string"}}),
                vec!["text"],
            )
        }
        async fn execute(
            &self,
            args: Value,
            _ctx: &ToolContext,
        ) -> crate::Result<crate::tools::ToolExecutionResult> {
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            Ok(crate::tools::ToolExecutionResult::success(text))
        }
    }

    #[tokio::test]
    async fn test_tool_capability_name_and_schema() {
        let cap = ToolCapability::new(Arc::new(EchoTool));
        assert_eq!(cap.name(), "echo");
        let schema = cap.param_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema.get("properties").is_some());
    }

    #[tokio::test]
    async fn test_tool_capability_execute() {
        let cap = ToolCapability::new(Arc::new(EchoTool));
        let result = cap
            .execute(serde_json::json!({"text": "hello"}))
            .await;
        assert!(result.success);
        let output = result.output.unwrap();
        assert_eq!(output["output"], "hello");
    }

    #[tokio::test]
    async fn test_tool_capability_execute_failure() {
        struct FailingTool;

        #[async_trait]
        impl Tool for FailingTool {
            fn name(&self) -> &str {
                "fail"
            }
            fn description(&self) -> &str {
                "Always fails"
            }
            fn parameters_schema(&self) -> Value {
                create_schema("Fail", serde_json::json!({}), Vec::<String>::new())
            }
            async fn execute(
                &self,
                _args: Value,
                _ctx: &ToolContext,
            ) -> crate::Result<crate::tools::ToolExecutionResult> {
                Err(crate::error::SyscityError::Validation("oops".into()))
            }
        }

        let cap = ToolCapability::new(Arc::new(FailingTool));
        let result = cap.execute(Value::Null).await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("oops"));
    }
}
