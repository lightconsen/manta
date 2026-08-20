//! Single hook-command execution: build the stdin payload, run the shell
//! command with a hard timeout, and parse its stdout into a decision.
//!
//! Everything here fails **open**: a crash, timeout, non-zero exit, or
//! unparsable output degrades to the permissive default (`Allow` / `Accept` /
//! pass-through) so a broken hook can never lock the agent out.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tracing::warn;

use crate::tools::hooks::PostExecuteDecision;
use crate::tools::process_runner::{run_collect, ProcessRequest};
use crate::tools::shell::resolve_shell;

/// Wall-clock cap for a single hook-command invocation.
pub const HOOK_TIMEOUT: Duration = Duration::from_secs(10);

/// Structured context serialized to a hook command's stdin (CC-compatible
/// subset of the fields CC exposes to hooks).
#[derive(Debug, Clone)]
pub struct HookContext {
    /// One of `PreToolUse`, `PostToolUse`, `UserPromptSubmit`, `Stop`.
    pub event: &'static str,
    /// Name of the tool being gated (tool events only).
    pub tool_name: Option<String>,
    /// Serialized tool call arguments (tool events only).
    pub tool_input: Option<Value>,
    /// Serialized tool result (post-execute events only).
    pub tool_response: Option<Value>,
    /// The user message text (prompt events only).
    pub prompt: Option<String>,
    /// Actor making the call.
    pub user_id: String,
    /// Originating channel, when known.
    pub channel: Option<String>,
    /// The agent being driven, when known.
    pub agent_id: Option<String>,
    /// The conversation / session id.
    pub session_id: Option<String>,
    /// Working directory to run the hook in.
    pub cwd: Option<PathBuf>,
    /// Workspace root exposed to the hook.
    pub workspace_dir: Option<PathBuf>,
}

impl HookContext {
    /// Build the stdin JSON document. `tool_response` is serialized from the
    /// `ToolExecutionResult`'s own `Serialize` impl (output/error/data/
    /// success/execution_time).
    fn to_stdin(&self) -> Vec<u8> {
        let mut obj = serde_json::Map::new();
        obj.insert("hook_event_name".to_string(), json!(self.event));
        obj.insert("user_id".to_string(), json!(self.user_id));
        if let Some(v) = &self.tool_name {
            obj.insert("tool_name".to_string(), json!(v));
        }
        if let Some(v) = &self.tool_input {
            obj.insert("tool_input".to_string(), v.clone());
        }
        if let Some(v) = &self.tool_response {
            obj.insert("tool_response".to_string(), v.clone());
        }
        if let Some(v) = &self.prompt {
            obj.insert("prompt".to_string(), json!(v));
        }
        if let Some(v) = &self.channel {
            obj.insert("channel".to_string(), json!(v));
        }
        if let Some(v) = &self.agent_id {
            obj.insert("agent_id".to_string(), json!(v));
        }
        if let Some(v) = &self.session_id {
            obj.insert("session_id".to_string(), json!(v));
        }
        if let Some(v) = &self.cwd {
            obj.insert("cwd".to_string(), json!(v.display().to_string()));
        }
        if let Some(v) = &self.workspace_dir {
            obj.insert("workspace_dir".to_string(), json!(v.display().to_string()));
        }
        serde_json::to_vec(&Value::Object(obj)).unwrap_or_default()
    }
}

/// Run `command` with `ctx` on stdin, returning its stdout (lossy).
///
/// Fail-open: spawn errors, timeouts, and non-zero exits all yield an empty
/// string (which downstream parsers treat as the permissive default).
pub async fn run_command(command: &str, ctx: &HookContext) -> String {
    let req = ProcessRequest {
        argv: vec![resolve_shell(), "-c".to_string(), command.to_string()],
        cwd: ctx.cwd.clone(),
        stdin: Some(ctx.to_stdin()),
        timeout: Some(HOOK_TIMEOUT),
        ..Default::default()
    };
    match run_collect(&req).await {
        Ok(out) if out.success() && !out.timed_out => out.stdout_string(),
        Ok(out) if out.timed_out => {
            warn!("hook command timed out after {}s: {}", HOOK_TIMEOUT.as_secs(), command);
            String::new()
        }
        Ok(out) => {
            warn!("hook command exited {:?}: {}", out.exit_code(), command);
            String::new()
        }
        Err(e) => {
            warn!("hook command failed to run ({}): {}", command, e);
            String::new()
        }
    }
}

// ── Decision parsing (all fail-open) ───────────────────────────

/// A pre-tool decision derived from a hook's stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreDecision {
    /// Proceed with the call.
    Allow,
    /// `{"permission":"deny", ...}` — hard block with a reason.
    Deny(String),
    /// `{"permission":"ask", ...}` — surface for human approval.
    Ask(String),
}

/// Parse a `PreToolUse` hook's stdout.
///
/// Claude Code's `updatedInput` is **parsed and deliberately ignored**: a
/// hook may inspect the rewritten input it would produce, but the rewrite is
/// never applied to the arguments the tool actually receives.
pub fn parse_pre(stdout: &str) -> PreDecision {
    let Some(value) = parse_json(stdout) else {
        return PreDecision::Allow;
    };
    match permission_of(&value) {
        "deny" => PreDecision::Deny(reason_of(&value, "Blocked by a shell hook policy.")),
        "ask" => PreDecision::Ask(reason_of(&value, "Tool call requires host approval.")),
        // Includes the explicit `allow` and the parsed-but-ignored
        // `updatedInput` variants.
        _ => PreDecision::Allow,
    }
}

/// Parse a `PostToolUse` hook's stdout.
pub fn parse_post(stdout: &str) -> PostExecuteDecision {
    let Some(value) = parse_json(stdout) else {
        return PostExecuteDecision::Accept;
    };
    match value.get("decision").and_then(|v| v.as_str()).unwrap_or("") {
        "block" => {
            PostExecuteDecision::Block(reason_of(&value, "Tool result withheld by a shell hook."))
        }
        "replace" => {
            // CC supplies replacement context as `additionalContext`; accept
            // `output` as a convenience alias.
            let replacement = value
                .get("additionalContext")
                .or_else(|| value.get("output"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if replacement.is_empty() {
                PostExecuteDecision::Accept
            } else {
                PostExecuteDecision::ReplaceOutput(replacement.to_string())
            }
        }
        _ => PostExecuteDecision::Accept,
    }
}

/// Parse a `UserPromptSubmit` hook's stdout: `Some(reason)` blocks the
/// message, `None` lets it through.
pub fn parse_prompt(stdout: &str) -> Option<String> {
    let value = parse_json(stdout)?;
    if value.get("decision").and_then(|v| v.as_str()) == Some("block") {
        Some(reason_of(&value, "Message blocked by a shell hook."))
    } else {
        None
    }
}

/// Best-effort JSON parse; invalid/empty output is a fail-open `None`.
fn parse_json(stdout: &str) -> Option<Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(v) => Some(v),
        Err(e) => {
            warn!("hook output is not valid JSON (failing open): {} — {:?}", e, trimmed);
            None
        }
    }
}

fn permission_of(value: &Value) -> &str {
    value
        .get("permission")
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn reason_of(value: &Value, fallback: &str) -> String {
    value
        .get("reason")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pre_empty_output_allow() {
        assert_eq!(parse_pre(""), PreDecision::Allow);
        assert_eq!(parse_pre("   \n  "), PreDecision::Allow);
    }

    #[test]
    fn test_parse_pre_garbage_allow() {
        assert_eq!(parse_pre("not json at all"), PreDecision::Allow);
    }

    #[test]
    fn test_parse_pre_deny() {
        assert_eq!(
            parse_pre(r#"{"permission":"deny","reason":"blocked-by-hook"}"#),
            PreDecision::Deny("blocked-by-hook".to_string())
        );
    }

    #[test]
    fn test_parse_pre_deny_without_reason_uses_fallback() {
        assert_eq!(
            parse_pre(r#"{"permission":"deny"}"#),
            PreDecision::Deny("Blocked by a shell hook policy.".to_string())
        );
    }

    #[test]
    fn test_parse_pre_ask() {
        assert_eq!(
            parse_pre(r#"{"permission":"ask","reason":"needs-check"}"#),
            PreDecision::Ask("needs-check".to_string())
        );
    }

    #[test]
    fn test_parse_pre_explicit_allow() {
        assert_eq!(parse_pre(r#"{"permission":"allow"}"#), PreDecision::Allow);
    }

    #[test]
    fn test_parse_pre_updated_input_ignored() {
        // `updatedInput` is parsed but NEVER applied — this must not deny.
        assert_eq!(parse_pre(r#"{"updatedInput":"echo pwned"}"#), PreDecision::Allow);
    }

    #[test]
    fn test_parse_post_empty_accept() {
        assert_eq!(parse_post(""), PostExecuteDecision::Accept);
        assert_eq!(parse_post("junk"), PostExecuteDecision::Accept);
    }

    #[test]
    fn test_parse_post_block() {
        assert_eq!(
            parse_post(r#"{"decision":"block","reason":"withheld"}"#),
            PostExecuteDecision::Block("withheld".to_string())
        );
    }

    #[test]
    fn test_parse_post_block_fallback_reason() {
        assert_eq!(
            parse_post(r#"{"decision":"block"}"#),
            PostExecuteDecision::Block("Tool result withheld by a shell hook.".to_string())
        );
    }

    #[test]
    fn test_parse_post_replace_additional_context() {
        assert_eq!(
            parse_post(r#"{"decision":"replace","additionalContext":"replacement text"}"#),
            PostExecuteDecision::ReplaceOutput("replacement text".to_string())
        );
    }

    #[test]
    fn test_parse_post_replace_empty_is_accept() {
        assert_eq!(parse_post(r#"{"decision":"replace"}"#), PostExecuteDecision::Accept);
        assert_eq!(
            parse_post(r#"{"decision":"replace","additionalContext":""}"#),
            PostExecuteDecision::Accept
        );
    }

    #[test]
    fn test_parse_post_accept() {
        assert_eq!(parse_post(r#"{"decision":"accept"}"#), PostExecuteDecision::Accept);
        assert_eq!(parse_post(r#"{}"#), PostExecuteDecision::Accept);
    }

    #[test]
    fn test_parse_prompt_block() {
        assert_eq!(
            parse_prompt(r#"{"decision":"block","reason":"prompt-blocked"}"#),
            Some("prompt-blocked".to_string())
        );
    }

    #[test]
    fn test_parse_prompt_pass() {
        assert_eq!(parse_prompt(""), None);
        assert_eq!(parse_prompt("garbage"), None);
        assert_eq!(parse_prompt(r#"{"decision":"allow"}"#), None);
        assert_eq!(parse_prompt(r#"{}"#), None);
    }
}
