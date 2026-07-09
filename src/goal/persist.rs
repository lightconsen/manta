//! Goal persistence — save/load goal runner state for restart recovery.
//!
//! Goals are persisted as individual JSON files in `~/.syscity/goals/`.
//! On gateway startup, all persisted goals are loaded and resumed. When a goal
//! completes or is aborted, its state file is deleted.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::goal::condition::CheckResult;
use crate::goal::plan::GoalPlan;

/// Directory name for goal state files under `~/.syscity/`.
const GOALS_DIR_NAME: &str = "goals";

/// Get the goals directory path (`~/.syscity/goals`).
pub fn goals_dir() -> PathBuf {
    crate::dirs::syscity_dir().join(GOALS_DIR_NAME)
}

/// Serializable state of a goal runner at a checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedGoalState {
    pub goal_id: String,
    pub parent_session_id: String,
    pub plan: GoalPlan,
    pub round: usize,
    pub condition_history: Vec<PersistedRoundResult>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Serializable round result for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedRoundResult {
    pub round: usize,
    pub results: Vec<CheckResult>,
}

/// File-based goal state store.
///
/// Each goal is stored as `~/.syscity/goals/{goal_id}.json`.
pub struct GoalStore {
    dir: PathBuf,
}

impl Default for GoalStore {
    fn default() -> Self {
        Self::new()
    }
}

impl GoalStore {
    /// Create a new goal store using the default goals directory.
    pub fn new() -> Self {
        Self { dir: goals_dir() }
    }

    /// Create a goal store with a custom directory (for testing).
    pub fn with_dir(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Ensure the goals directory exists.
    async fn ensure_dir(&self) -> crate::Result<()> {
        if !self.dir.exists() {
            tokio::fs::create_dir_all(&self.dir).await.map_err(|e| {
                crate::error::SyscityError::Storage {
                    context: format!("Failed to create goals directory: {:?}", self.dir),
                    details: e.to_string(),
                }
            })?;
        }
        Ok(())
    }

    /// Path to the state file for a given goal id.
    fn state_path(&self, goal_id: &str) -> PathBuf {
        self.dir.join(format!("{}.json", goal_id))
    }

    /// Save a goal's state to disk.
    pub async fn save(&self, state: &PersistedGoalState) -> crate::Result<()> {
        self.ensure_dir().await?;
        let path = self.state_path(&state.goal_id);
        let json = serde_json::to_string_pretty(state).map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to serialize goal state: {}", e))
        })?;
        tokio::fs::write(&path, &json)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: format!("Failed to write goal state: {:?}", path),
                details: e.to_string(),
            })?;
        Ok(())
    }

    /// Load all persisted goal states.
    pub async fn load_all(&self) -> Vec<PersistedGoalState> {
        let mut states = Vec::new();
        let mut entries = match tokio::fs::read_dir(&self.dir).await {
            Ok(e) => e,
            Err(_) => return states,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => match serde_json::from_str::<PersistedGoalState>(&content) {
                    Ok(state) => states.push(state),
                    Err(e) => {
                        tracing::warn!("[goal] Failed to parse persisted state {:?}: {}", path, e);
                    }
                },
                Err(e) => {
                    tracing::warn!("[goal] Failed to read persisted state {:?}: {}", path, e);
                }
            }
        }

        states
    }

    /// Delete a goal's state file.
    pub async fn delete(&self, goal_id: &str) {
        let path = self.state_path(goal_id);
        if path.exists() {
            if let Err(e) = tokio::fs::remove_file(&path).await {
                tracing::warn!("[goal] Failed to delete state file {:?}: {}", path, e);
            }
        }
    }
}

/// Convert runner's internal state to a persisted checkpoint.
pub fn to_persisted(
    goal_id: &str,
    parent_session_id: &str,
    plan: &GoalPlan,
    round: usize,
    condition_history: &[crate::goal::runner::RoundResult],
) -> PersistedGoalState {
    let now = Utc::now();
    PersistedGoalState {
        goal_id: goal_id.to_string(),
        parent_session_id: parent_session_id.to_string(),
        plan: plan.clone(),
        round,
        condition_history: condition_history
            .iter()
            .map(|r| PersistedRoundResult {
                round: r.round,
                results: r.results.clone(),
            })
            .collect(),
        created_at: now,
        updated_at: now,
    }
}

/// Convert a persisted goal state into parameters for recreating a GoalRunner.
pub fn to_runner_params(
    state: &PersistedGoalState,
) -> (String, String, GoalPlan, Vec<crate::goal::runner::RoundResult>) {
    let condition_history: Vec<crate::goal::runner::RoundResult> = state
        .condition_history
        .iter()
        .map(|pr| crate::goal::runner::RoundResult {
            round: pr.round,
            results: pr.results.clone(),
        })
        .collect();

    (
        state.goal_id.clone(),
        state.parent_session_id.clone(),
        state.plan.clone(),
        condition_history,
    )
}

/// Wrapper type for thread-safe shared access to GoalStore.
pub type SharedGoalStore = Arc<RwLock<GoalStore>>;

/// Create a shared goal store (convenience constructor).
pub fn shared_store() -> SharedGoalStore {
    Arc::new(RwLock::new(GoalStore::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::condition::Comparison;
    use crate::goal::runner::RoundResult;

    fn sample_state(goal_id: &str) -> PersistedGoalState {
        PersistedGoalState {
            goal_id: goal_id.to_string(),
            parent_session_id: "session_abc".to_string(),
            plan: crate::goal::GoalPlan::new("write tests").with_condition(
                crate::goal::GoalCondition::ExitCode {
                    command: "cargo test".to_string(),
                    expected: Some(0),
                },
            ),
            round: 2,
            condition_history: vec![PersistedRoundResult {
                round: 1,
                results: vec![crate::goal::CheckResult {
                    condition: crate::goal::GoalCondition::ExitCode {
                        command: "cargo test".to_string(),
                        expected: Some(0),
                    },
                    passed: false,
                    actual: "exit code: 1".to_string(),
                    detail: "tests failed".to_string(),
                }],
            }],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_to_persisted_round_trip() {
        let condition_history = vec![RoundResult {
            round: 1,
            results: vec![crate::goal::CheckResult {
                condition: crate::goal::GoalCondition::ExitCode {
                    command: "cargo test".to_string(),
                    expected: Some(0),
                },
                passed: true,
                actual: "exit code: 0".to_string(),
                detail: "passed".to_string(),
            }],
        }];
        let plan = crate::goal::GoalPlan::new("test").with_condition(
            crate::goal::GoalCondition::ExitCode {
                command: "true".to_string(),
                expected: Some(0),
            },
        );

        let state = to_persisted("goal_1", "session_1", &plan, 3, &condition_history);
        assert_eq!(state.goal_id, "goal_1");
        assert_eq!(state.parent_session_id, "session_1");
        assert_eq!(state.round, 3);
        assert_eq!(state.condition_history.len(), 1);

        let (gid, pid, restored_plan, restored_history) = to_runner_params(&state);
        assert_eq!(gid, "goal_1");
        assert_eq!(pid, "session_1");
        assert_eq!(restored_plan.description, "test");
        assert_eq!(restored_history.len(), 1);
        assert_eq!(restored_history[0].round, 1);
    }

    #[tokio::test]
    async fn test_goal_store_save_and_load() {
        let dir = std::env::temp_dir().join(format!("goal_test_{}", uuid::Uuid::new_v4()));
        let store = GoalStore::with_dir(dir.clone());

        let state = sample_state("goal_save_test");
        store.save(&state).await.unwrap();

        let loaded = store.load_all().await;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].goal_id, "goal_save_test");
        assert_eq!(loaded[0].round, 2);

        // Cleanup
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_goal_store_delete() {
        let dir = std::env::temp_dir().join(format!("goal_test_{}", uuid::Uuid::new_v4()));
        let store = GoalStore::with_dir(dir.clone());

        let state = sample_state("goal_delete_test");
        store.save(&state).await.unwrap();
        assert_eq!(store.load_all().await.len(), 1);

        store.delete("goal_delete_test").await;
        assert_eq!(store.load_all().await.len(), 0);

        // Cleanup
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_goal_store_load_empty_dir() {
        let dir = std::env::temp_dir().join(format!("goal_test_{}", uuid::Uuid::new_v4()));
        let store = GoalStore::with_dir(dir.clone());

        let loaded = store.load_all().await;
        assert!(loaded.is_empty());

        // Cleanup
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[test]
    fn test_goals_dir_ends_with_goals() {
        let dir = goals_dir();
        assert!(dir.to_string_lossy().ends_with("goals"));
    }
}
