//! Security commands for Manta
//!
//! Provides security audit, DM pairing, and access control management.

use crate::error::{MantaError, Result};
use clap::Subcommand;
use serde_json::json;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

#[derive(Debug, Subcommand)]
pub enum SecurityCommands {
    /// Run comprehensive security audit
    Audit {
        /// Output format
        #[arg(short, long, value_enum, default_value = "table")]
        format: super::OutputFormat,
        /// Check specific paths for secrets
        #[arg(short, long)]
        paths: Vec<String>,
        /// Skip data leak checks
        #[arg(long)]
        skip_leaks: bool,
        /// Skip sandbox verification
        #[arg(long)]
        skip_sandbox: bool,
    },
    /// Show security status summary
    Status,
    /// Manage DM pairing (approve/reject pending requests)
    Pairing {
        #[command(subcommand)]
        command: PairingCommands,
    },
    /// List authorized users (approved or allowlisted)
    List {
        /// Channel type to filter by
        #[arg(short, long)]
        channel: Option<String>,
    },
    /// Revoke user access
    Revoke {
        /// Channel type
        #[arg(short, long)]
        channel: String,
        /// User ID to revoke
        #[arg(short, long)]
        user_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum PairingCommands {
    /// List pending pairing requests
    List {
        /// Channel to filter by
        #[arg(short, long)]
        channel: Option<String>,
    },
    /// Approve a pending pairing request by code
    Approve {
        /// Channel (e.g., telegram, discord)
        channel: String,
        /// Pairing code (e.g., ABC123)
        code: String,
        /// Admin name/ID (for audit trail)
        #[arg(short, long)]
        as_admin: Option<String>,
    },
    /// Reject/deny a pending pairing request
    Reject {
        /// Channel
        channel: String,
        /// Pairing code
        code: String,
    },
    /// Add user directly to allowlist (bypass pairing flow)
    Allow {
        /// Channel
        #[arg(short, long)]
        channel: String,
        /// User ID
        #[arg(short, long)]
        user_id: String,
        /// Optional username/handle
        #[arg(short, long)]
        username: Option<String>,
    },
}

/// Run security commands
pub async fn run_security_command(command: &SecurityCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        SecurityCommands::Pairing { command } => {
            match command {
                PairingCommands::List { channel } => {
                    let mut url = format!("{}/api/v1/pairing/pending", DAEMON_URL);
                    if let Some(ch) = channel {
                        url.push_str(&format!("?channel={}", ch));
                    }
                    match client.get(&url).send().await {
                        Ok(resp) => {
                            let body: serde_json::Value = resp.json().await.unwrap_or_default();
                            if let Some(requests) = body.get("requests").and_then(|r| r.as_array()) {
                                if requests.is_empty() {
                                    println!("No pending pairing requests.");
                                } else {
                                    println!("Pending Pairing Requests:");
                                    println!("{:<12} {:<15} {:<20} {}", "Code", "Channel", "User ID", "Created");
                                    println!("{}", "-".repeat(70));
                                    for req in requests {
                                        println!("{:<12} {:<15} {:<20} {}",
                                            req.get("code").and_then(|c| c.as_str()).unwrap_or("-"),
                                            req.get("channel").and_then(|c| c.as_str()).unwrap_or("-"),
                                            req.get("user_id").and_then(|u| u.as_str()).unwrap_or("-"),
                                            req.get("created_at").and_then(|c| c.as_str()).unwrap_or("-"),
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to reach daemon: {}", e);
                            return Err(MantaError::Internal(e.to_string()));
                        }
                    }
                    Ok(())
                }
                PairingCommands::Approve { channel, code, as_admin } => {
                    let url = format!("{}/api/v1/pairing/approve", DAEMON_URL);
                    let body = json!({
                        "channel": channel,
                        "code": code,
                        "approved_by": as_admin,
                    });
                    match client.post(&url).json(&body).send().await {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                let body: serde_json::Value = resp.json().await.unwrap_or_default();
                                let user_id = body.get("user_id").and_then(|u| u.as_str()).unwrap_or("unknown");
                                println!("✅ Approved pairing request {} for user {} on {}", code, user_id, channel);
                            } else {
                                let text = resp.text().await.unwrap_or_default();
                                eprintln!("Failed to approve: {}", text);
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to reach daemon: {}", e);
                            return Err(MantaError::Internal(e.to_string()));
                        }
                    }
                    Ok(())
                }
                PairingCommands::Reject { channel, code } => {
                    let url = format!("{}/api/v1/pairing/reject", DAEMON_URL);
                    let body = json!({
                        "channel": channel,
                        "code": code,
                    });
                    match client.post(&url).json(&body).send().await {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                println!("❌ Rejected pairing request {} on {}", code, channel);
                            } else {
                                let text = resp.text().await.unwrap_or_default();
                                eprintln!("Failed to reject: {}", text);
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to reach daemon: {}", e);
                            return Err(MantaError::Internal(e.to_string()));
                        }
                    }
                    Ok(())
                }
                PairingCommands::Allow { channel, user_id, username } => {
                    let url = format!("{}/api/v1/pairing/allow", DAEMON_URL);
                    let body = json!({
                        "channel": channel,
                        "user_id": user_id,
                        "username": username,
                    });
                    match client.post(&url).json(&body).send().await {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                println!("✅ Added {} to allowlist for {}", user_id, channel);
                            } else {
                                let text = resp.text().await.unwrap_or_default();
                                eprintln!("Failed to add to allowlist: {}", text);
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to reach daemon: {}", e);
                            return Err(MantaError::Internal(e.to_string()));
                        }
                    }
                    Ok(())
                }
            }
        }

        SecurityCommands::Audit { format, paths, skip_leaks, skip_sandbox } => {
            // Run local security audit
            let config = crate::config::Config::load()?;
            let mut audit_config = crate::security::audit::AuditConfig::default();

            if !paths.is_empty() {
                audit_config.paths_to_check = paths.clone();
            }
            audit_config.check_log_leaks = !skip_leaks;
            audit_config.verify_sandbox = !skip_sandbox;

            let auditor = crate::security::audit::SecurityAuditor::with_config(audit_config);
            let report = auditor.run_audit().await;

            // Output based on format
            match format {
                super::OutputFormat::Json => {
                    let json = json!({
                        "timestamp": report.timestamp.duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default().as_secs(),
                        "score": report.score,
                        "critical_issues": report.critical_issues.len(),
                        "warnings": report.warnings.len(),
                        "recommendations": report.recommendations.len(),
                        "permissions": {
                            "total_checks": report.permissions.total_checks,
                            "passed": report.permissions.passed,
                            "failed": report.permissions.failed,
                        },
                        "tools": {
                            "total": report.tools.total_tools,
                            "passing": report.tools.passing,
                            "failing": report.tools.failing,
                        },
                        "data_leaks": {
                            "checks_performed": report.data_leaks.checks_performed,
                            "leaks_found": report.data_leaks.leaks_found,
                        },
                        "sandbox": {
                            "enabled": report.sandbox.enabled,
                            "features": report.sandbox.features.len(),
                        },
                        "critical_issues_list": report.critical_issues.iter().map(|i| {
                            json!({
                                "category": i.category,
                                "severity": format!("{:?}", i.severity),
                                "description": i.description,
                                "location": i.location,
                                "recommendation": i.recommendation,
                            })
                        }).collect::<Vec<_>>(),
                        "warnings_list": report.warnings.iter().map(|i| {
                            json!({
                                "category": i.category,
                                "severity": format!("{:?}", i.severity),
                                "description": i.description,
                                "location": i.location,
                            })
                        }).collect::<Vec<_>>(),
                        "recommendations": report.recommendations,
                    });
                    println!("{}", serde_json::to_string_pretty(&json).unwrap_or_default());
                }
                super::OutputFormat::Yaml => {
                    println!("Security Audit Report");
                    println!("====================");
                    println!("Score: {}/100", report.score);
                    println!("Critical Issues: {}", report.critical_issues.len());
                    println!("Warnings: {}", report.warnings.len());
                    println!();

                    if !report.critical_issues.is_empty() {
                        println!("CRITICAL ISSUES:");
                        for issue in &report.critical_issues {
                            println!("  [!] {}", issue.description);
                            println!("      Location: {}", issue.location);
                            println!("      Fix: {}", issue.recommendation);
                            println!();
                        }
                    }

                    if !report.warnings.is_empty() {
                        println!("WARNINGS:");
                        for warning in &report.warnings {
                            println!("  [-] {}", warning.description);
                            println!("      Location: {}", warning.location);
                            println!();
                        }
                    }

                    if !report.recommendations.is_empty() {
                        println!("RECOMMENDATIONS:");
                        for rec in &report.recommendations {
                            println!("  * {}", rec);
                        }
                    }
                }
                _ => {
                    // Table / Plain format
                    println!("╔══════════════════════════════════════════════════════════════╗");
                    println!("║              SECURITY AUDIT REPORT                           ║");
                    println!("╠══════════════════════════════════════════════════════════════╣");

                    let score_color = if report.score >= 80 {
                        "🟢"
                    } else if report.score >= 60 {
                        "🟡"
                    } else if report.score >= 40 {
                        "🟠"
                    } else {
                        "🔴"
                    };

                    println!("║  Overall Score: {} {}/100                           ║", score_color, report.score);
                    println!("║                                                              ║");
                    println!("║  Permissions:  {}/{} passed                           ║",
                        report.permissions.passed, report.permissions.total_checks);
                    println!("║  Tools:        {}/{} passing                          ║",
                        report.tools.passing, report.tools.total_tools);
                    println!("║  Data Leaks:   {} found in {} checks                  ║",
                        report.data_leaks.leaks_found, report.data_leaks.checks_performed);
                    println!("║  Sandbox:      {}                                    ║",
                        if report.sandbox.enabled { "✅ Enabled" } else { "❌ Disabled" });
                    println!("╚══════════════════════════════════════════════════════════════╝");
                    println!();

                    if !report.critical_issues.is_empty() {
                        println!("🔴 CRITICAL ISSUES ({}):", report.critical_issues.len());
                        for (i, issue) in report.critical_issues.iter().enumerate() {
                            println!("  {}. [{}] {}", i + 1, issue.category, issue.description);
                            println!("     Location: {}", issue.location);
                            println!("     Recommendation: {}", issue.recommendation);
                            println!();
                        }
                    }

                    if !report.warnings.is_empty() {
                        println!("🟡 WARNINGS ({}):", report.warnings.len());
                        for (i, warning) in report.warnings.iter().enumerate() {
                            println!("  {}. [{}] {}", i + 1, warning.category, warning.description);
                        }
                        println!();
                    }

                    if !report.recommendations.is_empty() {
                        println!("💡 RECOMMENDATIONS:");
                        for rec in &report.recommendations {
                            println!("  • {}", rec);
                        }
                    }

                    if report.critical_issues.is_empty() && report.warnings.is_empty() {
                        println!("✅ No critical issues or warnings found!");
                    }
                }
            }

            // Return error if critical issues found (for CI/CD use)
            if !report.critical_issues.is_empty() {
                return Err(MantaError::Validation(
                    format!("Security audit found {} critical issues", report.critical_issues.len())
                ));
            }

            Ok(())
        }

        SecurityCommands::Status => {
            let url = format!("{}/api/v1/security/status", DAEMON_URL);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    println!("{}", body);
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon at {}: {}", DAEMON_URL, e);
                    eprintln!("Is the daemon running? Try: manta start");
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }

        SecurityCommands::List { channel } => {
            let mut url = format!("{}/api/v1/security/authorized", DAEMON_URL);
            if let Some(ch) = channel {
                url.push_str(&format!("?channel={}", ch));
            }
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    println!("{}", body);
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }

        SecurityCommands::Revoke { channel, user_id } => {
            let url = format!("{}/api/v1/security/revoke", DAEMON_URL);
            let body = json!({
                "channel": channel,
                "user_id": user_id,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("✅ Revoked access for {} on {}", user_id, channel);
                    } else {
                        eprintln!("Failed to revoke ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
    }
}
