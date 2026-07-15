//! Human Review routing — persist low-confidence scoring cases for manual
//! review (§06).
//!
//! When `LayeredScorer` returns `Verdict::InsufficientInfo` (low confidence or
//! risk signals), the case is written to `evals/review/` as a JSON file.
//! A CLI subcommand (`syscity eval review`) lists pending cases.
//!
//! # File format
//!
//! Each review case is a single JSON file at
//! `evals/review/<task_id>_<trial>_<timestamp>.json`:
//!
//! ```json
//! {
//!   "task_id": "web_search",
//!   "trial_index": 2,
//!   "input": "What is ...?",
//!   "response": "...",
//!   "scoring_output": { ... },
//!   "status": "Pending",
//!   "created_at": "..."
//! }
//! ```

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::eval::scorer::ScoringOutput;
use crate::Result;

/// Review status of a human review case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewStatus {
    /// Awaiting human review.
    Pending,
    /// Reviewed by human.
    Reviewed,
    /// Skipped (no action needed).
    Skipped,
}

/// A single case routed for human review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanReviewCase {
    /// Original eval task id.
    pub task_id: String,
    /// Trial index within the task.
    pub trial_index: usize,
    /// User input that triggered the case.
    pub input: String,
    /// Agent response.
    pub response: String,
    /// The scoring output (verdict, confidence, etc.).
    pub scoring_output: ScoringOutput,
    /// Review status.
    pub status: ReviewStatus,
    /// When this case was created.
    pub created_at: SystemTime,
    /// Optional human verdict override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_verdict: Option<String>,
    /// Optional human comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_comment: Option<String>,
}

impl HumanReviewCase {
    /// Generate a filename for this case.
    fn filename(&self) -> String {
        let ts = short_timestamp();
        sanitize_id(&format!("{}_{}_{}", self.task_id, self.trial_index, ts))
    }
}

/// Store for persisting and loading human review cases.
pub struct HumanReviewStore {
    /// Directory where review cases are stored.
    dir: PathBuf,
}

impl HumanReviewStore {
    /// Create a new store writing to `evals/review/` under `evals_dir`.
    pub fn new(evals_dir: &Path) -> Self {
        Self { dir: evals_dir.join("review") }
    }

    /// Persist a review case to disk as JSON.
    pub fn write_case(&self, case: &HumanReviewCase) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.dir)?;

        let file_path = self.dir.join(format!("{}.json", case.filename()));
        let json = serde_json::to_string_pretty(case)
            .map_err(|e| crate::error::SyscityError::Validation(e.to_string()))?;
        std::fs::write(&file_path, json)?;

        Ok(file_path)
    }

    /// List all review cases, optionally filtered by status.
    pub fn list_cases(&self, status_filter: Option<ReviewStatus>) -> Result<Vec<PathBuf>> {
        if !self.dir.is_dir() {
            return Ok(Vec::new());
        }

        let mut cases = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(ref filter) = status_filter {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Ok(case) = serde_json::from_str::<HumanReviewCase>(&content) {
                            if case.status == *filter {
                                cases.push(path);
                            }
                        }
                    }
                } else {
                    cases.push(path);
                }
            }
        }
        cases.sort();
        Ok(cases)
    }

    /// Load a single review case from a file path.
    pub fn load_case(path: &Path) -> Result<HumanReviewCase> {
        let content = std::fs::read_to_string(path)?;
        let case: HumanReviewCase = serde_json::from_str(&content)
            .map_err(|e| crate::error::SyscityError::Validation(e.to_string()))?;
        Ok(case)
    }

    /// Update a review case's status (in-place file rewrite).
    pub fn update_status(&self, path: &Path, status: ReviewStatus) -> Result<()> {
        let mut case = Self::load_case(path)?;
        case.status = status;
        let json = serde_json::to_string_pretty(&case)
            .map_err(|e| crate::error::SyscityError::Validation(e.to_string()))?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Count cases by status.
    pub fn count_by_status(&self) -> Result<(usize, usize, usize)> {
        let all = self.list_cases(None)?;
        let mut pending = 0usize;
        let mut reviewed = 0usize;
        let mut skipped = 0usize;

        for path in &all {
            if let Ok(case) = Self::load_case(path) {
                match case.status {
                    ReviewStatus::Pending => pending += 1,
                    ReviewStatus::Reviewed => reviewed += 1,
                    ReviewStatus::Skipped => skipped += 1,
                }
            }
        }

        Ok((pending, reviewed, skipped))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn short_timestamp() -> String {
    let dur = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{:x}", dur.as_secs())
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::scorer::{ScreeningLayer, Verdict};

    fn dummy_scoring_output() -> ScoringOutput {
        ScoringOutput {
            verdict: Verdict::InsufficientInfo,
            score: 0.4,
            problem_category: None,
            confidence: 0.3,
            judgment_basis: "Low confidence, needs review".into(),
            screening_layer: ScreeningLayer::Fine,
        }
    }

    #[test]
    fn test_human_review_case_serde_roundtrip() {
        let case = HumanReviewCase {
            task_id: "test_task".into(),
            trial_index: 1,
            input: "Hello".into(),
            response: "Hi there".into(),
            scoring_output: dummy_scoring_output(),
            status: ReviewStatus::Pending,
            created_at: SystemTime::now(),
            human_verdict: None,
            human_comment: None,
        };

        let json = serde_json::to_string_pretty(&case).unwrap();
        let deserialized: HumanReviewCase = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.task_id, "test_task");
        assert_eq!(deserialized.trial_index, 1);
        assert_eq!(deserialized.status, ReviewStatus::Pending);
    }

    #[test]
    fn test_store_write_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let store = HumanReviewStore::new(dir.path());

        let case = HumanReviewCase {
            task_id: "web_search".into(),
            trial_index: 0,
            input: "test input".into(),
            response: "test response".into(),
            scoring_output: dummy_scoring_output(),
            status: ReviewStatus::Pending,
            created_at: SystemTime::now(),
            human_verdict: None,
            human_comment: None,
        };

        let path = store.write_case(&case).unwrap();
        assert!(path.exists());

        let cases = store.list_cases(None).unwrap();
        assert_eq!(cases.len(), 1);

        let loaded = HumanReviewStore::load_case(&cases[0]).unwrap();
        assert_eq!(loaded.task_id, "web_search");
    }

    #[test]
    fn test_store_count_by_status() {
        let dir = tempfile::tempdir().unwrap();
        let store = HumanReviewStore::new(dir.path());

        let case1 = HumanReviewCase {
            task_id: "t1".into(),
            trial_index: 0,
            input: "i1".into(),
            response: "r1".into(),
            scoring_output: dummy_scoring_output(),
            status: ReviewStatus::Pending,
            created_at: SystemTime::now(),
            human_verdict: None,
            human_comment: None,
        };
        let case2 = HumanReviewCase {
            task_id: "t2".into(),
            trial_index: 0,
            input: "i2".into(),
            response: "r2".into(),
            scoring_output: dummy_scoring_output(),
            status: ReviewStatus::Reviewed,
            created_at: SystemTime::now(),
            human_verdict: Some("Pass".into()),
            human_comment: Some("Looks good".into()),
        };

        store.write_case(&case1).unwrap();
        store.write_case(&case2).unwrap();

        let (pending, reviewed, skipped) = store.count_by_status().unwrap();
        assert_eq!(pending, 1);
        assert_eq!(reviewed, 1);
        assert_eq!(skipped, 0);
    }

    #[test]
    fn test_update_status() {
        let dir = tempfile::tempdir().unwrap();
        let store = HumanReviewStore::new(dir.path());

        let case = HumanReviewCase {
            task_id: "t1".into(),
            trial_index: 0,
            input: "i1".into(),
            response: "r1".into(),
            scoring_output: dummy_scoring_output(),
            status: ReviewStatus::Pending,
            created_at: SystemTime::now(),
            human_verdict: None,
            human_comment: None,
        };

        let path = store.write_case(&case).unwrap();
        store.update_status(&path, ReviewStatus::Reviewed).unwrap();

        let loaded = HumanReviewStore::load_case(&path).unwrap();
        assert_eq!(loaded.status, ReviewStatus::Reviewed);
    }
}
