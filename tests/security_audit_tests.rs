//! Security Audit Tests
//!
//! Integration tests for the security audit system's public API.
//! Tests for private methods (scoring, issue collection) are in
//! src/security/audit.rs's #[cfg(test)] module.

use manta::security::audit::*;

// ── RiskLevel Tests ──────────────────────────────────────────────────────────

#[test]
fn risk_level_default_is_low() {
    assert_eq!(RiskLevel::default(), RiskLevel::Low);
}

#[test]
fn risk_level_equality() {
    assert_eq!(RiskLevel::Low, RiskLevel::Low);
    assert_eq!(RiskLevel::Critical, RiskLevel::Critical);
    assert_ne!(RiskLevel::Low, RiskLevel::High);
}

#[test]
fn risk_level_copy() {
    let a = RiskLevel::High;
    let b = a;
    assert_eq!(a, b);
}

// ── AuditConfig Tests ────────────────────────────────────────────────────────

#[test]
fn audit_config_default_all_enabled() {
    let config = AuditConfig::default();

    assert!(config.check_log_leaks);
    assert!(config.verify_sandbox);
    assert!(config.audit_tools);
    assert!(config.review_permissions);
    assert_eq!(config.paths_to_check, vec!["src", "tests"]);
}

#[test]
fn security_auditor_new_uses_default_config() {
    let auditor = SecurityAuditor::new();
    // Just verify it creates without panic
    let _ = auditor;
}

#[test]
fn security_auditor_with_custom_config() {
    let config = AuditConfig {
        check_log_leaks: false,
        verify_sandbox: false,
        audit_tools: false,
        review_permissions: false,
        paths_to_check: vec![],
    };
    let auditor = SecurityAuditor::with_config(config);
    let _ = auditor;
}

#[test]
fn security_auditor_default_impl() {
    let auditor: SecurityAuditor = Default::default();
    let _ = auditor;
}

// ── Security Boundary Tests ──────────────────────────────────────────────────

#[test]
fn boundary_enforcement_default_counts() {
    let enforcement = BoundaryEnforcement::default();

    assert_eq!(enforcement.total, 0);
    assert_eq!(enforcement.enforced, 0);
    assert_eq!(enforcement.partial, 0);
    assert_eq!(enforcement.not_enforced, 0);
}

#[test]
fn security_boundaries_default_empty() {
    let boundaries = SecurityBoundaries::default();

    assert!(boundaries.boundaries.is_empty());
}

#[test]
fn security_boundary_creation() {
    let boundary = SecurityBoundary {
        name: "Test Boundary".to_string(),
        description: "A test".to_string(),
        boundary_type: BoundaryType::UserIsolation,
        enforcement: "Test enforcement".to_string(),
        verified: true,
    };

    assert_eq!(boundary.name, "Test Boundary");
    assert!(boundary.verified);
}

// ── ComponentPermissions Tests ───────────────────────────────────────────────

#[test]
fn component_permissions_default() {
    let perms = ComponentPermissions::default();

    assert!(perms.required.is_empty());
    assert!(perms.granted.is_empty());
    assert!(perms.missing.is_empty());
    assert!(perms.excessive.is_empty());
}

// ── ToolAuditResult Tests ────────────────────────────────────────────────────

#[test]
fn tool_audit_result_default() {
    let result = ToolAuditResult::default();

    assert!(!result.passed);
    assert!(result.issues.is_empty());
    assert_eq!(result.risk_level, RiskLevel::Low);
}

// ── SecurityIssue Tests ──────────────────────────────────────────────────────

#[test]
fn security_issue_creation() {
    let issue = SecurityIssue {
        category: "Test".to_string(),
        severity: RiskLevel::High,
        description: "Test issue".to_string(),
        location: "test.rs:1".to_string(),
        recommendation: "Fix it".to_string(),
    };

    assert_eq!(issue.category, "Test");
    assert_eq!(issue.severity, RiskLevel::High);
}

// ── SandboxFeatureCheck Tests ────────────────────────────────────────────────

#[test]
fn sandbox_feature_check_creation() {
    let check = SandboxFeatureCheck {
        name: "timeout".to_string(),
        enabled: true,
        verified: true,
        details: "30s timeout".to_string(),
    };

    assert!(check.enabled);
    assert!(check.verified);
}

// ── ResourceLimitsCheck Tests ────────────────────────────────────────────────

#[test]
fn resource_limits_default_all_false() {
    let limits = ResourceLimitsCheck::default();

    assert!(!limits.cpu_limits);
    assert!(!limits.memory_limits);
    assert!(!limits.time_limits);
    assert!(!limits.fd_limits);
    assert!(!limits.network_limits);
}

// ── IsolationCheck Tests ─────────────────────────────────────────────────────

#[test]
fn isolation_check_default_all_false() {
    let check = IsolationCheck::default();

    assert!(!check.process_isolation);
    assert!(!check.filesystem_isolation);
    assert!(!check.network_isolation);
    assert!(!check.env_isolation);
}

// ── PotentialLeak Tests ──────────────────────────────────────────────────────

#[test]
fn potential_leak_creation() {
    let leak = PotentialLeak {
        category: LeakCategory::LogLeak,
        description: "Password in log".to_string(),
        location: "logger.rs:42".to_string(),
        severity: RiskLevel::High,
        recommendation: "Use structured logging".to_string(),
    };

    assert_eq!(leak.category, LeakCategory::LogLeak);
    assert_eq!(leak.severity, RiskLevel::High);
}

// ── LeakCategory Tests ───────────────────────────────────────────────────────

#[test]
fn leak_category_equality() {
    assert_eq!(LeakCategory::LogLeak, LeakCategory::LogLeak);
    assert_ne!(LeakCategory::LogLeak, LeakCategory::CredentialExposure);
}

// ── ToolSecurityCheck Tests ──────────────────────────────────────────────────

#[test]
fn tool_security_check_creation() {
    let check = ToolSecurityCheck {
        name: "path_allowlist".to_string(),
        passed: true,
        description: "Validates paths".to_string(),
    };

    assert!(check.passed);
    assert_eq!(check.name, "path_allowlist");
}

// ── Full Audit Report Structure Tests ────────────────────────────────────────

#[tokio::test]
async fn full_audit_report_has_all_sections() {
    let auditor = SecurityAuditor::new();
    let report = auditor.run_audit().await;

    // Score should be computed
    assert!(report.score <= 100);

    // All sections should be present
    assert!(!report.permissions.components.is_empty());
    assert!(!report.tools.tool_results.is_empty());
    assert!(!report.boundaries.boundaries.is_empty());

    // Timestamp should be set
    let elapsed = report
        .timestamp
        .elapsed()
        .expect("timestamp should be valid");
    assert!(elapsed.as_secs() < 5, "Audit should complete quickly");
}

#[tokio::test]
async fn audit_with_disabled_checks_runs_fast() {
    let config = AuditConfig {
        check_log_leaks: false,
        verify_sandbox: false,
        audit_tools: false,
        review_permissions: false,
        paths_to_check: vec![],
    };
    let auditor = SecurityAuditor::with_config(config);
    let report = auditor.run_audit().await;

    // With checks disabled, permissions/tools/data_leaks are empty,
    // but boundaries and sandbox still have hardcoded defaults.
    // Network boundary is not enforced (-5), sandbox has missing
    // memory_limits (-10) and network_isolation (-5) = 80.
    assert_eq!(report.score, 80, "Score should reflect boundary/sandbox defaults");
    assert!(report.critical_issues.is_empty());
    assert!(report.permissions.components.is_empty());
    assert!(report.tools.tool_results.is_empty());
}

#[tokio::test]
async fn audit_collects_tool_issues_for_high_risk_tools() {
    let auditor = SecurityAuditor::new();
    let report = auditor.run_audit().await;

    // The hardcoded audit has code_execution as High risk
    let has_high_risk = report
        .tools
        .tool_results
        .values()
        .any(|r| r.risk_level == RiskLevel::High);
    assert!(has_high_risk, "Should detect high-risk tools");
}

#[tokio::test]
async fn audit_score_is_reasonable() {
    let auditor = SecurityAuditor::new();
    let report = auditor.run_audit().await;

    // Score should be between 0 and 100
    assert!(report.score <= 100);
    // With current hardcoded data, score should be in a reasonable range
    assert!(
        report.score > 0 && report.score < 100,
        "Score should reflect issues found, got {}",
        report.score
    );
}
