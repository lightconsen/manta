//! Computer Use loop — the canonical screenshot → decide → execute → verify cycle.
//!
//! This module implements the standard Anthropic Computer Use interaction
//! pattern without tying itself to any specific LLM provider.  The caller
//! supplies a `decide` closure that receives the current [`LoopState`] and
//! returns what to do next.
//!
//! ```text
//! while not done:
//!     screenshot = adapter.screenshot()          // perceive
//!     decision   = decide(state)                 // plan
//!     result     = adapter.execute(action)       // act
//!     ok         = verifier.verify(criteria)     // validate
//! ```

use crate::computer::{
    ActionResult, ComputerAdapter, DesktopAction, Result, Screenshot,
    VerificationConfig, VerificationCriteria, VerificationEngine,
};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the Computer Use loop.
#[derive(Debug, Clone, Copy)]
pub struct LoopConfig {
    /// Maximum number of steps before giving up.
    pub max_steps: usize,
    /// Delay after executing an action before verification (ms).
    pub settle_delay_ms: u64,
    /// Whether to run verification after every action.
    pub verify_after_each: bool,
    /// Verification configuration (retries, delay, etc.).
    pub verification: VerificationConfig,
    /// Region to screenshot each iteration (`None` = full screen).
    pub screenshot_region: Option<crate::computer::Rect>,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_steps: 30,
            settle_delay_ms: 500,
            verify_after_each: true,
            verification: VerificationConfig::default(),
            screenshot_region: None,
        }
    }
}

/// What the decision maker wants the loop to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopDecision {
    /// Execute a desktop action.
    Action(DesktopAction),
    /// Task is complete.
    Done { message: String },
    /// The agent is stuck and needs human help.
    NeedHelp { reason: String },
}

/// A single step that was executed.
#[derive(Debug, Clone)]
pub struct StepRecord {
    pub step: usize,
    pub action: DesktopAction,
    pub result: ActionResult,
    pub verified: bool,
    pub screenshot_before: Option<Screenshot>,
    pub screenshot_after: Option<Screenshot>,
}

/// Current state exposed to the decision maker.
#[derive(Debug, Clone)]
pub struct LoopState {
    /// Which step we are about to execute (0-indexed).
    pub step: usize,
    /// The original goal.
    pub goal: String,
    /// The most recent screenshot.
    pub screenshot: Screenshot,
    /// All previously executed steps.
    pub history: Vec<StepRecord>,
    /// Whether the previous step verified successfully.
    pub last_verified: Option<bool>,
    /// Error message from the previous step, if any.
    pub last_error: Option<String>,
}

/// Outcome of running the loop.
#[derive(Debug, Clone)]
pub struct LoopResult {
    pub success: bool,
    pub steps_taken: usize,
    pub history: Vec<StepRecord>,
    pub final_screenshot: Screenshot,
    pub message: String,
}

/// The canonical Computer Use orchestrator.
pub struct ComputerUseLoop {
    adapter: Arc<dyn ComputerAdapter>,
    verifier: VerificationEngine,
    config: LoopConfig,
}

impl ComputerUseLoop {
    pub fn new(adapter: Arc<dyn ComputerAdapter>) -> Self {
        let verifier = VerificationEngine::new(adapter.clone())
            .with_config(VerificationConfig::default());
        Self {
            adapter,
            verifier,
            config: LoopConfig::default(),
        }
    }

    pub fn with_config(mut self, config: LoopConfig) -> Self {
        self.verifier = VerificationEngine::new(self.adapter.clone())
            .with_config(config.verification);
        self.config = config;
        self
    }

    /// Run the Computer Use loop.
    ///
    /// `decide` is called every iteration with the current [`LoopState`].
    /// It must return a [`LoopDecision`] telling the loop what to do next.
    pub async fn run<F, Fut>(
        &self,
        goal: &str,
        mut decide: F,
    ) -> Result<LoopResult>
    where
        F: FnMut(LoopState) -> Fut,
        Fut: std::future::Future<Output = Result<LoopDecision>>,
    {
        let mut history = Vec::new();
        let mut last_verified: Option<bool> = None;
        let mut last_error: Option<String> = None;

        for step in 0..self.config.max_steps {
            // 1. Perceive — capture screenshot.
            let screenshot = self.adapter.screenshot(self.config.screenshot_region).await?;

            // 2. Plan — ask the decision maker what to do.
            let state = LoopState {
                step,
                goal: goal.to_string(),
                screenshot: screenshot.clone(),
                history: history.clone(),
                last_verified,
                last_error: last_error.clone(),
            };

            let decision = decide(state).await?;

            match decision {
                LoopDecision::Done { message } => {
                    return Ok(LoopResult {
                        success: true,
                        steps_taken: step,
                        history,
                        final_screenshot: screenshot,
                        message,
                    });
                }
                LoopDecision::NeedHelp { reason } => {
                    return Ok(LoopResult {
                        success: false,
                        steps_taken: step,
                        history,
                        final_screenshot: screenshot,
                        message: reason,
                    });
                }
                LoopDecision::Action(action) => {
                    // 3. Act — execute the action.
                    let screenshot_before = Some(screenshot);
                    let result = self.adapter.execute(action.clone()).await;

                    match result {
                        Ok(result) => {
                            // 4. Validate — verify the outcome.
                            tokio::time::sleep(Duration::from_millis(self.config.settle_delay_ms))
                                .await;

                            let verified = if self.config.verify_after_each {
                                self.verify_action(&action, &result).await.unwrap_or(false)
                            } else {
                                true
                            };

                            let screenshot_after = self
                                .adapter
                                .screenshot(self.config.screenshot_region)
                                .await
                                .ok();

                            last_verified = Some(verified);
                            last_error = None;

                            history.push(StepRecord {
                                step,
                                action,
                                result,
                                verified,
                                screenshot_before,
                                screenshot_after,
                            });
                        }
                        Err(e) => {
                            last_verified = Some(false);
                            last_error = Some(e.to_string());

                            history.push(StepRecord {
                                step,
                                action,
                                result: ActionResult::error(e.to_string()),
                                verified: false,
                                screenshot_before,
                                screenshot_after: None,
                            });
                        }
                    }
                }
            }
        }

        // Max steps reached.
        let final_screenshot = self.adapter.screenshot(self.config.screenshot_region).await?;
        Ok(LoopResult {
            success: false,
            steps_taken: history.len(),
            history,
            final_screenshot,
            message: format!("Max steps ({}) reached", self.config.max_steps),
        })
    }

    /// Verify a single action using heuristics.
    ///
    /// Returns `true` if the action appears to have succeeded.
    async fn verify_action(
        &self,
        action: &DesktopAction,
        _result: &ActionResult,
    ) -> Result<bool> {
        match action {
            DesktopAction::Screenshot { .. } => {
                // Screenshots are self-verifying.
                Ok(true)
            }
            DesktopAction::Click { .. } => {
                // After a click, verify the UI tree still exists and
                // nothing obvious went wrong.
                match self.adapter.read_ui_tree(None).await {
                    Ok(tree) => Ok(!tree.is_empty()),
                    Err(_) => Ok(true), // Accessibility not available — can't verify.
                }
            }
            DesktopAction::Type { .. } | DesktopAction::KeyPress { .. } => {
                // Text input is hard to verify without knowing the target.
                // A basic check: UI tree is still readable.
                match self.adapter.read_ui_tree(None).await {
                    Ok(tree) => Ok(!tree.is_empty()),
                    Err(_) => Ok(true),
                }
            }
            DesktopAction::LaunchApp { name, wait_for_ready, .. } => {
                if *wait_for_ready {
                    self.verifier
                        .verify(
                            &VerificationCriteria::ProcessRunning {
                                name: name.clone(),
                            },
                            &ActionResult::success(""),
                            None,
                        )
                        .await
                } else {
                    Ok(true)
                }
            }
            DesktopAction::Wait { .. } => Ok(true),
            DesktopAction::ClipboardGet | DesktopAction::ClipboardSet { .. } => Ok(true),
            DesktopAction::ActivateWindow { title_pattern } => {
                self.verifier
                    .verify(
                        &VerificationCriteria::WindowTitleContains {
                            pattern: title_pattern.clone(),
                        },
                        &ActionResult::success(""),
                        None,
                    )
                    .await
            }
            DesktopAction::KillProcess { name, .. } => {
                // Verify the process is no longer running.
                if let Some(process_name) = name {
                    self.verifier
                        .verify(
                            &VerificationCriteria::ProcessExited {
                                name: process_name.clone(),
                            },
                            &ActionResult::success(""),
                            None,
                        )
                        .await
                } else {
                    Ok(true)
                }
            }
            DesktopAction::GetSystemStatus | DesktopAction::ListProcesses { .. } => Ok(true),
            _ => Ok(true), // Other actions: assume success.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loop_config_default() {
        let cfg = LoopConfig::default();
        assert_eq!(cfg.max_steps, 30);
        assert!(cfg.verify_after_each);
    }

    #[test]
    fn test_loop_decision_serde() {
        let d = LoopDecision::Done {
            message: "finished".to_string(),
        };
        // LoopDecision does not derive Serialize, but we can at least clone it.
        let _cloned = d.clone();
        assert_eq!(d, LoopDecision::Done {
            message: "finished".to_string()
        });
    }

    #[test]
    fn test_loop_state_creation() {
        let state = LoopState {
            step: 0,
            goal: "test".to_string(),
            screenshot: Screenshot {
                base64: String::new(),
                width: 100,
                height: 100,
            },
            history: vec![],
            last_verified: None,
            last_error: None,
        };
        assert_eq!(state.step, 0);
        assert!(state.history.is_empty());
    }
}
