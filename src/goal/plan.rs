//! Goal plan — the parsed result of a `/goal` command.
//!
//! A [`GoalPlan`] contains the human-readable goal description and a list of
//! structured [`GoalCondition`](super::condition::GoalCondition)s to check.

use crate::error::SyscityError;
use crate::goal::condition::GoalCondition;
use crate::model_router::ModelRouter;
use crate::providers::Message;

/// System prompt for the goal parsing LLM call.
const GOAL_PARSE_SYSTEM_PROMPT: &str = r#"You are a goal analyzer. Given a goal description, output JSON:
{"description": "...", "conditions": [...], "max_rounds": 5}

Supported condition types:
- {"type": "exit_code", "command": "...", "expected": 0}
- {"type": "file_exists", "path": "..."}
- {"type": "numeric", "command": "...", "operator": ">="|"<="|">"|"<"|"==", "threshold": N}
- {"type": "pattern", "command": "...", "must_contain": "..."}
- {"type": "static_analysis", "command": "..."}

Generate conditions programmatically checkable for the goal. Return ONLY valid JSON."#;

/// A parsed goal plan produced by LLM interpretation of a `/goal` command.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GoalPlan {
    /// Human-readable description of the goal.
    pub description: String,
    /// Ordered list of conditions to check (AND — all must pass).
    pub conditions: Vec<GoalCondition>,
    /// Maximum number of retry rounds.
    #[serde(default = "default_max_rounds")]
    pub max_rounds: usize,
    /// Optional model override for the sub-agent.
    #[serde(default)]
    pub model_override: Option<String>,
}

fn default_max_rounds() -> usize {
    5
}

impl GoalPlan {
    /// Create a new goal plan.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            conditions: Vec::new(),
            max_rounds: default_max_rounds(),
            model_override: None,
        }
    }

    /// Add a condition to the plan.
    pub fn with_condition(mut self, condition: GoalCondition) -> Self {
        self.conditions.push(condition);
        self
    }

    /// Set max rounds.
    pub fn with_max_rounds(mut self, rounds: usize) -> Self {
        self.max_rounds = rounds.max(1);
        self
    }

    /// Set model override.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model_override = Some(model.into());
        self
    }

    /// Check if the plan is valid (has at least one condition).
    pub fn is_valid(&self) -> bool {
        !self.conditions.is_empty() && !self.description.is_empty()
    }

    /// Parse a goal description into a [`GoalPlan`] using an LLM.
    ///
    /// Sends the user's description to the model router with a system prompt
    /// and parses the JSON response into a `GoalPlan`.
    pub async fn parse_with_llm(
        router: &ModelRouter,
        description: &str,
        max_rounds: Option<usize>,
    ) -> crate::Result<Self> {
        let messages = vec![
            Message::system(GOAL_PARSE_SYSTEM_PROMPT),
            Message::user(description),
        ];

        let response = router.complete_auto(messages, None).await?;
        let content = response.message.content;

        // Find JSON in the response (handle possible markdown fences).
        let json_str = if let Some(start) = content.find('{') {
            let end = content.rfind('}').unwrap_or(content.len());
            &content[start..=end]
        } else {
            &content
        };

        let mut plan: GoalPlan = serde_json::from_str(json_str).map_err(|e| {
            SyscityError::Internal(format!(
                "Failed to parse LLM response as GoalPlan: {}. Response: {}",
                e, content
            ))
        })?;

        if let Some(r) = max_rounds {
            plan.max_rounds = r;
        }

        if !plan.is_valid() {
            return Err(SyscityError::Internal(format!(
                "LLM returned invalid plan: no conditions or empty description. Response: {}",
                content
            )));
        }

        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::condition::Comparison;

    #[test]
    fn test_goal_plan_new() {
        let plan = GoalPlan::new("write tests");
        assert_eq!(plan.description, "write tests");
        assert!(plan.conditions.is_empty());
        assert_eq!(plan.max_rounds, 5);
        assert!(plan.model_override.is_none());
    }

    #[test]
    fn test_goal_plan_with_condition() {
        let plan = GoalPlan::new("test")
            .with_condition(GoalCondition::ExitCode {
                command: "cargo test".to_string(),
                expected: Some(0),
            })
            .with_condition(GoalCondition::FileExists { path: "Cargo.toml".to_string() });
        assert_eq!(plan.conditions.len(), 2);
    }

    #[test]
    fn test_goal_plan_with_max_rounds() {
        let plan = GoalPlan::new("test").with_max_rounds(10);
        assert_eq!(plan.max_rounds, 10);

        // Minimum 1
        let plan = GoalPlan::new("test").with_max_rounds(0);
        assert_eq!(plan.max_rounds, 1);
    }

    #[test]
    fn test_goal_plan_with_model() {
        let plan = GoalPlan::new("test").with_model("claude-sonnet-4-6");
        assert_eq!(plan.model_override, Some("claude-sonnet-4-6".to_string()));
    }

    #[test]
    fn test_goal_plan_is_valid() {
        let plan = GoalPlan::new("test").with_condition(GoalCondition::ExitCode {
            command: "true".to_string(),
            expected: None,
        });
        assert!(plan.is_valid());
    }

    #[test]
    fn test_goal_plan_is_valid_empty_fails() {
        let plan = GoalPlan::new("");
        assert!(!plan.is_valid());

        let plan = GoalPlan::new("test");
        assert!(!plan.is_valid()); // no conditions
    }

    #[test]
    fn test_goal_plan_serialize_roundtrip() {
        let plan = GoalPlan::new("write tests")
            .with_condition(GoalCondition::ExitCode {
                command: "cargo test".to_string(),
                expected: Some(0),
            })
            .with_condition(GoalCondition::Numeric {
                command: "grep -c fn test_ src/lib.rs".to_string(),
                operator: Comparison::Ge,
                threshold: 5.0,
            })
            .with_max_rounds(3)
            .with_model("gpt-4o");

        let json = serde_json::to_string_pretty(&plan).unwrap();
        let deserialized: GoalPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.description, "write tests");
        assert_eq!(deserialized.conditions.len(), 2);
        assert_eq!(deserialized.max_rounds, 3);
        assert_eq!(deserialized.model_override, Some("gpt-4o".to_string()));
    }

    #[test]
    fn test_goal_plan_default_max_rounds_in_json() {
        // Missing max_rounds should default to 5
        let json = r#"{"description":"test","conditions":[]}"#;
        let plan: GoalPlan = serde_json::from_str(json).unwrap();
        assert_eq!(plan.max_rounds, 5);
    }

    #[test]
    fn test_goal_plan_deserialize_with_conditions() {
        let json = r#"{
            "description": "run tests",
            "conditions": [
                {"type": "exit_code", "command": "cargo test", "expected": 0},
                {"type": "file_exists", "path": "/tmp/report.txt"}
            ],
            "max_rounds": 5
        }"#;
        let plan: GoalPlan = serde_json::from_str(json).unwrap();
        assert_eq!(plan.description, "run tests");
        assert_eq!(plan.conditions.len(), 2);
    }
}
