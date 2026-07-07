//! Trajectory types for conversation-level reflection.
//!
//! Represents a window of the interaction history — user messages, assistant
//! responses, tool calls, and tool results — for trajectory-aware evaluation
//! by the [`NudgeEngine`](super::nudge::NudgeEngine).

/// A single step within a conversation turn.
#[derive(Debug, Clone)]
pub enum TrajectoryStep {
    /// The user's input message.
    UserMessage {
        content: String,
    },
    /// The assistant's textual response.
    AssistantResponse {
        content: String,
    },
    /// A tool call made by the assistant.
    ToolCall {
        name: String,
        args: String,
    },
    /// The result returned by a tool.
    ToolResult {
        name: String,
        content: String,
    },
}

/// One user→assistant exchange within a trajectory window.
#[derive(Debug, Clone)]
pub struct TrajectoryWindow {
    /// 0-based turn index within the thread.
    pub index: usize,
    /// The user's message that started this turn.
    pub user_message: String,
    /// Ordered steps within this turn (response, tool calls, results, …).
    pub steps: Vec<TrajectoryStep>,
}

/// A formatted conversation window for trajectory evaluation.
#[derive(Debug, Clone)]
pub struct Trajectory {
    /// Turns included in this window.
    pub turns: Vec<TrajectoryWindow>,
    /// Total number of turns in the full conversation.
    pub total_turns: usize,
    /// Number of turns in this window.
    pub window_size: usize,
}

impl Trajectory {
    /// Format the trajectory into a human-readable string for the critic prompt.
    pub fn format_for_prompt(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "=== CONVERSATION TRAJECTORY (last {} of {} turns) ===\n\n",
            self.window_size, self.total_turns
        ));

        for (i, turn) in self.turns.iter().enumerate() {
            out.push_str(&format!("--- Turn {} ---\n", i + 1));
            out.push_str(&format!("User: {}\n", turn.user_message));

            for step in &turn.steps {
                match step {
                    TrajectoryStep::UserMessage { content } => {
                        out.push_str(&format!("User: {}\n", content));
                    }
                    TrajectoryStep::AssistantResponse { content } => {
                        out.push_str(&format!("Assistant: {}\n", content));
                    }
                    TrajectoryStep::ToolCall { name, args } => {
                        // Truncate long args for readability
                        let args_preview = if args.len() > 200 {
                            format!("{}…", &args[..200])
                        } else {
                            args.clone()
                        };
                        out.push_str(&format!("[Tool call: {}({})]\n", name, args_preview));
                    }
                    TrajectoryStep::ToolResult { name, content } => {
                        // Truncate long results
                        let result_preview = if content.len() > 300 {
                            format!("{}…", &content[..300])
                        } else {
                            content.clone()
                        };
                        out.push_str(&format!("[Tool result: {} → {}]\n", name, result_preview));
                    }
                }
            }
            out.push('\n');
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_trajectory_single_turn() {
        let trajectory = Trajectory {
            turns: vec![TrajectoryWindow {
                index: 0,
                user_message: "Hello".to_string(),
                steps: vec![TrajectoryStep::AssistantResponse {
                    content: "Hi there!".to_string(),
                }],
            }],
            total_turns: 5,
            window_size: 1,
        };

        let formatted = trajectory.format_for_prompt();
        assert!(formatted.contains("CONVERSATION TRAJECTORY"));
        assert!(formatted.contains("last 1 of 5 turns"));
        assert!(formatted.contains("User: Hello"));
        assert!(formatted.contains("Assistant: Hi there!"));
    }

    #[test]
    fn test_format_trajectory_with_tool_calls() {
        let trajectory = Trajectory {
            turns: vec![TrajectoryWindow {
                index: 0,
                user_message: "Search for Rust".to_string(),
                steps: vec![
                    TrajectoryStep::ToolCall {
                        name: "search_web".to_string(),
                        args: r#"{"query": "Rust programming"}"#.to_string(),
                    },
                    TrajectoryStep::ToolResult {
                        name: "search_web".to_string(),
                        content: "Rust is a systems programming language…".to_string(),
                    },
                    TrajectoryStep::AssistantResponse {
                        content: "Here's what I found about Rust.".to_string(),
                    },
                ],
            }],
            total_turns: 1,
            window_size: 1,
        };

        let formatted = trajectory.format_for_prompt();
        assert!(formatted.contains("[Tool call: search_web("));
        assert!(formatted.contains("[Tool result: search_web →"));
    }

    #[test]
    fn test_format_trajectory_truncates_long_content() {
        let long_args = "x".repeat(500);
        let trajectory = Trajectory {
            turns: vec![TrajectoryWindow {
                index: 0,
                user_message: "Do something".to_string(),
                steps: vec![TrajectoryStep::ToolCall {
                    name: "big_tool".to_string(),
                    args: long_args,
                }],
            }],
            total_turns: 1,
            window_size: 1,
        };

        let formatted = trajectory.format_for_prompt();
        assert!(formatted.contains("…"));
        // Should be about 200 chars + ellipsis + label overhead
        let tool_line = formatted.lines().find(|l| l.contains("[Tool call: big_tool(")).unwrap();
        assert!(tool_line.len() < 350, "args should be truncated: {}", tool_line.len());
    }

    #[test]
    fn test_trajectory_multiple_turns() {
        let trajectory = Trajectory {
            turns: vec![
                TrajectoryWindow {
                    index: 0,
                    user_message: "Turn 1".to_string(),
                    steps: vec![TrajectoryStep::AssistantResponse {
                        content: "Response 1".to_string(),
                    }],
                },
                TrajectoryWindow {
                    index: 1,
                    user_message: "Turn 2".to_string(),
                    steps: vec![TrajectoryStep::AssistantResponse {
                        content: "Response 2".to_string(),
                    }],
                },
            ],
            total_turns: 10,
            window_size: 2,
        };

        let formatted = trajectory.format_for_prompt();
        assert!(formatted.contains("--- Turn 1 ---"));
        assert!(formatted.contains("--- Turn 2 ---"));
        assert!(formatted.contains("User: Turn 1"));
        assert!(formatted.contains("User: Turn 2"));
    }
}
