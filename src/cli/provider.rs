//! Provider management commands for Manta
//!
//! Top-level CLI for listing, enabling, disabling, and switching model providers.

use crate::error::{MantaError, Result};
use clap::Subcommand;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

#[derive(Debug, Subcommand)]
pub enum ProviderCommands {
    /// List available LLM providers
    List,
    /// Show provider health status
    Health {
        /// Provider ID
        id: String,
    },
    /// Enable a provider
    Enable {
        /// Provider ID
        id: String,
    },
    /// Disable a provider
    Disable {
        /// Provider ID
        id: String,
    },
    /// Switch the default model alias
    Switch {
        /// Model alias (fast, smart, default)
        alias: String,
    },
    /// Show current default model
    Default,
    /// Show provider usage statistics
    Usage {
        /// Provider ID (omit for all providers)
        id: Option<String>,
    },
    /// Authenticate a provider via OAuth 2.0 + PKCE
    Auth {
        /// Provider ID (for labeling the resulting credential)
        id: String,
        /// OAuth client ID
        #[arg(short, long)]
        client_id: String,
        /// Authorization endpoint URL
        #[arg(short = 'a', long)]
        auth_url: String,
        /// Token endpoint URL
        #[arg(short = 't', long)]
        token_url: String,
        /// Optional OAuth scope
        #[arg(short, long)]
        scope: Option<String>,
        /// Local redirect callback port (default: 18081)
        #[arg(short = 'p', long, default_value = "18081")]
        redirect_port: u16,
        /// Timeout in seconds for the callback (default: 300)
        #[arg(long, default_value = "300")]
        timeout: u64,
        /// Don't open browser automatically
        #[arg(long)]
        no_browser: bool,
    },
}

/// Run provider commands
pub async fn run_provider_command(command: &ProviderCommands, config: &crate::config::Config) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        ProviderCommands::List => {
            let url = format!("{}/api/v1/providers", DAEMON_URL);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    if let Some(providers) = body.get("providers").and_then(|p| p.as_array()) {
                        println!("Providers:");
                        println!("{:<20} {:<10} {:<10} {}", "ID", "Enabled", "Healthy", "Name");
                        println!("{}", "-".repeat(60));
                        for p in providers {
                            println!(
                                "{:<20} {:<10} {:<10} {}",
                                p.get("id").and_then(|c| c.as_str()).unwrap_or("-"),
                                if p.get("enabled").and_then(|c| c.as_bool()).unwrap_or(false) {
                                    "yes"
                                } else {
                                    "no"
                                },
                                if p.get("healthy").and_then(|c| c.as_bool()).unwrap_or(false) {
                                    "yes"
                                } else {
                                    "no"
                                },
                                p.get("name").and_then(|c| c.as_str()).unwrap_or("-"),
                            );
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
        ProviderCommands::Health { id } => {
            let url = format!("{}/api/v1/providers/{}/health", DAEMON_URL, id);
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
        ProviderCommands::Enable { id } => {
            let url = format!("{}/api/v1/providers/{}/enable", DAEMON_URL, id);
            match client.post(&url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("✅ Enabled provider {}", id);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to enable: {}", text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
        ProviderCommands::Disable { id } => {
            let url = format!("{}/api/v1/providers/{}/disable", DAEMON_URL, id);
            match client.post(&url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("✅ Disabled provider {}", id);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to disable: {}", text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
        ProviderCommands::Switch { alias } => {
            let url = format!("{}/api/v1/providers/switch", DAEMON_URL);
            let body = serde_json::json!({ "model": alias });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("✅ Switched default model to {}", alias);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to switch: {}", text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
        ProviderCommands::Default => {
            let url = format!("{}/api/v1/models/default", DAEMON_URL);
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
        ProviderCommands::Usage { id } => {
            let url = if let Some(ref provider_id) = id {
                format!("{}/api/v1/providers/usage/{}", DAEMON_URL, provider_id)
            } else {
                format!("{}/api/v1/providers/usage", DAEMON_URL)
            };
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    // Try to parse as formatted usage snapshots
                    if let Ok(snapshots) = serde_json::from_value::<
                        Vec<crate::model_router::ProviderUsageSnapshot>,
                    >(body.clone())
                    {
                        if id.is_some() {
                            for snapshot in &snapshots {
                                println!(
                                    "{}",
                                    crate::model_router::format_provider_snapshot(snapshot)
                                );
                            }
                        } else {
                            println!("{}", crate::model_router::format_usage_report(&snapshots));
                        }
                    } else {
                        // Fallback to pretty-printed JSON
                        println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
        ProviderCommands::Auth {
            id,
            client_id,
            auth_url,
            token_url,
            scope,
            redirect_port,
            timeout,
            no_browser,
        } => {
            run_auth_command(
                id,
                client_id,
                auth_url,
                token_url,
                scope.as_deref(),
                *redirect_port,
                *timeout,
                *no_browser,
            )
            .await
        }
    }
}

async fn run_auth_command(
    provider_id: &str,
    client_id: &str,
    auth_url: &str,
    token_url: &str,
    scope: Option<&str>,
    redirect_port: u16,
    timeout_secs: u64,
    no_browser: bool,
) -> Result<()> {
    use crate::model_router::{oauth_callback, OAuthConfig, OAuthFlow};

    let oauth_config = OAuthConfig {
        client_id: client_id.to_string(),
        auth_url: auth_url.to_string(),
        token_url: token_url.to_string(),
        scope: scope.map(|s| s.to_string()),
        redirect_port,
    };

    let flow = OAuthFlow::new();
    let authorization_url = flow.authorization_url(&oauth_config);

    println!("\n🔐  OAuth Authorization for '{}'\n", provider_id);
    println!("Open this URL in your browser:\n");
    println!("  {}\n", authorization_url);

    if !no_browser {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(&authorization_url).spawn();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open").arg(&authorization_url).spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", &authorization_url])
                .spawn();
        }
    }

    println!(
        "Waiting for callback on port {} (timeout: {}s)...\n",
        redirect_port, timeout_secs
    );

    let (code, returned_state) =
        oauth_callback::wait_for_callback(redirect_port, timeout_secs).await?;

    if returned_state != flow.state() {
        return Err(MantaError::ExternalService {
            source: "OAuth state mismatch — possible CSRF attack".to_string(),
            cause: None,
        });
    }

    println!("Exchanging authorization code for tokens...\n");

    let credential = flow.exchange_code(&code, &oauth_config).await?;

    println!("✅  Authorization successful for '{}'\n", provider_id);
    println!("Credential (add to your config):\n");

    match credential {
        crate::model_router::Credential::OAuth2 {
            access_token,
            refresh_token,
            expires_at,
            token_url,
            client_id,
            scope,
            ..
        } => {
            println!("[providers.{}.auth_profile]", provider_id);
            if let Some(ref rt) = refresh_token {
                println!("refresh_token = \"{}\"", rt);
            }
            println!("access_token  = \"{}\"", access_token);
            println!("expires_at    = \"{}\"", expires_at.to_rfc3339());
            println!("token_url     = \"{}\"", token_url);
            println!("client_id     = \"{}\"", client_id);
            if let Some(ref s) = scope {
                println!("scope         = \"{}\"", s);
            }
        }
        _ => {
            println!("{:?}", credential);
        }
    }

    Ok(())
}
