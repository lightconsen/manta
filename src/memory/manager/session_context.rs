//! SessionContext: episodic + semantic context for one conversation.

use super::*;

/// Session context returned by `session_context()`.
#[derive(Debug, Clone)]
pub struct SessionContext {
    /// Recent chat messages (episodic memory)
    pub messages: Vec<ChatMessage>,
    /// Relevant semantic memories
    pub memories: Vec<Memory>,
    /// Multimodal file references (e.g. "[Image file: photo.png]")
    pub multimodal_references: Vec<String>,
}

impl SessionContext {
    /// Format the context as a system message injection.
    ///
    /// This produces the string that gets injected into the agent's
    /// context window before the conversation.
    pub fn format_for_injection(&self) -> String {
        let mut parts = vec![];

        // Multimodal references
        if !self.multimodal_references.is_empty() {
            parts.push(format!("## Attached Files\n{}", self.multimodal_references.join("\n")));
        }

        // Learned Interaction Patterns — group "interaction_pattern" memories
        // separately with importance labels, before other context.
        let (patterns, other_memories): (Vec<&Memory>, Vec<&Memory>) = self
            .memories
            .iter()
            .partition(|m| m.memory_type.as_str() == "interaction_pattern");

        if !patterns.is_empty() {
            let pattern_lines: Vec<String> = patterns
                .iter()
                .map(|m| {
                    let label = if m.importance_score >= 0.8 {
                        "HIGH IMPORTANCE"
                    } else if m.importance_score >= 0.5 {
                        "medium importance"
                    } else {
                        "low importance"
                    };
                    format!("- [{}] {} ({})", m.memory_type, m.content, label)
                })
                .collect();
            parts.push(format!("## Learned Interaction Patterns\n{}", pattern_lines.join("\n")));
        }

        // Other semantic memories
        if !other_memories.is_empty() {
            let mem_lines: Vec<String> = other_memories
                .iter()
                .map(|m| format!("- [{}] {}", m.memory_type, m.content))
                .collect();
            parts.push(format!("## Relevant Context\n{}", mem_lines.join("\n")));
        }

        parts.join("\n\n")
    }
}
