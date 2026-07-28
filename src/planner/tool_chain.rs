//! Tool-chain reasoning — infer prerequisite checks before executing a goal.
//!
//! Given a high-level goal (e.g. "deploy this project to a server") the
//! [`ToolChainReasoner`] analyses what *must* be true beforehand and emits a
//! chain of prerequisite tasks:
//!
//! ```text
//! deploy code → need SSH → SSH needs key → check ~/.ssh/ exists
//! ```
//!
//! The reasoner can work in two modes:
//! 1. **LLM-based** — asks the configured provider to reason about the chain.
//! 2. **Heuristic** — uses a built-in rule base for common patterns.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::computer::DesktopAction;
use crate::planner::{Task, TaskId};
use crate::providers::{CompletionRequest, Message, Provider};

/// A single link in the inferred prerequisite chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainLink {
    /// Unique id for this link.
    pub id: TaskId,
    /// Human-readable description of the prerequisite check.
    pub description: String,
    /// The action to execute to verify / satisfy this prerequisite.
    pub action: DesktopAction,
    /// IDs of earlier links that must succeed before this one.
    pub dependencies: Vec<TaskId>,
}

/// Result of tool-chain reasoning.
#[derive(Debug, Clone, Default)]
pub struct ChainAnalysis {
    /// Prerequisite links that should be checked before the main goal.
    pub prerequisites: Vec<ChainLink>,
    /// Whether the analysis is confident enough to auto-execute.
    pub confidence: f32,
}

/// Infers prerequisite tool chains for a goal.
#[derive(Clone)]
pub struct ToolChainReasoner {
    provider: Option<Arc<dyn Provider>>,
}

impl ToolChainReasoner {
    /// Create a heuristic-only reasoner (no LLM).
    pub fn new() -> Self {
        Self { provider: None }
    }

    /// Create a reasoner backed by an LLM for more complex inference.
    pub fn with_provider(provider: Arc<dyn Provider>) -> Self {
        Self { provider: Some(provider) }
    }

    /// Analyse a goal and return prerequisite chain links.
    ///
    /// First applies heuristic rules; if the LLM provider is available and
    /// the heuristic produces low confidence, falls back to LLM reasoning.
    pub async fn analyse(&self, goal: &str) -> crate::Result<ChainAnalysis> {
        let heuristic = self.heuristic_analyse(goal);

        if heuristic.confidence < 0.7 {
            if let Some(ref provider) = self.provider {
                let llm_result = self.llm_analyse(goal, provider).await?;
                if llm_result.confidence > heuristic.confidence {
                    return Ok(llm_result);
                }
            }
        }

        Ok(heuristic)
    }

    /// Convert prerequisite links into [`Task`]s that can be added to a plan.
    pub fn links_to_tasks(links: &[ChainLink]) -> Vec<Task> {
        links
            .iter()
            .map(|link| {
                let mut task = Task::new(&link.id, &link.description, link.action.clone());
                for dep in &link.dependencies {
                    task.dependencies.push(dep.clone());
                }
                task
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Heuristic rule engine
    // -----------------------------------------------------------------------

    fn heuristic_analyse(&self, goal: &str) -> ChainAnalysis {
        let lower = goal.to_lowercase();
        let mut links = Vec::new();
        let mut confidence = 0.5f32;

        // Rule: deploy → SSH → key check
        if lower.contains("deploy")
            || lower.contains("push")
            || lower.contains("scp")
            || lower.contains("rsync")
        {
            links.push(ChainLink {
                id: "check-ssh-key".to_string(),
                description: "Check SSH key exists for remote access".to_string(),
                action: DesktopAction::BrowseFiles {
                    path: "~/.ssh".to_string(),
                    filter_description: Some("private keys".to_string()),
                    max_results: Some(10),
                },
                dependencies: vec![],
            });
            links.push(ChainLink {
                id: "test-ssh-connect".to_string(),
                description: "Test SSH connectivity to target server".to_string(),
                action: DesktopAction::TestTcpConnect {
                    target: "{{server_host}}".to_string(),
                    port: 22,
                    timeout_ms: Some(5000),
                },
                dependencies: vec!["check-ssh-key".to_string()],
            });
            confidence = 0.85;
        }

        // Rule: build / compile → check toolchain
        if lower.contains("build")
            || lower.contains("compile")
            || lower.contains("cargo build")
            || lower.contains("npm run build")
        {
            links.push(ChainLink {
                id: "check-build-tool".to_string(),
                description: "Verify build toolchain is installed".to_string(),
                action: DesktopAction::LaunchApp {
                    name: "which".to_string(),
                    args: vec!["cargo".to_string()],
                    wait_for_ready: true,
                },
                dependencies: vec![],
            });
            confidence = confidence.max(0.7);
        }

        // Rule: git operations → check git installed + repo exists
        if lower.contains("git clone") || lower.contains("git pull") || lower.contains("git push") {
            links.push(ChainLink {
                id: "check-git-installed".to_string(),
                description: "Verify git is installed".to_string(),
                action: DesktopAction::LaunchApp {
                    name: "git".to_string(),
                    args: vec!["--version".to_string()],
                    wait_for_ready: true,
                },
                dependencies: vec![],
            });
            confidence = confidence.max(0.8);
        }

        // Rule: docker operations → check docker daemon
        if lower.contains("docker") || lower.contains("container") || lower.contains("dockerfile") {
            links.push(ChainLink {
                id: "check-docker-running".to_string(),
                description: "Verify Docker daemon is running".to_string(),
                action: DesktopAction::ListProcesses {
                    filter: Some("docker".to_string()),
                    limit: Some(5),
                },
                dependencies: vec![],
            });
            confidence = confidence.max(0.8);
        }

        // Rule: database operations → check connection
        if lower.contains("database")
            || lower.contains("db migrate")
            || lower.contains("sql")
            || lower.contains("postgres")
            || lower.contains("mysql")
        {
            links.push(ChainLink {
                id: "check-db-port".to_string(),
                description: "Check database port is reachable".to_string(),
                action: DesktopAction::TestTcpConnect {
                    target: "{{db_host}}".to_string(),
                    port: 5432,
                    timeout_ms: Some(3000),
                },
                dependencies: vec![],
            });
            confidence = confidence.max(0.75);
        }

        ChainAnalysis {
            prerequisites: links,
            confidence,
        }
    }

    // -----------------------------------------------------------------------
    // LLM fallback
    // -----------------------------------------------------------------------

    async fn llm_analyse(
        &self,
        goal: &str,
        provider: &Arc<dyn Provider>,
    ) -> crate::Result<ChainAnalysis> {
        // action_type values below map to DesktopAction variants
        // (src/computer/desktop_action.rs). Keep in sync when adding new
        // action types.
        let prompt = format!(
            r#"Analyse the following goal and list prerequisite checks that must pass before it can be executed.

Goal: {}

For each prerequisite, output a JSON object with:
- id: kebab-case identifier
- description: what to check
- action_type: one of [launch_app, browse_files, list_processes, tcp_connect]
- action_params: JSON object with the parameters for that action
- depends_on: array of prerequisite ids that must come before this one

Output ONLY a JSON array. No markdown, no explanations."#,
            goal
        );

        let request = CompletionRequest {
            messages: vec![Message::user(prompt)],
            temperature: Some(0.2),
            max_tokens: Some(2048),
            stream: false,
            requires_reasoning: true,
            ..Default::default()
        };

        let response = provider.complete(request).await?;
        let content = response.message.content.trim();
        let json_str = strip_code_fences(content);

        #[derive(Deserialize)]
        struct LlmLink {
            id: String,
            description: String,
            action_type: String,
            #[serde(default)]
            action_params: serde_json::Value,
            #[serde(default)]
            depends_on: Vec<String>,
        }

        let llm_links: Vec<LlmLink> = serde_json::from_str(json_str).map_err(|e| {
            crate::error::SyscityError::Validation(format!(
                "Failed to parse tool chain from LLM: {}. Raw: {}",
                e,
                &content[..content.len().min(300)]
            ))
        })?;

        let mut links = Vec::new();
        for ll in llm_links {
            let action = parse_llm_action(&ll.action_type, &ll.action_params);
            links.push(ChainLink {
                id: ll.id,
                description: ll.description,
                action,
                dependencies: ll.depends_on,
            });
        }

        Ok(ChainAnalysis {
            prerequisites: links,
            confidence: 0.75,
        })
    }
}

impl Default for ToolChainReasoner {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn strip_code_fences(text: &str) -> &str {
    crate::planner::util::strip_code_fences(text)
}

fn parse_llm_action(action_type: &str, params: &serde_json::Value) -> DesktopAction {
    match action_type {
        "launch_app" => {
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let args: Vec<String> = params
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            DesktopAction::LaunchApp {
                name,
                args,
                wait_for_ready: params
                    .get("wait_for_ready")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
            }
        }
        "browse_files" => {
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".")
                .to_string();
            DesktopAction::BrowseFiles {
                path,
                filter_description: params
                    .get("filter")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                max_results: params
                    .get("max_results")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .or(Some(10)),
            }
        }
        "list_processes" => {
            let filter = params
                .get("filter")
                .and_then(|v| v.as_str())
                .map(String::from);
            let limit = params
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            DesktopAction::ListProcesses { filter, limit }
        }
        "tcp_connect" => {
            let target = params
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or("localhost")
                .to_string();
            let port = params.get("port").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
            let timeout = params
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .or(Some(5000));
            DesktopAction::TestTcpConnect {
                target,
                port,
                timeout_ms: timeout,
            }
        }
        _ => DesktopAction::Wait { milliseconds: 0 },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heuristic_deploy_ssh() {
        let reasoner = ToolChainReasoner::new();
        let analysis = reasoner.heuristic_analyse("Deploy this project to the server");
        assert!(analysis.confidence >= 0.8, "deploy goal should have high confidence");
        let ids: Vec<_> = analysis.prerequisites.iter().map(|l| &l.id).collect();
        assert!(ids.contains(&&"check-ssh-key".to_string()));
        assert!(ids.contains(&&"test-ssh-connect".to_string()));
    }

    #[test]
    fn test_heuristic_git() {
        let reasoner = ToolChainReasoner::new();
        let analysis = reasoner.heuristic_analyse("git clone the repo and build it");
        assert!(analysis.confidence >= 0.7);
        let ids: Vec<_> = analysis.prerequisites.iter().map(|l| &l.id).collect();
        assert!(ids.contains(&&"check-git-installed".to_string()));
    }

    #[test]
    fn test_heuristic_docker() {
        let reasoner = ToolChainReasoner::new();
        let analysis = reasoner.heuristic_analyse("Run the docker container");
        assert!(analysis.confidence >= 0.7);
        let ids: Vec<_> = analysis.prerequisites.iter().map(|l| &l.id).collect();
        assert!(ids.contains(&&"check-docker-running".to_string()));
    }

    #[test]
    fn test_links_to_tasks() {
        let links = vec![
            ChainLink {
                id: "a".to_string(),
                description: "Check A".to_string(),
                action: DesktopAction::Wait { milliseconds: 0 },
                dependencies: vec![],
            },
            ChainLink {
                id: "b".to_string(),
                description: "Check B".to_string(),
                action: DesktopAction::Wait { milliseconds: 0 },
                dependencies: vec!["a".to_string()],
            },
        ];
        let tasks = ToolChainReasoner::links_to_tasks(&links);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[1].dependencies, vec!["a"]);
    }
}
