//! Dynamic Prompt Builder for Manta
//!
//! Builds adaptive system prompts based on context, tools, task type, and conversation state.
//! Provides runtime prompt customization for better agent performance.

use crate::providers::{FunctionDefinition, Message};
use crate::tools::ToolContext;
use std::collections::HashMap;

/// Section of a dynamic prompt
#[derive(Debug, Clone)]
pub struct PromptSection {
    /// Section name/identifier
    pub name: String,
    /// Section content
    pub content: String,
    /// Priority (higher = more important, less likely to be pruned)
    pub priority: u8,
    /// Estimated token count
    pub token_estimate: usize,
    /// Whether this section is dynamic (can change during conversation)
    pub is_dynamic: bool,
}

impl PromptSection {
    /// Create a new prompt section
    pub fn new(name: impl Into<String>, content: impl Into<String>) -> Self {
        let content = content.into();
        // Rough token estimate: ~4 chars per token
        let token_estimate = content.len() / 4;

        Self {
            name: name.into(),
            content,
            priority: 5, // Default medium priority
            token_estimate,
            is_dynamic: false,
        }
    }

    /// Set priority
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority.clamp(1, 10);
        self
    }

    /// Mark as dynamic
    pub fn dynamic(mut self) -> Self {
        self.is_dynamic = true;
        self
    }
}

/// Context for building a dynamic prompt
#[derive(Debug, Clone)]
pub struct PromptContext {
    /// User's current message
    pub user_message: String,
    /// Current task/plan context (if any)
    pub task_context: Option<String>,
    /// Available tools
    pub available_tools: Vec<FunctionDefinition>,
    /// Conversation phase
    pub phase: ConversationPhase,
    /// Recent message history
    pub recent_history: Vec<Message>,
    /// User preferences/patterns
    pub user_preferences: HashMap<String, String>,
    /// Task type (detected or specified)
    pub task_type: TaskType,
    /// Whether this is a follow-up question
    pub is_follow_up: bool,
}

impl PromptContext {
    /// Create a new prompt context
    pub fn new(user_message: impl Into<String>) -> Self {
        Self {
            user_message: user_message.into(),
            task_context: None,
            available_tools: Vec::new(),
            phase: ConversationPhase::New,
            recent_history: Vec::new(),
            user_preferences: HashMap::new(),
            task_type: TaskType::General,
            is_follow_up: false,
        }
    }

    /// Detect task type from user message
    pub fn detect_task_type(&mut self) {
        let msg = self.user_message.to_lowercase();

        self.task_type = if msg.contains("code")
            || msg.contains("implement")
            || msg.contains("function")
            || msg.contains("class")
            || msg.contains("refactor")
        {
            TaskType::Coding
        } else if msg.contains("debug") || msg.contains("fix") || msg.contains("error") {
            TaskType::Debugging
        } else if msg.contains("explain")
            || msg.contains("how")
            || msg.contains("what")
            || msg.contains("why")
        {
            TaskType::Explanation
        } else if msg.contains("write") || msg.contains("draft") || msg.contains("compose") {
            TaskType::Writing
        } else if msg.contains("search") || msg.contains("find") || msg.contains("look up") {
            TaskType::Research
        } else if msg.contains("shell")
            || msg.contains("command")
            || msg.contains("terminal")
            || msg.contains("bash")
        {
            TaskType::System
        } else if msg.contains("plan") || msg.contains("steps") || msg.contains("break down") {
            TaskType::Planning
        } else if self.is_follow_up {
            TaskType::FollowUp
        } else {
            TaskType::General
        };
    }

    /// Set phase based on history length
    pub fn set_phase(mut self, history_len: usize) -> Self {
        self.phase = match history_len {
            0 => ConversationPhase::New,
            1..=5 => ConversationPhase::Early,
            6..=20 => ConversationPhase::Established,
            _ => ConversationPhase::Deep,
        };
        self
    }
}

/// Types of tasks the agent might handle
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskType {
    General,
    Coding,
    Debugging,
    Explanation,
    Writing,
    Research,
    System,
    Planning,
    FollowUp,
}

impl TaskType {
    /// Get task-specific instructions
    pub fn instructions(self) -> &'static str {
        match self {
            TaskType::Coding => {
                r#"## Coding Task Guidelines

When writing or modifying code:
- Follow language-specific best practices and conventions
- Add clear comments for complex logic
- Consider edge cases and error handling
- Prefer idiomatic solutions over clever ones
- Test your changes if possible
- Use type hints where appropriate
- Keep functions focused and modular"#
            }
            TaskType::Debugging => {
                r#"## Debugging Guidelines

When debugging:
- First understand the error message and context
- Check for common issues (typos, missing imports, type mismatches)
- Use systematic approach: isolate, reproduce, fix, verify
- Consider adding logging or print statements
- Test the fix thoroughly
- Explain what caused the issue and how you fixed it"#
            }
            TaskType::Explanation => {
                r#"## Explanation Guidelines

When explaining concepts:
- Start with a high-level overview
- Use analogies when helpful
- Provide concrete examples
- Break down complex ideas into simpler parts
- Adjust depth based on context clues
- Encourage questions if anything is unclear"#
            }
            TaskType::Writing => {
                r#"## Writing Guidelines

When drafting content:
- Consider the audience and purpose
- Use clear, concise language
- Structure with headings and sections
- Include specific examples and evidence
- Maintain consistent tone and style
- Review for clarity and flow"#
            }
            TaskType::Research => {
                r#"## Research Guidelines

When researching:
- Use web_search and browser tools to find current information
- Verify facts from multiple sources when possible
- Distinguish between facts and opinions
- Note the date of information (prefer recent sources)
- Summarize findings clearly
- Cite sources where appropriate"#
            }
            TaskType::System => {
                r#"## System Task Guidelines

When working with system commands:
- Explain what each command does before running
- Prefer safe, read-only commands when exploring
- Be cautious with destructive operations
- Check file paths and permissions
- Use shell tool with appropriate timeout
- Verify results after execution"#
            }
            TaskType::Planning => {
                r#"## Planning Guidelines

When creating plans:
- Break down into logical, actionable steps
- Consider dependencies between tasks
- Estimate complexity and effort
- Suggest appropriate tools for each step
- Allow for iterative refinement
- Track progress and adjust as needed"#
            }
            TaskType::FollowUp => {
                r#"## Follow-up Guidelines

This appears to be a follow-up question:
- Refer to previous context as needed
- Maintain consistency with earlier responses
- Ask for clarification if the reference is unclear
- Build upon previous work rather than starting over"#
            }
            TaskType::General => "",
        }
    }
}

/// Conversation phases
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationPhase {
    /// First interaction
    New,
    /// Early in conversation (1-5 messages)
    Early,
    /// Established conversation (6-20 messages)
    Established,
    /// Deep conversation (20+ messages)
    Deep,
}

impl ConversationPhase {
    /// Get phase-specific context
    pub fn context(self) -> &'static str {
        match self {
            ConversationPhase::New => {
                "This is the start of our conversation. I'm getting to know your style and preferences."
            }
            ConversationPhase::Early => {
                "We're still early in our conversation. I'm learning your preferences and building context."
            }
            ConversationPhase::Established => {
                "We have an established conversation with good context. I'll maintain consistency with previous work."
            }
            ConversationPhase::Deep => {
                "This is a deep, ongoing conversation. I have extensive context and will reference it appropriately while staying focused on the current task."
            }
        }
    }
}

/// Dynamic prompt builder
pub struct PromptBuilder {
    /// Base system prompt
    base_prompt: String,
    /// Maximum tokens for the full prompt
    max_tokens: usize,
    /// Current token count
    current_tokens: usize,
    /// Sections to include
    sections: Vec<PromptSection>,
}

impl PromptBuilder {
    /// Create a new prompt builder
    pub fn new(base_prompt: impl Into<String>) -> Self {
        let base = base_prompt.into();
        let base_tokens = base.len() / 4;

        Self {
            base_prompt: base,
            max_tokens: 4096,
            current_tokens: base_tokens,
            sections: Vec::new(),
        }
    }

    /// Set maximum tokens
    pub fn with_max_tokens(mut self, max: usize) -> Self {
        self.max_tokens = max;
        self
    }

    /// Add a section to the prompt
    pub fn add_section(&mut self, section: PromptSection) {
        self.current_tokens += section.token_estimate;
        self.sections.push(section);
    }

    /// Add task-specific instructions based on task type
    pub fn add_task_instructions(&mut self, task_type: TaskType) {
        let instructions = task_type.instructions();
        if !instructions.is_empty() {
            self.add_section(
                PromptSection::new("task_instructions", instructions)
                    .with_priority(8)
                    .dynamic(),
            );
        }
    }

    /// Add tool-specific guidance
    pub fn add_tool_guidance(&mut self, tools: &[FunctionDefinition], task_type: TaskType) {
        let relevant_tools: Vec<_> = tools
            .iter()
            .filter(|t| Self::is_tool_relevant(&t.name, task_type))
            .map(|t| format!("- {}: {}", t.name, t.description))
            .collect();

        if !relevant_tools.is_empty() {
            let content = format!(
                "## Relevant Tools for This Task\n\nFocus on these tools:\n{}",
                relevant_tools.join("\n")
            );

            self.add_section(
                PromptSection::new("tool_guidance", content)
                    .with_priority(7)
                    .dynamic(),
            );
        }
    }

    /// Add conversation phase context
    pub fn add_phase_context(&mut self, phase: ConversationPhase) {
        self.add_section(
            PromptSection::new("phase_context", phase.context())
                .with_priority(3)
                .dynamic(),
        );
    }

    /// Add active task/plan context
    pub fn add_task_context(&mut self, context: &str) {
        self.add_section(
            PromptSection::new("active_task", format!("## Current Task Context\n\n{}", context))
                .with_priority(9)
                .dynamic(),
        );
    }

    /// Add progress information
    pub fn add_progress(&mut self, completed: usize, total: usize, current_task: &str) {
        let content = format!(
            "## Progress\n\nCompleted: {}/{} tasks\nCurrent: {}\nProgress: {}%",
            completed,
            total,
            current_task,
            (completed as f32 / total as f32 * 100.0) as u8
        );

        self.add_section(
            PromptSection::new("progress", content)
                .with_priority(8)
                .dynamic(),
        );
    }

    /// Add recent conversation history context
    pub fn add_recent_context(&mut self, messages: &[Message], max_messages: usize) {
        if messages.is_empty() {
            return;
        }

        let recent: Vec<_> = messages.iter().rev().take(max_messages).rev().collect();

        let summary = recent
            .iter()
            .map(|m| format!("{:?}: {}", m.role, m.content.chars().take(100).collect::<String>()))
            .collect::<Vec<_>>()
            .join("\n");

        let content = format!("## Recent Conversation\n\n{}", summary);

        self.add_section(
            PromptSection::new("recent_context", content)
                .with_priority(4)
                .dynamic(),
        );
    }

    /// Add user preferences
    pub fn add_user_preferences(&mut self, preferences: &HashMap<String, String>) {
        if preferences.is_empty() {
            return;
        }

        let prefs: Vec<_> = preferences
            .iter()
            .map(|(k, v)| format!("- {}: {}", k, v))
            .collect();

        let content = format!("## User Preferences\n\n{}", prefs.join("\n"));

        self.add_section(
            PromptSection::new("user_preferences", content)
                .with_priority(6)
                .dynamic(),
        );
    }

    /// Build the final prompt, pruning if necessary
    pub fn build(self) -> String {
        // Sort sections by priority (highest first)
        let mut sections = self.sections;
        sections.sort_by(|a, b| b.priority.cmp(&a.priority));

        // Keep adding sections until we hit token limit
        let mut included = Vec::new();
        let mut total_tokens = self.current_tokens;

        for section in sections {
            if total_tokens + section.token_estimate <= self.max_tokens {
                total_tokens += section.token_estimate;
                included.push(section);
            }
        }

        // Sort included sections by name for consistent ordering
        included.sort_by(|a, b| a.name.cmp(&b.name));

        // Build final prompt
        let mut parts = vec![self.base_prompt];

        for section in included {
            parts.push(section.content);
        }

        parts.join("\n\n")
    }

    /// Build from a complete context in one call
    pub fn build_from_context(base_prompt: &str, ctx: &PromptContext, max_tokens: usize) -> String {
        let mut builder = Self::new(base_prompt).with_max_tokens(max_tokens);

        // Add phase context
        builder.add_phase_context(ctx.phase);

        // Add task instructions based on detected type
        builder.add_task_instructions(ctx.task_type);

        // Add tool guidance if tools are available
        if !ctx.available_tools.is_empty() {
            builder.add_tool_guidance(&ctx.available_tools, ctx.task_type);
        }

        // Add task context if present
        if let Some(ref task_ctx) = ctx.task_context {
            builder.add_task_context(task_ctx);
        }

        // Add recent conversation context
        if !ctx.recent_history.is_empty() {
            builder.add_recent_context(&ctx.recent_history, 3);
        }

        // Add user preferences
        if !ctx.user_preferences.is_empty() {
            builder.add_user_preferences(&ctx.user_preferences);
        }

        builder.build()
    }

    /// Check if a tool is relevant for a task type
    fn is_tool_relevant(tool_name: &str, task_type: TaskType) -> bool {
        match task_type {
            TaskType::Coding => {
                matches!(
                    tool_name,
                    "file_read"
                        | "file_write"
                        | "file_edit"
                        | "glob"
                        | "grep"
                        | "shell"
                        | "code_execution"
                )
            }
            TaskType::Debugging => {
                matches!(
                    tool_name,
                    "file_read" | "grep" | "shell" | "code_execution" | "browser" | "web_search"
                )
            }
            TaskType::Research => {
                matches!(tool_name, "web_search" | "web_fetch" | "browser" | "file_read" | "shell")
            }
            TaskType::System => {
                matches!(tool_name, "shell" | "file_read" | "file_write" | "browser")
            }
            TaskType::Planning => {
                matches!(tool_name, "todo" | "file_read" | "shell" | "browser" | "web_search")
            }
            _ => true, // All tools relevant for general tasks
        }
    }
}

/// Extension trait for easy prompt building integration
#[async_trait::async_trait]
pub trait DynamicPrompt {
    /// Build dynamic system prompt for current context
    async fn build_system_prompt(
        &self,
        base_prompt: &str,
        user_message: &str,
        available_tools: &[FunctionDefinition],
    ) -> String;
}

// Re-export for convenience
pub use async_trait::async_trait;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_section_creation() {
        let section = PromptSection::new("test", "This is test content").with_priority(8);

        assert_eq!(section.name, "test");
        assert_eq!(section.priority, 8);
        assert!(section.token_estimate > 0);
    }

    #[test]
    fn test_task_type_detection() {
        let mut ctx = PromptContext::new("How do I implement a binary search in Rust?");
        ctx.detect_task_type();
        assert_eq!(ctx.task_type, TaskType::Coding);

        let mut ctx2 = PromptContext::new("Explain quantum computing to me");
        ctx2.detect_task_type();
        assert_eq!(ctx2.task_type, TaskType::Explanation);

        let mut ctx3 = PromptContext::new("Search for recent AI news");
        ctx3.detect_task_type();
        assert_eq!(ctx3.task_type, TaskType::Research);
    }

    #[test]
    fn test_prompt_builder() {
        let mut builder = PromptBuilder::new("Base prompt").with_max_tokens(1000);

        builder.add_section(PromptSection::new("section1", "Content 1").with_priority(5));
        builder.add_section(PromptSection::new("section2", "Content 2").with_priority(9));

        let prompt = builder.build();
        assert!(prompt.contains("Base prompt"));
        assert!(prompt.contains("Content 1"));
        assert!(prompt.contains("Content 2"));
    }

    #[test]
    fn test_task_type_instructions() {
        let coding = TaskType::Coding.instructions();
        assert!(coding.contains("Coding"));
        assert!(coding.contains("best practices"));

        let debug = TaskType::Debugging.instructions();
        assert!(debug.contains("Debugging"));
        assert!(debug.contains("error"));
    }

    #[test]
    fn test_prompt_pruning() {
        // Create a builder with small token limit to test pruning
        let mut builder = PromptBuilder::new("Base").with_max_tokens(50);

        // Add many large sections
        for i in 0..10 {
            builder.add_section(
                PromptSection::new(
                    format!("section{}", i),
                    "A".repeat(100), // ~25 tokens each
                )
                .with_priority(i as u8),
            );
        }

        let prompt = builder.build();
        // Should have been pruned to fit within token limit
        assert!(prompt.len() < 1000);
    }
}
