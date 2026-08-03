//! Delegation scope — the contract threaded from a parent agent to a child
//! agent through message metadata.
//!
//! A [`DelegationScope`] tells a child agent which shared task it is working
//! on (`root_id`/`task_id`), how deep it may recurse, which tools it may use,
//! and how many tool iterations it may spend.  It is carried inside
//! `MessageMetadata.extra["delegation_scope"]` and applied to the child's
//! [`Context`](crate::agent::Context) when its turn starts.
//!
//! When a scope is present, tool calls are filtered by `is_tool_allowed` and
//! recursive delegation is gated by `can_delegate`.  When no scope is present
//! (ordinary top-level conversations) no behavior changes.

use serde::{Deserialize, Serialize};

/// Metadata key used to pass the delegation scope to a child agent.
pub const DELEGATION_SCOPE_KEY: &str = "delegation_scope";

/// Tools no delegated child may use, regardless of its allowlist.
///
/// `delegate` is deliberately absent — it is gated separately on delegation
/// depth via [`DelegationScope::can_delegate`], so interior nodes keep the
/// ability to recurse while leaves lose it.
pub const DELEGATION_BLOCKED_TOOLS: &[&str] =
    &["clarify", "memory", "send_message", "execute_code"];

/// Shared-task delegation contract for one child agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationScope {
    /// Root task id of the whole delegation tree.  Shared by every task that
    /// descends from the original top-level delegation.
    pub root_id: String,
    /// The child's own task id (equal to the registry run id).
    pub task_id: String,
    /// Nesting depth of this child (top-level delegation = 1).
    pub depth: u32,
    /// Maximum allowed nesting depth.  `can_delegate()` is false at or beyond
    /// this depth.
    pub max_depth: u32,
    /// Task id of this child's parent in the delegation tree (`None` for a
    /// tree root).  Used to link rows; not directly actionable by the child.
    #[serde(default)]
    pub parent_task_id: Option<String>,
    /// Explicit tool allowlist for this child.  `None` means every tool is
    /// allowed (subject to the ordinary tool registry / approval gates).
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Optional tool-iteration cap for this child.
    #[serde(default)]
    pub max_iterations: Option<usize>,
}

impl DelegationScope {
    /// Create a new scope for a child task.
    pub fn new(
        root_id: impl Into<String>,
        task_id: impl Into<String>,
        depth: u32,
        max_depth: u32,
    ) -> Self {
        Self {
            root_id: root_id.into(),
            task_id: task_id.into(),
            depth,
            max_depth,
            parent_task_id: None,
            allowed_tools: None,
            max_iterations: None,
        }
    }

    /// Whether this child may use the named tool.
    ///
    /// Three rules apply in order: the hard-blocked delegation tools are never
    /// available, `delegate` is gated on remaining depth, and then the
    /// allowlist (`None` = everything else) decides.
    pub fn is_tool_allowed(&self, name: &str) -> bool {
        // Hard blocks apply to every delegated child, whatever the allowlist.
        if DELEGATION_BLOCKED_TOOLS.contains(&name) {
            return false;
        }
        // Recursive delegation is depth-gated: a leaf may not recurse even
        // when the allowlist is open (`None`).
        if name == "delegate" && !self.can_delegate() {
            return false;
        }
        match &self.allowed_tools {
            None => true,
            Some(allowed) => allowed.iter().any(|t| t == name),
        }
    }

    /// Whether this child may itself delegate.  False once the tree has
    /// reached `max_depth` (leaves lose the ability to recurse).
    pub fn can_delegate(&self) -> bool {
        self.depth < self.max_depth
    }

    /// Derive the scope for a child spawned by this task (one level deeper).
    pub fn child_scope(
        &self,
        child_task_id: impl Into<String>,
        allowed_tools: Option<Vec<String>>,
        max_iterations: Option<usize>,
    ) -> Self {
        Self {
            root_id: self.root_id.clone(),
            task_id: child_task_id.into(),
            depth: self.depth + 1,
            max_depth: self.max_depth,
            parent_task_id: Some(self.task_id.clone()),
            allowed_tools,
            max_iterations,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_serde_roundtrip() {
        let scope = DelegationScope {
            root_id: "root-1".to_string(),
            task_id: "task-1".to_string(),
            depth: 2,
            max_depth: 3,
            parent_task_id: Some("parent-0".to_string()),
            allowed_tools: Some(vec!["file_read".to_string(), "file_write".to_string()]),
            max_iterations: Some(12),
        };
        let json = serde_json::to_value(&scope).unwrap();
        let back: DelegationScope = serde_json::from_value(json).unwrap();
        assert_eq!(back, scope);
    }

    #[test]
    fn test_scope_defaults_for_missing_fields() {
        // Older messages without allowed_tools/max_iterations must deserialize.
        let json = serde_json::json!({
            "root_id": "r",
            "task_id": "t",
            "depth": 1,
            "max_depth": 3,
        });
        let scope: DelegationScope = serde_json::from_value(json).unwrap();
        assert_eq!(scope.allowed_tools, None);
        assert_eq!(scope.max_iterations, None);
        assert!(scope.is_tool_allowed("anything"));
    }

    #[test]
    fn test_is_tool_allowed() {
        let any = DelegationScope::new("r", "t", 1, 3);
        assert!(any.is_tool_allowed("file_read"));
        assert!(any.is_tool_allowed("delegate"));

        let restricted = DelegationScope {
            allowed_tools: Some(vec!["file_read".to_string()]),
            ..DelegationScope::new("r", "t", 1, 3)
        };
        assert!(restricted.is_tool_allowed("file_read"));
        assert!(!restricted.is_tool_allowed("file_write"));
        assert!(!restricted.is_tool_allowed("delegate"));
    }

    #[test]
    fn test_hard_blocked_tools_never_allowed() {
        // Even with an open allowlist (`None`), a delegated child cannot use
        // the hard-blocked tools.  `delegate` is NOT hard-blocked — interior
        // nodes keep it (see `test_delegate_gated_on_depth`).
        let any = DelegationScope::new("r", "t", 1, 3);
        for tool in DELEGATION_BLOCKED_TOOLS {
            assert!(!any.is_tool_allowed(tool), "{} should be hard-blocked", tool);
        }
        assert!(any.is_tool_allowed("file_read"));

        // Explicitly listing a blocked tool does not grant it.
        let listed = DelegationScope {
            allowed_tools: Some(vec!["execute_code".to_string(), "file_read".to_string()]),
            ..DelegationScope::new("r", "t", 1, 3)
        };
        assert!(!listed.is_tool_allowed("execute_code"));
        assert!(listed.is_tool_allowed("file_read"));
    }

    #[test]
    fn test_delegate_gated_on_depth() {
        // root → manager → worker → leaf with max_depth 3: interior nodes may
        // delegate, the leaf may not, even though the allowlist stays open.
        let manager = DelegationScope::new("r", "manager", 1, 3);
        assert!(manager.is_tool_allowed("delegate"));
        let worker = DelegationScope::new("r", "worker", 2, 3);
        assert!(worker.is_tool_allowed("delegate"));
        let leaf = DelegationScope::new("r", "leaf", 3, 3);
        assert!(!leaf.is_tool_allowed("delegate"));

        // A leaf with an explicit allowlist that names `delegate` still cannot
        // recurse — depth gating wins over the allowlist.
        let leaf_listed = DelegationScope {
            allowed_tools: Some(vec!["delegate".to_string()]),
            ..DelegationScope::new("r", "leaf", 3, 3)
        };
        assert!(!leaf_listed.is_tool_allowed("delegate"));
    }

    #[test]
    fn test_can_delegate() {
        let inner = DelegationScope::new("r", "t", 1, 3);
        assert!(inner.can_delegate());
        let mid = DelegationScope::new("r", "t", 2, 3);
        assert!(mid.can_delegate());
        let leaf = DelegationScope::new("r", "t", 3, 3);
        assert!(!leaf.can_delegate());
    }

    #[test]
    fn test_child_scope_deepens_and_keeps_root() {
        let parent = DelegationScope::new("root-9", "parent", 1, 3);
        let child = parent.child_scope("child", Some(vec!["file_read".to_string()]), Some(5));
        assert_eq!(child.root_id, "root-9");
        assert_eq!(child.task_id, "child");
        assert_eq!(child.depth, 2);
        assert_eq!(child.max_depth, 3);
        assert_eq!(child.parent_task_id.as_deref(), Some("parent"));
        assert_eq!(child.allowed_tools.as_deref(), Some(&["file_read".to_string()][..]));
        assert_eq!(child.max_iterations, Some(5));
    }
}
