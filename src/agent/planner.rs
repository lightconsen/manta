//! Natural Language Task Planning for Manta
//!
//! Automatically decomposes complex user requests into structured task plans
//! using LLM-based analysis. Integrates with the Todo system for execution.

use super::todo::{Task, TaskStatus, TodoStore};
use crate::providers::{CompletionRequest, Message, Provider, Role};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// A planned task with dependencies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedTask {
    /// Task ID
    pub id: String,
    /// Task description
    pub description: String,
    /// Estimated complexity (1-5)
    pub complexity: u8,
    /// Dependencies (task IDs that must complete first)
    pub dependencies: Vec<String>,
    /// Suggested tools to use
    pub suggested_tools: Vec<String>,
    /// Expected outcome
    pub expected_outcome: String,
}

/// A complete task plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlan {
    /// Plan ID
    pub id: String,
    /// Original user request
    pub original_request: String,
    /// Overall goal
    pub goal: String,
    /// List of tasks in execution order
    pub tasks: Vec<PlannedTask>,
    /// Current task index
    pub current_task_index: usize,
    /// Whether the plan is complete
    pub is_complete: bool,
}

impl TaskPlan {
    /// Create a new task plan
    pub fn new(original_request: impl Into<String>, goal: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            original_request: original_request.into(),
            goal: goal.into(),
            tasks: Vec::new(),
            current_task_index: 0,
            is_complete: false,
        }
    }

    /// Get the current task
    pub fn current_task(&self) -> Option<&PlannedTask> {
        self.tasks.get(self.current_task_index)
    }

    /// Move to the next task
    pub fn advance(&mut self) {
        if self.current_task_index < self.tasks.len().saturating_sub(1) {
            self.current_task_index += 1;
        } else {
            self.is_complete = true;
        }
    }

    /// Check if all dependencies for a task are satisfied
    pub fn dependencies_met(&self, task: &PlannedTask, completed_tasks: &[String]) -> bool {
        task.dependencies.iter().all(|dep| completed_tasks.contains(dep))
    }

    /// Get progress as percentage
    pub fn progress_percent(&self) -> u8 {
        if self.tasks.is_empty() {
            return 100;
        }
        ((self.current_task_index as f32 / self.tasks.len() as f32) * 100.0) as u8
    }

    /// Format plan for display
    pub fn format_summary(&self) -> String {
        let mut lines = vec![
            format!("🎯 Goal: {}", self.goal),
            format!("📋 Tasks: {}/{}", self.current_task_index + 1, self.tasks.len()),
            format!("📊 Progress: {}%", self.progress_percent()),
            "".to_string(),
        ];

        for (i, task) in self.tasks.iter().enumerate() {
            let status = if i < self.current_task_index {
                "✅"
            } else if i == self.current_task_index {
                "🔄"
            } else {
                "⏳"
            };
            lines.push(format!("{} {}. {}", status, i + 1, task.description));
        }

        lines.join("\n")
    }
}

/// Task planner using LLM for natural language decomposition
pub struct TaskPlanner {
    provider: Arc<dyn Provider>,
    model: Option<String>,
}

impl TaskPlanner {
    /// Create a new task planner
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self {
            provider,
            model: None,
        }
    }

    /// Set the model to use for completions
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Analyze if a request needs task planning (is it complex?)
    pub async fn needs_planning(&self, request: &str) -> bool {
        // Quick heuristic check first
        let complexity_indicators = [
            "steps",
            "plan",
            "break down",
            "decompose",
            "implement",
            "build",
            "create",
            "design",
            "refactor",
            "migrate",
            "set up",
            "configure",
            "and then",
            "first",
            "after that",
            "finally",
            "multiple",
            "complex",
            "complicated",
        ];

        let request_lower = request.to_lowercase();
        let indicator_count = complexity_indicators
            .iter()
            .filter(|&&indicator| request_lower.contains(indicator))
            .count();

        // If multiple complexity indicators, definitely needs planning
        if indicator_count >= 2 {
            return true;
        }

        // If very short, probably doesn't need planning
        if request.len() < 30 {
            return false;
        }

        // Use LLM to analyze complexity
        let prompt = format!(
            r#"Analyze this user request and determine if it requires multiple steps to complete.

Request: "{}"

A request NEEDS multi-step planning if it:
- Requires implementing/building/creating something
- Has multiple distinct phases or components
- Needs research followed by action
- Involves "do X, then Y, then Z"
- Is a complex task that should be broken down

A request does NOT need planning if it:
- Is a simple question
- Asks for explanation/advice
- Is a single straightforward action
- Is conversational/greeting

Reply with ONLY "PLAN" if it needs multi-step planning, or "SIMPLE" if it's a simple request."#,
            request
        );

        let completion_req = CompletionRequest {
            model: self.model.clone(),
            messages: vec![Message::user(&prompt)],
            temperature: Some(0.0),
            max_tokens: Some(10),
            stream: false,
            ..Default::default()
        };

        match self.provider.complete(completion_req).await {
            Ok(response) => {
                let content = response.message.content.trim().to_uppercase();
                content.contains("PLAN")
            }
            Err(e) => {
                warn!("Failed to analyze complexity: {}, defaulting to simple", e);
                false
            }
        }
    }

    /// Create a task plan from a natural language request
    pub async fn create_plan(&self, request: &str) -> crate::Result<TaskPlan> {
        info!("Creating task plan for request: {}", request);

        let prompt = format!(
            r#"You are a task planning assistant. Break down the following request into a structured plan.

User Request: "{}"

Create a detailed plan with 3-7 specific, actionable tasks. For each task:
1. Give it a clear, actionable description
2. Estimate complexity (1-5, where 5 is most complex)
3. List dependencies (which task numbers must complete first)
4. Suggest relevant tools (file_read, file_write, shell, web_search, browser, code_execution, etc.)
5. Describe the expected outcome

Format your response as a JSON object with this structure:
{{
  "goal": "Brief overall goal",
  "tasks": [
    {{
      "id": "task_1",
      "description": "Specific action to take",
      "complexity": 3,
      "dependencies": [],
      "suggested_tools": ["tool_name"],
      "expected_outcome": "What this accomplishes"
    }}
  ]
}}

Ensure tasks are in logical execution order. Tasks with no dependencies should come first."#,
            request
        );

        let completion_req = CompletionRequest {
            model: self.model.clone(),
            messages: vec![Message::user(&prompt)],
            temperature: Some(0.3),
            max_tokens: Some(2000),
            stream: false,
            ..Default::default()
        };

        let response = self.provider.complete(completion_req).await?;
        let content = response.message.content;

        // Extract JSON from response (handle markdown code blocks)
        let json_str = Self::extract_json(&content)?;

        // Parse the plan
        let plan_data: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| crate::error::MantaError::Validation(
                format!("Failed to parse task plan: {}", e)
            ))?;

        let goal = plan_data["goal"]
            .as_str()
            .unwrap_or("Complete the request")
            .to_string();

        let mut plan = TaskPlan::new(request, goal);

        // Parse tasks
        if let Some(tasks) = plan_data["tasks"].as_array() {
            for (i, task_data) in tasks.iter().enumerate() {
                let task = PlannedTask {
                    id: task_data["id"]
                        .as_str()
                        .unwrap_or(&format!("task_{}", i + 1))
                        .to_string(),
                    description: task_data["description"]
                        .as_str()
                        .unwrap_or("Unnamed task")
                        .to_string(),
                    complexity: task_data["complexity"].as_u64().unwrap_or(3) as u8,
                    dependencies: task_data["dependencies"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    suggested_tools: task_data["suggested_tools"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    expected_outcome: task_data["expected_outcome"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                };
                plan.tasks.push(task);
            }
        }

        info!("Created plan with {} tasks", plan.tasks.len());
        Ok(plan)
    }

    /// Convert a TaskPlan to a TodoStore for execution
    pub fn plan_to_todos(&self, plan: &TaskPlan) -> TodoStore {
        let mut store = TodoStore::new();

        for planned_task in &plan.tasks {
            let content = format!(
                "{} (Tools: {})",
                planned_task.description,
                planned_task.suggested_tools.join(", ")
            );

            let mut task = store.create_task_with_id(&planned_task.id, content);

            // Set priority based on complexity (higher complexity = higher priority to tackle early)
            task.set_priority(planned_task.complexity);

            // Store metadata
            task = task.with_metadata("expected_outcome", planned_task.expected_outcome.clone());
            task = task.with_metadata("complexity", planned_task.complexity as i32);

            // Update the store with our configured task
            store.update(task);
        }

        store
    }

    /// Refine a plan based on intermediate results
    pub async fn refine_plan(
        &self,
        plan: &mut TaskPlan,
        completed_task_id: &str,
        result_summary: &str,
    ) -> crate::Result<()> {
        debug!("Refining plan after completing: {}", completed_task_id);

        // Check if we need to adjust remaining tasks based on results
        let prompt = format!(
            r#"A task in the plan has been completed. Review and potentially adjust remaining tasks.

Original Goal: {}
Completed Task: {}
Result Summary: {}

Remaining Tasks:
{}

Should any remaining tasks be modified, added, or removed based on this result?
If no changes needed, reply "NO_CHANGES".

If changes are needed, provide a JSON object with:
{{
  "updates": [
    {{"task_id": "task_2", "new_description": "Updated description"}}
  ],
  "additions": [
    {{"after_task": "task_2", "description": "New task", "complexity": 3}}
  ],
  "removals": ["task_3"]
}}"#,
            plan.goal,
            completed_task_id,
            result_summary,
            plan.tasks
                .iter()
                .skip(plan.current_task_index)
                .map(|t| format!("- {}: {}", t.id, t.description))
                .collect::<Vec<_>>()
                .join("\n")
        );

        let completion_req = CompletionRequest {
            model: self.model.clone(),
            messages: vec![Message::user(&prompt)],
            temperature: Some(0.3),
            max_tokens: Some(1000),
            stream: false,
            ..Default::default()
        };

        let response = self.provider.complete(completion_req).await?;
        let content = response.message.content.trim();

        if content == "NO_CHANGES" || content.contains("NO_CHANGES") {
            return Ok(());
        }

        // Try to parse and apply changes
        if let Ok(json_str) = Self::extract_json(content) {
            if let Ok(changes) = serde_json::from_str::<serde_json::Value>(&json_str) {
                // Apply updates
                if let Some(updates) = changes["updates"].as_array() {
                    for update in updates {
                        if let (Some(task_id), Some(new_desc)) = (
                            update["task_id"].as_str(),
                            update["new_description"].as_str(),
                        ) {
                            if let Some(task) = plan.tasks.iter_mut().find(|t| t.id == task_id) {
                                task.description = new_desc.to_string();
                            }
                        }
                    }
                }

                // Note: Additions and removals would require more complex logic
                // to maintain task dependencies and ordering
            }
        }

        Ok(())
    }

    /// Extract JSON from text that might contain markdown or other content
    fn extract_json(text: &str) -> crate::Result<String> {
        // Try to find JSON between code blocks
        if let Some(start) = text.find("```json") {
            if let Some(end) = text[start + 7..].find("```") {
                return Ok(text[start + 7..start + 7 + end].trim().to_string());
            }
        }

        // Try to find JSON between generic code blocks
        if let Some(start) = text.find("```") {
            if let Some(end) = text[start + 3..].find("```") {
                let candidate = text[start + 3..start + 3 + end].trim();
                if candidate.starts_with('{') || candidate.starts_with('[') {
                    return Ok(candidate.to_string());
                }
            }
        }

        // Look for JSON object/array directly
        if let Some(start) = text.find('{') {
            if let Some(end) = text.rfind('}') {
                if end > start {
                    return Ok(text[start..=end].to_string());
                }
            }
        }

        // Assume the whole text is JSON
        Ok(text.trim().to_string())
    }
}

/// Active plan tracking for a conversation
pub struct ActivePlan {
    /// The plan being executed
    pub plan: TaskPlan,
    /// Todo store for tracking
    pub todos: TodoStore,
    /// Completed task IDs
    pub completed_tasks: Vec<String>,
}

impl ActivePlan {
    /// Mark current task as complete and advance
    pub fn complete_current_task(&mut self) {
        if let Some(task) = self.plan.current_task() {
            self.completed_tasks.push(task.id.clone());

            // Update todo store
            if let Some(todo) = self.todos.get_mut(&task.id) {
                todo.complete();
            }
        }
        self.plan.advance();
    }

    /// Get current task description for the agent
    pub fn current_task_prompt(&self) -> Option<String> {
        self.plan.current_task().map(|task| {
            format!(
                "Current Task ({} of {}): {}\nExpected Outcome: {}\nSuggested Tools: {}",
                self.plan.current_task_index + 1,
                self.plan.tasks.len(),
                task.description,
                task.expected_outcome,
                task.suggested_tools.join(", ")
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_plan_progress() {
        let mut plan = TaskPlan::new("test request", "test goal");
        plan.tasks.push(PlannedTask {
            id: "task_1".to_string(),
            description: "First task".to_string(),
            complexity: 2,
            dependencies: vec![],
            suggested_tools: vec![],
            expected_outcome: "Done".to_string(),
        });
        plan.tasks.push(PlannedTask {
            id: "task_2".to_string(),
            description: "Second task".to_string(),
            complexity: 3,
            dependencies: vec!["task_1".to_string()],
            suggested_tools: vec![],
            expected_outcome: "Done".to_string(),
        });

        assert_eq!(plan.progress_percent(), 0);
        plan.advance();
        assert_eq!(plan.progress_percent(), 50);
        plan.advance();
        assert!(plan.is_complete);
    }

    #[test]
    fn test_extract_json() {
        let text = r#"Some text
```json
{"key": "value"}
```
More text"#;
        let result = TaskPlanner::extract_json(text).unwrap();
        assert!(result.contains("\"key\": \"value\""));

        let text2 = r#"{"direct": "json"}"#;
        let result2 = TaskPlanner::extract_json(text2).unwrap();
        assert!(result2.contains("\"direct\": \"json\""));
    }
}
