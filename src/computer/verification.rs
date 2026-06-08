//! Verification engine — execute desktop actions and verify results.
//!
//! Wraps [`ComputerAdapter`] with automatic retry and post-action
//! validation so the Agent can be sure an action had the intended
//! effect before moving on.

use crate::computer::{
    ActionResult, ComputerAdapter, ComputerError, DesktopAction, Result, Screenshot,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Criteria used to verify that an action produced the expected outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationCriteria {
    /// UI tree must contain an element with the given role (and optionally label).
    UiTreeContains { role: String, label: Option<String> },
    /// Screenshot must differ from the baseline by no more than `max_pixel_diff`.
    /// A baseline is captured automatically before the action.
    ScreenshotChanged { max_pixel_diff: u32 },
    /// Screenshot must be stable (two consecutive screenshots differ by no more
    /// than `max_pixel_diff`). Useful after waiting for animations to finish.
    ScreenshotStable { max_pixel_diff: u32, poll_ms: u64 },
    /// A process with the given name must be running.
    ProcessRunning { name: String },
    /// The active window title must contain the given pattern.
    WindowTitleContains { pattern: String },
    /// A file must exist at the given path.
    FileExists { path: String },
    /// The action result message must contain the given text.
    OutputContains { text: String },
    /// The action result must indicate success.
    Success,
}

/// Configuration for the verification / retry loop.
#[derive(Debug, Clone, Copy)]
pub struct VerificationConfig {
    /// Maximum number of retry attempts (0 = no retries).
    pub max_retries: u32,
    /// Delay between retries (also used as settle time before first verification).
    pub retry_delay_ms: u64,
    /// Delay before capturing the "before" baseline screenshot.
    pub baseline_delay_ms: u64,
}

impl Default for VerificationConfig {
    fn default() -> Self {
        Self {
            max_retries: 2,
            retry_delay_ms: 500,
            baseline_delay_ms: 200,
        }
    }
}

/// Execute actions and verify their outcomes.
#[derive(Clone)]
pub struct VerificationEngine {
    adapter: Arc<dyn ComputerAdapter>,
    config: VerificationConfig,
}

impl VerificationEngine {
    pub fn new(adapter: Arc<dyn ComputerAdapter>) -> Self {
        Self {
            adapter,
            config: VerificationConfig::default(),
        }
    }

    pub fn with_config(mut self, config: VerificationConfig) -> Self {
        self.config = config;
        self
    }

    /// Execute an action and verify the result.
    ///
    /// 1. Capture baseline (screenshot / UI tree) if needed by criteria.
    /// 2. Execute the action.
    /// 3. Wait a short settle time.
    /// 4. Verify against criteria.
    /// 5. Retry up to `max_retries` if verification fails.
    pub async fn execute_with_verification(
        &self,
        action: DesktopAction,
        criteria: VerificationCriteria,
    ) -> Result<ActionResult> {
        let mut baseline: Option<Screenshot> = None;

        // Capture baseline screenshot if needed.
        if matches!(
            criteria,
            VerificationCriteria::ScreenshotChanged { .. }
                | VerificationCriteria::ScreenshotStable { .. }
        ) {
            tokio::time::sleep(Duration::from_millis(self.config.baseline_delay_ms)).await;
            baseline = Some(self.adapter.screenshot(None).await?);
        }

        for attempt in 0..=self.config.max_retries {
            let result = self.adapter.execute(action.clone()).await?;

            // Wait for UI to settle before verifying.
            tokio::time::sleep(Duration::from_millis(self.config.retry_delay_ms)).await;

            match self.verify(&criteria, &result, baseline.as_ref()).await {
                Ok(true) => return Ok(result),
                Ok(false) => {
                    if attempt < self.config.max_retries {
                        tracing::warn!(
                            "Verification failed (attempt {}/{}), retrying...",
                            attempt + 1,
                            self.config.max_retries + 1
                        );
                        tokio::time::sleep(Duration::from_millis(self.config.retry_delay_ms)).await;
                    }
                }
                Err(e) => {
                    // Verification itself errored — abort unless we can retry.
                    if attempt < self.config.max_retries {
                        tracing::warn!(
                            "Verification error (attempt {}/{}): {}, retrying...",
                            attempt + 1,
                            self.config.max_retries + 1,
                            e
                        );
                        tokio::time::sleep(Duration::from_millis(self.config.retry_delay_ms)).await;
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Err(ComputerError::Other(format!(
            "Verification failed after {} attempts",
            self.config.max_retries + 1
        )))
    }

    /// Verify that the current state matches the criteria.
    pub async fn verify(
        &self,
        criteria: &VerificationCriteria,
        result: &ActionResult,
        baseline: Option<&Screenshot>,
    ) -> Result<bool> {
        match criteria {
            VerificationCriteria::UiTreeContains { role, label } => {
                let tree = self.adapter.read_ui_tree(None).await?;
                Ok(tree.iter().any(|e| {
                    e.role == *role
                        && label
                            .as_ref()
                            .map(|l| e.label.as_ref().map(|el| el.contains(l)).unwrap_or(false))
                            .unwrap_or(true)
                }))
            }
            VerificationCriteria::ScreenshotChanged { max_pixel_diff } => {
                let before = baseline.ok_or_else(|| {
                    ComputerError::Other("No baseline screenshot for diff verification".to_string())
                })?;
                let after = self.adapter.screenshot(None).await?;
                let diff = compute_screenshot_diff(before, &after);
                Ok(diff <= *max_pixel_diff)
            }
            VerificationCriteria::ScreenshotStable {
                max_pixel_diff,
                poll_ms,
            } => {
                let first = self.adapter.screenshot(None).await?;
                tokio::time::sleep(Duration::from_millis(*poll_ms)).await;
                let second = self.adapter.screenshot(None).await?;
                let diff = compute_screenshot_diff(&first, &second);
                Ok(diff <= *max_pixel_diff)
            }
            VerificationCriteria::ProcessRunning { name } => {
                self.adapter
                    .wait_for(
                        crate::computer::WaitCondition::ProcessRunning {
                            name: name.clone(),
                        },
                        Duration::from_secs(1),
                    )
                    .await
            }
            VerificationCriteria::WindowTitleContains { pattern } => {
                self.adapter
                    .wait_for(
                        crate::computer::WaitCondition::WindowTitleContains {
                            pattern: pattern.clone(),
                        },
                        Duration::from_secs(1),
                    )
                    .await
            }
            VerificationCriteria::FileExists { path } => {
                self.adapter
                    .wait_for(
                        crate::computer::WaitCondition::FileExists {
                            path: path.clone(),
                        },
                        Duration::from_secs(1),
                    )
                    .await
            }
            VerificationCriteria::OutputContains { text } => Ok(result.message.contains(text)),
            VerificationCriteria::Success => Ok(result.success),
        }
    }
}

/// Compute a simple difference metric between two screenshots.
///
/// Returns 0 for identical images, `u32::MAX` for any difference.
/// A future enhancement could decode the PNGs and count per-pixel differences.
fn compute_screenshot_diff(a: &Screenshot, b: &Screenshot) -> u32 {
    if a.base64 == b.base64 {
        return 0;
    }
    // Compare decoded bytes for a slightly more accurate metric.
    match (
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &a.base64),
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &b.base64),
    ) {
        (Ok(a_bytes), Ok(b_bytes)) => {
            if a_bytes == b_bytes {
                0
            } else {
                u32::MAX
            }
        }
        _ => u32::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verification_config_default() {
        let cfg = VerificationConfig::default();
        assert_eq!(cfg.max_retries, 2);
        assert_eq!(cfg.retry_delay_ms, 500);
    }

    #[test]
    fn test_screenshot_diff_identical() {
        let ss = Screenshot {
            base64: "aGVsbG8=".to_string(),
            width: 100,
            height: 100,
        };
        assert_eq!(compute_screenshot_diff(&ss, &ss), 0);
    }

    #[test]
    fn test_screenshot_diff_different() {
        let a = Screenshot {
            base64: "aGVsbG8=".to_string(),
            width: 100,
            height: 100,
        };
        let b = Screenshot {
            base64: "d29ybGQ=".to_string(),
            width: 100,
            height: 100,
        };
        assert_eq!(compute_screenshot_diff(&a, &b), u32::MAX);
    }

    #[test]
    fn test_verification_criteria_clone_eq() {
        let c1 = VerificationCriteria::Success;
        let c2 = VerificationCriteria::Success;
        assert_eq!(c1, c2);

        let c3 = VerificationCriteria::UiTreeContains {
            role: "button".to_string(),
            label: Some("OK".to_string()),
        };
        let c4 = c3.clone();
        assert_eq!(c3, c4);
    }
}
