//! Goal conditions — structured, executable stop conditions for goal-based execution.
//!
//! Each [`GoalCondition`] variant maps to a deterministic check that can be
//! evaluated without an LLM. Conditions are ANDed — all must pass for the
//! goal to be considered complete.

use std::path::Path;

/// Comparison operator for numeric conditions.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Comparison {
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = ">=")]
    Ge,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = "==")]
    Eq,
}

impl Comparison {
    fn evaluate(&self, a: f64, b: f64) -> bool {
        match self {
            Comparison::Gt => a > b,
            Comparison::Lt => a < b,
            Comparison::Ge => a >= b,
            Comparison::Le => a <= b,
            Comparison::Eq => (a - b).abs() < f64::EPSILON,
        }
    }
}

/// A structured, executable stop condition for goal-based execution.
///
/// Each condition is self-contained with its own command/path/pattern and can
/// be checked by calling [`check`](GoalCondition::check).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum GoalCondition {
    #[serde(rename = "exit_code")]
    ExitCode {
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected: Option<i32>,
    },
    #[serde(rename = "file_exists")]
    FileExists {
        path: String,
    },
    #[serde(rename = "numeric")]
    Numeric {
        command: String,
        operator: Comparison,
        threshold: f64,
    },
    #[serde(rename = "pattern")]
    Pattern {
        command: String,
        must_contain: String,
    },
    #[serde(rename = "static_analysis")]
    StaticAnalysis {
        #[serde(default = "default_clippy_command")]
        command: String,
    },
}

fn default_clippy_command() -> String {
    "cargo clippy -- -D warnings".to_string()
}

/// Result of checking a single condition.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckResult {
    pub condition: GoalCondition,
    pub passed: bool,
    pub actual: String,
    pub detail: String,
}

impl GoalCondition {
    /// Check whether this condition is currently met.
    ///
    /// Runs the command / check and returns a [`CheckResult`] with the output.
    pub async fn check(&self) -> CheckResult {
        match self {
            GoalCondition::ExitCode { command, expected } => {
                Self::check_exit_code(command, *expected).await
            }
            GoalCondition::FileExists { path } => Self::check_file_exists(path),
            GoalCondition::Numeric { command, operator, threshold } => {
                Self::check_numeric(command, operator, *threshold).await
            }
            GoalCondition::Pattern { command, must_contain } => {
                Self::check_pattern(command, must_contain).await
            }
            GoalCondition::StaticAnalysis { command } => {
                Self::check_exit_code(command, Some(0)).await
            }
        }
    }

    fn description(&self) -> String {
        match self {
            GoalCondition::ExitCode { command, expected } => {
                format!("`{}` exits with {}", command, expected.unwrap_or(0))
            }
            GoalCondition::FileExists { path } => format!("`{}` exists", path),
            GoalCondition::Numeric { command, operator, threshold } => {
                format!("`{}` {} {}", command, match operator {
                    Comparison::Gt => ">",
                    Comparison::Lt => "<",
                    Comparison::Ge => ">=",
                    Comparison::Le => "<=",
                    Comparison::Eq => "==",
                }, threshold)
            }
            GoalCondition::Pattern { command, must_contain } => {
                format!("`{}` contains {:?}", command, must_contain)
            }
            GoalCondition::StaticAnalysis { command } => {
                format!("`{}` passes", command)
            }
        }
    }

    /// Return a stable "failure signature" for loop detection.
    /// Two failures with the same signature means the same thing went wrong.
    pub fn failure_signature(&self, actual: &str) -> String {
        format!("{:?}:{}", self, actual)
    }

    async fn check_exit_code(command: &str, expected: Option<i32>) -> CheckResult {
        let expected = expected.unwrap_or(0);
        let cond = GoalCondition::ExitCode {
            command: command.to_string(),
            expected: Some(expected),
        };
        let _desc = format!("exit code {} (expected {})", expected, expected);

        match tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .await
        {
            Ok(output) => {
                let code = output.status.code().unwrap_or(-1);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                let detail = if !stdout.trim().is_empty() {
                    stdout.trim().to_string()
                } else if !stderr.trim().is_empty() {
                    stderr.trim().to_string()
                } else {
                    format!("exit code {}", code)
                };
                CheckResult {
                    condition: cond,
                    passed: code == expected,
                    actual: format!("exit code: {}", code),
                    detail,
                }
            }
            Err(e) => CheckResult {
                condition: cond,
                passed: false,
                actual: format!("error: {}", e),
                detail: String::new(),
            },
        }
    }

    fn check_file_exists(path_str: &str) -> CheckResult {
        let cond = GoalCondition::FileExists {
            path: path_str.to_string(),
        };
        let path = Path::new(path_str);
        let exists = path.exists();
        CheckResult {
            condition: cond,
            passed: exists,
            actual: if exists { "found".to_string() } else { "not found".to_string() },
            detail: if exists {
                format!("{} exists ({} bytes)", path_str, std::fs::metadata(path_str).map(|m| m.len()).unwrap_or(0))
            } else {
                format!("{} does not exist", path_str)
            },
        }
    }

    async fn check_numeric(command: &str, operator: &Comparison, threshold: f64) -> CheckResult {
        let cond = GoalCondition::Numeric {
            command: command.to_string(),
            operator: operator.clone(),
            threshold,
        };

        match tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .await
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let trimmed = stdout.trim();
                let value: f64 = trimmed.parse().unwrap_or(0.0);
                let passed = operator.evaluate(value, threshold);
                CheckResult {
                    passed,
                    actual: format!("{}", value),
                    detail: format!("`{}` → {} (threshold: {}, {}: {})",
                        command, value, threshold,
                        if passed { "PASS" } else { "FAIL" },
                        trimmed),
                    condition: cond,
                }
            }
            Err(e) => CheckResult {
                condition: cond,
                passed: false,
                actual: format!("error: {}", e),
                detail: String::new(),
            },
        }
    }

    async fn check_pattern(command: &str, must_contain: &str) -> CheckResult {
        let cond = GoalCondition::Pattern {
            command: command.to_string(),
            must_contain: must_contain.to_string(),
        };

        match tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .await
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let contains = stdout.contains(must_contain);
                CheckResult {
                    condition: cond,
                    passed: contains,
                    actual: if contains { "matched".to_string() } else { "no match".to_string() },
                    detail: if contains {
                        format!("output contains {:?}", must_contain)
                    } else {
                        format!("output does not contain {:?}\n---\n{}", must_contain, stdout)
                    },
                }
            }
            Err(e) => CheckResult {
                condition: cond,
                passed: false,
                actual: format!("error: {}", e),
                detail: String::new(),
            },
        }
    }
}

impl std::fmt::Display for GoalCondition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.description())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Comparison ─────────────────────────────────────────────────

    #[test]
    fn test_comparison_gt() {
        assert!(Comparison::Gt.evaluate(5.0, 3.0));
        assert!(!Comparison::Gt.evaluate(3.0, 5.0));
        assert!(!Comparison::Gt.evaluate(3.0, 3.0));
    }

    #[test]
    fn test_comparison_lt() {
        assert!(Comparison::Lt.evaluate(3.0, 5.0));
        assert!(!Comparison::Lt.evaluate(5.0, 3.0));
        assert!(!Comparison::Lt.evaluate(3.0, 3.0));
    }

    #[test]
    fn test_comparison_ge() {
        assert!(Comparison::Ge.evaluate(5.0, 3.0));
        assert!(Comparison::Ge.evaluate(3.0, 3.0));
        assert!(!Comparison::Ge.evaluate(2.0, 3.0));
    }

    #[test]
    fn test_comparison_le() {
        assert!(Comparison::Le.evaluate(3.0, 5.0));
        assert!(Comparison::Le.evaluate(3.0, 3.0));
        assert!(!Comparison::Le.evaluate(5.0, 3.0));
    }

    #[test]
    fn test_comparison_eq() {
        assert!(Comparison::Eq.evaluate(3.0, 3.0));
        assert!(!Comparison::Eq.evaluate(3.0, 4.0));
        assert!(Comparison::Eq.evaluate(0.1 + 0.2, 0.3)); // within EPSILON
    }

    // ── GoalCondition checks ──────────────────────────────────────

    #[tokio::test]
    async fn test_check_file_exists_found() {
        let result = GoalCondition::FileExists {
            path: "/tmp".to_string(),
        }
        .check()
        .await;
        assert!(result.passed, "expected /tmp to exist");
        assert_eq!(result.actual, "found");
    }

    #[tokio::test]
    async fn test_check_file_exists_not_found() {
        let result = GoalCondition::FileExists {
            path: "/nonexistent_path_xyz".to_string(),
        }
        .check()
        .await;
        assert!(!result.passed);
        assert_eq!(result.actual, "not found");
    }

    #[tokio::test]
    async fn test_check_exit_code_zero() {
        let result = GoalCondition::ExitCode {
            command: "true".to_string(),
            expected: None,
        }
        .check()
        .await;
        assert!(result.passed);
    }

    #[tokio::test]
    async fn test_check_exit_code_nonzero() {
        let result = GoalCondition::ExitCode {
            command: "false".to_string(),
            expected: None,
        }
        .check()
        .await;
        assert!(!result.passed);
    }

    #[tokio::test]
    async fn test_check_exit_code_custom_expected() {
        let result = GoalCondition::ExitCode {
            command: "exit 42".to_string(),
            expected: Some(42),
        }
        .check()
        .await;
        assert!(result.passed);
    }

    #[tokio::test]
    async fn test_check_pattern_match() {
        let result = GoalCondition::Pattern {
            command: "echo hello world".to_string(),
            must_contain: "hello".to_string(),
        }
        .check()
        .await;
        assert!(result.passed);
        assert_eq!(result.actual, "matched");
    }

    #[tokio::test]
    async fn test_check_pattern_no_match() {
        let result = GoalCondition::Pattern {
            command: "echo hello world".to_string(),
            must_contain: "goodbye".to_string(),
        }
        .check()
        .await;
        assert!(!result.passed);
        assert_eq!(result.actual, "no match");
    }

    #[tokio::test]
    async fn test_check_numeric_ge() {
        let result = GoalCondition::Numeric {
            command: "echo 5".to_string(),
            operator: Comparison::Ge,
            threshold: 3.0,
        }
        .check()
        .await;
        assert!(result.passed);
    }

    #[tokio::test]
    async fn test_check_numeric_lt() {
        let result = GoalCondition::Numeric {
            command: "echo 2".to_string(),
            operator: Comparison::Lt,
            threshold: 5.0,
        }
        .check()
        .await;
        assert!(result.passed);
    }

    #[tokio::test]
    async fn test_check_numeric_fail() {
        let result = GoalCondition::Numeric {
            command: "echo 10".to_string(),
            operator: Comparison::Lt,
            threshold: 5.0,
        }
        .check()
        .await;
        assert!(!result.passed);
    }

    // ── Serialization ─────────────────────────────────────────────

    #[test]
    fn test_serialize_exit_code() {
        let cond = GoalCondition::ExitCode {
            command: "cargo test".to_string(),
            expected: Some(0),
        };
        let json = serde_json::to_string(&cond).unwrap();
        assert!(json.contains("exit_code"));
        assert!(json.contains("cargo test"));
    }

    #[test]
    fn test_deserialize_file_exists() {
        let json = r#"{"type":"file_exists","path":"/tmp/test.txt"}"#;
        let cond: GoalCondition = serde_json::from_str(json).unwrap();
        assert_eq!(
            cond,
            GoalCondition::FileExists {
                path: "/tmp/test.txt".to_string()
            }
        );
    }

    #[test]
    fn test_deserialize_numeric() {
        let json = r#"{"type":"numeric","command":"wc -l","operator":">=","threshold":5.0}"#;
        let cond: GoalCondition = serde_json::from_str(json).unwrap();
        assert!(matches!(cond, GoalCondition::Numeric { .. }));
    }

    #[test]
    fn test_deserialize_static_analysis() {
        let json = r#"{"type":"static_analysis"}"#;
        let cond: GoalCondition = serde_json::from_str(json).unwrap();
        assert!(matches!(cond, GoalCondition::StaticAnalysis { .. }));
        if let GoalCondition::StaticAnalysis { command } = &cond {
            assert_eq!(command, "cargo clippy -- -D warnings");
        }
    }

    // ── Failure signature ─────────────────────────────────────────

    #[test]
    fn test_failure_signature_stable() {
        let cond = GoalCondition::ExitCode {
            command: "cargo test".to_string(),
            expected: Some(0),
        };
        let sig1 = cond.failure_signature("exit code: 1");
        let sig2 = cond.failure_signature("exit code: 1");
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_failure_signature_differs_by_actual() {
        let cond = GoalCondition::ExitCode {
            command: "cargo test".to_string(),
            expected: Some(0),
        };
        let sig1 = cond.failure_signature("exit code: 1");
        let sig2 = cond.failure_signature("exit code: 2");
        assert_ne!(sig1, sig2);
    }

    // ── Display ───────────────────────────────────────────────────

    #[test]
    fn test_display_exit_code() {
        let cond = GoalCondition::ExitCode {
            command: "cargo test".to_string(),
            expected: Some(0),
        };
        let s = cond.to_string();
        assert!(s.contains("cargo test"));
    }

    #[test]
    fn test_display_file_exists() {
        let cond = GoalCondition::FileExists {
            path: "/tmp/test".to_string(),
        };
        let s = cond.to_string();
        assert!(s.contains("/tmp/test"));
    }
}
