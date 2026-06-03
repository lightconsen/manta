//! Audit log commands for Syscity
//!
//! View system audit logs and security audit results.

use crate::error::{SyscityError, Result};
use clap::Subcommand;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

#[derive(Debug, Subcommand)]
pub enum AuditCommands {
    /// View the system audit log
    Log {
        /// Number of entries to show (default: 50)
        #[arg(short = 'n', long, default_value = "50")]
        limit: usize,
        /// Filter by event type
        #[arg(short, long)]
        event_type: Option<String>,
    },
    /// Run a local security audit
    Security {
        /// Output format
        #[arg(short, long, value_enum, default_value = "table")]
        format: super::OutputFormat,
    },
}

/// Run audit commands
pub async fn run_audit_command(command: &AuditCommands) -> Result<()> {
    match command {
        AuditCommands::Log { limit, event_type } => {
            let client = reqwest::Client::new();
            let mut url = format!("{}/api/v1/audit/log?limit={}", DAEMON_URL, limit);
            if let Some(et) = event_type {
                url.push_str(&format!("&event_type={}", et));
            }
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    if let Some(entries) = body.get("entries").and_then(|e| e.as_array()) {
                        if entries.is_empty() {
                            println!("No audit log entries.");
                        } else {
                            println!("Audit Log:");
                            println!("{:<20} {:<15} {:<20} Details", "Timestamp", "Event", "User");
                            println!("{}", "-".repeat(90));
                            for entry in entries {
                                println!(
                                    "{:<20} {:<15} {:<20} {}",
                                    entry
                                        .get("timestamp")
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("-"),
                                    entry
                                        .get("event_type")
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("-"),
                                    entry.get("user_id").and_then(|c| c.as_str()).unwrap_or("-"),
                                    entry
                                        .get("details")
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("-")
                                        .chars()
                                        .take(40)
                                        .collect::<String>(),
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
        AuditCommands::Security { format } => {
            // Run local security audit (same as `syscity security audit`)
            let _config = crate::config::Config::load()?;
            let auditor = crate::security::audit::SecurityAuditor::with_config(
                crate::security::audit::AuditConfig::default(),
            );
            let result = auditor.run_audit().await;

            match format {
                super::OutputFormat::Json | super::OutputFormat::Yaml => {
                    println!("Security Audit Results");
                    println!("======================");
                    println!("Score: {}/100", result.score);
                    println!("Timestamp: {:?}", result.timestamp);
                    println!(
                        "Permissions: {}/{} passed",
                        result.permissions.passed, result.permissions.total_checks
                    );
                    println!(
                        "Tools: {}/{} passing",
                        result.tools.passing, result.tools.total_tools
                    );
                    println!(
                        "Data Leaks: {} found in {} checks",
                        result.data_leaks.leaks_found, result.data_leaks.checks_performed
                    );
                }
                _ => {
                    println!("Security Audit Results");
                    println!("======================");
                    println!("Score: {}/100", result.score);
                    println!("Timestamp: {:?}", result.timestamp);
                    println!(
                        "Permissions: {}/{} passed",
                        result.permissions.passed, result.permissions.total_checks
                    );
                    println!(
                        "Tools: {}/{} passing",
                        result.tools.passing, result.tools.total_tools
                    );
                    println!(
                        "Data Leaks: {} found in {} checks",
                        result.data_leaks.leaks_found, result.data_leaks.checks_performed
                    );
                    if !result.critical_issues.is_empty() {
                        println!("\n❌ Critical Issues:");
                        for issue in &result.critical_issues {
                            println!("  - {}", issue.description);
                        }
                    }
                    if !result.warnings.is_empty() {
                        println!("\n⚠️  Warnings:");
                        for warning in &result.warnings {
                            println!("  - {}", warning.description);
                        }
                    }
                    if !result.recommendations.is_empty() {
                        println!("\n💡 Recommendations:");
                        for rec in &result.recommendations {
                            println!("  - {}", rec);
                        }
                    }
                }
            }
            Ok(())
        }
    }
}
