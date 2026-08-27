//! Mutable, versioned tool metadata (§十一 工具描述成为可搜索的数据).
//!
//! A tool's static `Tool::description()` is a compile-time string. The
//! structural proposer needs to rewrite LLM-facing tool descriptions at
//! runtime, so the [`ToolRegistry`](crate::tools::ToolRegistry) keeps an
//! optional [`ToolDescriptionMeta`] override per tool. When present it replaces the
//! static description in every emitted `FunctionDefinition`, and the registry
//! is shared with all running agents — so a rewrite takes effect on the very
//! next turn without any agent restart.

use serde::{Deserialize, Serialize};

/// Versioned, mutable description override for a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDescriptionMeta {
    /// Monotonic version — bumped on every rewrite so adoptions are auditable
    /// and a stale candidate cannot overwrite a newer rewrite.
    pub version: u32,
    /// The current LLM-facing description.
    pub description: String,
}

impl ToolDescriptionMeta {
    pub fn new(version: u32, description: impl Into<String>) -> Self {
        Self {
            version,
            description: description.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_is_versioned_and_serializable() {
        let meta = ToolDescriptionMeta::new(3, "list files and directories");
        assert_eq!(meta.version, 3);
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("\"description\""));
        let back: ToolDescriptionMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back, meta);
    }
}
