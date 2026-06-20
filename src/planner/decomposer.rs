//! LLM-based goal decomposer — break high-level goals into executable subtask DAGs.
//!
//! The [`GoalDecomposer`] uses an LLM provider to analyse a goal, the available
//! tool set, and emit a JSON array of [`SubTask`]s with dependency edges.
//! These subtasks are then converted into the planner's [`Task`] / [`Plan`] types.

use crate::computer::{DesktopAction, VerificationCriteria};
use crate::providers::{CompletionRequest, Message, Provider};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A single subtask produced by the LLM decomposer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTask {
    /// Unique identifier (kebab-case recommended, e.g. "open-browser").
    pub id: String,
    /// Human-readable description of what this step does.
    pub description: String,
    /// IDs of subtasks that must complete before this one starts.
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Optional hint about which tool to use (e.g. "macos_screenshot", "browser").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_hint: Option<String>,
    /// The concrete desktop action to execute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<DesktopAction>,
    /// How to verify this subtask succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<VerificationCriteria>,
    /// Retry budget for this subtask (0 = no retries).
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

fn default_max_retries() -> u32 {
    2
}

impl SubTask {
    /// Convert a [`SubTask`] into a planner [`Task`].
    ///
    /// If no action was provided by the LLM, a zero-duration [`DesktopAction::Wait`]
    /// is used as a placeholder so the executor can still run the step and apply
    /// verification / retry logic.
    pub fn into_task(self) -> super::Task {
        let action = self.action.unwrap_or(DesktopAction::Wait { milliseconds: 0 });
        let mut task = super::Task::new(self.id, self.description, action);
        for dep in self.dependencies {
            task = task.depends_on(dep);
        }
        if let Some(v) = self.verification {
            task = task.with_verification(v);
        }
        task = task.with_retries(self.max_retries);
        task
    }
}

const DEFAULT_MAX_TASKS: usize = 50;

/// Decomposes high-level natural-language goals into executable subtask DAGs.
#[derive(Clone)]
pub struct GoalDecomposer {
    provider: Arc<dyn Provider>,
    max_tasks: usize,
}

impl GoalDecomposer {
    /// Create a new decomposer backed by the given LLM provider.
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self {
            provider,
            max_tasks: DEFAULT_MAX_TASKS,
        }
    }

    /// Set the maximum number of subtasks allowed (default: 50).
    pub fn with_max_tasks(mut self, n: usize) -> Self {
        self.max_tasks = n;
        self
    }

    /// Decompose a goal into a list of [`SubTask`]s.
    ///
    /// # Arguments
    /// * `goal` — High-level natural language description of what to achieve.
    /// * `available_tools` — Names of tools the agent has access to (injected into
    ///   the prompt so the LLM can pick appropriate ones).
    ///
    /// # Errors
    /// Returns an error if the LLM call fails or if the response cannot be parsed
    /// into valid subtasks.
    pub async fn decompose(
        &self,
        goal: &str,
        available_tools: &[String],
    ) -> crate::Result<Vec<SubTask>> {
        self.decompose_with_context(goal, available_tools, "").await
    }

    /// Decompose a goal with additional context (e.g., past experiences).
    ///
    /// The `extra_context` string is appended to the user prompt so the LLM
    /// can learn from previous similar plans.
    pub async fn decompose_with_context(
        &self,
        goal: &str,
        available_tools: &[String],
        extra_context: &str,
    ) -> crate::Result<Vec<SubTask>> {
        let prompt = build_decomposition_prompt(goal, available_tools, extra_context);
        let request = CompletionRequest {
            messages: vec![
                Message::system(DECOMPOSITION_SYSTEM_PROMPT),
                Message::user(prompt),
            ],
            temperature: Some(0.2),
            max_tokens: Some(4096),
            stream: false,
            requires_reasoning: true,
            ..Default::default()
        };

        let response = self.provider.complete(request).await?;
        let content = response.message.content.trim();

        // LLMs may wrap JSON in markdown code fences; strip them.
        let json_str = strip_code_fences(content);

        let subtasks: Vec<SubTask> = serde_json::from_str(json_str).map_err(|e| {
            crate::error::SyscityError::Validation(format!(
                "Failed to parse decomposed subtasks: {}. Raw response: {}",
                e,
                &content[..content.len().min(500)]
            ))
        })?;

        // Validate: no self-dependencies.
        for st in &subtasks {
            if st.dependencies.contains(&st.id) {
                return Err(crate::error::SyscityError::Validation(format!(
                    "Subtask '{}' depends on itself",
                    st.id
                )));
            }
        }

        // Validate: all dependency IDs exist.
        let ids: std::collections::HashSet<_> = subtasks.iter().map(|s| &s.id).collect();
        for st in &subtasks {
            for dep in &st.dependencies {
                if !ids.contains(dep) {
                    return Err(crate::error::SyscityError::Validation(format!(
                        "Subtask '{}' depends on unknown task '{}'",
                        st.id, dep
                    )));
                }
            }
        }

        // Validate: no cycles (Kahn's algorithm).
        if let Some(cycle) = detect_cycle(&subtasks) {
            return Err(crate::error::SyscityError::Validation(format!(
                "Cycle detected in subtask dependencies: {:?}",
                cycle
            )));
        }

        // Limit: cap total subtasks to prevent runaway generation.
        if subtasks.len() > self.max_tasks {
            return Err(crate::error::SyscityError::Validation(format!(
                "Decomposition produced {} subtasks, exceeding the maximum of {}",
                subtasks.len(),
                self.max_tasks
            )));
        }

        Ok(subtasks)
    }
}

const DECOMPOSITION_SYSTEM_PROMPT: &str = r#"You are a task-decomposition engine for an AI agent that controls a computer.

Your job is to break a high-level goal into small, concrete, executable subtasks arranged as a DAG (directed acyclic graph).

Rules:
1. Output ONLY a JSON array. No markdown, no explanations outside the JSON.
2. Each subtask must have a unique "id" (kebab-case), a "description", and optionally "dependencies" (array of ids).
3. The "action" field must be a valid DesktopAction JSON object when the step is something the computer can execute directly (click, type, screenshot, launch_app, etc.). If you are unsure of the exact parameters, you may omit "action".
4. Use "verification" to describe how success is checked (e.g. ui_tree_contains, screenshot_changed, success).
5. Prefer small steps over large ones. A subtask should map to roughly one user interaction.
6. "tool_hint" can suggest which platform tool to use (e.g. "browser", "macos_accessibility", "windows_accessibility").

Available DesktopAction types (JSON shape):
- { "screenshot": { "region": null } }
- { "click": { "target": { "coordinate": { "x": 100, "y": 200 } }, "button": "left" } }
- { "double_click": { "target": { "coordinate": { "x": 100, "y": 200 } }, "button": "left" } }
- { "type": { "text": "hello world" } }
- { "key_press": { "keys": ["ctrl", "c"] } }
- { "scroll": { "target": { "coordinate": { "x": 100, "y": 200 } }, "direction": "down", "amount": 3 } }
- { "drag": { "from": { "coordinate": { "x": 0, "y": 0 } }, "to": { "coordinate": { "x": 100, "y": 100 } } } }
- { "read_ui_tree": { "app": null } }
- { "launch_app": { "name": "Safari", "args": [], "wait_for_ready": true } }
- { "activate_window": { "title_pattern": "Safari" } }
- { "close_window": { "title_pattern": "Untitled" } }
- { "wait": { "milliseconds": 500 } }
- { "clipboard_get": {} }
- { "clipboard_set": { "text": "copied" } }
- { "key_sequence": { "keys": ["ctrl", "a", "delete"], "delays_ms": [100, 50, 0] } }
- { "install_package": { "manager": "brew", "packages": ["node"], "timeout_secs": 300 } }
- { "browse_files": { "path": "/var/log", "filter_description": "recently modified logs", "max_results": 10 } }
- { "read_file_chunked": { "path": "/tmp/big.log", "offset": 0, "limit_bytes": 8192 } }
- { "edit_file": { "path": "/tmp/config.ini", "search": "old_value", "replace": "new_value" } }
- { "compress": { "sources": ["/tmp/logs"], "destination": "/tmp/logs.zip", "format": "zip" } }
- { "decompress": { "archive": "/tmp/logs.zip", "destination": "/tmp/extracted" } }
- { "transfer_file": { "source": "/tmp/file.txt", "destination": "user@host:/tmp/file.txt", "method": "scp" } }
- { "tool_call": { "tool_name": "device_oscilloscope_01_read_waveform", "args": { "channel": 1 } } }

Note: Tool names starting with "device_" (e.g. "device_oscilloscope_01_read_waveform", "device_motor_03_move_to") are device capabilities registered by connected hardware. Use "tool_call" for any device operation. The list of available tool names is provided in the available_tools list.

When to use `device_*` vs generic `shell`: Prefer device tools for hardware interaction (oscilloscope, motor, sensor read) because they use the device's structured API. Use `shell` only for software-side operations (file manipulation, network queries, process management) where no device tool exists.

Available verification types:
- { "success": {} }
- { "ui_tree_contains": { "role": "button", "label": "OK" } }
- { "screenshot_changed": { "max_pixel_diff": 1000 } }
- { "screenshot_stable": { "max_pixel_diff": 100, "poll_ms": 500 } }
- { "process_running": { "name": "Chrome" } }
- { "window_title_contains": { "pattern": "Settings" } }
- { "file_exists": { "path": "/tmp/result.txt" } }
- { "output_contains": { "text": "success" } }

Example output:
[
  {
    "id": "read-oscilloscope",
    "description": "Read channel 1 waveform from connected oscilloscope via device tool",
    "dependencies": [],
    "action": { "tool_call": { "tool_name": "device_oscilloscope_01_read_waveform", "args": { "channel": 1 } } },
    "verification": { "success": {} },
    "max_retries": 2
  },
  {
    "id": "open-browser",
    "description": "Launch the web browser",
    "dependencies": [],
    "action": { "launch_app": { "name": "Safari", "args": [], "wait_for_ready": true } },
    "verification": { "process_running": { "name": "Safari" } },
    "max_retries": 2
  },
  {
    "id": "navigate-search",
    "description": "Click the address bar and type the search query",
    "dependencies": ["open-browser"],
    "action": { "click": { "target": { "coordinate": { "x": 500, "y": 80 } }, "button": "left" } },
    "verification": { "success": {} },
    "max_retries": 2
  }
]"#;

fn build_decomposition_prompt(goal: &str, available_tools: &[String], extra_context: &str) -> String {
    let tools_str = if available_tools.is_empty() {
        "No specific tool list provided.".to_string()
    } else {
        format!("Available tools:\n{}", available_tools.join(", "))
    };

    if extra_context.is_empty() {
        format!(
            "Goal: {}\n\n{}\n\nPlease decompose the goal into subtasks and output ONLY the JSON array.",
            goal, tools_str
        )
    } else {
        format!(
            "Goal: {}\n\n{}\n{}\n\nPlease decompose the goal into subtasks and output ONLY the JSON array.",
            goal, tools_str, extra_context
        )
    }
}

fn strip_code_fences(text: &str) -> &str {
    let trimmed = text.trim();
    if trimmed.starts_with("```json") {
        trimmed
            .strip_prefix("```json")
            .and_then(|s| s.strip_suffix("```"))
            .unwrap_or(trimmed)
            .trim()
    } else if trimmed.starts_with("```") {
        trimmed
            .strip_prefix("```")
            .and_then(|s| s.strip_suffix("```"))
            .unwrap_or(trimmed)
            .trim()
    } else {
        trimmed
    }
}

fn detect_cycle(subtasks: &[SubTask]) -> Option<Vec<String>> {
    let mut adj: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let mut in_degree: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for st in subtasks {
        in_degree.entry(st.id.clone()).or_insert(0);
    }
    for st in subtasks {
        for dep in &st.dependencies {
            adj.entry(dep.clone()).or_default().push(st.id.clone());
            *in_degree.entry(st.id.clone()).or_insert(0) += 1;
        }
    }

    let mut queue: std::collections::VecDeque<String> = in_degree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(id, _)| id.clone())
        .collect();

    let mut visited = 0usize;
    while let Some(id) = queue.pop_front() {
        visited += 1;
        if let Some(children) = adj.get(&id) {
            for child in children {
                let d = in_degree.get_mut(child).unwrap();
                *d -= 1;
                if *d == 0 {
                    queue.push_back(child.clone());
                }
            }
        }
    }

    if visited == subtasks.len() {
        None
    } else {
        let cycle_tasks: Vec<String> = in_degree
            .iter()
            .filter(|(_, d)| **d > 0)
            .map(|(id, _)| id.clone())
            .collect();
        Some(cycle_tasks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer::{ClickTarget, MouseButton, Point};

    #[test]
    fn test_subtask_into_task() {
        let sub = SubTask {
            id: "click-ok".to_string(),
            description: "Click OK".to_string(),
            dependencies: vec!["open-dialog".to_string()],
            tool_hint: None,
            action: Some(DesktopAction::Click {
                target: ClickTarget::Coordinate(Point::new(100, 200)),
                button: MouseButton::Left,
            }),
            verification: Some(VerificationCriteria::Success),
            max_retries: 3,
        };

        let task = sub.into_task();
        assert_eq!(task.id, "click-ok");
        assert_eq!(task.dependencies, vec!["open-dialog"]);
        assert_eq!(task.max_retries, 3);
    }

    #[test]
    fn test_subtask_without_action_becomes_wait() {
        let sub = SubTask {
            id: "think".to_string(),
            description: "Think step".to_string(),
            dependencies: vec![],
            tool_hint: None,
            action: None,
            verification: None,
            max_retries: 0,
        };

        let task = sub.into_task();
        assert!(matches!(task.action, DesktopAction::Wait { milliseconds: 0 }));
    }

    #[test]
    fn test_strip_code_fences() {
        assert_eq!(strip_code_fences("```json\n[{}]\n```"), "[{}]");
        assert_eq!(strip_code_fences("```\n[{}]\n```"), "[{}]");
        assert_eq!(strip_code_fences("[{}]"), "[{}]");
    }

    #[test]
    fn test_detect_cycle_none() {
        let subs = vec![
            SubTask {
                id: "a".to_string(),
                description: "A".to_string(),
                dependencies: vec![],
                tool_hint: None,
                action: None,
                verification: None,
                max_retries: 0,
            },
            SubTask {
                id: "b".to_string(),
                description: "B".to_string(),
                dependencies: vec!["a".to_string()],
                tool_hint: None,
                action: None,
                verification: None,
                max_retries: 0,
            },
        ];
        assert!(detect_cycle(&subs).is_none());
    }

    #[test]
    fn test_detect_cycle_found() {
        let subs = vec![
            SubTask {
                id: "a".to_string(),
                description: "A".to_string(),
                dependencies: vec!["c".to_string()],
                tool_hint: None,
                action: None,
                verification: None,
                max_retries: 0,
            },
            SubTask {
                id: "b".to_string(),
                description: "B".to_string(),
                dependencies: vec!["a".to_string()],
                tool_hint: None,
                action: None,
                verification: None,
                max_retries: 0,
            },
            SubTask {
                id: "c".to_string(),
                description: "C".to_string(),
                dependencies: vec!["b".to_string()],
                tool_hint: None,
                action: None,
                verification: None,
                max_retries: 0,
            },
        ];
        assert!(detect_cycle(&subs).is_some());
    }
}
