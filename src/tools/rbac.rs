//! Role-based access control helpers for tool execution.
//!
//! Provides per-user roles, per-tool policies, and evaluation logic used by
//! [`ToolRegistry::is_excluded`](crate::tools::ToolRegistry::is_excluded) to
//! filter the tool set exposed to a caller.
//!
//! The policy system also supports fine-grained *tool gating* via model,
//! provider, sender, plugin, and sandbox dimensions.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A user role used for tool-level access control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Unauthenticated / read-only user.
    #[default]
    Guest = 0,
    /// Standard authenticated user.
    User = 1,
    /// Trusted user with elevated permissions.
    PowerUser = 2,
    /// Administrator.
    Admin = 3,
    /// System owner.
    Owner = 4,
}

/// Per-user context carrying roles and groups for RBAC decisions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserContext {
    /// Roles assigned to the user.
    pub roles: Vec<Role>,
    /// Named groups the user belongs to.
    pub groups: Vec<String>,
    /// Whether the user is the system owner.
    pub is_owner: bool,
}

impl UserContext {
    /// Highest role assigned to the user.
    pub fn highest_role(&self) -> Role {
        self.roles.iter().copied().max().unwrap_or_default()
    }

    /// Create a context for an owner.
    pub fn owner() -> Self {
        Self {
            roles: vec![Role::Owner],
            is_owner: true,
            ..Default::default()
        }
    }

    /// Create a context for a standard user.
    pub fn user() -> Self {
        Self {
            roles: vec![Role::User],
            ..Default::default()
        }
    }
}

/// Capabilities of the model currently driving the agent.
///
/// Used for model-gating decisions (e.g. vision tools require a vision
/// model).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Whether the model supports image/vision inputs.
    pub has_vision: bool,
    /// Whether the model supports native tool use.
    pub supports_tool_use: bool,
    /// Approximate maximum context length, if known.
    pub max_context_length: Option<usize>,
}

/// Sandbox policy applied to tool execution.
///
/// This policy layer is *in addition* to the per-context `ToolContext`
/// sandbox flags. It lets administrators require that certain tools run
/// sandboxed, restrict file/network access, cap risk levels, and set path
/// allow/block lists.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// When true, only tools that advertise `capabilities.sandboxed` may be
    /// exposed or executed.
    pub require_sandboxed: bool,
    /// When false, file-accessing tools are denied.
    pub allow_file_access: bool,
    /// When false, network-accessing tools are denied.
    pub allow_network_access: bool,
    /// Paths that tools are explicitly allowed to access.
    pub allowed_paths: Vec<std::path::PathBuf>,
    /// Paths that tools are explicitly blocked from accessing.
    pub blocked_paths: Vec<std::path::PathBuf>,
    /// Optional global timeout in seconds.
    pub timeout_seconds: Option<u64>,
    /// Optional maximum risk level allowed under this policy.
    pub max_risk_level: Option<crate::tools::approval::RiskLevel>,
}

/// A policy that determines whether a tool is available to a caller.
///
/// `ToolPolicy` combines RBAC (role, groups, owner), capability filtering
/// (risk, categories), and advanced gating (model, provider, sender,
/// plugin, sandbox). Deny rules take precedence over allow rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPolicy {
    /// Minimum role required.
    pub required_role: Role,
    /// Maximum risk level allowed.
    pub max_risk_level: Option<crate::tools::approval::RiskLevel>,
    /// Allowed tool categories (empty = all).
    pub allowed_categories: Vec<String>,
    /// Denied tool categories.
    pub denied_categories: Vec<String>,
    /// Explicitly denied tool names.
    pub denied_tools: Vec<String>,
    /// Explicitly allowed tool names (empty = not restricted).
    pub allowed_tools: Vec<String>,

    // ── Plugin gating ───────────────────────────────────────────────────────
    /// Plugin tool prefixes/names that are explicitly allowed.
    pub plugin_tool_allowlist: Vec<String>,
    /// Plugin tool prefixes/names that are explicitly denied.
    pub plugin_tool_denylist: Vec<String>,
    /// When false, plugin-registered tools are denied.
    pub allow_plugin_tools: bool,

    // ── Model gating ────────────────────────────────────────────────────────
    /// Model names that are explicitly allowed to drive these tools.
    pub allowed_models: Vec<String>,
    /// Model names that are explicitly denied.
    pub denied_models: Vec<String>,
    /// When true, only tools that work with vision models are allowed.
    pub require_model_vision: bool,

    // ── Provider gating ─────────────────────────────────────────────────────
    /// Provider names that are explicitly allowed.
    pub allowed_providers: Vec<String>,
    /// Provider names that are explicitly denied.
    pub denied_providers: Vec<String>,

    // ── Sender gating ───────────────────────────────────────────────────────
    /// When true, only the system owner may use these tools.
    pub sender_must_be_owner: bool,
    /// Sender IDs that are explicitly allowed.
    pub allowed_senders: Vec<String>,
    /// Sender IDs that are explicitly denied.
    pub denied_senders: Vec<String>,

    // ── Sandbox gating ──────────────────────────────────────────────────────
    /// Sandbox policy applied to tool execution.
    pub sandbox_policy: SandboxPolicy,
    /// When true, only tools that advertise `capabilities.sandboxed` are
    /// allowed.
    pub require_sandboxed: bool,

    // ── Source gating ───────────────────────────────────────────────────────
    /// When false, dynamically-registered tools (including MCP) are denied.
    pub allow_dynamic_tools: bool,
    /// When false, MCP-discovered tools are denied.
    pub allow_mcp_tools: bool,
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            required_role: Role::default(),
            max_risk_level: None,
            allowed_categories: Vec::new(),
            denied_categories: Vec::new(),
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            plugin_tool_allowlist: Vec::new(),
            plugin_tool_denylist: Vec::new(),
            allow_plugin_tools: true,
            allowed_models: Vec::new(),
            denied_models: Vec::new(),
            require_model_vision: false,
            allowed_providers: Vec::new(),
            denied_providers: Vec::new(),
            sender_must_be_owner: false,
            allowed_senders: Vec::new(),
            denied_senders: Vec::new(),
            sandbox_policy: SandboxPolicy::default(),
            require_sandboxed: false,
            allow_dynamic_tools: true,
            allow_mcp_tools: true,
        }
    }
}

/// Runtime context used during policy evaluation.
///
/// This is separate from [`ToolContext`](crate::tools::ToolContext) so that
/// policy evaluation only needs the fields relevant to gating decisions.
#[derive(Debug, Clone, Default)]
pub struct PolicyEvaluationContext {
    /// The model name driving the current invocation.
    pub model_name: Option<String>,
    /// The provider name driving the current invocation.
    pub provider_name: Option<String>,
    /// The sender/user identifier.
    pub sender_id: Option<String>,
    /// Whether the sender is the system owner.
    pub sender_is_owner: bool,
    /// Plugin tool allowlist for this invocation.
    pub plugin_allowlist: Option<Vec<String>>,
    /// Capabilities of the current model.
    pub model_capabilities: ModelCapabilities,
    /// Whether the tool is dynamically registered (e.g. MCP).
    pub is_dynamic: bool,
    /// Whether the tool originates from an MCP server.
    pub is_mcp: bool,
}

impl ToolPolicy {
    /// Evaluate whether `tool_name` is available to `user` given its
    /// advertised capabilities and the runtime evaluation context.
    ///
    /// Deny rules take precedence over allow rules. An empty allow list
    /// means "no restriction" for that dimension.
    pub fn evaluate(
        &self,
        user: &UserContext,
        tool_name: &str,
        capabilities: &crate::tools::sdk::ToolCapabilities,
    ) -> bool {
        self.evaluate_with_context(user, tool_name, capabilities, &PolicyEvaluationContext::default())
    }

    /// Evaluate policy with full runtime context.
    pub fn evaluate_with_context(
        &self,
        user: &UserContext,
        tool_name: &str,
        capabilities: &crate::tools::sdk::ToolCapabilities,
        eval: &PolicyEvaluationContext,
    ) -> bool {
        // ── Explicit denials take precedence ───────────────────────────────
        if self.denied_tools.iter().any(|d| d == tool_name) {
            return false;
        }

        // ── Explicit allow-list ────────────────────────────────────────────
        if !self.allowed_tools.is_empty() && !self.allowed_tools.iter().any(|a| a == tool_name) {
            return false;
        }

        // ── Role check ─────────────────────────────────────────────────────
        // Owners bypass role requirements.
        if user.highest_role() < self.required_role && !user.is_owner {
            return false;
        }

        // ── Risk-level ceiling ─────────────────────────────────────────────
        if let Some(max) = self.max_risk_level {
            if capabilities.risk_level > max {
                return false;
            }
        }

        // ── Category allow/block ───────────────────────────────────────────
        if !self.allowed_categories.is_empty() {
            let allowed: HashSet<&str> =
                self.allowed_categories.iter().map(|s| s.as_str()).collect();
            if !capabilities
                .categories
                .iter()
                .any(|c| allowed.contains(c.as_str()))
            {
                return false;
            }
        }
        if !self.denied_categories.is_empty() {
            let denied: HashSet<&str> = self.denied_categories.iter().map(|s| s.as_str()).collect();
            if capabilities
                .categories
                .iter()
                .any(|c| denied.contains(c.as_str()))
            {
                return false;
            }
        }

        // ── Plugin gating ──────────────────────────────────────────────────
        if !self.allow_plugin_tools && self.is_plugin_tool(tool_name) {
            return false;
        }
        if !self.plugin_tool_denylist.is_empty()
            && self
                .plugin_tool_denylist
                .iter()
                .any(|d| tool_name == d || tool_name.starts_with(d))
        {
            return false;
        }
        if !self.plugin_tool_allowlist.is_empty()
            && self.is_plugin_tool(tool_name)
            && !self
                .plugin_tool_allowlist
                .iter()
                .any(|a| tool_name == a || tool_name.starts_with(a))
        {
            return false;
        }

        // ── Model gating ───────────────────────────────────────────────────
        if let Some(ref model) = eval.model_name {
            if !self.allowed_models.is_empty() && !self.allowed_models.iter().any(|a| a == model) {
                return false;
            }
            if self.denied_models.iter().any(|d| d == model) {
                return false;
            }
        }
        if self.require_model_vision && !eval.model_capabilities.has_vision {
            return false;
        }

        // ── Provider gating ────────────────────────────────────────────────
        if let Some(ref provider) = eval.provider_name {
            if !self.allowed_providers.is_empty()
                && !self.allowed_providers.iter().any(|a| a == provider)
            {
                return false;
            }
            if self.denied_providers.iter().any(|d| d == provider) {
                return false;
            }
        }

        // ── Sender gating ──────────────────────────────────────────────────
        if self.sender_must_be_owner && !eval.sender_is_owner {
            return false;
        }
        if let Some(ref sender) = eval.sender_id {
            if !self.allowed_senders.is_empty()
                && !self.allowed_senders.iter().any(|a| a == sender)
            {
                return false;
            }
            if self.denied_senders.iter().any(|d| d == sender) {
                return false;
            }
        }

        // ── Sandbox gating ─────────────────────────────────────────────────
        if self.require_sandboxed && !capabilities.sandboxed {
            return false;
        }
        if self.sandbox_policy.require_sandboxed && !capabilities.sandboxed {
            return false;
        }
        if let Some(max) = self.sandbox_policy.max_risk_level {
            if capabilities.risk_level > max {
                return false;
            }
        }

        // ── Source gating ──────────────────────────────────────────────────
        if !self.allow_dynamic_tools && eval.is_dynamic {
            return false;
        }
        if !self.allow_mcp_tools && eval.is_mcp {
            return false;
        }

        true
    }

    /// Heuristic: tool names containing `__` are treated as plugin/MCP tools.
    fn is_plugin_tool(&self, tool_name: &str) -> bool {
        tool_name.contains("__")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::approval::RiskLevel;
    use crate::tools::sdk::ToolCapabilities;

    fn caps(risk: RiskLevel, categories: &[&str]) -> ToolCapabilities {
        ToolCapabilities {
            risk_level: risk,
            categories: categories.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn test_role_requirement() {
        let policy = ToolPolicy {
            required_role: Role::Admin,
            ..Default::default()
        };
        let admin = UserContext {
            roles: vec![Role::Admin],
            ..Default::default()
        };
        let user = UserContext::user();
        let caps = caps(RiskLevel::Low, &[]);

        assert!(policy.evaluate(&admin, "foo", &caps));
        assert!(!policy.evaluate(&user, "foo", &caps));
    }

    #[test]
    fn test_owner_bypasses_role() {
        let policy = ToolPolicy {
            required_role: Role::Owner,
            ..Default::default()
        };
        let owner = UserContext::owner();
        let caps = caps(RiskLevel::Critical, &[]);
        assert!(policy.evaluate(&owner, "foo", &caps));
    }

    #[test]
    fn test_denied_tools() {
        let policy = ToolPolicy {
            denied_tools: vec!["shell".to_string()],
            ..Default::default()
        };
        let user = UserContext::owner();
        let caps = caps(RiskLevel::Low, &[]);
        assert!(!policy.evaluate(&user, "shell", &caps));
        assert!(policy.evaluate(&user, "read_file", &caps));
    }

    #[test]
    fn test_allowed_tools() {
        let policy = ToolPolicy {
            allowed_tools: vec!["read_file".to_string(), "write_file".to_string()],
            ..Default::default()
        };
        let user = UserContext::owner();
        let caps = caps(RiskLevel::Low, &[]);
        assert!(policy.evaluate(&user, "read_file", &caps));
        assert!(!policy.evaluate(&user, "shell", &caps));
    }

    #[test]
    fn test_max_risk_level() {
        let policy = ToolPolicy {
            max_risk_level: Some(RiskLevel::Medium),
            ..Default::default()
        };
        let user = UserContext::owner();
        let low = caps(RiskLevel::Low, &[]);
        let high = caps(RiskLevel::High, &[]);
        assert!(policy.evaluate(&user, "foo", &low));
        assert!(!policy.evaluate(&user, "foo", &high));
    }

    #[test]
    fn test_allowed_categories() {
        let policy = ToolPolicy {
            allowed_categories: vec!["file".to_string(), "memory".to_string()],
            ..Default::default()
        };
        let user = UserContext::owner();
        let file = caps(RiskLevel::Low, &["file"]);
        let net = caps(RiskLevel::Low, &["network"]);
        assert!(policy.evaluate(&user, "read_file", &file));
        assert!(!policy.evaluate(&user, "web_search", &net));
    }

    #[test]
    fn test_denied_categories() {
        let policy = ToolPolicy {
            denied_categories: vec!["network".to_string()],
            ..Default::default()
        };
        let user = UserContext::owner();
        let file = caps(RiskLevel::Low, &["file"]);
        let net = caps(RiskLevel::Low, &["network"]);
        assert!(policy.evaluate(&user, "read_file", &file));
        assert!(!policy.evaluate(&user, "web_search", &net));
    }

    #[test]
    fn test_plugin_tool_allowlist() {
        let policy = ToolPolicy {
            allow_plugin_tools: true,
            plugin_tool_allowlist: vec!["plugin__".to_string()],
            ..Default::default()
        };
        let user = UserContext::owner();
        let caps = caps(RiskLevel::Low, &[]);
        let eval = PolicyEvaluationContext {
            is_dynamic: true,
            ..Default::default()
        };
        assert!(policy.evaluate_with_context(&user, "plugin__foo", &caps, &eval));
        assert!(!policy.evaluate_with_context(&user, "other__bar", &caps, &eval));
    }

    #[test]
    fn test_plugin_tool_denylist() {
        let policy = ToolPolicy {
            plugin_tool_denylist: vec!["bad__".to_string()],
            ..Default::default()
        };
        let user = UserContext::owner();
        let caps = caps(RiskLevel::Low, &[]);
        let eval = PolicyEvaluationContext::default();
        assert!(policy.evaluate_with_context(&user, "good__foo", &caps, &eval));
        assert!(!policy.evaluate_with_context(&user, "bad__bar", &caps, &eval));
    }

    #[test]
    fn test_model_gating() {
        let policy = ToolPolicy {
            allowed_models: vec!["claude-sonnet".to_string()],
            ..Default::default()
        };
        let user = UserContext::owner();
        let caps = caps(RiskLevel::Low, &[]);
        let allowed = PolicyEvaluationContext {
            model_name: Some("claude-sonnet".to_string()),
            ..Default::default()
        };
        let denied = PolicyEvaluationContext {
            model_name: Some("gpt-4".to_string()),
            ..Default::default()
        };
        assert!(policy.evaluate_with_context(&user, "foo", &caps, &allowed));
        assert!(!policy.evaluate_with_context(&user, "foo", &caps, &denied));
    }

    #[test]
    fn test_provider_gating() {
        let policy = ToolPolicy {
            denied_providers: vec!["openai".to_string()],
            ..Default::default()
        };
        let user = UserContext::owner();
        let caps = caps(RiskLevel::Low, &[]);
        let anthropic = PolicyEvaluationContext {
            provider_name: Some("anthropic".to_string()),
            ..Default::default()
        };
        let openai = PolicyEvaluationContext {
            provider_name: Some("openai".to_string()),
            ..Default::default()
        };
        assert!(policy.evaluate_with_context(&user, "foo", &caps, &anthropic));
        assert!(!policy.evaluate_with_context(&user, "foo", &caps, &openai));
    }

    #[test]
    fn test_sender_must_be_owner() {
        let policy = ToolPolicy {
            sender_must_be_owner: true,
            ..Default::default()
        };
        let user = UserContext::owner();
        let caps = caps(RiskLevel::Low, &[]);
        let owner = PolicyEvaluationContext {
            sender_is_owner: true,
            ..Default::default()
        };
        let guest = PolicyEvaluationContext {
            sender_is_owner: false,
            ..Default::default()
        };
        assert!(policy.evaluate_with_context(&user, "foo", &caps, &owner));
        assert!(!policy.evaluate_with_context(&user, "foo", &caps, &guest));
    }

    #[test]
    fn test_require_model_vision() {
        let policy = ToolPolicy {
            require_model_vision: true,
            ..Default::default()
        };
        let user = UserContext::owner();
        let caps = caps(RiskLevel::Low, &[]);
        let vision = PolicyEvaluationContext {
            model_capabilities: ModelCapabilities {
                has_vision: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let no_vision = PolicyEvaluationContext {
            model_capabilities: ModelCapabilities {
                has_vision: false,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(policy.evaluate_with_context(&user, "image_tool", &caps, &vision));
        assert!(!policy.evaluate_with_context(&user, "image_tool", &caps, &no_vision));
    }

    #[test]
    fn test_sandbox_policy_require_sandboxed() {
        let policy = ToolPolicy {
            sandbox_policy: SandboxPolicy {
                require_sandboxed: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let user = UserContext::owner();
        let sandboxed = ToolCapabilities {
            sandboxed: true,
            ..Default::default()
        };
        let unsandboxed = ToolCapabilities {
            sandboxed: false,
            ..Default::default()
        };
        assert!(policy.evaluate(&user, "code_exec", &sandboxed));
        assert!(!policy.evaluate(&user, "shell", &unsandboxed));
    }

    #[test]
    fn test_source_gating() {
        let policy = ToolPolicy {
            allow_dynamic_tools: false,
            allow_mcp_tools: false,
            ..Default::default()
        };
        let user = UserContext::owner();
        let caps = caps(RiskLevel::Low, &[]);
        let dynamic = PolicyEvaluationContext {
            is_dynamic: true,
            is_mcp: false,
            ..Default::default()
        };
        let mcp = PolicyEvaluationContext {
            is_dynamic: true,
            is_mcp: true,
            ..Default::default()
        };
        assert!(!policy.evaluate_with_context(&user, "dynamic_tool", &caps, &dynamic));
        assert!(!policy.evaluate_with_context(&user, "mcp__server__tool", &caps, &mcp));
    }
}
