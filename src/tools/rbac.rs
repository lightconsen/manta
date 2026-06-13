//! Role-based access control helpers for tool execution.
//!
//! Provides per-user roles, per-tool policies, and evaluation logic used by
//! [`ToolRegistry::is_excluded`](crate::tools::ToolRegistry::is_excluded) to
//! filter the tool set exposed to a caller.

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

/// A policy that determines whether a tool is available to a user.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolPolicy {
    /// Minimum role required.
    pub required_role: Role,
    /// Maximum risk level allowed.
    pub max_risk_level: Option<crate::tools::approval::RiskLevel>,
    /// Allowed tool categories (empty = all).
    pub allowed_categories: Vec<String>,
    /// Explicitly denied tool names.
    pub denied_tools: Vec<String>,
    /// Explicitly allowed tool names (empty = not restricted).
    pub allowed_tools: Vec<String>,
}

impl ToolPolicy {
    /// Evaluate whether `tool_name` is available to `user` given its
    /// advertised capabilities.
    ///
    /// Deny rules take precedence over allow rules. An empty
    /// `allowed_tools`/`allowed_categories` list means "no restriction" for
    /// that dimension.
    pub fn evaluate(
        &self,
        user: &UserContext,
        tool_name: &str,
        capabilities: &crate::tools::sdk::ToolCapabilities,
    ) -> bool {
        // Explicit denials take precedence.
        if self.denied_tools.iter().any(|d| d == tool_name) {
            return false;
        }

        // If an allow-list is defined, the tool must be named in it.
        if !self.allowed_tools.is_empty() && !self.allowed_tools.iter().any(|a| a == tool_name) {
            return false;
        }

        // Role check. Owners bypass role requirements.
        if user.highest_role() < self.required_role && !user.is_owner {
            return false;
        }

        // Risk-level ceiling.
        if let Some(max) = self.max_risk_level {
            if capabilities.risk_level > max {
                return false;
            }
        }

        // Category restriction.
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

        true
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
}
