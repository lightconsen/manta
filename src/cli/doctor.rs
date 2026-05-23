//! Doctor diagnostic system for Manta
//!
//! Provides `manta doctor run` and `manta doctor report` commands for
//! diagnosing provider health, auth status, circuit state, and generating
//! actionable recommendations.

use crate::error::{MantaError, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

/// Path to cache the last diagnostic report.
fn report_cache_path() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| std::env::temp_dir())
        .join("manta")
        .join("last_doctor_report.json")
}

/// Doctor subcommands.
#[derive(Debug, Subcommand)]
pub enum DoctorCommands {
    /// Run diagnostics against the daemon
    Run {
        /// Filter to a specific provider
        #[arg(short, long)]
        provider: Option<String>,
        /// Show verbose output
        #[arg(short, long)]
        verbose: bool,
    },
    /// Show the last diagnostic report
    Report,
}

/// Overall health grade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthGrade {
    Healthy,
    Degraded,
    Critical,
}

impl std::fmt::Display for HealthGrade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthGrade::Healthy => write!(f, "Healthy"),
            HealthGrade::Degraded => write!(f, "Degraded"),
            HealthGrade::Critical => write!(f, "Critical"),
        }
    }
}

/// Diagnostic result for a single provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDiagnostic {
    pub provider: String,
    pub health: HealthGrade,
    pub circuit_state: String,
    pub auth_status: String,
    pub usage_status: String,
    pub last_error: Option<String>,
    pub recommendation: Option<String>,
}

/// Full doctor report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub overall_health: HealthGrade,
    pub provider_diagnostics: Vec<ProviderDiagnostic>,
    pub auth_diagnostics: Vec<AuthDiagnostic>,
    pub recommendations: Vec<String>,
    pub timestamp: String,
}

/// Auth diagnostic info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthDiagnostic {
    pub provider: String,
    pub total_keys: usize,
    pub available_keys: usize,
    pub status: String,
}

/// Run doctor commands.
pub async fn run_doctor_command(command: &DoctorCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        DoctorCommands::Run { provider, verbose } => {
            let report = run_diagnostics(&client, provider.as_deref(), *verbose).await?;
            print_report(&report, *verbose);

            // Cache report to disk
            if let Ok(json) = serde_json::to_string_pretty(&report) {
                let path = report_cache_path();
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&path, json);
            }

            Ok(())
        }
        DoctorCommands::Report => {
            let path = report_cache_path();
            if !path.exists() {
                println!("No diagnostic report found. Run `manta doctor run` first.");
                return Ok(());
            }
            let contents = std::fs::read_to_string(&path)
                .map_err(|e| MantaError::Internal(format!("Failed to read report: {}", e)))?;
            let report: DoctorReport = serde_json::from_str(&contents)
                .map_err(|e| MantaError::Internal(format!("Failed to parse report: {}", e)))?;
            print_report(&report, false);
            Ok(())
        }
    }
}

async fn run_diagnostics(
    client: &reqwest::Client,
    filter_provider: Option<&str>,
    verbose: bool,
) -> Result<DoctorReport> {
    let mut provider_diagnostics = Vec::new();
    let mut auth_diagnostics = Vec::new();
    let mut recommendations = Vec::new();

    // Fetch provider list
    let providers_resp = client
        .get(format!("{}/api/v1/providers", DAEMON_URL))
        .send()
        .await;

    let providers: Vec<serde_json::Value> = match providers_resp {
        Ok(resp) => resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v.get("providers").cloned())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default(),
        Err(e) => {
            return Ok(DoctorReport {
                overall_health: HealthGrade::Critical,
                provider_diagnostics: Vec::new(),
                auth_diagnostics: Vec::new(),
                recommendations: vec![format!(
                    "Cannot reach daemon at {}: {}. Is manta running?",
                    DAEMON_URL, e
                )],
                timestamp: chrono::Utc::now().to_rfc3339(),
            });
        }
    };

    for p in &providers {
        let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");

        if let Some(filter) = filter_provider {
            if id != filter {
                continue;
            }
        }

        let name = p.get("name").and_then(|v| v.as_str()).unwrap_or(id);
        let enabled = p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        let healthy = p.get("healthy").and_then(|v| v.as_bool()).unwrap_or(false);

        // Health check endpoint
        let health_body: Option<JsonValue> = match client
            .get(format!("{}/api/v1/providers/{}/health", DAEMON_URL, id))
            .send()
            .await
        {
            Ok(resp) => resp.json().await.ok(),
            Err(_) => None,
        };

        let circuit_state = health_body
            .as_ref()
            .and_then(|b| b.get("circuit_state").and_then(|v| v.as_str()))
            .unwrap_or("unknown")
            .to_string();

        // Auth status
        let auth_resp = client
            .get(format!("{}/api/v1/providers/{}/check", DAEMON_URL, id))
            .send()
            .await;

        let (auth_status, auth_body) = match auth_resp {
            Ok(resp) if resp.status().is_success() => {
                let body: Option<JsonValue> = resp.json().await.ok();
                ("ok".to_string(), body)
            }
            Ok(resp) => (format!("error {}", resp.status()), None),
            Err(_) => ("unreachable".to_string(), None),
        };

        // Usage status
        let usage_body: Option<JsonValue> = match client
            .get(format!("{}/api/v1/providers/usage/{}", DAEMON_URL, id))
            .send()
            .await
        {
            Ok(resp) => resp.json().await.ok(),
            Err(_) => None,
        };

        let usage_status = usage_body
            .as_ref()
            .and_then(|b| {
                let total = b
                    .get("total_tokens")
                    .and_then(|v| v.get("total_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if total > 0 {
                    Some(format!("{} tokens", total))
                } else {
                    Some("no usage".to_string())
                }
            })
            .unwrap_or_else(|| "unknown".to_string());

        let mut health = HealthGrade::Healthy;
        let mut last_error = None;
        let mut recommendation = None;

        if !enabled {
            health = HealthGrade::Degraded;
            recommendation = Some(format!(
                "Provider '{}' is disabled — run `manta provider enable {}`",
                name, id
            ));
        } else if !healthy {
            health = HealthGrade::Critical;
            recommendation =
                Some(format!("Provider '{}' is unhealthy — check API key and network", name));
        } else if circuit_state == "Open" {
            health = HealthGrade::Degraded;
            recommendation = Some(format!(
                "Provider '{}' circuit breaker is open — wait for cooldown or investigate failures",
                name
            ));
        } else if auth_status != "ok" {
            health = HealthGrade::Degraded;
            recommendation =
                Some(format!("Provider '{}' auth check failed — verify credentials", name));
        }

        if verbose {
            if let Some(ref body) = health_body {
                if let Some(err) = body.get("last_error").and_then(|v| v.as_str()) {
                    last_error = Some(err.to_string());
                }
            }
        }

        provider_diagnostics.push(ProviderDiagnostic {
            provider: name.to_string(),
            health,
            circuit_state,
            auth_status,
            usage_status,
            last_error,
            recommendation: recommendation.clone(),
        });

        if let Some(rec) = recommendation {
            recommendations.push(rec);
        }

        // Auth diagnostics
        if let Some(body) = auth_body {
            let total_keys = body.get("total_keys").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let available_keys = body
                .get("available_keys")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let status = if available_keys == 0 && total_keys > 0 {
                "all_keys_disabled".to_string()
            } else if available_keys < total_keys {
                format!("{}/{} keys available", available_keys, total_keys)
            } else {
                "all_keys_available".to_string()
            };

            auth_diagnostics.push(AuthDiagnostic {
                provider: name.to_string(),
                total_keys,
                available_keys,
                status,
            });
        }
    }

    // Determine overall health
    let overall_health = if provider_diagnostics
        .iter()
        .any(|d| d.health == HealthGrade::Critical)
    {
        HealthGrade::Critical
    } else if provider_diagnostics
        .iter()
        .any(|d| d.health == HealthGrade::Degraded)
    {
        HealthGrade::Degraded
    } else if provider_diagnostics.is_empty() {
        HealthGrade::Critical
    } else {
        HealthGrade::Healthy
    };

    if provider_diagnostics.is_empty() && filter_provider.is_none() {
        recommendations
            .push("No providers configured. Run `manta setup` to configure providers.".to_string());
    }

    Ok(DoctorReport {
        overall_health,
        provider_diagnostics,
        auth_diagnostics,
        recommendations,
        timestamp: chrono::Utc::now().to_rfc3339(),
    })
}

fn print_report(report: &DoctorReport, verbose: bool) {
    println!("\nManta Diagnostic Report");
    println!("{}", "=".repeat(50));
    println!("Timestamp: {}", report.timestamp);
    println!(
        "Overall Health: {}\n",
        match report.overall_health {
            HealthGrade::Healthy => "✅ Healthy",
            HealthGrade::Degraded => "⚠️  Degraded",
            HealthGrade::Critical => "❌ Critical",
        }
    );

    if !report.provider_diagnostics.is_empty() {
        println!("Provider Diagnostics:");
        println!("{}", "-".repeat(50));
        for d in &report.provider_diagnostics {
            let health_icon = match d.health {
                HealthGrade::Healthy => "✅",
                HealthGrade::Degraded => "⚠️",
                HealthGrade::Critical => "❌",
            };
            println!(
                "{} {:<20} | Circuit: {:<12} | Auth: {:<12} | Usage: {}",
                health_icon, d.provider, d.circuit_state, d.auth_status, d.usage_status
            );
            if verbose {
                if let Some(ref err) = d.last_error {
                    println!("    Last error: {}", err);
                }
                if let Some(ref rec) = d.recommendation {
                    println!("    → {}", rec);
                }
            }
        }
        println!();
    }

    if !report.auth_diagnostics.is_empty() {
        println!("Auth Diagnostics:");
        println!("{}", "-".repeat(50));
        for a in &report.auth_diagnostics {
            println!(
                "  {:<20} | Keys: {}/{} | {}",
                a.provider, a.available_keys, a.total_keys, a.status
            );
        }
        println!();
    }

    if !report.recommendations.is_empty() {
        println!("Recommendations:");
        println!("{}", "-".repeat(50));
        for (i, rec) in report.recommendations.iter().enumerate() {
            println!("  {}. {}", i + 1, rec);
        }
        println!();
    }
}
