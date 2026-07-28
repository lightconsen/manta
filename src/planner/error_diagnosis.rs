//! Error diagnosis — parse execution errors, locate root causes, suggest fixes.
//!
//! The [`ErrorDiagnosisEngine`] analyses error messages from failed tasks
//! and produces a [`Diagnosis`] with root cause, severity, and suggested
//! remediation. It combines heuristic pattern matching with optional LLM
//! fallback for novel errors.
//!
//! ```text
//! "npm ERR! EACCES: permission denied" → root: permission, fix: sudo / chown
//! "connection refused 127.0.0.1:5432"   → root: service down, fix: start postgres
//! ```

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::providers::{CompletionRequest, Message, Provider};

/// Severity of a diagnosed issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    /// Informational only — no action needed.
    Info,
    /// Warning — may cause issues but not blocking.
    Warning,
    /// Error — blocking, but auto-recoverable.
    Error,
    /// Critical — blocking, requires human attention.
    Critical,
}

/// A single diagnosed root cause.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootCause {
    /// Human-readable description.
    pub description: String,
    /// Category for grouping.
    pub category: ErrorCategory,
    /// Confidence [0.0, 1.0].
    pub confidence: f32,
}

/// Broad category of error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCategory {
    /// Missing binary / library / package.
    MissingDependency,
    /// Permission denied or insufficient privileges.
    PermissionDenied,
    /// Network unreachable / connection refused / timeout.
    NetworkIssue,
    /// Configuration file or environment variable problem.
    ConfigurationError,
    /// Resource exhaustion (disk full, memory, file descriptors).
    ResourceExhaustion,
    /// Syntax / logic bug in user code or script.
    CodeError,
    /// Service not running / crashed.
    ServiceDown,
    /// Race condition or transient failure.
    TransientFailure,
    /// Unknown / uncategorised.
    Unknown,
}

/// Suggested remediation step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationStep {
    /// Description of what to do.
    pub description: String,
    /// An action that can be executed (shell command, tool call, etc.).
    pub action: Option<String>,
    /// Whether this step is safe to auto-execute.
    pub auto_safe: bool,
}

/// Full diagnosis of a failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnosis {
    /// Root cause(s) identified.
    pub root_causes: Vec<RootCause>,
    /// Overall severity.
    pub severity: Severity,
    /// Ordered remediation steps.
    pub remediation: Vec<RemediationStep>,
    /// Whether the error is likely transient (retry may succeed).
    pub is_transient: bool,
    /// Raw confidence of the entire diagnosis.
    pub confidence: f32,
}

impl Diagnosis {
    /// Create a minimal diagnosis for a transient failure.
    pub fn transient(reason: impl Into<String>) -> Self {
        Self {
            root_causes: vec![RootCause {
                description: reason.into(),
                category: ErrorCategory::TransientFailure,
                confidence: 0.7,
            }],
            severity: Severity::Warning,
            remediation: vec![RemediationStep {
                description: "Wait a moment and retry".to_string(),
                action: None,
                auto_safe: true,
            }],
            is_transient: true,
            confidence: 0.7,
        }
    }

    /// Create a diagnosis with a single root cause.
    pub fn single(
        category: ErrorCategory,
        description: impl Into<String>,
        severity: Severity,
        remediation: Vec<RemediationStep>,
    ) -> Self {
        Self {
            root_causes: vec![RootCause {
                description: description.into(),
                category,
                confidence: 0.85,
            }],
            severity,
            remediation,
            is_transient: false,
            confidence: 0.85,
        }
    }
}

/// Engine for diagnosing execution failures.
#[derive(Clone)]
pub struct ErrorDiagnosisEngine {
    provider: Option<Arc<dyn Provider>>,
}

impl ErrorDiagnosisEngine {
    /// Create a heuristic-only diagnosis engine.
    pub fn new() -> Self {
        Self { provider: None }
    }

    /// Create a diagnosis engine backed by an LLM for novel errors.
    pub fn with_provider(provider: Arc<dyn Provider>) -> Self {
        Self { provider: Some(provider) }
    }

    /// Diagnose an error message and suggest fixes.
    ///
    /// First applies heuristic rules; if confidence is low and an LLM
    /// provider is available, falls back to LLM-based diagnosis.
    pub async fn diagnose(&self, error: &str, context: &str) -> crate::Result<Diagnosis> {
        let heuristic = self.heuristic_diagnose(error, context);

        if heuristic.confidence < 0.6 {
            if let Some(ref provider) = self.provider {
                let llm_diag = self.llm_diagnose(error, context, provider).await?;
                if llm_diag.confidence > heuristic.confidence {
                    return Ok(llm_diag);
                }
            }
        }

        Ok(heuristic)
    }

    /// Diagnose multiple errors and return the most severe diagnosis.
    pub async fn diagnose_batch(
        &self,
        errors: &[(String, String)],
    ) -> crate::Result<Vec<Diagnosis>> {
        let mut results = Vec::with_capacity(errors.len());
        for (error, context) in errors {
            results.push(self.diagnose(error, context).await?);
        }
        Ok(results)
    }

    // -----------------------------------------------------------------------
    // Heuristic rule engine
    // -----------------------------------------------------------------------

    fn heuristic_diagnose(&self, error: &str, _context: &str) -> Diagnosis {
        let lower = error.to_lowercase();

        // Rule: permission denied.
        if lower.contains("permission denied")
            || lower.contains("eacces")
            || lower.contains("operation not permitted")
        {
            return Diagnosis::single(
                ErrorCategory::PermissionDenied,
                "Insufficient privileges to perform the operation",
                Severity::Error,
                vec![
                    RemediationStep {
                        description: "Check file ownership and permissions".to_string(),
                        action: Some("ls -la <path>".to_string()),
                        auto_safe: true,
                    },
                    RemediationStep {
                        description: "Run with elevated privileges if safe".to_string(),
                        action: Some("sudo <command>".to_string()),
                        auto_safe: false,
                    },
                    RemediationStep {
                        description: "Change ownership to current user".to_string(),
                        action: Some("sudo chown -R $(whoami) <path>".to_string()),
                        auto_safe: false,
                    },
                ],
            );
        }

        // Rule: command not found / missing dependency.
        if lower.contains("command not found")
            || lower.contains("not recognized")
            || lower.contains("no such file or directory")
            || lower.contains("cannot find")
            || lower.contains("module not found")
            || lower.contains("package not found")
        {
            return Diagnosis::single(
                ErrorCategory::MissingDependency,
                "Required program, package, or module is not installed",
                Severity::Error,
                vec![
                    RemediationStep {
                        description: "Install the missing package".to_string(),
                        action: Some("brew install <pkg> || apt-get install <pkg>".to_string()),
                        auto_safe: true,
                    },
                    RemediationStep {
                        description: "Check PATH environment variable".to_string(),
                        action: Some("echo $PATH".to_string()),
                        auto_safe: true,
                    },
                ],
            );
        }

        // Rule: connection refused / network issues.
        if lower.contains("connection refused")
            || lower.contains("econnrefused")
            || lower.contains("no route to host")
            || lower.contains("network is unreachable")
            || lower.contains("ename_not_resolved")
            || lower.contains("getaddrinfo failed")
        {
            return Diagnosis::single(
                ErrorCategory::NetworkIssue,
                "Cannot reach the target host or service",
                Severity::Error,
                vec![
                    RemediationStep {
                        description: "Check network connectivity".to_string(),
                        action: Some("ping <host>".to_string()),
                        auto_safe: true,
                    },
                    RemediationStep {
                        description: "Check if target service is running".to_string(),
                        action: Some("ss -tlnp | grep <port>".to_string()),
                        auto_safe: true,
                    },
                    RemediationStep {
                        description: "Check firewall rules".to_string(),
                        action: Some("sudo iptables -L".to_string()),
                        auto_safe: false,
                    },
                ],
            );
        }

        // Rule: timeout.
        if lower.contains("timeout")
            || lower.contains("timed out")
            || lower.contains("etimedout")
            || lower.contains("deadline exceeded")
        {
            return Diagnosis::transient("Operation timed out — target may be slow or unreachable");
        }

        // Rule: resource exhaustion.
        if lower.contains("no space left")
            || lower.contains("disk full")
            || lower.contains("out of memory")
            || lower.contains("enomem")
            || lower.contains("too many open files")
        {
            return Diagnosis::single(
                ErrorCategory::ResourceExhaustion,
                "System resource exhausted (disk, memory, or file descriptors)",
                Severity::Critical,
                vec![
                    RemediationStep {
                        description: "Check disk usage".to_string(),
                        action: Some("df -h".to_string()),
                        auto_safe: true,
                    },
                    RemediationStep {
                        description: "Check memory usage".to_string(),
                        action: Some("free -h".to_string()),
                        auto_safe: true,
                    },
                    RemediationStep {
                        description: "Clean up temporary files".to_string(),
                        action: Some("rm -rf /tmp/*".to_string()),
                        auto_safe: false,
                    },
                ],
            );
        }

        // Rule: configuration error.
        if lower.contains("invalid config")
            || lower.contains("parse error")
            || lower.contains("syntax error")
            || lower.contains("unexpected token")
            || lower.contains("yaml error")
            || lower.contains("toml error")
        {
            return Diagnosis::single(
                ErrorCategory::ConfigurationError,
                "Configuration file has syntax errors or invalid values",
                Severity::Error,
                vec![
                    RemediationStep {
                        description: "Validate configuration with a linter".to_string(),
                        action: None,
                        auto_safe: true,
                    },
                    RemediationStep {
                        description: "Restore from known-good backup".to_string(),
                        action: None,
                        auto_safe: true,
                    },
                ],
            );
        }

        // Rule: service down (port unreachable but host is up).
        if lower.contains("could not connect to server")
            || lower.contains("refused")
            || lower.contains("service unavailable")
        {
            return Diagnosis::single(
                ErrorCategory::ServiceDown,
                "Target service is not running or not accepting connections",
                Severity::Error,
                vec![
                    RemediationStep {
                        description: "Start the service".to_string(),
                        action: Some("systemctl start <service>".to_string()),
                        auto_safe: false,
                    },
                    RemediationStep {
                        description: "Check service logs".to_string(),
                        action: Some("journalctl -u <service>".to_string()),
                        auto_safe: true,
                    },
                ],
            );
        }

        // Rule: code / compilation error.
        if lower.contains("compilation error")
            || lower.contains("build failed")
            || lower.contains("error: ")
            || lower.contains("failed to compile")
        {
            return Diagnosis::single(
                ErrorCategory::CodeError,
                "Build or compilation failure in source code",
                Severity::Error,
                vec![
                    RemediationStep {
                        description: "Read the full compiler error message".to_string(),
                        action: None,
                        auto_safe: true,
                    },
                    RemediationStep {
                        description: "Fix the reported line and re-build".to_string(),
                        action: None,
                        auto_safe: true,
                    },
                ],
            );
        }

        // Low-confidence fallback.
        Diagnosis {
            root_causes: vec![RootCause {
                description: "Unknown error — no heuristic match".to_string(),
                category: ErrorCategory::Unknown,
                confidence: 0.3,
            }],
            severity: Severity::Warning,
            remediation: vec![RemediationStep {
                description: "Retry the operation".to_string(),
                action: None,
                auto_safe: true,
            }],
            is_transient: false,
            confidence: 0.3,
        }
    }

    // -----------------------------------------------------------------------
    // LLM fallback
    // -----------------------------------------------------------------------

    async fn llm_diagnose(
        &self,
        error: &str,
        context: &str,
        provider: &Arc<dyn Provider>,
    ) -> crate::Result<Diagnosis> {
        let prompt = format!(
            r#"Analyse the error and output JSON:
category: [MissingDependency|PermissionDenied|NetworkIssue|ConfigurationError|ResourceExhaustion|CodeError|ServiceDown|TransientFailure|Unknown]
severity: [Info|Warning|Error|Critical]
description: brief root cause
is_transient: bool
remediation: [{{description, action?, auto_safe: bool}}]

Context: {}
Error: {}

Output ONLY valid JSON."#,
            context, error
        );

        let request = CompletionRequest {
            messages: vec![
                Message::system(
                    "You are a root cause analysis engine. Analyse errors and output structured JSON diagnoses.",
                ),
                Message::user(prompt),
            ],
            temperature: Some(0.2),
            max_tokens: Some(1024),
            stream: false,
            requires_reasoning: true,
            ..Default::default()
        };

        let response = provider.complete(request).await?;
        let content = response.message.content.trim();
        let json_str = strip_code_fences(content);

        #[derive(Deserialize)]
        struct LlmDiagnosis {
            category: String,
            severity: String,
            description: String,
            #[serde(default)]
            is_transient: bool,
            #[serde(default)]
            remediation: Vec<LlmRemediation>,
        }

        #[derive(Deserialize)]
        struct LlmRemediation {
            description: String,
            action: Option<String>,
            #[serde(default)]
            auto_safe: bool,
        }

        let llm: LlmDiagnosis = serde_json::from_str(json_str).map_err(|e| {
            crate::error::SyscityError::Validation(format!(
                "Failed to parse LLM diagnosis: {}. Raw: {}",
                e,
                &content[..content.len().min(300)]
            ))
        })?;

        let category = parse_category(&llm.category);
        let severity = parse_severity(&llm.severity);

        let remediation = llm
            .remediation
            .into_iter()
            .map(|r| RemediationStep {
                description: r.description,
                action: r.action,
                auto_safe: r.auto_safe,
            })
            .collect();

        Ok(Diagnosis {
            root_causes: vec![RootCause {
                description: llm.description,
                category,
                confidence: 0.75,
            }],
            severity,
            remediation,
            is_transient: llm.is_transient,
            confidence: 0.75,
        })
    }
}

impl Default for ErrorDiagnosisEngine {
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

fn parse_category(s: &str) -> ErrorCategory {
    match s.to_lowercase().as_str() {
        "missingdependency" | "missing_dependency" => ErrorCategory::MissingDependency,
        "permissiondenied" | "permission_denied" => ErrorCategory::PermissionDenied,
        "networkissue" | "network_issue" => ErrorCategory::NetworkIssue,
        "configurationerror" | "configuration_error" => ErrorCategory::ConfigurationError,
        "resourceexhaustion" | "resource_exhaustion" => ErrorCategory::ResourceExhaustion,
        "codeerror" | "code_error" => ErrorCategory::CodeError,
        "servicedown" | "service_down" => ErrorCategory::ServiceDown,
        "transientfailure" | "transient_failure" => ErrorCategory::TransientFailure,
        _ => ErrorCategory::Unknown,
    }
}

fn parse_severity(s: &str) -> Severity {
    match s.to_lowercase().as_str() {
        "info" => Severity::Info,
        "warning" => Severity::Warning,
        "error" => Severity::Error,
        "critical" => Severity::Critical,
        _ => Severity::Warning,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diagnose_permission_denied() {
        let engine = ErrorDiagnosisEngine::new();
        let diag = engine.heuristic_diagnose("EACCES: permission denied, open '/etc/shadow'", "");
        assert_eq!(diag.root_causes[0].category, ErrorCategory::PermissionDenied);
        assert!(diag.severity >= Severity::Error);
        assert!(!diag.remediation.is_empty());
    }

    #[test]
    fn test_diagnose_command_not_found() {
        let engine = ErrorDiagnosisEngine::new();
        let diag = engine.heuristic_diagnose("bash: xdotool: command not found", "");
        assert_eq!(diag.root_causes[0].category, ErrorCategory::MissingDependency);
        assert!(diag.severity >= Severity::Error);
    }

    #[test]
    fn test_diagnose_connection_refused() {
        let engine = ErrorDiagnosisEngine::new();
        let diag = engine.heuristic_diagnose("connect ECONNREFUSED 127.0.0.1:5432", "");
        assert_eq!(diag.root_causes[0].category, ErrorCategory::NetworkIssue);
    }

    #[test]
    fn test_diagnose_timeout() {
        let engine = ErrorDiagnosisEngine::new();
        let diag = engine.heuristic_diagnose("Request timed out after 30000ms", "");
        assert_eq!(diag.root_causes[0].category, ErrorCategory::TransientFailure);
        assert!(diag.is_transient);
    }

    #[test]
    fn test_diagnose_resource_exhaustion() {
        let engine = ErrorDiagnosisEngine::new();
        let diag = engine.heuristic_diagnose("ENOSPC: no space left on device", "");
        assert_eq!(diag.root_causes[0].category, ErrorCategory::ResourceExhaustion);
        assert_eq!(diag.severity, Severity::Critical);
    }

    #[test]
    fn test_diagnose_unknown() {
        let engine = ErrorDiagnosisEngine::new();
        let diag = engine.heuristic_diagnose("something weird happened", "");
        assert_eq!(diag.root_causes[0].category, ErrorCategory::Unknown);
        assert!(diag.confidence < 0.5);
    }

    #[test]
    fn test_parse_category() {
        assert!(matches!(parse_category("PermissionDenied"), ErrorCategory::PermissionDenied));
        assert!(matches!(parse_category("network_issue"), ErrorCategory::NetworkIssue));
        assert!(matches!(parse_category("foo"), ErrorCategory::Unknown));
    }

    #[test]
    fn test_parse_severity() {
        assert!(matches!(parse_severity("Critical"), Severity::Critical));
        assert!(matches!(parse_severity("error"), Severity::Error));
        assert!(matches!(parse_severity("unknown"), Severity::Warning));
    }
}
